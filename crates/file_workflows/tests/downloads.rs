use file_workflows::{execute, run_queue, CancelToken, JobState, WorkflowKind, WorkflowRequest};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

fn request(root: &Path, url: &str) -> WorkflowRequest {
    WorkflowRequest {
        kind: WorkflowKind::Download,
        inputs: vec![],
        output_dir: root.join("out"),
        options: json!({"url":url,"filename":"fixture.bin"}),
    }
}
struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}
fn server(body: Vec<u8>, slow: bool, redirect: bool) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/fixture.wav", listener.local_addr().unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let done = stop.clone();
    let worker = std::thread::spawn(move || {
        let started = Instant::now();
        while !done.load(Ordering::Acquire) && started.elapsed() < Duration::from_secs(30) {
            let (mut stream, _) = match listener.accept() {
                Ok(p) => p,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut buf = [0; 8192];
            let count = stream.read(&mut buf).unwrap_or(0);
            let input = String::from_utf8_lossy(&buf[..count]);
            if redirect && input.starts_with("GET /fixture.wav ") {
                let _=write!(stream,"HTTP/1.1 302 Found\r\nLocation: /actual\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                continue;
            }
            let content_type = if input.contains("fixture.mp4") {
                "video/mp4"
            } else {
                "audio/wav"
            };
            let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
            if input.starts_with("HEAD ") {
                continue;
            }
            for chunk in body.chunks(if slow { 64 } else { 8192 }) {
                if done.load(Ordering::Acquire) || stream.write_all(chunk).is_err() {
                    break;
                }
                if slow {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    });
    Server {
        url,
        stop,
        worker: Some(worker),
    }
}

#[test]
fn rejects_non_http_credentials_invalid_names_and_unknown_options() {
    let root = tempfile::tempdir().unwrap();
    for options in [
        json!({"url":"file:///etc/passwd"}),
        json!({"url":"ftp://example.com/a"}),
        json!({"url":"https://user:pass@example.com/a"}),
        json!({"url":"https://example.com","filename":"../escape"}),
        json!({"url":"https://example.com","shell":"touch"}),
        json!({"url":"https://example.com","mode":"direct","format":"mp3"}),
    ] {
        let mut req = request(root.path(), "https://example.com");
        req.options = options;
        assert!(execute(&req, &CancelToken::default(), |_| {}).is_err());
        assert_eq!(
            std::fs::read_dir(root.path().join("out")).unwrap().count(),
            0
        );
    }
}

#[test]
#[ignore = "requires curl and local HTTP sockets; run explicitly with --ignored"]
fn direct_download_follows_redirects_preserves_bytes_and_never_overwrites() {
    let data = b"local download fixture\n".repeat(2000);
    let server = server(data.clone(), false, true);
    let root = tempfile::tempdir().unwrap();
    let req = request(root.path(), &server.url);
    let mut progress = vec![];
    let result = execute(&req, &CancelToken::default(), |event| {
        progress.push(event.fraction)
    })
    .unwrap();
    assert_eq!(std::fs::read(&result.outputs[0]).unwrap(), data);
    assert_eq!(progress.last(), Some(&1.0));
    assert!(progress.windows(2).all(|p| p[0] <= p[1]));
    assert!(execute(&req, &CancelToken::default(), |_| {})
        .unwrap_err()
        .to_string()
        .contains("already exists"));
    assert_eq!(std::fs::read(&result.outputs[0]).unwrap(), data);
    assert_eq!(std::fs::read_dir(req.output_dir).unwrap().count(), 1);
    let mut reserved = request(root.path(), &server.url);
    reserved.output_dir = root.path().join("reserved");
    reserved.options["filename"] = json!(".download-headers");
    let result = execute(&reserved, &CancelToken::default(), |_| {}).unwrap();
    assert_eq!(std::fs::read(&result.outputs[0]).unwrap(), data);
}

#[test]
#[ignore = "requires curl and local HTTP sockets; run explicitly with --ignored"]
fn cancellation_and_size_errors_leave_no_partial_outputs() {
    let server = server(vec![42; 2 * 1024 * 1024], true, false);
    let root = tempfile::tempdir().unwrap();
    let mut req = request(root.path(), &server.url);
    req.options["max_mb"] = json!(1);
    assert!(execute(&req, &CancelToken::default(), |_| {}).is_err());
    assert_eq!(std::fs::read_dir(&req.output_dir).unwrap().count(), 0);
    let cancel = CancelToken::default();
    let trigger = cancel.clone();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        trigger.cancel();
    });
    req.options["max_mb"] = json!(4);
    let started = Instant::now();
    let report = run_queue(&[req.clone()], &cancel, |_| {});
    worker.join().unwrap();
    assert_eq!(report.jobs[0].state, JobState::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(std::fs::read_dir(req.output_dir).unwrap().count(), 0);
}

#[test]
#[ignore = "requires yt-dlp, FFmpeg, ffprobe and local HTTP sockets; run explicitly with --ignored"]
fn media_link_download_extracts_and_fully_decodes_audio() {
    let root = tempfile::tempdir().unwrap();
    let wave = root.path().join("source.wav");
    let status = std::process::Command::new(
        file_workflows::support::find_tool("ffmpeg").expect("FFmpeg required"),
    )
    .args([
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=400:duration=0.3",
    ])
    .arg(&wave)
    .status()
    .unwrap();
    assert!(status.success());
    let server = server(std::fs::read(wave).unwrap(), false, false);
    let mut req = request(root.path(), &server.url);
    for format in ["mp3", "m4a", "original"] {
        req.output_dir = root.path().join(format);
        req.options = json!({"url":server.url,"mode":"media","format":format,"filename":"audio"});
        let result = execute(&req, &CancelToken::default(), |_| {}).unwrap();
        assert_eq!(
            result.outputs[0].extension().unwrap(),
            if format == "original" { "wav" } else { format }
        );
        assert!(result.outputs[0].metadata().unwrap().len() > 0);
    }
}

#[test]
#[ignore = "requires yt-dlp, FFmpeg, ffprobe and local HTTP sockets; run explicitly with --ignored"]
fn media_link_download_produces_verified_mp4_and_webm() {
    let root = tempfile::tempdir().unwrap();
    let video = root.path().join("source.mp4");
    let status = std::process::Command::new(
        file_workflows::support::find_tool("ffmpeg").expect("FFmpeg required"),
    )
    .args([
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=red:s=80x64:d=0.3",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=400:duration=0.3",
        "-shortest",
        "-c:v",
        "libx264",
        "-c:a",
        "aac",
    ])
    .arg(&video)
    .status()
    .unwrap();
    assert!(status.success());
    let mut server = server(std::fs::read(video).unwrap(), false, false);
    server.url = server.url.replace("fixture.wav", "fixture.mp4");
    for format in ["mp4", "webm"] {
        let req = WorkflowRequest {
            kind: WorkflowKind::Download,
            inputs: vec![],
            output_dir: root.path().join(format),
            options: json!({"url":server.url,"mode":"media","format":format,"filename":"video"}),
        };
        let result = execute(&req, &CancelToken::default(), |_| {}).unwrap();
        assert_eq!(result.outputs[0].extension().unwrap(), format);
    }
}
