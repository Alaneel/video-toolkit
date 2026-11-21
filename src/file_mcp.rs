//! Tools-only MCP stdio adapter. File policy lives in the shared workflow cores.
use anyhow::{bail, Context, Result};
use file_conversion::{output_requests, run_queue, CancelToken, TargetFormat};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MAX_MESSAGE: usize = 1024 * 1024;
const MAX_OUTPUT_MESSAGE: usize = 4 * MAX_MESSAGE;
const INPUT_QUEUE_DEPTH: usize = 1;
const OUTPUT_QUEUE_DEPTH: usize = 4;
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(2);
const IO_POLL: Duration = Duration::from_millis(5);
const IO_SHUTDOWN_GRACE: Duration = Duration::from_millis(50);
const VERSIONS: &[&str] = &["2025-06-18", "2024-11-05"];
type Writer = Arc<OutputState>;

struct OutputMessage {
    bytes: Vec<u8>,
    // Admission to the queue is not delivery: only the I/O owner acknowledges
    // a complete write and flush. Progress notifications are best effort.
    acknowledged: Option<SyncSender<bool>>,
    cancel: Option<CancelToken>,
}

struct OutputState {
    sender: SyncSender<OutputMessage>,
    failed: AtomicBool,
    stopping: AtomicBool,
    active_cancel: Mutex<Option<CancelToken>>,
    timeout: Duration,
}

impl OutputState {
    fn closed(&self) -> bool {
        self.failed.load(Ordering::Acquire) || self.stopping.load(Ordering::Acquire)
    }

    fn cancel_active(&self) {
        if let Ok(active) = self.active_cancel.lock() {
            if let Some(cancel) = active.as_ref() {
                cancel.cancel();
            }
        }
    }

    fn fail(&self) {
        self.failed.store(true, Ordering::Release);
        self.cancel_active();
    }

    fn bind_cancel(&self, cancel: &CancelToken) {
        if let Ok(mut active) = self.active_cancel.lock() {
            *active = Some(cancel.clone());
        }
        if self.closed() {
            cancel.cancel();
        }
    }

    fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        self.cancel_active();
    }
}

/// Exactly one output owner per stdio session, with a bounded queue and bounded
/// producer waits. A portable `Read`/`Write` cannot interrupt an OS-level blocked
/// stdio call. Each direction therefore joins its fixed owner only for a bounded
/// grace period; if still blocked, that thread is detached until process exit.
/// File-operation workers never write stdout and are still cancelled/joined.
/// This is not a zero-thread-residue guarantee for long-lived embedding of
/// `serve_stream`; the production entry point is a single-session process.
struct Output {
    state: Writer,
    worker: Option<JoinHandle<()>>,
}

