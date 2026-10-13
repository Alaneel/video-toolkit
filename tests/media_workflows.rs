//! End-to-end media checks run separately from the dependency-free test suite.
//!
//! Run with `cargo test --test media_workflows -- --ignored` after installing
//! FFmpeg and ffprobe with libx264, libvpx-vp9, AAC, and libopus support.

use common::formats::VideoFormat;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "video-toolkit-media-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create isolated media test directory");
        Self { path }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct Fixtures {
    directory: TestDirectory,
    video: PathBuf,
    second_video: PathBuf,
    audio: PathBuf,
}

impl Fixtures {
    fn new() -> Self {
        Self::with_video_source("color=c=navy:s=1920x1080:r=10:d=0.4")
    }

    fn low_resolution() -> Self {
        Self::with_video_source("color=c=navy:s=160x96:r=10:d=0.1")
    }

    fn with_video_source(video_source: &str) -> Self {
        let directory = TestDirectory::new();
        let video = directory.path.join("first input with spaces.mp4");
        let second_video = directory.path.join("second input with spaces.mp4");
        let audio = directory.path.join("replacement audio with spaces.wav");

        // Four static frames keep even the full-height slicing checks cheap.
        let mut generate_video = Command::new("ffmpeg");
        generate_video.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            video_source,
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=0.4",
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
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
            "-ac",
            "6",
        ]);
        generate_video.arg(&video);
        run_success(&mut generate_video);
        fs::copy(&video, &second_video).expect("copy the second batch input");

        let mut generate_audio = Command::new("ffmpeg");
        generate_audio.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=880:sample_rate=48000:duration=0.4",
            "-ac",
            "2",
            "-c:a",
            "pcm_s16le",
        ]);
        generate_audio.arg(&audio);
        run_success(&mut generate_audio);

        // Automatic FFmpeg selection prefers the original six-channel track.
        // Replacing it with stereo therefore proves explicit stream selection.
        assert_eq!(stream(&probe(&video), "audio")["channels"], 6);
        assert_eq!(stream(&probe(&audio), "audio")["channels"], 2);

        Self {
            directory,
            video,
            second_video,
            audio,
        }
    }

    fn output(&self, name: &str) -> PathBuf {
        self.directory.path.join(name)
    }

    fn cli(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_video-toolkit"));
        command.current_dir(&self.directory.path);
        command
    }
}

fn run(command: &mut Command) -> Output {
    command.stdin(Stdio::null());
    command
        .output()
        .unwrap_or_else(|error| panic!("could not run {command:?}: {error}"))
}

fn run_success(command: &mut Command) -> Output {
    let output = run(command);
    assert!(
        output.status.success(),
        "command failed: {command:?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn probe(path: &Path) -> Value {
    let mut command = Command::new("ffprobe");
    command.args([
        "-v",
        "error",
        "-show_entries",
        "stream=codec_type,codec_name,width,height,channels:format=format_name",
        "-of",
        "json",
    ]);
    command.arg(path);
    serde_json::from_slice(&run_success(&mut command).stdout).expect("valid ffprobe JSON")
}

fn stream<'a>(metadata: &'a Value, kind: &str) -> &'a Value {
    metadata["streams"]
        .as_array()
        .expect("ffprobe stream list")
        .iter()
        .find(|stream| stream["codec_type"] == kind)
        .unwrap_or_else(|| panic!("missing {kind} stream in {metadata}"))
}

fn output_files(directory: &Path, extension: &str, count: usize) -> Vec<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read output directory {}: {error}", directory.display()))
        .map(|entry| entry.expect("read output entry").path())
        .collect();
    files.sort();
    assert_eq!(files.len(), count, "unexpected outputs: {files:?}");
    for path in &files {
        assert_eq!(
            path.extension().and_then(|extension| extension.to_str()),
            Some(extension),
            "incorrect output extension: {}",
            path.display()
        );
        assert!(
            fs::metadata(path).expect("output metadata").len() > 0,
            "empty output: {}",
            path.display()
        );
    }
    files
}

fn assert_codecs(metadata: &Value, video_codec: &str, audio_codec: &str) {
    assert_eq!(stream(metadata, "video")["codec_name"], video_codec);
    assert_eq!(stream(metadata, "audio")["codec_name"], audio_codec);
    assert!(
        metadata["format"]["format_name"]
            .as_str()
            .expect("container name")
            .contains("matroska"),
        "expected a Matroska/WebM container: {metadata}"
    );
}

