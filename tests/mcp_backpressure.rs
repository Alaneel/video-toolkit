//! Synthetic stdio transport regressions. No dependency tools or network calls.
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

enum ReadRequest {
    Line,
    One,
    Drain,
}

struct Server {
    child: Child,
    input: Option<ChildStdin>,
    reads: Option<Sender<ReadRequest>>,
    output: Receiver<Vec<u8>>,
    reader: Option<JoinHandle<()>>,
}

impl Server {
    fn new(root: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_video-toolkit"))
            .args(["mcp", "--root"])
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (reads, commands) = mpsc::channel();
        let (received, output) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            while let Ok(command) = commands.recv() {
                let mut bytes = Vec::new();
                let read = match command {
                    ReadRequest::Line => stdout.read_until(b'\n', &mut bytes),
                    ReadRequest::One => {
                        let mut first = [0];
                        stdout.read_exact(&mut first).map(|()| {
                            bytes.push(first[0]);
                            1
                        })
                    }
                    ReadRequest::Drain => stdout.read_to_end(&mut bytes),
                };
                if read.is_err() || received.send(bytes).is_err() {
                    break;
                }
            }
        });
        let mut server = Self {
            child,
            input,
            reads: Some(reads),
            output,
            reader: Some(reader),
        };
        server.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2025-06-18"}}));
        assert_eq!(server.line()["id"], 1);
        server.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        server
    }

    fn send(&mut self, value: Value) {
        let input = self.input.as_mut().unwrap();
        serde_json::to_writer(&mut *input, &value).unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
    }

    fn read(&self, request: ReadRequest) -> Vec<u8> {
        self.reads.as_ref().unwrap().send(request).unwrap();
        self.output.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    fn line(&self) -> Value {
        serde_json::from_slice(&self.read(ReadRequest::Line)).unwrap()
    }

    fn wait(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        drop(self.input.take());
        // Release stdout backpressure before cleanup, including on a failed
        // assertion against an older server. No media child is ever spawned.
        if let Some(reads) = self.reads.take() {
            let _ = reads.send(ReadRequest::Drain);
            drop(reads);
        }
        if self.wait(Duration::from_secs(3)).is_none() {
            // Last-resort cleanup targets only the exact private child handle,
            // never another process or a negative process-group identifier.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, Vec<PathBuf>) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let out = root.join("out");
    fs::create_dir(&out).unwrap();
    let inputs = (0..256)
        .map(|index| {
            let path = root.join(format!("{}-{index:03}.bin", "x".repeat(96)));
            fs::write(&path, []).unwrap();
            path
        })
        .collect();
    (directory, root, out, inputs)
}

fn request(inputs: &[PathBuf], out: &std::path::Path) -> Value {
    json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
        "name":"convert_files","arguments":{"inputs":inputs,"output_dir":out,"format":"png"}}})
}

#[test]
fn mcp_input_eof_exits_within_three_seconds_when_client_stops_reading_stdout() {
    let (_directory, root, out, inputs) = fixture();
    let mut server = Server::new(&root);
    server.send(request(&inputs, &out));
    // Observe the beginning of the large final response, then deliberately
    // stop reading. This cannot be a cancellation before admission/rendering.
    assert_eq!(server.read(ReadRequest::One), b"{");
    thread::sleep(Duration::from_millis(100));
    drop(server.input.take());
    let status = server.wait(Duration::from_secs(3));
    assert!(
        status.is_some(),
        "MCP EOF cleanup must not join a file worker blocked on stdout"
    );
    assert!(status.unwrap().success());
    assert_eq!(fs::read_dir(out).unwrap().count(), 0);
    for input in inputs {
        assert_eq!(fs::metadata(input).unwrap().len(), 0);
    }
}

#[test]
fn mcp_output_timeout_exits_within_four_seconds_even_with_stdin_open_and_idle() {
    let (_directory, root, out, inputs) = fixture();
    let mut server = Server::new(&root);
    server.send(request(&inputs, &out));
    assert_eq!(server.read(ReadRequest::One), b"{");
    // Keep stdin open, send no next request, and stop reading stdout. The
    // asynchronous response's failed ACK must wake the session coordinator.
    let status = server.wait(Duration::from_secs(4));
    assert!(
        status.is_some(),
        "MCP output delivery failure must end even an idle-input session"
    );
    assert!(!status.unwrap().success());
    assert!(server.input.is_some());
    assert_eq!(fs::read_dir(out).unwrap().count(), 0);
    for input in inputs {
        assert_eq!(fs::metadata(input).unwrap().len(), 0);
    }
}

#[test]
fn mcp_large_response_is_complete_when_stdout_is_promptly_drained() {
    let (_directory, root, out, inputs) = fixture();
    let mut server = Server::new(&root);
    server.send(request(&inputs, &out));
    let result = server.line();
    assert_eq!(result["id"], 2);
    assert_eq!(result["result"]["isError"], true);
    let structured = &result["result"]["structuredContent"];
    assert_eq!(structured["ok"], false);
    let jobs = structured["report"]["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 256);
    assert!(jobs.iter().all(|job| job["state"] == "failed"));
    let text: Value =
        serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(&text, structured);
    server.send(json!({"jsonrpc":"2.0","id":3,"method":"ping"}));
    assert_eq!(server.line()["id"], 3);
    drop(server.input.take());
    assert!(server.wait(Duration::from_secs(3)).unwrap().success());
    assert_eq!(fs::read_dir(out).unwrap().count(), 0);
}
