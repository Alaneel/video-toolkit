//! Binary-level conversion and MCP checks, using only temporary synthetic files.
//! Default cases do not execute media tools; ignored cases require FFmpeg/ffprobe/ImageMagick.
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const PROCESS_TIMEOUT: Duration = Duration::from_secs(40);
const MESSAGE_TIMEOUT: Duration = Duration::from_secs(15);

struct Fixture {
    _directory: TempDir,
    root: PathBuf,
    empty_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let empty_path = root.join("no executables");
        fs::create_dir(&empty_path).unwrap();
        Self {
            _directory: directory,
            root,
            empty_path,
        }
    }

    fn cli(&self, real_tools: bool) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_video-toolkit"));
        command.current_dir(&self.root).stdin(Stdio::null());
        if !real_tools {
            command.env("PATH", &self.empty_path);
        }
        command
    }

    fn convert(&self, inputs: &[&Path], format: &str, output: &Path, exact: bool) -> Command {
        let mut command = self.cli(true);
        command.arg("convert");
        command.args(inputs);
        command.args(["--format", format, "--json"]);
        command.arg(if exact { "--output" } else { "--output-dir" });
        command.arg(output);
        command
    }
}

/// A panic or timeout always kills and reaps the direct process under test.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn read_bytes(reader: impl Read + Send + 'static) -> Receiver<std::io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = reader;
        let mut bytes = Vec::new();
        let result = reader.read_to_end(&mut bytes).map(|_| bytes);
        let _ = sender.send(result);
    });
    receiver
}

fn wait_for_exit(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "Subprocess exceeded its timeout");
        thread::sleep(Duration::from_millis(10));
    }
}

fn output(mut command: Command) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = ChildGuard(command.spawn().expect("spawn subprocess"));
    let stdout = read_bytes(child.0.stdout.take().unwrap());
    let stderr = read_bytes(child.0.stderr.take().unwrap());
    let status = wait_for_exit(&mut child.0);
    Output {
        status,
        stdout: stdout
            .recv_timeout(MESSAGE_TIMEOUT)
            .expect("stdout reader did not finish")
            .unwrap(),
        stderr: stderr
            .recv_timeout(MESSAGE_TIMEOUT)
            .expect("stderr reader did not finish")
            .unwrap(),
    }
}

fn conversion_response(output: &Output, expected_ok: bool) -> Value {
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout must be one JSON object: {error}; stdout={}; stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert!(value.is_object());
    assert_eq!(value["schema"], "file-conversion/v1");
    assert_eq!(value["ok"], expected_ok);
    assert_eq!(output.status.success(), expected_ok);
    assert!(value.get("result").is_some());
    assert!(value.get("error").is_some());
    if expected_ok {
        assert!(value["error"].is_null());
    } else {
        assert!(value["error"].is_string() || value["result"]["jobs"].is_array());
    }
    // JSON mode permits structured progress on stderr, never human log prefixes.
    for line in String::from_utf8_lossy(&output.stderr).lines() {
        let event: Value = serde_json::from_str(line).expect("stderr must be JSON progress");
        assert!(event["index"].is_u64());
        assert!(event["state"].is_string());
        let progress = event["progress"].as_f64().unwrap();
        assert!((0.0..=1.0).contains(&progress));
        assert!(event["message"].is_string());
    }
    value
}