fn assert_decodable(path: &Path) {
    let mut decode = Command::new("ffmpeg");
    decode
        .args(["-hide_banner", "-loglevel", "error", "-xerror", "-i"])
        .arg(path)
        .args(["-t", "0.1", "-f", "null", "-"]);
    run_success(&mut decode);
}

#[test]
#[ignore = "requires FFmpeg and ffprobe"]
fn clipper_and_batch_clipper_honor_output_formats() {
    let fixtures = Fixtures::new();
    let output = fixtures.output("webm clips with spaces");
    let mut command = fixtures.cli();
    command.arg("clipper").arg(&fixtures.video).args([
        "--ranges",
        "0-0.2",
        "0.2-0.4",
        "--format",
        "webm",
        "--output-dir",
    ]);
    command.arg(&output);
    run_success(&mut command);
    for path in output_files(&output, "webm", 2) {
        assert_codecs(&probe(&path), "vp9", "opus");
    }

    let chained_output = fixtures.output("batch clips from exported webm");
    let mut command = fixtures.cli();
    command
        .args(["batch", "clipper"])
        .arg(&output)
        .args(["--ranges", "0-0.1", "--format", "mp4", "--output-dir"])
        .arg(&chained_output);
    run_success(&mut command);
    for path in output_files(&chained_output, "mp4", 2) {
        let metadata = probe(&path);
        assert_eq!(stream(&metadata, "video")["codec_name"], "h264");
        assert_eq!(stream(&metadata, "audio")["codec_name"], "aac");
    }

    let batch_output = fixtures.output("batch mkv clips with spaces");
    let mut command = fixtures.cli();
    command
        .args(["batch", "clipper"])
        .arg(&fixtures.video)
        .arg(&fixtures.second_video)
        .args(["--ranges", "0-0.2", "--format", "mkv", "--output-dir"])
        .arg(&batch_output);
    run_success(&mut command);
    for path in output_files(&batch_output, "mkv", 2) {
        assert_codecs(&probe(&path), "h264", "aac");
    }
}

#[test]
#[ignore = "requires FFmpeg and ffprobe"]
fn splitter_and_batch_splitter_honor_output_formats() {
    let fixtures = Fixtures::new();
    let output = fixtures.output("webm slices with spaces");
    let mut command = fixtures.cli();
    command
        .arg("splitter")
        .arg(&fixtures.video)
        .args([
            "--format",
            "webm",
            "--custom-encode",
            "-c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 100k -threads 1 -c:a libopus",
            "--output-dir",
        ])
        .arg(&output);
    run_success(&mut command);
    for path in output_files(&output, "webm", 5) {
        let metadata = probe(&path);
        assert_codecs(&metadata, "vp9", "opus");
        assert_eq!(stream(&metadata, "video")["width"], 384);
        assert_eq!(stream(&metadata, "video")["height"], 1080);
    }

    let batch_output = fixtures.output("batch mkv slices with spaces");
    let mut command = fixtures.cli();
    command
        .args(["batch", "splitter"])
        .arg(&fixtures.video)
        .arg(&fixtures.second_video)
        .args([
            "--format",
            "mkv",
            "--custom-encode",
            "-c:v libx264 -preset ultrafast -threads 1 -c:a aac",
            "--output-dir",
        ])
        .arg(&batch_output);
    run_success(&mut command);
    let paths = output_files(&batch_output, "mkv", 10);
    for input in [&fixtures.video, &fixtures.second_video] {
        let stem = input.file_stem().unwrap().to_string_lossy();
        for number in 1..=5 {
            assert!(
                paths.contains(&batch_output.join(format!("{stem}_slice_{number}.mkv"))),
                "missing a separately named slice for {}",
                input.display()
            );
        }
    }
    for path in paths {
        let metadata = probe(&path);
        assert_codecs(&metadata, "h264", "aac");
        assert_eq!(stream(&metadata, "video")["width"], 384);
        assert_eq!(stream(&metadata, "video")["height"], 1080);
    }
}