impl Output {
    fn new(mut sink: Box<dyn Write + Send>, timeout: Duration) -> Self {
        let (sender, receiver): (SyncSender<OutputMessage>, Receiver<OutputMessage>) =
            mpsc::sync_channel(OUTPUT_QUEUE_DEPTH);
        let state = Arc::new(OutputState {
            sender,
            failed: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            active_cancel: Mutex::new(None),
            timeout,
        });
        let owner = state.clone();
        let worker = std::thread::spawn(move || {
            while !owner.closed() {
                let message = match receiver.recv_timeout(IO_POLL) {
                    Ok(message) => message,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                if message
                    .cancel
                    .as_ref()
                    .is_some_and(CancelToken::is_cancelled)
                {
                    if let Some(acknowledged) = message.acknowledged {
                        let _ = acknowledged.try_send(false);
                    }
                    continue;
                }
                let delivered = sink
                    .write_all(&message.bytes)
                    .and_then(|()| sink.flush())
                    .is_ok();
                if let Some(acknowledged) = message.acknowledged {
                    let _ = acknowledged.try_send(delivered);
                }
                if !delivered {
                    owner.fail();
                    break;
                }
            }
        });
        Self {
            state,
            worker: Some(worker),
        }
    }

    fn writer(&self) -> Writer {
        self.state.clone()
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.state.stop();
        if let Some(worker) = self.worker.take() {
            finish_io_owner(worker);
        }
    }
}

fn finish_io_owner(worker: JoinHandle<()>) {
    let deadline = Instant::now() + IO_SHUTDOWN_GRACE;
    while !worker.is_finished() && Instant::now() < deadline {
        std::thread::sleep(IO_POLL);
    }
    if worker.is_finished() {
        let _ = worker.join();
    }
    // Dropping a still-blocked handle detaches only one of the two fixed stdio
    // owners. Never use this bounded-detach policy for file workers/children.
}

enum InputMessage {
    Line(Vec<u8>),
    Eof,
    Error(std::io::Error),
}

/// The coordinator must not block on stdin while an asynchronous response can
/// fail on stdout. A single input owner, with one queued line and at most one
/// in-flight line, permits polling that failure even if stdin stays open/idle.
/// Each line is capped before parsing, including unterminated oversized input.
/// Like `Output`, an uninterruptible native read may outlive `serve_stream` in
/// a long-lived embedding; only the single-session process reclaims it on exit.
struct Input {
    receiver: Receiver<InputMessage>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Input {
    fn new(mut source: impl BufRead + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::sync_channel(INPUT_QUEUE_DEPTH);
        let stopping = Arc::new(AtomicBool::new(false));
        let owner_stopping = stopping.clone();
        let worker = std::thread::spawn(move || {
            while !owner_stopping.load(Ordering::Acquire) {
                let mut line = Vec::new();
                let mut message = match source
                    .by_ref()
                    .take((MAX_MESSAGE + 1) as u64)
                    .read_until(b'\n', &mut line)
                {
                    Ok(0) => InputMessage::Eof,
                    Ok(_) => InputMessage::Line(line),
                    Err(error) => InputMessage::Error(error),
                };
                let terminal =
                    !matches!(&message, InputMessage::Line(line) if line.len() <= MAX_MESSAGE);
                loop {
                    if owner_stopping.load(Ordering::Acquire) {
                        return;
                    }
                    match sender.try_send(message) {
                        Ok(()) => break,
                        Err(TrySendError::Full(unsent)) => message = unsent,
                        Err(TrySendError::Disconnected(_)) => return,
                    }
                    std::thread::sleep(IO_POLL);
                }
                if terminal {
                    break;
                }
            }
        });
        Self {
            receiver,
            stopping,
            worker: Some(worker),
        }
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            finish_io_owner(worker);
        }
    }
}

struct BoundedOutput(Vec<u8>);

impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_OUTPUT_MESSAGE.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("MCP response exceeds output limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encoded(value: &Value) -> Option<Vec<u8>> {
    let mut buffer = BoundedOutput(Vec::new());
    serde_json::to_writer(&mut buffer, value).ok()?;
    buffer.write_all(b"\n").ok()?;
    Some(buffer.0)
}

fn response_bytes(value: &Value) -> Option<Vec<u8>> {
    encoded(value).or_else(|| {
        // Preserve the request identity, but never emit partial/truncated JSON
        // or claim the oversized successful result was delivered.
        encoded(
            &json!({"jsonrpc":"2.0","id":value.get("id").cloned().unwrap_or(Value::Null),
            "error":{"code":-32603,"message":"MCP response exceeds output limit"}}),
        )
    })
}

struct Active {
    id: Value,
    cancel: CancelToken,
    done: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for Active {
    fn drop(&mut self) {
        if !self.done.load(Ordering::Acquire) {
            self.cancel.cancel();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConvertInput {
    inputs: Vec<PathBuf>,
    output_dir: PathBuf,
    format: TargetFormat,
}

enum PendingJob {
    Conversion(Vec<file_conversion::ConversionRequest>),
    Workflow(file_workflows::WorkflowRequest),
}

fn send(writer: &Writer, value: Value) {
    let _ = send_wait(writer, value, None);
}

fn send_wait(writer: &Writer, value: Value, cancel: Option<&CancelToken>) -> bool {
    let Some(bytes) = response_bytes(&value) else {
        writer.fail();
        return false;
    };
    let deadline = Instant::now() + writer.timeout;
    let (acknowledged, receipt) = mpsc::sync_channel(1);
    let mut message = OutputMessage {
        bytes,
        acknowledged: Some(acknowledged),
        cancel: cancel.cloned(),
    };
    loop {
        if writer.closed() || cancel.is_some_and(CancelToken::is_cancelled) {
            return false;
        }
        if Instant::now() >= deadline {
            writer.fail();
            return false;
        }
        match writer.sender.try_send(message) {
            Ok(()) => break,
            Err(TrySendError::Full(unsent)) => message = unsent,
            Err(TrySendError::Disconnected(_)) => {
                writer.fail();
                return false;
            }
        }
        std::thread::sleep(IO_POLL);
    }
    loop {
        if writer.closed() || cancel.is_some_and(CancelToken::is_cancelled) {
            return false;
        }
        if Instant::now() >= deadline {
            writer.fail();
            return false;
        }
        match receipt.recv_timeout(IO_POLL) {
            Ok(true) => return true,
            Ok(false) => {
                if !cancel.is_some_and(CancelToken::is_cancelled) {
                    writer.fail();
                }
                return false;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                writer.fail();
                return false;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn send_progress(writer: &Writer, value: Value) {
    if writer.closed() {
        return;
    }
    let Some(bytes) = encoded(&value) else {
        return;
    };
    match writer.sender.try_send(OutputMessage {
        bytes,
        acknowledged: None,
        cancel: None,
    }) {
        Ok(()) | Err(TrySendError::Full(_)) => {}
        Err(TrySendError::Disconnected(_)) => writer.fail(),
    }
}

fn error(writer: &Writer, id: Value, code: i32, message: &str) {
    send(
        writer,
        json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}),
    );
}

fn tool_result(writer: &Writer, id: Value, value: Value, failed: bool) {
    send(writer, result_message(id, value, failed));
}

fn result_message(id: Value, value: Value, failed: bool) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":{
        "content":[{"type":"text","text":value.to_string()}],
        "structuredContent":value,"isError":failed
    }})
}

fn tools() -> Value {
    json!({"tools":[
        {"name":"list_formats","description":"List output formats supported by the local conversion engine.",
         "inputSchema":{"type":"object","properties":{},"additionalProperties":false},
         "annotations":{"readOnlyHint":true,"openWorldHint":false}},
        {"name":"dependency_status","description":"Inspect local media and PDF/document/download dependency availability.",
         "inputSchema":{"type":"object","properties":{},"additionalProperties":false},
         "annotations":{"readOnlyHint":true,"openWorldHint":false}},
        {"name":"convert_files","description":"Convert local video/audio/raster files in a sequential queue. Absolute input paths and an existing output folder must be inside server --root folders. Never overwrites. Outputs are decoded before success. Send notifications/cancelled with this requestId to cancel. PDF, subtitles and documents use run_workflow.",
         "inputSchema":{"type":"object","properties":{
             "inputs":{"type":"array","minItems":1,"maxItems":256,"items":{"type":"string"}},
             "output_dir":{"type":"string"},
             "format":{"type":"string","enum":TargetFormat::all().iter().map(ToString::to_string).collect::<Vec<_>>()}
         },"required":["inputs","output_dir","format"],"additionalProperties":false},
         "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":false,"openWorldHint":false}},
        {"name":"workflow_tools","description":"Read available PDF, subtitle, document and download workflows, formats and exact option schemas.",
         "inputSchema":{"type":"object","properties":{},"additionalProperties":false},
         "annotations":{"readOnlyHint":true,"openWorldHint":false}},
        {"name":"run_workflow","description":"Run one explicitly selected file workflow asynchronously. Local inputs and an existing output_dir must resolve inside server --root folders. Never replaces existing files. Use workflow_tools for supported options. Only the download kind accesses an explicitly supplied URL. Send notifications/cancelled with this requestId to cancel.",
         "inputSchema":{"type":"object","properties":{
             "kind":{"type":"string","enum":file_workflows::tools().iter().map(|tool|tool.kind.to_string()).collect::<Vec<_>>()},
             "inputs":{"type":"array","maxItems":256,"items":{"type":"string"},"default":[]},
             "output_dir":{"type":"string"},"options":{"type":"object","default":{}}
         },"required":["kind","output_dir"],"additionalProperties":false},
         "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":false,"openWorldHint":true}}
    ]})
}

fn inside(path: &Path, roots: &[PathBuf]) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("MCP paths must be absolute: {}", path.display());
    }
    let path = path
        .canonicalize()
        .with_context(|| format!("Cannot resolve {}", path.display()))?;
    if !roots.iter().any(|root| path.starts_with(root)) {
        bail!("Path is outside allowed --root folders: {}", path.display());
    }
    Ok(path)
}

