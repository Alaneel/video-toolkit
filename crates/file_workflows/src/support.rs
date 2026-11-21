//! Dependency discovery and cancellable, bounded child-process execution.
use crate::{CancelToken, Progress, WorkflowKind};
use anyhow::{bail, Context, Result};
use file_conversion::process_io::{
    join_pipe, InterruptiblePipe, OwnedProcess, PipeHandle, PIPE_POLL_INTERVAL, PIPE_STOP_GRACE,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const PIPE_DRAIN_GRACE: Duration = Duration::from_millis(250);

fn project_root() -> PathBuf {
    // Source builds and the local .app both live beneath their checkout.
    if let Ok(executable) = std::env::current_exe() {
        for parent in executable.ancestors().skip(1).take(8) {
            if parent.join("Cargo.toml").is_file()
                && parent.join("scripts/requirements-workflows.txt").is_file()
            {
                return parent.to_path_buf();
            }
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .into()
}

pub fn find_tool(name: &str) -> Option<PathBuf> {
    let key = format!("VIDEO_TOOLKIT_{}", name.to_uppercase().replace('-', "_"));
    if let Some(path) = std::env::var_os(key) {
        return PathBuf::from(path)
            .canonicalize()
            .ok()
            .filter(|p| p.is_file());
    }
    let root = project_root();
    let mut candidates = vec![
        root.join(".toolkit-venv/bin").join(name),
        root.join(".toolkit-tools/bin").join(name),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let bundled =
            PathBuf::from(home).join(".cache/codex-runtimes/codex-primary-runtime/dependencies");
        if name == "soffice" {
            candidates.push(bundled.join("bin/override/soffice"));
        }
    }
    if name == "soffice" {
        candidates.push(PathBuf::from(
            "/Applications/LibreOffice.app/Contents/MacOS/soffice",
        ));
    }
    if name == "pandoc" {
        if let Ok(entries) = std::fs::read_dir(root.join(".toolkit-venv/lib")) {
            for entry in entries.flatten() {
                candidates.push(entry.path().join("site-packages/pypandoc/files/pandoc"));
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&paths).map(|p| p.join(name)));
    }
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        candidates.push(Path::new(dir).join(name));
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .and_then(|p| p.canonicalize().ok())
}

pub fn find_python() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("VIDEO_TOOLKIT_PYTHON") {
        let path = PathBuf::from(path);
        if !path.is_file() {
            bail!("VIDEO_TOOLKIT_PYTHON does not point to a Python executable");
        }
        // Keep the venv symlink path: canonicalizing it discards Python's venv discovery.
        return Ok(path);
    }
    let local = project_root().join(".toolkit-venv/bin/python");
    if local.is_file() {
        return Ok(local);
    }
    find_tool("python3")
        .context("Python is unavailable. Run scripts/setup-file-workflows.sh first.")
}

pub fn check_cancelled(cancel: &CancelToken) -> Result<()> {
    if cancel.is_cancelled() {
        bail!("Cancelled");
    }
    Ok(())
}

pub fn copy_cancelled(
    source: &Path,
    destination: &mut impl Write,
    cancel: &CancelToken,
) -> Result<()> {
    let mut input = std::fs::File::open(source)?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check_cancelled(cancel)?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        destination.write_all(&buffer[..count])?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
#[derive(Debug)]
struct Captured {
    bytes: Vec<u8>,
    oversized: bool,
}

fn drain(mut pipe: impl Read, limit: usize) -> std::io::Result<Captured> {
    let mut result = Vec::new();
    let mut oversized = false;
    let mut buffer = [0u8; 8192];
    loop {
        let count = match pipe.read(&mut buffer) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if count == 0 {
            break;
        }
        let retain = count.min(limit.saturating_sub(result.len()));
        result.extend_from_slice(&buffer[..retain]);
        oversized |= retain < count;
    }
    Ok(Captured {
        bytes: result,
        oversized,
    })
}

fn input_stopped() -> std::io::Error {
    std::io::Error::other("Workflow dependency input stopped before complete delivery")
}

#[cfg(unix)]
fn write_input(
    mut pipe: impl Write + PipeHandle,
    bytes: &[u8],
    stopped: &AtomicBool,
) -> std::io::Result<()> {
    let descriptor = pipe.as_raw_fd();
    // This is the exclusively owned child stdin, not a shared application FD.
    // Nonblocking writes close the readiness-to-write race for large payloads.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let mut written = 0;
    while written < bytes.len() {
        if stopped.load(Ordering::Acquire) {
            return Err(input_stopped());
        }
        let mut descriptor = libc::pollfd {
            fd: descriptor,
            events: libc::POLLOUT,
            revents: 0,
        };
        let ready = unsafe {
            libc::poll(
                &mut descriptor,
                1,
                PIPE_POLL_INTERVAL.as_millis() as libc::c_int,
            )
        };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready == 0 {
            continue;
        }
        if descriptor.revents & libc::POLLNVAL != 0 {
            return Err(std::io::Error::other(
                "Workflow dependency input pipe is invalid",
            ));
        }
        let end = bytes.len().min(written.saturating_add(8192));
        match pipe.write(&bytes[written..end]) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(count) => written += count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => return Err(error),
        }
    }
    if stopped.load(Ordering::Acquire) {
        return Err(input_stopped());
    }
    pipe.flush()
}

#[cfg(not(unix))]
fn write_input(
    mut pipe: impl Write + PipeHandle,
    bytes: &[u8],
    stopped: &AtomicBool,
) -> std::io::Result<()> {
    // Windows anonymous stdin writes are synchronous. Without an independently
    // verified thread-cancellation handle, native Write cannot be interrupted
    // portably. The caller never joins an unfinished helper: bounded failure,
    // not successful delivery. A blocked helper may remain until the inherited
    // pipe closes/process exits; do not claim zero residual threads here.
    for chunk in bytes.chunks(8192) {
        if stopped.load(Ordering::Acquire) {
            return Err(input_stopped());
        }
        pipe.write_all(chunk)?;
    }
    if stopped.load(Ordering::Acquire) {
        return Err(input_stopped());
    }
    pipe.flush()
}

type Reader = JoinHandle<std::io::Result<Captured>>;
type InputWriter = JoinHandle<std::io::Result<()>>;

struct CommandPipes {
    stopped: Arc<AtomicBool>,
    stdout: Option<Reader>,
    stderr: Option<Reader>,
    stdin: Option<InputWriter>,
}

impl CommandPipes {
    fn finished(&self) -> bool {
        self.stdout.as_ref().is_none_or(JoinHandle::is_finished)
            && self.stderr.as_ref().is_none_or(JoinHandle::is_finished)
            && self.stdin.as_ref().is_none_or(JoinHandle::is_finished)
    }

    fn stop_and_wait(&self) {
        self.stopped.store(true, Ordering::Release);
        let started = Instant::now();
        while !self.finished() && started.elapsed() < PIPE_STOP_GRACE {
            std::thread::sleep(PIPE_POLL_INTERVAL);
        }
    }

    fn finish(
        mut self,
        drain_allowed: bool,
        cancel: &CancelToken,
        started: Instant,
        timeout: Duration,
        tick: &mut dyn FnMut() -> Result<()>,
    ) -> Result<CommandOutput> {
        let drain_started = Instant::now();
        let mut failure = None;
        while drain_allowed && !self.finished() && drain_started.elapsed() < PIPE_DRAIN_GRACE {
            if let Err(error) = check_cancelled(cancel).and_then(|()| {
                if started.elapsed() >= timeout {
                    bail!(
                        "Workflow dependency timed out after {} seconds",
                        timeout.as_secs()
                    );
                }
                tick()
            }) {
                failure = Some(error);
                break;
            }
            std::thread::sleep(PIPE_POLL_INTERVAL);
        }
        self.stop_and_wait();
        // Never unconditionally join a pipe helper: descendants can retain an
        // inherited pipe after the owned leader/group exits. On Unix the read
        // and write helpers are interruptible; Windows reads are interruptible,
        // but a native blocked stdin write may outlive this bounded failure.
        let stdout = join_pipe(self.stdout.take().unwrap(), "workflow stdout");
        let stderr = join_pipe(self.stderr.take().unwrap(), "workflow stderr");
        let stdin = self
            .stdin
            .take()
            .map_or(Ok(()), |writer| join_pipe(writer, "workflow stdin writer"));
        if let Some(error) = failure {
            return Err(error);
        }
        let stdout = stdout?;
        let stderr = stderr?;
        stdin.context("Cannot completely deliver workflow dependency input")?;
        if stdout.oversized {
            bail!("Workflow dependency stdout exceeded the 4 MiB safety limit");
        }
        Ok(CommandOutput {
            stdout: stdout.bytes,
            // Diagnostics retain a bounded prefix, but the pipe must reach EOF.
            stderr: stderr.bytes,
        })
    }
}

impl Drop for CommandPipes {
    fn drop(&mut self) {
        self.stop_and_wait();
        if let Some(reader) = self.stdout.take() {
            let _ = join_pipe(reader, "workflow stdout");
        }
        if let Some(reader) = self.stderr.take() {
            let _ = join_pipe(reader, "workflow stderr");
        }
        if let Some(writer) = self.stdin.take() {
            let _ = join_pipe(writer, "workflow stdin writer");
        }
    }
}

pub fn run_command(
    command: Command,
    cancel: &CancelToken,
    timeout: Duration,
) -> Result<CommandOutput> {
    run_command_input(command, None, cancel, timeout, &mut || Ok(()))
}

pub fn run_command_monitored(
    command: Command,
    cancel: &CancelToken,
    timeout: Duration,
    tick: &mut dyn FnMut() -> Result<()>,
) -> Result<CommandOutput> {
    run_command_input(command, None, cancel, timeout, tick)
}

fn run_command_input(
    mut command: Command,
    input: Option<&[u8]>,
    cancel: &CancelToken,
    timeout: Duration,
    tick: &mut dyn FnMut() -> Result<()>,
) -> Result<CommandOutput> {
    check_cancelled(cancel)?;
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut process =
        OwnedProcess::spawn(&mut command).context("Cannot start workflow dependency")?;
    let out = process
        .take_stdout()
        .context("Cannot capture dependency stdout")?;
    let err = process
        .take_stderr()
        .context("Cannot capture dependency stderr")?;
    let stopped = Arc::new(AtomicBool::new(false));
    let out_stopped = stopped.clone();
    let err_stopped = stopped.clone();
    let stdout = std::thread::spawn(move || {
        drain(InterruptiblePipe::new(out, out_stopped), 4 * 1024 * 1024)
    });
    let stderr =
        std::thread::spawn(move || drain(InterruptiblePipe::new(err, err_stopped), 128 * 1024));
    // At most one input helper. Large JSON or a dependency that never reads its
    // inherited stdin must not make cancellation join an unfinished writer.
    let stdin = input.map(|bytes| {
        let bytes = bytes.to_vec();
        let stdin = process.take_stdin().unwrap();
        let input_stopped = stopped.clone();
        std::thread::spawn(move || write_input(stdin, &bytes, &input_stopped))
    });
    let pipes = CommandPipes {
        stopped,
        stdout: Some(stdout),
        stderr: Some(stderr),
        stdin,
    };
    let started = Instant::now();
    let status = loop {
        if let Err(error) = tick() {
            break Err(error);
        }
        if cancel.is_cancelled() || started.elapsed() >= timeout {
            if cancel.is_cancelled() {
                break Err(anyhow::anyhow!("Cancelled"));
            }
            break Err(anyhow::anyhow!(
                "Workflow dependency timed out after {} seconds",
                timeout.as_secs()
            ));
        }
        match process.poll_exit() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(error) => break Err(error),
        }
    };
    if status.is_err() {
        let _ = process.stop();
    }
    let output = pipes.finish(status.is_ok(), cancel, started, timeout, tick);
    let status = status?;
    let output = output?;
    if !status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        bail!("Workflow dependency failed ({status}): {}", detail.trim());
    }
    check_cancelled(cancel)?;
    if started.elapsed() >= timeout {
        bail!(
            "Workflow dependency timed out after {} seconds",
            timeout.as_secs()
        );
    }
    Ok(output)
}