#[test]
#[ignore = "requires FFmpeg and ffprobe"]
fn every_advertised_video_format_produces_decodable_video_and_audio() {
    // One small video frame is enough to exercise muxer/encoder compatibility
    // without multiplying the cost of the full-resolution workflow tests.
    let fixtures = Fixtures::low_resolution();
    for format in VideoFormat::all() {
        let output = fixtures.output(&format!("all formats.{}", format.extension()));
        let mut command = fixtures.cli();
        command
            .arg("merger")
            .arg(&fixtures.video)
            .arg(&fixtures.audio)
            .arg("--format")
            .arg(format.extension())
            .arg("--output")
            .arg(&output);
        run_success(&mut command);
        let metadata = probe(&output);
        assert_eq!(stream(&metadata, "video")["width"], 160);
        assert_eq!(stream(&metadata, "video")["height"], 96);
        assert_eq!(stream(&metadata, "audio")["channels"], 2);

        match format {
            VideoFormat::MPEG => {
                assert_eq!(stream(&metadata, "video")["codec_name"], "mpeg2video");
                assert_eq!(stream(&metadata, "audio")["codec_name"], "mp2");
            }
            VideoFormat::VOB => {
                assert_eq!(stream(&metadata, "video")["codec_name"], "mpeg2video");
                assert_eq!(stream(&metadata, "audio")["codec_name"], "ac3");
            }
            VideoFormat::M4V => {
                assert_eq!(stream(&metadata, "video")["codec_name"], "h264");
                assert_eq!(stream(&metadata, "audio")["codec_name"], "aac");
                assert!(
                    metadata["format"]["format_name"]
                        .as_str()
                        .expect("M4V container name")
                        .contains("mov"),
                    "M4V must use a container that retains the audio stream: {metadata}"
                );
            }
            VideoFormat::WebM => assert_codecs(&metadata, "vp9", "opus"),
            VideoFormat::MKV => assert_codecs(&metadata, "h264", "aac"),
            _ => {}
        }

        assert_decodable(&output);

        // The source has six-channel audio and a 10 fps video frame. Legacy
        // encoders must receive compatible channel counts and frame rates.
        let clips = fixtures.output(&format!("direct six-channel clips {}", format.extension()));
        let mut command = fixtures.cli();
        command
            .arg("clipper")
            .arg(&fixtures.video)
            .args(["--ranges", "0-0.1", "--format"])
            .arg(format.extension())
            .arg("--output-dir")
            .arg(&clips);
        run_success(&mut command);
        for path in output_files(&clips, format.extension(), 1) {
            let metadata = probe(&path);
            assert_eq!(stream(&metadata, "video")["width"], 160);
            assert_eq!(stream(&metadata, "video")["height"], 96);
            let expected_channels = if matches!(
                format,
                VideoFormat::AVI | VideoFormat::WMV | VideoFormat::MPEG | VideoFormat::VOB
            ) {
                2
            } else {
                6
            };
            assert_eq!(stream(&metadata, "audio")["channels"], expected_channels);
            assert_decodable(&path);
        }
    }
}