fn requests(
    arguments: Value,
    roots: &[PathBuf],
) -> Result<Vec<file_conversion::ConversionRequest>> {
    let input: ConvertInput = serde_json::from_value(arguments)?;
    if input.inputs.is_empty() || input.inputs.len() > 256 {
        bail!("Choose 1 to 256 inputs");
    }
    let inputs = input
        .inputs
        .iter()
        .map(|p| inside(p, roots))
        .collect::<Result<Vec<_>>>()?;
    let output_dir = inside(&input.output_dir, roots)?;
    if !output_dir.is_dir() {
        bail!("output_dir must be an existing directory");
    }
    output_requests(&inputs, &output_dir, input.format)
}

fn workflow_request(
    arguments: Value,
    roots: &[PathBuf],
) -> Result<file_workflows::WorkflowRequest> {
    let mut request: file_workflows::WorkflowRequest = serde_json::from_value(arguments)?;
    if request.inputs.len() > 256 {
        bail!("Choose at most 256 workflow inputs");
    }
    request.inputs = request
        .inputs
        .iter()
        .map(|path| inside(path, roots))
        .collect::<Result<Vec<_>>>()?;
    request.output_dir = inside(&request.output_dir, roots)?;
    if !request.output_dir.is_dir() {
        bail!("output_dir must be an existing directory");
    }
    if !request.options.is_object() {
        bail!("Workflow options must be a JSON object");
    }
    if request.kind == file_workflows::WorkflowKind::Download {
        if !request.inputs.is_empty() {
            bail!("Downloads accept an explicit options.url, not local inputs");
        }
        if !request.options.get("url").is_some_and(Value::is_string) {
            bail!("A download requires an explicit options.url");
        }
    } else if request.inputs.is_empty() {
        bail!("Choose at least one workflow input");
    }
    Ok(request)
}