pub fn run_python(
    script: &str,
    kind: WorkflowKind,
    inputs: &[PathBuf],
    staging: &Path,
    options: &Value,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(Progress),
) -> Result<Vec<PathBuf>> {
    let private = tempfile::Builder::new()
        .prefix("video-toolkit-worker-")
        .tempdir()?;
    let worker = private.path().join("worker.py");
    std::fs::write(&worker, script)?;
    let payload = serde_json::to_vec(
        &json!({"kind":kind,"inputs":inputs,"staging":staging,"options":options}),
    )?;
    let mut command = Command::new(find_python()?);
    command
        .arg("-I")
        .arg(&worker)
        .current_dir(private.path())
        .env("PYTHONUNBUFFERED", "1");
    for name in ["pandoc", "gs", "magick", "soffice", "ffmpeg", "ffprobe"] {
        if let Some(path) = find_tool(name) {
            command.env(format!("VIDEO_TOOLKIT_{}", name.to_uppercase()), path);
        }
    }
    progress(Progress {
        fraction: 0.12,
        message: format!("Processing {}", kind),
    });
    let result = run_command_input(command, Some(&payload), cancel, Duration::from_secs(1200), &mut || Ok(()))
        .context("Install workflow dependencies with scripts/setup-file-workflows.sh if a module or executable is missing")?;
    #[derive(Deserialize)]
    struct Response {
        outputs: Vec<PathBuf>,
    }
    let response: Response = serde_json::from_slice(&result.stdout)
        .context("Dependency returned an invalid output manifest")?;
    progress(Progress {
        fraction: 0.9,
        message: "Checking generated files".into(),
    });
    Ok(response.outputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ErrorAfterPrefix(bool);
    impl Read for ErrorAfterPrefix {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            if std::mem::replace(&mut self.0, true) {
                return Err(std::io::Error::other("Synthetic pipe read failure"));
            }
            let prefix = b"{\"outputs\":[]}";
            bytes[..prefix.len()].copy_from_slice(prefix);
            Ok(prefix.len())
        }
    }

    #[test]
    fn read_error_cannot_become_a_successful_manifest() {
        let error = drain(ErrorAfterPrefix(false), 1024).unwrap_err();
        assert!(error.to_string().contains("Synthetic pipe read failure"));
    }

    #[test]
    #[cfg(unix)]
    fn retained_pipe_with_valid_manifest_is_interruptible_and_never_complete() {
        let (reader, mut retained) = std::os::unix::net::UnixStream::pair().unwrap();
        retained.write_all(b"{\"outputs\":[]}").unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let reader_stop = stopped.clone();
        let reader =
            std::thread::spawn(move || drain(InterruptiblePipe::new(reader, reader_stop), 1024));
        std::thread::sleep(PIPE_POLL_INTERVAL);
        let started = Instant::now();
        stopped.store(true, Ordering::Release);
        while !reader.is_finished() && started.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(PIPE_POLL_INTERVAL);
        }
        let error = join_pipe(reader, "test stdout").unwrap_err();
        assert!(format!("{error:#}").contains("stopped before EOF"));
        assert!(started.elapsed() < Duration::from_secs(1));
        // The writer is deliberately still alive; no signal/child is involved.
        drop(retained);
    }

    #[test]
    #[cfg(unix)]
    fn backpressured_input_writer_stops_without_complete_delivery() {
        let (writer, retained_reader) = std::os::unix::net::UnixStream::pair().unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let writer_stop = stopped.clone();
        let writer = std::thread::spawn(move || {
            write_input(writer, &vec![b'x'; 4 * 1024 * 1024], &writer_stop)
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !writer.is_finished(),
            "Synthetic input must encounter backpressure"
        );
        let started = Instant::now();
        stopped.store(true, Ordering::Release);
        while !writer.is_finished() && started.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(PIPE_POLL_INTERVAL);
        }
        let error = join_pipe(writer, "test stdin").unwrap_err();
        assert!(format!("{error:#}").contains("before complete delivery"));
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(retained_reader);
    }

    #[test]
    fn output_capture_is_bounded_but_records_overflow_and_eof() {
        let captured = drain(std::io::Cursor::new(vec![b'x'; 64]), 16).unwrap();
        assert_eq!(captured.bytes.len(), 16);
        assert!(captured.oversized);
        let captured = drain(std::io::Cursor::new(b"complete"), 64).unwrap();
        assert_eq!(captured.bytes, b"complete");
        assert!(!captured.oversized);
    }

    #[test]
    fn unfinished_input_helper_has_a_bounded_failure_not_a_successful_delivery() {
        let (release, resume) = std::sync::mpsc::sync_channel(1);
        let (finished, stopped) = std::sync::mpsc::sync_channel(1);
        let writer = std::thread::spawn(move || {
            let _ = resume.recv();
            let _ = finished.send(());
            Err(input_stopped())
        });
        let reader = || {
            std::thread::spawn(|| {
                Ok(Captured {
                    bytes: Vec::new(),
                    oversized: false,
                })
            })
        };
        let pipes = CommandPipes {
            stopped: Arc::new(AtomicBool::new(false)),
            stdout: Some(reader()),
            stderr: Some(reader()),
            stdin: Some(writer),
        };
        let started = Instant::now();
        let result = pipes.finish(
            true,
            &CancelToken::default(),
            started,
            Duration::from_secs(2),
            &mut || Ok(()),
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        // This also models Windows' synchronous-write fallback, without an OS
        // child or an unkillable native call. Release and observe its completion.
        release.send(()).unwrap();
        stopped.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(format!("{:#}", result.unwrap_err()).contains("did not stop"));
    }

    #[test]
    #[cfg(unix)]
    fn command_input_and_output_are_complete_when_dependency_consumes_them() {
        let bytes = vec![b'x'; 256 * 1024];
        let output = run_command_input(
            Command::new("/bin/cat"),
            Some(&bytes),
            &CancelToken::default(),
            Duration::from_secs(3),
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(output.stdout, bytes);
        assert!(output.stderr.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn oversized_stdout_cannot_publish_a_valid_manifest_prefix() {
        let mut bytes = vec![b' '; 4 * 1024 * 1024 + 1];
        let prefix = b"{\"outputs\":[]}";
        bytes[..prefix.len()].copy_from_slice(prefix);
        // The captured prefix alone would parse as valid JSON. A successful
        // child and valid-looking prefix do not authorize truncated output.
        let error = run_command_input(
            Command::new("/bin/cat"),
            Some(&bytes),
            &CancelToken::default(),
            Duration::from_secs(5),
            &mut || Ok(()),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("4 MiB safety limit"));
    }

    #[test]
    #[cfg(unix)]
    fn cancelled_process_is_reaped_quickly() {
        let cancel = CancelToken::default();
        let trigger = cancel.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            trigger.cancel();
        });
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30 & wait"]);
        let started = Instant::now();
        assert!(run_command(command, &cancel, Duration::from_secs(40))
            .unwrap_err()
            .to_string()
            .contains("Cancelled"));
        assert!(started.elapsed() < Duration::from_secs(3));
        thread.join().unwrap();
    }
    #[test]
    #[cfg(unix)]
    fn parent_exit_cleans_grandchildren_inheriting_pipes() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30 & exit 0"]);
        let started = Instant::now();
        run_command(command, &CancelToken::default(), Duration::from_secs(2)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