#[test]
fn doctor_json_requires_no_media_tools_and_lists_the_conversion_contract() {
    let fixture = Fixture::new();
    let mut command = fixture.cli(false);
    command.args(["doctor", "--json"]);
    let output = output(command);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "file-conversion/v1");
    assert_eq!(
        value["formats"],
        json!([
            "mp4", "mkv", "mov", "webm", "mp3", "m4a", "wav", "flac", "ogg", "png", "jpg", "webp",
            "tiff"
        ])
    );
    let dependencies = value["dependencies"].as_array().unwrap();
    assert_eq!(dependencies.len(), 3);
    for dependency in dependencies {
        assert!(dependency["name"].is_string());
        // Finder-friendly discovery may still find Homebrew or system paths
        // when PATH is empty. Reporting those tools must remain truthful.
        assert_eq!(dependency["available"], dependency["path"].is_string());
        if let Some(path) = dependency["path"].as_str() {
            assert!(Path::new(path).is_absolute());
            assert!(Path::new(path).is_file());
        }
    }
    assert!(value["workflow_dependencies"].is_array());
    assert!(value["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["kind"] == "subtitle_convert"));
}

#[test]
fn invalid_request_is_json_and_does_not_change_inputs_or_existing_outputs() {
    let fixture = Fixture::new();
    let first = fixture.root.join("first 源 'quoted'.png");
    let second = fixture.root.join("second.png");
    let existing = fixture.root.join("existing.png");
    fs::write(&first, b"synthetic source one").unwrap();
    fs::write(&second, b"synthetic source two").unwrap();
    fs::write(&existing, b"existing output must survive").unwrap();
    let original_first = fs::read(&first).unwrap();
    let original_second = fs::read(&second).unwrap();
    let original_output = fs::read(&existing).unwrap();

    let mut multiple = fixture.cli(false);
    multiple
        .arg("convert")
        .args([&first, &second])
        .args(["--format", "png", "--json", "--output"])
        .arg(&existing);
    let response = conversion_response(&output(multiple), false);
    assert!(response["result"].is_null());
    assert!(response["error"].as_str().unwrap().contains("one input"));

    let mut overwrite = fixture.cli(false);
    overwrite
        .arg("convert")
        .arg(&first)
        .args(["--format", "png", "--json", "--output"])
        .arg(&existing);
    let refused = conversion_response(&output(overwrite), false);
    assert!(refused.to_string().contains("already exists"));

    let mut missing = fixture.cli(false);
    missing
        .arg("convert")
        .arg(fixture.root.join("missing.png"))
        .args(["--format", "png", "--json", "--output-dir"])
        .arg(fixture.root.join("new outputs"));
    conversion_response(&output(missing), false);

    assert_eq!(fs::read(first).unwrap(), original_first);
    assert_eq!(fs::read(second).unwrap(), original_second);
    assert_eq!(fs::read(existing).unwrap(), original_output);
}

struct McpClient {
    child: ChildGuard,
    stdin: Option<ChildStdin>,
    messages: Receiver<Result<Value, String>>,
    stderr: Receiver<std::io::Result<Vec<u8>>>,
}

impl McpClient {
    fn new(fixture: &Fixture, real_tools: bool) -> Self {
        let mut command = fixture.cli(real_tools);
        command
            .arg("mcp")
            .arg("--root")
            .arg(&fixture.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = ChildGuard(command.spawn().expect("spawn MCP stdio server"));
        let stdin = Some(child.0.stdin.take().unwrap());
        let stdout = child.0.stdout.take().unwrap();
        let stderr = read_bytes(child.0.stderr.take().unwrap());
        let (sender, messages) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let message = line.map_err(|error| error.to_string()).and_then(|line| {
                    serde_json::from_str(&line)
                        .map_err(|error| format!("Invalid MCP stdout: {error}: {line}"))
                });
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            messages,
            stderr,
        }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *stdin, &message).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    fn receive(&self) -> Value {
        let message = self
            .messages
            .recv_timeout(MESSAGE_TIMEOUT)
            .expect("MCP response timed out or stdout disconnected")
            .expect("MCP stdout must contain JSON-RPC messages only");
        assert_eq!(message["jsonrpc"], "2.0");
        message
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        let response = self.receive();
        assert_eq!(response["id"], id);
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    fn initialize_and_list(&mut self) {
        let initialized = self.request(
            1,
            "initialize",
            json!({
                "protocolVersion":"2025-06-18", "capabilities":{},
                "clientInfo":{"name":"video-toolkit-integration-test","version":"1"}
            }),
        );
        assert_eq!(initialized["protocolVersion"], "2025-06-18");
        assert_eq!(initialized["serverInfo"]["name"], "video-toolkit");
        assert!(initialized["capabilities"]["tools"].is_object());
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        let listed = self.request(2, "tools/list", json!({}));
        let names: Vec<_> = listed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        for expected in [
            "list_formats",
            "dependency_status",
            "convert_files",
            "workflow_tools",
            "run_workflow",
        ] {
            assert!(names.contains(&expected), "missing MCP tool {expected}");
        }
        let formats = self.request(
            3,
            "tools/call",
            json!({"name":"list_formats","arguments":{}}),
        );
        assert_eq!(formats["isError"], false);
        assert!(formats["structuredContent"]["formats"]
            .as_array()
            .unwrap()
            .iter()
            .any(|format| format == "webp"));
        let text: Value =
            serde_json::from_str(formats["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, formats["structuredContent"]);
    }

    fn convert_image(&mut self, id: u64, input: &Path, out: &Path, token: &str) -> Value {
        self.send(json!({
            "jsonrpc":"2.0","id":id,"method":"tools/call","params":{
                "name":"convert_files","arguments":{
                    "inputs":[input],"output_dir":out,"format":"webp"
                },
                "_meta":{"progressToken":token}
            }
        }));
        let deadline = Instant::now() + PROCESS_TIMEOUT;
        let mut progress = Vec::new();
        let result = loop {
            assert!(
                Instant::now() < deadline,
                "MCP conversion exceeded its timeout"
            );
            let message = self.receive();
            if message.get("id").is_some() {
                assert_eq!(message["id"], id);
                assert!(message.get("error").is_none(), "{message}");
                break message["result"].clone();
            }
            assert_eq!(message["method"], "notifications/progress");
            let params = &message["params"];
            assert_eq!(params["progressToken"], token);
            assert_eq!(params["total"], 1);
            let next = params["progress"].as_f64().unwrap();
            assert!((0.0..=1.0).contains(&next));
            if let Some(previous) = progress.last() {
                assert!(
                    next > *previous,
                    "MCP progress must strictly increase: {progress:?}, {next}"
                );
            }
            progress.push(next);
        };
        assert!(!progress.is_empty(), "Requested progress token was ignored");
        assert_eq!(progress.last().copied(), Some(1.0));
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["ok"], true);
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, result["structuredContent"]);
        result
    }

    fn finish(mut self) {
        // EOF is the MCP stdio shutdown signal; the server must exit and reap workers.
        drop(self.stdin.take());
        let status = wait_for_exit(&mut self.child.0);
        let stderr = self
            .stderr
            .recv_timeout(MESSAGE_TIMEOUT)
            .expect("MCP stderr did not close")
            .unwrap();
        assert!(
            status.success(),
            "MCP failed: {}",
            String::from_utf8_lossy(&stderr)
        );
        assert!(
            stderr.is_empty(),
            "MCP diagnostics leaked during valid session"
        );
    }
}

#[test]
fn mcp_stdio_initializes_lists_tools_and_rejects_root_escapes_without_media_tools() {
    let fixture = Fixture::new();
    let outside = Fixture::new();
    let external_file = outside.root.join("private outside image.png");
    fs::write(
        &external_file,
        b"outside source must not be read by conversion",
    )
    .unwrap();
    let local_file = fixture.root.join("local.png");
    fs::write(&local_file, b"local fixture").unwrap();
    let out = fixture.root.join("outputs");
    fs::create_dir(&out).unwrap();
    let mut client = McpClient::new(&fixture, false);
    client.initialize_and_list();
    let response = client.request(
        4,
        "tools/call",
        json!({"name":"convert_files","arguments":{
            "inputs":[external_file],"output_dir":out,"format":"png"
        }}),
    );
    assert_eq!(response["isError"], true);
    assert!(response["structuredContent"]["error"]
        .as_str()
        .unwrap()
        .contains("outside"));

    let response = client.request(
        5,
        "tools/call",
        json!({"name":"convert_files","arguments":{
            "inputs":[local_file],"output_dir":outside.root,"format":"png"
        }}),
    );
    assert_eq!(response["isError"], true);
    assert!(response["structuredContent"]["error"]
        .as_str()
        .unwrap()
        .contains("outside"));

    #[cfg(unix)]
    {
        let source_link = fixture.root.join("source escape.png");
        std::os::unix::fs::symlink(&external_file, &source_link).unwrap();
        let output_link = fixture.root.join("folder escape");
        std::os::unix::fs::symlink(&outside.root, &output_link).unwrap();
        for (id, input, output_directory) in
            [(6, &source_link, &out), (7, &local_file, &output_link)]
        {
            let response = client.request(
                id,
                "tools/call",
                json!({"name":"convert_files","arguments":{
                    "inputs":[input],"output_dir":output_directory,"format":"png"
                }}),
            );
            assert_eq!(response["isError"], true);
            assert!(response["structuredContent"]["error"]
                .as_str()
                .unwrap()
                .contains("outside"));
        }
    }
    client.finish();
    assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
    assert_eq!(fs::read(&local_file).unwrap(), b"local fixture");
    assert_eq!(
        fs::read(&external_file).unwrap(),
        b"outside source must not be read by conversion"
    );
}

fn run_tool(mut command: Command) -> Output {
    command.stdin(Stdio::null());
    let result = output(command);
    assert!(
        result.status.success(),
        "Synthetic fixture or verification failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result
}

fn generate_video(path: &Path) {
    let mut ffmpeg = Command::new("ffmpeg");
    ffmpeg
        .args([
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=64x48:r=25:d=0.4",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=0.4",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-threads",
            "1",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(path);
    run_tool(ffmpeg);
}

fn generate_image(path: &Path) {
    let mut magick = Command::new("magick");
    magick.args(["-size", "8x6", "xc:red"]).arg(path);
    run_tool(magick);
}

fn probe(path: &Path) -> Value {
    let mut ffprobe = Command::new("ffprobe");
    ffprobe
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path);
    serde_json::from_slice(&run_tool(ffprobe).stdout).unwrap()
}

fn assert_decodes(path: &Path) {
    let mut ffmpeg = Command::new("ffmpeg");
    ffmpeg
        .args(["-v", "error", "-xerror", "-nostdin", "-i"])
        .arg(path)
        .args(["-f", "null", "-"]);
    run_tool(ffmpeg);
}

fn assert_webp(path: &Path) {
    let mut magick = Command::new("magick");
    magick.args(["identify", "-format", "%m %w %h"]).arg(path);
    let image = run_tool(magick);
    assert_eq!(String::from_utf8_lossy(&image.stdout), "WEBP 8 6");
}

fn only_output(response: &Value) -> PathBuf {
    let jobs = response["result"]["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["state"], "completed");
    assert!(jobs[0]["error"].is_null());
    PathBuf::from(jobs[0]["output"].as_str().unwrap())
}

#[test]
#[ignore = "requires FFmpeg/ffprobe and ImageMagick; real synthetic CLI conversions"]
fn imagemagick_real_cli_converts_video_audio_images_and_continues_after_errors() {
    let fixture = Fixture::new();
    let video = fixture.root.join("0.4 秒 synthetic 'video'.mp4");
    let image = fixture.root.join("图片 source 'quoted'.png");
    generate_video(&video);
    generate_image(&image);
    let original_video = fs::read(&video).unwrap();
    let original_image = fs::read(&image).unwrap();

    let mkv = fixture.root.join("verified 视频 'result'.mkv");
    let response =
        conversion_response(&output(fixture.convert(&[&video], "mkv", &mkv, true)), true);
    assert_eq!(only_output(&response), mkv);
    let video_probe = probe(&mkv);
    assert!(video_probe["streams"]
        .as_array()
        .unwrap()
        .iter()
        .any(|stream| stream["codec_type"] == "video"));
    assert_decodes(&mkv);
    let original_output = fs::read(&mkv).unwrap();
    conversion_response(
        &output(fixture.convert(&[&video], "mkv", &mkv, true)),
        false,
    );
    assert_eq!(fs::read(&mkv).unwrap(), original_output);

    let audio = fixture.root.join("extracted audio.mp3");
    let response = conversion_response(
        &output(fixture.convert(&[&video], "mp3", &audio, true)),
        true,
    );
    assert_eq!(only_output(&response), audio);
    let audio_probe = probe(&audio);
    let streams = audio_probe["streams"].as_array().unwrap();
    assert!(streams.iter().any(|stream| stream["codec_name"] == "mp3"));
    assert!(streams.iter().all(|stream| stream["codec_type"] == "audio"));
    assert_decodes(&audio);

    let out = fixture.root.join("converted 图片");
    let response = conversion_response(
        &output(fixture.convert(&[&image], "webp", &out, false)),
        true,
    );
    let webp = only_output(&response);
    assert_eq!(webp.parent(), Some(out.as_path()));
    assert_webp(&webp);

    let corrupt = fixture.root.join("corrupt.png");
    fs::write(&corrupt, b"intentionally invalid raster fixture").unwrap();
    let batch_out = fixture.root.join("continue after failure");
    let batch = conversion_response(
        &output(fixture.convert(&[&corrupt, &image], "webp", &batch_out, false)),
        false,
    );
    let jobs = batch["result"]["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0]["state"], "failed");
    assert!(jobs[0]["error"].is_string());
    assert!(!Path::new(jobs[0]["output"].as_str().unwrap()).exists());
    assert_eq!(jobs[1]["state"], "completed");
    assert_webp(Path::new(jobs[1]["output"].as_str().unwrap()));
    assert_eq!(fs::read(&video).unwrap(), original_video);
    assert_eq!(fs::read(&image).unwrap(), original_image);
    assert_eq!(fs::read(&mkv).unwrap(), original_output);
    assert_eq!(
        fs::read(&corrupt).unwrap(),
        b"intentionally invalid raster fixture"
    );
}

#[test]
#[ignore = "requires ImageMagick; real MCP stdio image conversion and progress"]
fn imagemagick_real_mcp_converts_twice_with_increasing_progress_and_verified_results() {
    let fixture = Fixture::new();
    let image = fixture.root.join("MCP 图片 with 'quotes'.png");
    let out = fixture.root.join("MCP outputs");
    fs::create_dir(&out).unwrap();
    generate_image(&image);
    let original_image = fs::read(&image).unwrap();
    let mut client = McpClient::new(&fixture, true);
    client.initialize_and_list();
    let result = client.convert_image(10, &image, &out, "synthetic-conversion-progress");
    let report = &result["structuredContent"]["report"];
    let jobs = report["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["state"], "completed");
    assert!(jobs[0]["error"].is_null());
    let converted = PathBuf::from(jobs[0]["output"].as_str().unwrap());
    assert_eq!(converted.parent(), Some(out.as_path()));
    assert_webp(&converted);
    let original_output = fs::read(&converted).unwrap();

    // Immediately queue another request after the result. A completed worker
    // must release the MCP task gate before a client can observe that result.
    let second_out = fixture.root.join("second MCP outputs");
    fs::create_dir(&second_out).unwrap();
    let second = client.convert_image(11, &image, &second_out, "second-conversion-progress");
    let second_jobs = second["structuredContent"]["report"]["jobs"]
        .as_array()
        .unwrap();
    assert_eq!(second_jobs.len(), 1);
    assert_eq!(second_jobs[0]["state"], "completed");
    let second_converted = PathBuf::from(second_jobs[0]["output"].as_str().unwrap());
    assert_eq!(second_converted.parent(), Some(second_out.as_path()));
    assert_webp(&second_converted);
    assert_eq!(fs::read(&converted).unwrap(), original_output);
    client.finish();
    assert_eq!(fs::read(&image).unwrap(), original_image);
}

const SUBTITLE_FIXTURE: &str = "1\n00:00:01,000 --> 00:00:02,000\n字幕 \"quoted\"\nSecond line\n\n";

#[test]
fn workflow_cli_discovers_runs_subtitles_and_returns_structured_errors() {
    let fixture = Fixture::new();
    let source = fixture.root.join("captions 字幕.srt");
    fs::write(&source, SUBTITLE_FIXTURE).unwrap();
    let mut list = fixture.cli(false);
    list.args(["workflows", "--json"]);
    let listed = output(list);
    assert!(listed.status.success());
    let tools: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(tools["schema"], "file-workflows/v1");
    assert!(tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["kind"] == "subtitle_shift"));
    let out = fixture.root.join("subtitle outputs");
    let mut run = fixture.cli(false);
    run.args(["workflow", "subtitle_convert"])
        .arg(&source)
        .arg("--output-dir")
        .arg(&out)
        .args(["--options", r#"{"format":"vtt"}"#, "--json"]);
    let result = output(run);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["schema"], "file-workflows/v1");
    assert_eq!(value["ok"], true);
    let saved = PathBuf::from(value["result"]["jobs"][0]["outputs"][0].as_str().unwrap());
    assert!(fs::read_to_string(&saved).unwrap().contains("WEBVTT"));
    let mut last = 0.0;
    for line in String::from_utf8_lossy(&result.stderr).lines() {
        let event: Value = serde_json::from_str(line).unwrap();
        let progress = event["progress"].as_f64().unwrap();
        assert!(progress >= last);
        last = progress;
    }
    assert_eq!(last, 1.0);
    let original = fs::read(&saved).unwrap();
    let mut repeated = fixture.cli(false);
    repeated
        .args(["workflow", "subtitle_convert"])
        .arg(&source)
        .arg("--output-dir")
        .arg(&out)
        .args(["--options", r#"{"format":"vtt"}"#, "--json"]);
    let rejected = output(repeated);
    assert!(!rejected.status.success());
    let error: Value = serde_json::from_slice(&rejected.stdout).unwrap();
    assert_eq!(error["ok"], false);
    assert_eq!(fs::read(&saved).unwrap(), original);
    assert_eq!(fs::read_to_string(&source).unwrap(), SUBTITLE_FIXTURE);
    let mut parse = fixture.cli(false);
    parse.args(["workflow", "not-a-workflow", "--json"]);
    let parsed = output(parse);
    let error: Value = serde_json::from_slice(&parsed.stdout).unwrap();
    assert_eq!(error["schema"], "file-workflows/v1");
    assert_eq!(error["ok"], false);
}

#[test]
fn mcp_subtitle_workflow_is_discoverable_async_and_reports_strict_progress() {
    let fixture = Fixture::new();
    let source = fixture.root.join("captions.srt");
    let out = fixture.root.join("out");
    fs::write(&source, SUBTITLE_FIXTURE).unwrap();
    fs::create_dir(&out).unwrap();
    let mut client = McpClient::new(&fixture, false);
    client.initialize_and_list();
    let discovery = client.request(
        4,
        "tools/call",
        json!({"name":"workflow_tools","arguments":{}}),
    );
    assert_eq!(discovery["isError"], false);
    assert!(discovery["structuredContent"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["kind"] == "subtitle_convert"));
    client.send(json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{
        "name":"run_workflow","arguments":{"kind":"subtitle_convert","inputs":[source],"output_dir":out,"options":{"format":"vtt"}},
        "_meta":{"progressToken":"subtitle-progress"}
    }}));
    let mut previous = None;
    let response = loop {
        let message = client.receive();
        if message.get("id").is_some() {
            assert_eq!(message["id"], 5);
            break message["result"].clone();
        }
        assert_eq!(message["method"], "notifications/progress");
        assert_eq!(message["params"]["progressToken"], "subtitle-progress");
        let progress = message["params"]["progress"].as_f64().unwrap();
        if let Some(previous) = previous {
            assert!(progress > previous);
        }
        previous = Some(progress);
    };
    assert_eq!(previous, Some(1.0));
    assert_eq!(response["isError"], false);
    assert_eq!(response["structuredContent"]["ok"], true);
    let saved = PathBuf::from(
        response["structuredContent"]["report"]["jobs"][0]["outputs"][0]
            .as_str()
            .unwrap(),
    );
    assert!(fs::read_to_string(saved)
        .unwrap()
        .contains("字幕 \"quoted\"\nSecond line"));
    client.finish();
    assert_eq!(fs::read_to_string(source).unwrap(), SUBTITLE_FIXTURE);
}

#[test]
fn mcp_workflow_rejects_external_paths_and_requires_explicit_download_url() {
    let fixture = Fixture::new();
    let outside = Fixture::new();
    let source = outside.root.join("outside.srt");
    fs::write(&source, SUBTITLE_FIXTURE).unwrap();
    let out = fixture.root.join("out");
    fs::create_dir(&out).unwrap();
    let mut client = McpClient::new(&fixture, false);
    client.initialize_and_list();
    for (id, arguments) in [
        (
            4,
            json!({"kind":"subtitle_convert","inputs":[source],"output_dir":out,"options":{"format":"vtt"}}),
        ),
        (
            5,
            json!({"kind":"subtitle_convert","inputs":[],"output_dir":outside.root,"options":{"format":"vtt"}}),
        ),
        (6, json!({"kind":"download","output_dir":out,"options":{}})),
    ] {
        let response = client.request(
            id,
            "tools/call",
            json!({"name":"run_workflow","arguments":arguments}),
        );
        assert_eq!(response["isError"], true, "{response}");
    }
    client.finish();
    assert_eq!(fs::read_dir(out).unwrap().count(), 0);
    assert_eq!(fs::read_to_string(source).unwrap(), SUBTITLE_FIXTURE);
}

#[test]
fn mcp_subtitle_workflow_can_be_cancelled_without_publishing() {
    let fixture = Fixture::new();
    let source = fixture.root.join("large.srt");
    let out = fixture.root.join("out");
    fs::create_dir(&out).unwrap();
    let mut text = String::new();
    for index in 0..40_000 {
        let seconds = index * 2;
        text.push_str(&format!(
            "{}\n{:02}:{:02}:{:02},000 --> {:02}:{:02}:{:02},500\nCaption {}\n\n",
            index + 1,
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60,
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60,
            index
        ));
    }
    fs::write(&source, &text).unwrap();
    let mut client = McpClient::new(&fixture, false);
    client.initialize_and_list();
    client.send(json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
        "name":"run_workflow","arguments":{"kind":"subtitle_convert","inputs":[source],"output_dir":out,"options":{"format":"vtt"}},
        "_meta":{"progressToken":"cancel-subtitle"}
    }}));
    let first = client.receive();
    assert_eq!(first["method"], "notifications/progress", "{first}");
    client
        .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":4}}));
    client.send(json!({"jsonrpc":"2.0","id":5,"method":"ping"}));
    loop {
        let message = client.receive();
        if message.get("id").is_some() {
            assert_eq!(
                message["id"], 5,
                "Cancelled workflow must not return a result"
            );
            break;
        }
    }
    client.finish();
    assert_eq!(fs::read_dir(out).unwrap().count(), 0);
    assert_eq!(fs::read_to_string(source).unwrap(), text);
}