pub fn serve(roots: Vec<PathBuf>) -> Result<()> {
    let roots = roots
        .iter()
        .map(|root| {
            let root = root
                .canonicalize()
                .with_context(|| format!("Cannot open root {}", root.display()))?;
            if !root.is_dir() {
                bail!("MCP root must be an existing directory");
            }
            Ok(root)
        })
        .collect::<Result<Vec<_>>>()?;
    if roots.is_empty() {
        bail!("At least one --root folder is required");
    }
    let output = Output::new(Box::new(std::io::stdout()), OUTPUT_TIMEOUT);
    serve_stream(
        std::io::BufReader::new(std::io::stdin()),
        output.writer(),
        roots,
    )
}

fn serve_stream(
    input: impl BufRead + Send + 'static,
    writer: Writer,
    roots: Vec<PathBuf>,
) -> Result<()> {
    let input = Input::new(input);
    let mut initialized = false;
    let mut ready = false;
    let mut active: Option<Active> = None;
    loop {
        if writer.closed() {
            break;
        }
        if active
            .as_ref()
            .is_some_and(|a| a.done.load(Ordering::Acquire))
        {
            drop(active.take());
        }
        let line = match input.receiver.recv_timeout(IO_POLL) {
            Ok(InputMessage::Line(line)) => line,
            Ok(InputMessage::Eof) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(InputMessage::Error(error)) => return Err(error.into()),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
        };
        if writer.closed() {
            break;
        }
        if line.len() > MAX_MESSAGE {
            error(&writer, Value::Null, -32600, "Message exceeds 1 MiB");
            break;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let message: Value = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(_) => {
                error(&writer, Value::Null, -32700, "Invalid JSON");
                continue;
            }
        };
        // Completion can arrive while the coordinator waits for the next line.
        if active
            .as_ref()
            .is_some_and(|a| a.done.load(Ordering::Acquire))
        {
            drop(active.take());
        }
        let id = message.get("id").cloned();
        let valid_id = id
            .as_ref()
            .is_none_or(|id| id.is_string() || id.is_i64() || id.is_u64());
        let Some(method) = message
            .get("method")
            .and_then(Value::as_str)
            .filter(|_| message["jsonrpc"] == "2.0" && valid_id)
        else {
            error(
                &writer,
                Value::Null,
                -32600,
                "Expected a JSON-RPC 2.0 request or notification",
            );
            continue;
        };
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        if id.is_none() {
            match method {
                "notifications/initialized" if initialized => ready = true,
                "notifications/cancelled" => {
                    if let Some(task) = &active {
                        if params.get("requestId") == Some(&task.id) {
                            task.cancel.cancel();
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let id = id.unwrap();
        match method {
            "initialize" => {
                if initialized {
                    error(&writer, id, -32600, "Session is already initialized");
                    continue;
                }
                let Some(version) = params["protocolVersion"].as_str() else {
                    error(&writer, id, -32602, "protocolVersion is required");
                    continue;
                };
                let version = if VERSIONS.contains(&version) {
                    version
                } else {
                    VERSIONS[0]
                };
                send(
                    &writer,
                    json!({"jsonrpc":"2.0","id":id,"result":{
                        "protocolVersion":version,"capabilities":{"tools":{"listChanged":false}},
                        "serverInfo":{"name":"video-toolkit","version":env!("CARGO_PKG_VERSION")},
                        "instructions":"Use explicit absolute local paths within server --root folders. File tools write new verified outputs and refuse existing paths. Network access occurs only for an explicitly requested download workflow URL."
                    }}),
                );
                initialized = true;
            }
            "ping" => send(&writer, json!({"jsonrpc":"2.0","id":id,"result":{}})),
            _ if !ready => error(&writer, id, -32600, "Initialize the MCP session first"),
            "tools/list" => send(&writer, json!({"jsonrpc":"2.0","id":id,"result":tools()})),
            "tools/call" => {
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match params["name"].as_str() {
                    Some("list_formats" | "dependency_status" | "workflow_tools")
                        if arguments != json!({}) =>
                    {
                        error(
                            &writer,
                            id,
                            -32602,
                            "This tool accepts an empty arguments object",
                        );
                    }
                    Some("list_formats") => tool_result(
                        &writer,
                        id,
                        json!({"formats":TargetFormat::all().iter().map(ToString::to_string).collect::<Vec<_>>()}),
                        false,
                    ),
                    Some("dependency_status") => tool_result(
                        &writer,
                        id,
                        json!({"dependencies":file_conversion::dependencies(),"workflow_dependencies":file_workflows::dependencies()}),
                        false,
                    ),
                    Some("workflow_tools") => {
                        tool_result(&writer, id, json!({"tools":file_workflows::tools()}), false)
                    }
                    Some("convert_files" | "run_workflow") => {
                        if active.is_some() {
                            tool_result(
                                &writer,
                                id,
                                json!({"error":"A file operation is already running"}),
                                true,
                            );
                            continue;
                        }
                        let pending = if params["name"] == "convert_files" {
                            requests(arguments, &roots).map(PendingJob::Conversion)
                        } else {
                            workflow_request(arguments, &roots).map(PendingJob::Workflow)
                        };
                        let pending = match pending {
                            Ok(pending) => pending,
                            Err(e) => {
                                tool_result(&writer, id, json!({"error":format!("{e:#}")}), true);
                                continue;
                            }
                        };
                        let token = params
                            .pointer("/_meta/progressToken")
                            .cloned()
                            .filter(|t| t.is_string() || t.is_i64() || t.is_u64());
                        let cancel = CancelToken::default();
                        writer.bind_cancel(&cancel);
                        let worker_cancel = cancel.clone();
                        let worker_id = id.clone();
                        let writer = writer.clone();
                        let done = Arc::new(AtomicBool::new(false));
                        let worker_done = done.clone();
                        let worker = std::thread::spawn(move || {
                            let total = match &pending {
                                PendingJob::Conversion(requests) => requests.len(),
                                PendingJob::Workflow(_) => 1,
                            };
                            let mut last_progress: Option<f32> = None;
                            let mut notify =
                                |index: usize,
                                 state: file_conversion::JobState,
                                 fraction: f32,
                                 message: String| {
                                    if matches!(state, file_conversion::JobState::Queued)
                                        || worker_cancel.is_cancelled()
                                    {
                                        return;
                                    }
                                    let progress = index as f32 + fraction;
                                    if !progress.is_finite()
                                        || last_progress.is_some_and(|last| progress <= last)
                                    {
                                        return;
                                    }
                                    last_progress = Some(progress);
                                    if let Some(token) = &token {
                                        send_progress(
                                            &writer,
                                            json!({"jsonrpc":"2.0","method":"notifications/progress","params":{
                                                "progressToken":token,"progress":progress,
                                                "total":total,"message":format!("[{}] {:?}: {}",index+1,state,message)
                                            }}),
                                        );
                                    }
                                };
                            let (report, failed) = match pending {
                                PendingJob::Conversion(requests) => {
                                    let report = run_queue(&requests, &worker_cancel, |event| {
                                        notify(
                                            event.index,
                                            event.state,
                                            event.progress,
                                            event.message,
                                        )
                                    });
                                    let failed = !report.success();
                                    (json!(report), failed)
                                }
                                PendingJob::Workflow(request) => {
                                    let report = file_workflows::run_queue(
                                        &[request],
                                        &worker_cancel,
                                        |event| {
                                            notify(
                                                event.index,
                                                event.state,
                                                event.progress,
                                                event.message,
                                            )
                                        },
                                    );
                                    let failed = !report.success();
                                    (json!(report), failed)
                                }
                            };
                            if !worker_cancel.is_cancelled() {
                                let _ = send_wait(
                                    &writer,
                                    result_message(
                                        worker_id,
                                        json!({"ok":!failed,"report":report}),
                                        failed,
                                    ),
                                    Some(&worker_cancel),
                                );
                            }
                            // Finished means this operation worker has stopped,
                            // not merely that its response entered an I/O queue.
                            worker_done.store(true, Ordering::Release);
                        });
                        active = Some(Active {
                            id,
                            cancel,
                            done,
                            worker: Some(worker),
                        });
                    }
                    _ => error(&writer, id, -32602, "Unknown tool name"),
                }
            }
            _ => error(&writer, id, -32601, "Method not found"),
        }
    }
    // Disconnect cancels the active queue and waits until its child has been reaped.
    drop(active);
    if writer.failed.load(Ordering::Acquire) {
        bail!("MCP output failed or exceeded the bounded delivery timeout");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn stdio_handshake_errors_and_tools_are_valid_json() {
        let buffer = Buffer(Arc::new(Mutex::new(Vec::new())));
        let output = Output::new(Box::new(buffer.clone()), OUTPUT_TIMEOUT);
        let writer = output.writer();
        let lines = [
            "broken".into(),
            json!({"jsonrpc":"2.0","id":0,"method":"tools/list"}).to_string(),
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}).to_string(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}).to_string(),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_formats","arguments":{}}}).to_string(),
        ].join("\n");
        serve_stream(std::io::Cursor::new(lines), writer, vec![]).unwrap();
        let bytes = buffer.0.lock().unwrap();
        let messages: Vec<Value> = String::from_utf8_lossy(&bytes)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(messages[0]["error"]["code"], -32700);
        assert_eq!(messages[1]["error"]["code"], -32600);
        assert_eq!(messages[2]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(messages[3]["result"]["tools"].as_array().unwrap().len(), 5);
        assert_eq!(messages[4]["result"]["isError"], false);
    }

    #[test]
    fn output_is_acknowledged_only_after_writing_and_oversized_responses_stay_valid_json() {
        let buffer = Buffer(Arc::new(Mutex::new(Vec::new())));
        let output = Output::new(Box::new(buffer.clone()), OUTPUT_TIMEOUT);
        assert!(send_wait(
            &output.writer(),
            json!({"jsonrpc":"2.0","id":7,"result":{"ok":true}}),
            None
        ));
        assert!(send_wait(
            &output.writer(),
            json!({"jsonrpc":"2.0","id":8,"result":"x".repeat(MAX_OUTPUT_MESSAGE)}),
            None
        ));
        let bytes = buffer.0.lock().unwrap();
        assert!(bytes.len() < MAX_OUTPUT_MESSAGE);
        let messages: Vec<Value> = String::from_utf8_lossy(&bytes)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["id"], 7);
        assert_eq!(messages[0]["result"]["ok"], true);
        assert_eq!(messages[1]["id"], 8);
        assert_eq!(messages[1]["error"]["code"], -32603);
        assert!(messages[1].get("result").is_none());
    }

    struct BlockedSink {
        entered: SyncSender<()>,
        release: Receiver<()>,
        finished: SyncSender<()>,
    }

    impl Write for BlockedSink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let _ = self.entered.try_send(());
            self.release
                .recv()
                .map_err(|_| std::io::Error::other("Synthetic sink closed"))?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Drop for BlockedSink {
        fn drop(&mut self) {
            let _ = self.finished.try_send(());
        }
    }

    #[test]
    fn stalled_output_has_bounded_progress_and_delivery_waits_and_cancels_work() {
        let (entered, started) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        let (finished, stopped) = mpsc::sync_channel(1);
        let output = Output::new(
            Box::new(BlockedSink {
                entered,
                release: resume,
                finished,
            }),
            Duration::from_millis(60),
        );
        let writer = output.writer();
        let cancel = CancelToken::default();
        writer.bind_cancel(&cancel);
        send_progress(&writer, json!({"progress":0}));
        started.recv_timeout(Duration::from_secs(1)).unwrap();
        let start = Instant::now();
        for index in 1..1000 {
            send_progress(&writer, json!({"progress":index}));
        }
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!send_wait(
            &writer,
            json!({"jsonrpc":"2.0","id":1,"result":{}}),
            Some(&cancel)
        ));
        assert!(writer.failed.load(Ordering::Acquire));
        assert!(cancel.is_cancelled());
        assert!(!send_wait(
            &writer,
            json!({"jsonrpc":"2.0","id":2,"result":{}}),
            None
        ));
        let start = Instant::now();
        drop(output);
        assert!(start.elapsed() < Duration::from_millis(500));
        // The bounded portable shutdown can detach the sole blocked I/O
        // owner; releasing our synthetic sink proves it subsequently exits.
        release.send(()).unwrap();
        stopped.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    struct FirstWriteGate {
        buffer: Buffer,
        entered: SyncSender<()>,
        release: Option<Receiver<()>>,
    }

    impl Write for FirstWriteGate {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if let Some(release) = self.release.take() {
                let _ = self.entered.try_send(());
                release
                    .recv()
                    .map_err(|_| std::io::Error::other("Synthetic gate closed"))?;
            }
            self.buffer.write(bytes)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.buffer.flush()
        }
    }

    #[test]
    fn cancelled_queued_final_response_is_not_written_or_marked_delivered() {
        let buffer = Buffer(Arc::new(Mutex::new(Vec::new())));
        let (entered, started) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        let output = Output::new(
            Box::new(FirstWriteGate {
                buffer: buffer.clone(),
                entered,
                release: Some(resume),
            }),
            OUTPUT_TIMEOUT,
        );
        let writer = output.writer();
        send_progress(&writer, json!({"progress":0}));
        started.recv_timeout(Duration::from_secs(1)).unwrap();
        let cancel = CancelToken::default();
        let (acknowledged, receipt) = mpsc::sync_channel(1);
        writer
            .sender
            .try_send(OutputMessage {
                bytes: encoded(&json!({"jsonrpc":"2.0","id":9,"result":{}})).unwrap(),
                acknowledged: Some(acknowledged),
                cancel: Some(cancel.clone()),
            })
            .unwrap();
        cancel.cancel();
        release.send(()).unwrap();
        assert!(!receipt.recv_timeout(Duration::from_secs(1)).unwrap());
        assert!(!writer.failed.load(Ordering::Acquire));
        assert!(send_wait(
            &writer,
            json!({"jsonrpc":"2.0","id":10,"result":{}}),
            None
        ));
        let bytes = buffer.0.lock().unwrap();
        let messages: Vec<Value> = String::from_utf8_lossy(&bytes)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|message| message["id"] != 9));
        assert_eq!(messages[1]["id"], 10);
    }

    struct BlockedSource {
        entered: SyncSender<()>,
        release: Receiver<()>,
        finished: SyncSender<()>,
    }

    impl Read for BlockedSource {
        fn read(&mut self, _bytes: &mut [u8]) -> std::io::Result<usize> {
            let _ = self.entered.try_send(());
            self.release
                .recv()
                .map_err(|_| std::io::Error::other("Synthetic source closed"))?;
            Ok(0)
        }
    }

    impl Drop for BlockedSource {
        fn drop(&mut self) {
            let _ = self.finished.try_send(());
        }
    }

    #[test]
    fn input_shutdown_is_bounded_and_releases_its_owner_when_read_unblocks() {
        let (entered, started) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        let (finished, stopped) = mpsc::sync_channel(1);
        let input = Input::new(std::io::BufReader::new(BlockedSource {
            entered,
            release: resume,
            finished,
        }));
        started.recv_timeout(Duration::from_secs(1)).unwrap();
        let start = Instant::now();
        drop(input);
        assert!(start.elapsed() < Duration::from_millis(500));
        release.send(()).unwrap();
        stopped.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn input_owner_caps_unterminated_messages_before_parsing() {
        let input = Input::new(std::io::Cursor::new(vec![b'x'; MAX_MESSAGE + 2]));
        match input.receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(InputMessage::Line(line)) => assert_eq!(line.len(), MAX_MESSAGE + 1),
            _ => panic!("Expected a capped oversized line"),
        }
        assert!(matches!(
            input.receiver.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }

    #[test]
    fn rejects_paths_outside_roots_and_unknown_arguments() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let roots = vec![allowed.path().canonicalize().unwrap()];
        assert!(inside(outside.path(), &roots).is_err());
        assert!(inside(Path::new("relative.png"), &roots).is_err());
        assert!(requests(
            json!({"inputs":[],"output_dir":allowed.path(),"format":"png","command":"evil"}),
            &roots
        )
        .is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), allowed.path().join("escape")).unwrap();
            assert!(inside(&allowed.path().join("escape"), &roots).is_err());
        }
    }
}