#[test]
#[ignore = "requires FFmpeg and ffprobe"]
fn merger_and_batch_merger_use_the_replacement_audio_and_requested_container() {
    let fixtures = Fixtures::new();
    for (requested_name, explicit_format, actual_name) in [
        ("inferred format.webm", None, "inferred format.webm"),
        ("explicit format.webm", Some("webm"), "explicit format.webm"),
        ("default no extension", None, "default no extension.mp4"),
        (
            "explicit no extension",
            Some("webm"),
            "explicit no extension.webm",
        ),
    ] {
        let output = fixtures.output(requested_name);
        let mut command = fixtures.cli();
        command
            .arg("merger")
            .arg(&fixtures.video)
            .arg(&fixtures.audio)
            .arg("--output")
            .arg(&output)
            .arg("--shortest");
        if let Some(format) = explicit_format {
            command.args(["--format", format]);
        }
        run_success(&mut command);
        let actual_output = fixtures.output(actual_name);
        let metadata = probe(&actual_output);
        if actual_output.extension().unwrap() == "mp4" {
            assert_eq!(stream(&metadata, "video")["codec_name"], "h264");
            assert_eq!(stream(&metadata, "audio")["codec_name"], "aac");
        } else {
            assert_codecs(&metadata, "vp9", "opus");
        }
        assert_eq!(stream(&metadata, "audio")["channels"], 2);
        if requested_name != actual_name {
            assert!(!output.exists(), "created an extensionless media file");
        }
    }

    // An audio source may itself be a video, as in the desktop audio picker.
    let embedded_audio = fixtures.output("soundtrack embedded in a video.mkv");
    let mut generate_embedded_audio = Command::new("ffmpeg");
    generate_embedded_audio
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&fixtures.video)
        .arg("-i")
        .arg(&fixtures.audio)
        .args([
            "-map", "0:v:0", "-map", "1:a:0", "-c:v", "copy", "-c:a", "aac",
        ])
        .arg(&embedded_audio);
    run_success(&mut generate_embedded_audio);
    let embedded_output = fixtures.output("merged from embedded audio.mp4");
    let mut command = fixtures.cli();
    command
        .arg("merger")
        .arg(&fixtures.video)
        .arg(&embedded_audio)
        .arg("--output")
        .arg(&embedded_output);
    run_success(&mut command);
    let metadata = probe(&embedded_output);
    assert_eq!(stream(&metadata, "video")["codec_name"], "h264");
    assert_eq!(stream(&metadata, "audio")["codec_name"], "aac");
    assert_eq!(stream(&metadata, "audio")["channels"], 2);
    assert_eq!(metadata["streams"].as_array().unwrap().len(), 2);

    let batch_output = fixtures.output("batch merged mkv with spaces");
    let mut command = fixtures.cli();
    command
        .args(["batch", "merger"])
        .arg(&fixtures.video)
        .arg(&fixtures.second_video)
        .arg("--audio")
        .arg(&fixtures.audio)
        .args(["--format", "mkv", "--shortest", "--output-dir"])
        .arg(&batch_output);
    run_success(&mut command);
    for path in output_files(&batch_output, "mkv", 2) {
        let metadata = probe(&path);
        assert_codecs(&metadata, "h264", "aac");
        assert_eq!(stream(&metadata, "audio")["channels"], 2);
    }
}

#[test]
#[ignore = "requires FFmpeg and ffprobe"]
fn invalid_formats_and_merger_extension_mismatches_create_no_output() {
    let fixtures = Fixtures::new();
    for operation in ["clipper", "splitter", "merger"] {
        for batch in [false, true] {
            let output = fixtures.output(&format!("invalid-{operation}-{batch}"));
            let mut command = fixtures.cli();
            if batch {
                command.arg("batch");
            }
            command.arg(operation).arg(&fixtures.video);
            match operation {
                "clipper" => {
                    command.args(["--ranges", "0-0.2"]);
                }
                "merger" if batch => {
                    command.arg("--audio").arg(&fixtures.audio);
                }
                "merger" => {
                    command.arg(&fixtures.audio);
                }
                _ => {}
            }
            command
                .args(["--format", "unsupported-format"])
                .arg(if operation == "merger" && !batch {
                    "--output"
                } else {
                    "--output-dir"
                })
                .arg(&output);
            let result = run(&mut command);
            assert!(!result.status.success(), "accepted an unsupported format");
            let error = String::from_utf8_lossy(&result.stderr);
            assert!(
                error.contains("unsupported-format") && error.contains("format"),
                "unexpected rejection: {error}"
            );
            assert!(!output.exists(), "created output for an invalid format");
        }
    }

    let mismatch = fixtures.output("mismatched container.mp4");
    let mut command = fixtures.cli();
    command
        .arg("merger")
        .arg(&fixtures.video)
        .arg(&fixtures.audio)
        .args(["--format", "webm", "--output"])
        .arg(&mismatch);
    let result = run(&mut command);
    assert!(
        !result.status.success(),
        "accepted a WebM format with an MP4 output extension"
    );
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(
        error.contains("webm") && error.contains("mp4"),
        "unexpected mismatch error: {error}"
    );
    assert!(!mismatch.exists(), "created output for mismatched formats");

    for batch in [false, true] {
        let output = fixtures.output(&format!("invalid-time-ranges-{batch}"));
        let mut command = fixtures.cli();
        if batch {
            command.arg("batch");
        }
        command
            .arg("clipper")
            .arg(&fixtures.video)
            .args([
                "--ranges",
                "0-0.2",
                "not-a-time-range",
                "--format",
                "webm",
                "--output-dir",
            ])
            .arg(&output);
        let result = run(&mut command);
        assert!(
            !result.status.success(),
            "accepted a mixture of valid and invalid clip ranges"
        );
        assert!(
            !output.exists(),
            "partially processed mixed valid and invalid clip ranges"
        );
    }
}
