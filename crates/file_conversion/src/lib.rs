//! Local-only conversion shared by the CLI, desktop interface, and MCP adapter.
//! Commands always receive argument arrays; inputs are regular local files,
//! media demuxers/protocols and image coders are restricted, and publishing never
//! replaces an existing output. Image conversions use the first frame/page.

use anyhow::{anyhow, bail, Context, Result};
use common::VideoFormat;
use process_io::{join_pipe, InterruptiblePipe, OwnedProcess, PIPE_POLL_INTERVAL, PIPE_STOP_GRACE};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

const MEDIA_FORMATS: &str =
    "mov,matroska,avi,flv,asf,mpegts,mpeg,mpegvideo,m4v,ogg,mp3,aac,wav,flac,aiff";
const CAPTURE_LIMIT: usize = 1024 * 1024;
const STDERR_LIMIT: usize = 32 * 1024;
const CONVERSION_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const PIPE_DRAIN_GRACE: Duration = Duration::from_millis(250);
const PROGRESS_QUEUE_LIMIT: usize = 64;
const PROGRESS_LINES_PER_TICK: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetFormat {
    Mp4,
    Mkv,
    Mov,
    Webm,
    Mp3,
    M4a,
    Wav,
    Flac,
    Ogg,
    Png,
    Jpg,
    Webp,
    Tiff,
}

impl TargetFormat {
    pub fn all() -> &'static [Self] {
        &[
            Self::Mp4,
            Self::Mkv,
            Self::Mov,
            Self::Webm,
            Self::Mp3,
            Self::M4a,
            Self::Wav,
            Self::Flac,
            Self::Ogg,
            Self::Png,
            Self::Jpg,
            Self::Webp,
            Self::Tiff,
        ]
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "mkv",
            Self::Mov => "mov",
            Self::Webm => "webm",
            Self::Mp3 => "mp3",
            Self::M4a => "m4a",
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
            Self::Png => "png",
            Self::Jpg => "jpg",
            Self::Webp => "webp",
            Self::Tiff => "tiff",
        }
    }

    pub fn is_image(self) -> bool {
        matches!(self, Self::Png | Self::Jpg | Self::Webp | Self::Tiff)
    }

    pub fn is_audio(self) -> bool {
        matches!(
            self,
            Self::Mp3 | Self::M4a | Self::Wav | Self::Flac | Self::Ogg
        )
    }

    fn video(self) -> Option<VideoFormat> {
        match self {
            Self::Mp4 => Some(VideoFormat::MP4),
            Self::Mkv => Some(VideoFormat::MKV),
            Self::Mov => Some(VideoFormat::MOV),
            Self::Webm => Some(VideoFormat::WebM),
            _ => None,
        }
    }

    fn coder(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpg => "JPEG",
            Self::Webp => "WEBP",
            Self::Tiff => "TIFF",
            _ => unreachable!("coder requested for a non-image target"),
        }
    }
}

impl fmt::Display for TargetFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.extension())
    }
}

impl FromStr for TargetFormat {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        let value = value.to_ascii_lowercase();
        let value = match value.as_str() {
            "jpeg" => "jpg",
            "tif" => "tiff",
            other => other,
        };
        Self::all()
            .iter()
            .copied()
            .find(|format| format.extension() == value)
            .ok_or_else(|| anyhow!("Unsupported target format: {value}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionRequest {
    pub input: PathBuf,
    pub output: PathBuf,
    pub format: TargetFormat,
}

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);
impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub fraction: f32,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionResult {
    pub output: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Validating,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueEvent {
    pub index: usize,
    pub state: JobState,
    pub progress: f32,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResult {
    pub index: usize,
    pub state: JobState,
    pub output: PathBuf,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueReport {
    pub jobs: Vec<JobResult>,
}
impl QueueReport {
    pub fn success(&self) -> bool {
        self.jobs.iter().all(|job| job.state == JobState::Completed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyStatus {
    pub name: String,
    pub available: bool,
    pub path: Option<PathBuf>,
}

/// Executable discovery works for Finder-launched apps as well as login shells.
pub fn dependencies() -> Vec<DependencyStatus> {
    [
        ("FFmpeg", "ffmpeg"),
        ("ffprobe", "ffprobe"),
        ("ImageMagick", "magick"),
    ]
    .into_iter()
    .map(|(name, executable)| {
        let path = find_executable(executable);
        DependencyStatus {
            name: name.into(),
            available: path.is_some(),
            path,
        }
    })
    .collect()
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let mut directories: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ]);
    directories
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|path| {
            let Ok(metadata) = fs::metadata(path) else {
                return false;
            };
            if !metadata.is_file() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        })
}

fn required_tool(name: &str) -> Result<PathBuf> {
    find_executable(name).ok_or_else(|| {
        anyhow!("Required tool '{name}' was not found. Install FFmpeg / ImageMagick and retry.")
    })
}

/// Plan stable names before any conversion. Existing files and duplicate output
/// names are rejected so callers can present the entire batch before execution.
pub fn output_requests(
    inputs: &[PathBuf],
    output_dir: &Path,
    format: TargetFormat,
) -> Result<Vec<ConversionRequest>> {
    if inputs.is_empty() {
        bail!("Choose at least one input file");
    }
    let mut outputs = HashSet::new();
    let mut requests = Vec::with_capacity(inputs.len());
    for input in inputs {
        let canonical = local_input(input)?;
        let stem = input
            .file_stem()
            .filter(|stem| !stem.is_empty())
            .ok_or_else(|| anyhow!("Input has no filename: {}", input.display()))?;
        let mut filename = stem.to_os_string();
        filename.push(format!(".converted.{}", format.extension()));
        let output = normalize_output(&output_dir.join(filename), format, &canonical)?;
        // Conservatively reject case-only collisions, including on macOS's
        // default case-insensitive filesystem. Publishing is no-clobber too.
        if !outputs.insert(output.to_string_lossy().to_lowercase()) {
            bail!("Duplicate output name in batch: {}. Choose separate destinations or rename the inputs.", output.display());
        }
        requests.push(ConversionRequest {
            input: canonical,
            output,
            format,
        });
    }
    Ok(requests)
}

fn local_input(path: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("Cannot open local input: {}", path.display()))?;
    if !fs::metadata(&canonical)?.is_file() {
        bail!("Input must be a regular local file: {}", path.display());
    }
    Ok(canonical)
}

fn normalize_output(path: &Path, format: TargetFormat, input: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .ok_or_else(|| anyhow!("Output must have a filename"))?;
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .ok_or_else(|| anyhow!("Output filename needs a .{} extension", format.extension()))?;
    if extension.parse::<TargetFormat>().ok() != Some(format) {
        bail!("Output extension '.{extension}' does not match requested format {format}");
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let resolved = normalize_directory(parent)?.join(filename);
    if resolved == input {
        bail!("Input and output must be different files");
    }
    if fs::symlink_metadata(&resolved).is_ok() {
        bail!("Output already exists: {}", resolved.display());
    }
    Ok(resolved)
}

fn normalize_directory(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut result = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => {
                result.push(other.as_os_str());
            }
        }
        if result.exists() {
            result = fs::canonicalize(&result)?;
        }
    }
    if result.exists() && !result.is_dir() {
        bail!("Output parent is not a directory: {}", result.display());
    }
    Ok(result)
}

pub fn run_queue(
    requests: &[ConversionRequest],
    cancel: &CancelToken,
    mut on_event: impl FnMut(QueueEvent),
) -> QueueReport {
    for index in 0..requests.len() {
        on_event(QueueEvent {
            index,
            state: JobState::Queued,
            progress: 0.0,
            message: "Queued".into(),
        });
    }
    let mut jobs = Vec::with_capacity(requests.len());
    for (index, request) in requests.iter().enumerate() {
        let mut last_progress = 0.0_f32;
        let result = convert(request, cancel, |progress| {
            // The queue emits its terminal event after convert returns. Do not
            // briefly regress from Validating to Running at fraction 1.0.
            if progress.fraction >= 1.0 {
                return;
            }
            last_progress = last_progress.max(progress.fraction);
            let state = if progress.message.starts_with("Validating") {
                JobState::Validating
            } else {
                JobState::Running
            };
            on_event(QueueEvent {
                index,
                state,
                progress: last_progress,
                message: progress.message,
            });
        });
        let (state, error) = match result {
            Ok(_) => (JobState::Completed, None),
            Err(error) if cancel.is_cancelled() => {
                (JobState::Cancelled, Some(format!("{error:#}")))
            }
            Err(error) => (JobState::Failed, Some(format!("{error:#}"))),
        };
        on_event(QueueEvent {
            index,
            state,
            progress: if state == JobState::Completed {
                1.0
            } else {
                last_progress
            },
            message: error
                .clone()
                .unwrap_or_else(|| "Completed and validated".into()),
        });
        jobs.push(JobResult {
            index,
            state,
            output: request.output.clone(),
            error,
        });
    }
    QueueReport { jobs }
}

pub fn convert(
    request: &ConversionRequest,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(Progress),
) -> Result<ConversionResult> {
    check_cancelled(cancel)?;
    emit(
        &mut on_progress,
        0.0,
        "Checking local input and destination",
    );
    let input = local_input(&request.input)?;
    let output = normalize_output(&request.output, request.format, &input)?;
    let parent = output.parent().context("Output has no parent directory")?;
    let source_image = image_coder(&input).is_some();
    if request.format.is_image() != source_image {
        bail!("Image conversions require a supported raster image input; audio/video conversions require a media input");
    }
    // Complete dependency/input inspection before creating the destination.
    let tools = if request.format.is_image() {
        vec![required_tool("magick")?]
    } else {
        validate_media_extension(&input)?;
        vec![required_tool("ffmpeg")?, required_tool("ffprobe")?]
    };
    fs::create_dir_all(parent)
        .with_context(|| format!("Cannot create output directory: {}", parent.display()))?;
    let temporary = tempfile::Builder::new()
        .prefix(".video-toolkit-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .context("Cannot create a temporary output in the destination directory")?;
    if request.format.is_image() {
        convert_image(
            &tools[0],
            &input,
            temporary.path(),
            request.format,
            cancel,
            &mut on_progress,
        )?;
    } else {
        convert_media(
            &tools[0],
            &tools[1],
            &input,
            temporary.path(),
            request.format,
            cancel,
            &mut on_progress,
        )?;
    }
    check_cancelled(cancel)?;
    let bytes = fs::metadata(temporary.path())?.len();
    if bytes == 0 {
        bail!("Conversion produced an empty output");
    }
    // Reopen the path because an encoder may replace its temporary output inode.
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(temporary.path())?
        .sync_all()?;
    check_cancelled(cancel)?;
    temporary.persist_noclobber(&output).map_err(|error| {
        anyhow!("Cannot publish output without replacing an existing file: {error}")
    })?;
    emit(&mut on_progress, 1.0, "Completed and validated");
    Ok(ConversionResult { output, bytes })
}

fn emit(callback: &mut impl FnMut(Progress), fraction: f32, message: &str) {
    callback(Progress {
        fraction: fraction.clamp(0.0, 1.0),
        message: message.into(),
    });
}

fn check_cancelled(cancel: &CancelToken) -> Result<()> {
    if cancel.is_cancelled() {
        bail!("Conversion cancelled");
    }
    Ok(())
}

fn image_coder(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("PNG"),
        "jpg" | "jpeg" => Some("JPEG"),
        "webp" => Some("WEBP"),
        "tif" | "tiff" => Some("TIFF"),
        "gif" => Some("GIF"),
        "bmp" => Some("BMP"),
        // ImageMagick exposes both filename extensions through its HEIC coder.
        // Decoding is conditional on the installed libheif support; HEIC/HEIF
        // are inputs only and never expand the supported output format list.
        "heic" | "heif" => Some("HEIC"),
        _ => None,
    }
}

fn validate_media_extension(path: &Path) -> Result<()> {
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if ![
        "mp4", "mkv", "mov", "webm", "avi", "flv", "wmv", "ts", "m4v", "mpeg", "mpg", "vob", "ogv",
        "mp3", "m4a", "aac", "wav", "flac", "ogg", "aif", "aiff",
    ]
    .contains(&extension.as_str())
    {
        bail!("Unsupported local media input extension '.{extension}'. Playlists and remote inputs are not accepted.");
    }
    Ok(())
}

#[derive(Deserialize)]
struct Probe {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<ProbeFormat>,
}
#[derive(Deserialize)]
struct Stream {
    codec_type: Option<String>,
    duration: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
}
#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}
impl Probe {
    fn has(&self, kind: &str) -> bool {
        self.streams
            .iter()
            .any(|stream| stream.codec_type.as_deref() == Some(kind))
    }
    fn duration(&self) -> Option<f64> {
        self.format
            .as_ref()
            .and_then(|format| format.duration.as_deref())
            .and_then(|value| value.parse::<f64>().ok())
            .or_else(|| {
                self.streams
                    .iter()
                    .filter_map(|stream| stream.duration.as_deref()?.parse::<f64>().ok())
                    .reduce(f64::max)
            })
            .filter(|duration| duration.is_finite() && *duration > 0.0)
    }

    fn stream_duration(&self, kind: &str) -> Option<f64> {
        self.streams
            .iter()
            .filter(|stream| stream.codec_type.as_deref() == Some(kind))
            .find_map(|stream| stream.duration.as_deref()?.parse::<f64>().ok())
            .filter(|duration| duration.is_finite() && *duration > 0.0)
    }
}

fn restricted_input(command: &mut Command, input: &Path) {
    command
        .args([
            "-protocol_whitelist",
            "file",
            "-format_whitelist",
            MEDIA_FORMATS,
            "-i",
        ])
        .arg(input);
}

fn probe(tool: &Path, input: &Path, cancel: &CancelToken) -> Result<Probe> {
    let mut command = Command::new(tool);
    command.args([
        "-v",
        "error",
        "-show_streams",
        "-show_format",
        "-of",
        "json",
    ]);
    restricted_input(&mut command, input);
    let result = run_process(command, cancel, PROBE_TIMEOUT, |_| {})?;
    serde_json::from_slice(&result.stdout).context("ffprobe returned invalid or excessive metadata")
}

fn convert_media(
    ffmpeg: &Path,
    ffprobe: &Path,
    input: &Path,
    temporary: &Path,
    format: TargetFormat,
    cancel: &CancelToken,
    callback: &mut impl FnMut(Progress),
) -> Result<()> {
    emit(callback, 0.02, "Inspecting media streams");
    let info = probe(ffprobe, input, cancel).context("Cannot inspect local media input")?;
    let required = if format.is_audio() { "audio" } else { "video" };
    if !info.has(required) {
        bail!("Input does not contain a {required} stream required for {format}");
    }
    let mut command = Command::new(ffmpeg);
    command.args([
        "-hide_banner",
        "-v",
        "error",
        "-nostdin",
        "-y",
        "-progress",
        "pipe:1",
        "-stats_period",
        "0.2",
    ]);
    restricted_input(&mut command, input);
    command.args(["-map_metadata", "-1", "-map_chapters", "-1", "-sn", "-dn"]);
    if let Some(video) = format.video() {
        command.args(["-map", "0:v:0", "-map", "0:a:0?"]);
        command.args(video.encoding_options());
        command.args(["-f", video.muxer()]);
    } else {
        command.args(["-map", "0:a:0", "-vn"]);
        command.args(match format {
            TargetFormat::Mp3 => &["-c:a", "libmp3lame", "-q:a", "2", "-f", "mp3"][..],
            TargetFormat::M4a => &["-c:a", "aac", "-b:a", "192k", "-f", "ipod"][..],
            TargetFormat::Wav => &["-c:a", "pcm_s16le", "-f", "wav"][..],
            TargetFormat::Flac => &["-c:a", "flac", "-f", "flac"][..],
            TargetFormat::Ogg => &["-c:a", "libvorbis", "-q:a", "5", "-f", "ogg"][..],
            _ => unreachable!(),
        });
    }
    command.arg(temporary);
    emit(callback, 0.05, "Converting media");
    let mut last_fraction = 0.05f32;
    run_process(command, cancel, CONVERSION_TIMEOUT, |line| {
        if let Some(fraction) = media_fraction(line, info.duration()) {
            last_fraction = last_fraction.max(0.05 + 0.8 * fraction);
            emit(callback, last_fraction, "Converting media");
        }
    })
    .context("FFmpeg conversion failed")?;
    emit(
        callback,
        0.9,
        "Validating output streams and decoding the complete output",
    );
    let result = probe(ffprobe, temporary, cancel).context("Output stream validation failed")?;
    if !result.has(required) {
        bail!("Output is missing its required {required} stream");
    }
    if format.video().is_some() {
        if !result.streams.iter().any(|stream| {
            stream.codec_type.as_deref() == Some("video")
                && stream.width.unwrap_or(0) > 0
                && stream.height.unwrap_or(0) > 0
        }) {
            bail!("Output video has invalid dimensions");
        }
        if info.has("audio") && !result.has("audio") {
            bail!("Output lost the input audio stream");
        }
    }
    // Prefer stream durations because a video's audio track may legitimately
    // end before its video. Container duration is a fallback for containers
    // such as Matroska which do not always report per-stream duration.
    let (input_duration, output_duration) = match (
        info.stream_duration(required),
        result.stream_duration(required),
    ) {
        (Some(expected), Some(actual)) => (Some(expected), Some(actual)),
        _ if !format.is_audio() || !info.has("video") => (info.duration(), result.duration()),
        (Some(expected), None) => (Some(expected), result.duration()),
        // Do not mistake a longer video track's container duration for an
        // unknown audio track duration during audio extraction.
        _ => (None, None),
    };
    if let (Some(expected), Some(actual)) = (input_duration, output_duration) {
        let tolerance = 0.5_f64.max(expected * 0.05);
        if (expected - actual).abs() > tolerance {
            bail!("Output duration differs from input: expected {expected:.3}s, got {actual:.3}s (tolerance {tolerance:.3}s)");
        }
    }
    let mut decode = Command::new(ffmpeg);
    decode.args(["-hide_banner", "-v", "error", "-nostdin", "-xerror"]);
    restricted_input(&mut decode, temporary);
    decode.args(["-map", "0:v?", "-map", "0:a?", "-f", "null", "-"]);
    run_process(decode, cancel, CONVERSION_TIMEOUT, |_| {})
        .context("Complete output decode failed")?;
    Ok(())
}

fn media_fraction(line: &str, duration: Option<f64>) -> Option<f32> {
    let duration = duration?;
    let micros = line
        .strip_prefix("out_time_us=")?
        .trim()
        .parse::<f64>()
        .ok()?;
    let fraction = micros / 1_000_000.0 / duration;
    fraction
        .is_finite()
        .then_some(fraction.clamp(0.0, 1.0) as f32)
}

// A private ImageMagick policy blocks delegates, URL/pseudo-image coders and
// indirect @file reads. Explicit raster prefixes prevent content sniffing from
// selecting a different coder. Copying to a safe name avoids filename syntax.
const IMAGE_POLICY: &str = r#"<policymap>
<policy domain="delegate" rights="none" pattern="*"/>
<policy domain="filter" rights="none" pattern="*"/>
<policy domain="coder" rights="none" pattern="*"/>
<policy domain="coder" rights="read|write" pattern="{PNG,JPEG,WEBP,TIFF,GIF,BMP,NULL}"/>
<policy domain="coder" rights="read" pattern="HEIC"/>
<policy domain="path" rights="none" pattern="@*"/>
<policy domain="resource" name="memory" value="256MiB"/>
<policy domain="resource" name="map" value="512MiB"/>
<policy domain="resource" name="disk" value="1GiB"/>
<policy domain="resource" name="width" value="32KP"/>
<policy domain="resource" name="height" value="32KP"/>
<policy domain="resource" name="list-length" value="256"/>
<policy domain="resource" name="thread" value="2"/>
<policy domain="resource" name="time" value="300"/>
</policymap>"#;

fn image_command(tool: &Path, sandbox: &Path) -> Command {
    let mut command = Command::new(tool);
    command
        .env("MAGICK_CONFIGURE_PATH", sandbox)
        .env("MAGICK_TEMPORARY_PATH", sandbox);
    command
}

fn coder_path(coder: &str, path: &Path, first_frame: bool) -> OsString {
    let mut argument = OsString::from(format!("{coder}:"));
    argument.push(path.as_os_str());
    if first_frame {
        argument.push("[0]");
    }
    argument
}

fn convert_image(
    magick: &Path,
    input: &Path,
    temporary: &Path,
    format: TargetFormat,
    cancel: &CancelToken,
    callback: &mut impl FnMut(Progress),
) -> Result<()> {
    let sandbox = tempfile::Builder::new()
        .prefix("video-toolkit-image-")
        .tempdir()?;
    fs::write(sandbox.path().join("policy.xml"), IMAGE_POLICY)?;
    let source = sandbox.path().join("source.raster");
    fs::copy(input, &source).context("Cannot stage the local image")?;
    check_cancelled(cancel)?;
    let coder = image_coder(input).context("Unsupported raster input")?;
    emit(callback, 0.1, "Converting raster image (first frame/page)");
    let mut command = image_command(magick, sandbox.path());
    command
        .arg(coder_path(coder, &source, true))
        .args(["-auto-orient", "-strip"]);
    if format == TargetFormat::Jpg {
        command.args([
            "-background",
            "white",
            "-alpha",
            "remove",
            "-alpha",
            "off",
            "-quality",
            "90",
        ]);
    }
    command.arg(coder_path(format.coder(), temporary, false));
    run_process(command, cancel, Duration::from_secs(300), |_| {}).with_context(|| {
        if coder == "HEIC" {
            "HEIC/HEIF input could not be converted; use a valid local image and an ImageMagick build with libheif decoding support"
        } else {
            "ImageMagick conversion failed"
        }
    })?;
    emit(
        callback,
        0.9,
        "Validating image dimensions and decoding the complete output",
    );
    let mut identify = image_command(magick, sandbox.path());
    identify
        .args(["identify", "-regard-warnings", "-format", "%m %w %h"])
        .arg(coder_path(format.coder(), temporary, false));
    let result = run_process(identify, cancel, Duration::from_secs(300), |_| {})
        .context("Image output inspection failed")?;
    let metadata = String::from_utf8_lossy(&result.stdout);
    let fields: Vec<_> = metadata.split_whitespace().collect();
    if fields.len() != 3
        || fields[0] != format.coder()
        || fields[1].parse::<u64>().unwrap_or(0) == 0
        || fields[2].parse::<u64>().unwrap_or(0) == 0
    {
        bail!("Image output has an invalid format or dimensions: {metadata}");
    }
    let mut decode = image_command(magick, sandbox.path());
    decode
        .arg("-regard-warnings")
        .arg(coder_path(format.coder(), temporary, false))
        .arg("null:");
    run_process(decode, cancel, Duration::from_secs(300), |_| {})
        .context("Complete image output decode failed")?;
    Ok(())
}

struct ProcessOutput {
    stdout: Vec<u8>,
}

struct StopPipeReaders(Arc<AtomicBool>);

impl Drop for StopPipeReaders {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Shared subprocess primitives for local conversion/workflow adapters.
/// This implementation module is not a stable public API.
#[doc(hidden)]
pub mod process_io {
    use anyhow::{anyhow, bail, Context, Result};
    use std::io::Read;
    use std::process::{Child, Command, ExitStatus};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    pub const PIPE_POLL_INTERVAL: Duration = Duration::from_millis(25);
    pub const PIPE_STOP_GRACE: Duration = Duration::from_millis(100);

    pub struct OwnedProcess {
        child: Child,
        reaped: bool,
    }

    impl OwnedProcess {
        pub fn spawn(command: &mut Command) -> std::io::Result<Self> {
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.process_group(0);
                unsafe {
                    command.pre_exec(|| {
                        // Repeat setpgid here so identity verification does not depend
                        // on std's relative process_group/pre_exec implementation order.
                        // Only async-signal-safe calls and fixed OS errors before exec.
                        if libc::setpgid(0, 0) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        let pid = libc::getpid();
                        let group = libc::getpgid(0);
                        let parent_group = libc::getpgid(libc::getppid());
                        if pid <= 1 || group != pid || parent_group < 0 || group == parent_group {
                            return Err(std::io::Error::from_raw_os_error(libc::EPERM));
                        }
                        Ok(())
                    });
                }
            }
            Ok(Self {
                child: command.spawn()?,
                reaped: false,
            })
        }

        pub fn take_stdin(&mut self) -> Option<std::process::ChildStdin> {
            self.child.stdin.take()
        }

        pub fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
            self.child.stdout.take()
        }

        pub fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
            self.child.stderr.take()
        }

        #[cfg(unix)]
        fn observe_exit(&mut self) -> Result<bool> {
            // WNOWAIT pins our child PID, including after leader exit, until group cleanup.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            loop {
                let result = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        self.child.id() as libc::id_t,
                        &mut info,
                        libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                    )
                };
                if result == 0 {
                    return Ok(unsafe { info.si_pid() } != 0);
                }
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                if error.raw_os_error() == Some(libc::ECHILD) {
                    // Another reaper/host policy has released ownership. Never signal this PID.
                    self.reaped = true;
                }
                return Err(error).context("Cannot verify owned conversion process");
            }
        }

        #[cfg(unix)]
        fn kill_owned_group(&mut self) -> Result<()> {
            if self.reaped {
                return Ok(());
            }
            let mut exited = self.observe_exit()?;
            let pid = libc::pid_t::try_from(self.child.id())
                .context("Conversion process ID is out of range")?;
            let group = unsafe { libc::getpgid(pid) };
            let group_error = (group < 0).then(std::io::Error::last_os_error);
            let parent_group = unsafe { libc::getpgrp() };
            if !exited
                && group_error.as_ref().and_then(std::io::Error::raw_os_error) == Some(libc::ESRCH)
            {
                // The leader may have exited between the first observation and getpgid.
                exited = self.observe_exit()?;
            }
            // macOS hides an exited leader from getpgid even before wait reaps it.
            // spawn's pre_exec verified the original group; current WNOWAIT ownership
            // still pins its numeric PID/group. No other lookup error permits fallback.
            let exited_group = exited
                && group_error.as_ref().and_then(std::io::Error::raw_os_error) == Some(libc::ESRCH);
            if pid <= 1 || (group != pid && !exited_group) || pid == parent_group {
                bail!("Cannot verify private conversion process group; refusing group signal");
            }
            // The unreaped, identity-checked leader prevents numeric PID/group reuse here.
            if unsafe { libc::kill(-pid, libc::SIGKILL) } != 0 {
                let error = std::io::Error::last_os_error();
                // A group containing only the exited leader has no signalable members.
                if error.raw_os_error() != Some(libc::ESRCH) {
                    #[cfg(target_os = "macos")]
                    if error.raw_os_error() == Some(libc::EPERM)
                        && self.macos_group_has_no_live_members(pid)?
                    {
                        return Ok(());
                    }
                    return Err(error).context("Cannot stop conversion process group");
                }
            }
            Ok(())
        }

        #[cfg(target_os = "macos")]
        fn macos_group_has_no_live_members(&mut self, group: libc::pid_t) -> Result<bool> {
            // Darwin killpg reports EPERM when it finds only zombies. Do not swallow
            // that error for a live or inaccessible group: prove a complete, stable
            // snapshot of this owned group has no live members while its leader stays
            // unreaped. macOS hides the leader from proc_pidinfo, so WNOWAIT supplies
            // its positive exited-state evidence instead.
            if !self.observe_exit()? {
                return Ok(false);
            }
            let members = macos_group_members(group)?;
            if !members.contains(&group) {
                bail!("Exited conversion leader is missing from its process group snapshot");
            }
            for &pid in &members {
                if pid == group {
                    continue;
                }
                let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
                let size = std::mem::size_of_val(&info) as libc::c_int;
                let bytes = unsafe {
                    libc::proc_pidinfo(
                        pid,
                        libc::PROC_PIDTBSDINFO,
                        0,
                        (&mut info as *mut libc::proc_bsdinfo).cast(),
                        size,
                    )
                };
                if bytes != size || info.pbi_pid != pid as u32 || info.pbi_pgid != group as u32 {
                    bail!("Cannot verify conversion process group member state");
                }
                if info.pbi_status != libc::SZOMB {
                    return Ok(false);
                }
            }
            if macos_group_members(group)? != members {
                bail!("Conversion process group changed during exited-state verification");
            }
            self.observe_exit()
        }

        pub fn poll_exit(&mut self) -> Result<Option<ExitStatus>> {
            #[cfg(unix)]
            {
                if !self.observe_exit()? {
                    return Ok(None);
                }
                self.kill_owned_group()?;
                let status = self
                    .child
                    .wait()
                    .context("Cannot reap conversion process")?;
                self.reaped = true;
                Ok(Some(status))
            }
            #[cfg(not(unix))]
            {
                let status = self.child.try_wait()?;
                if status.is_some() {
                    self.reaped = true;
                }
                Ok(status)
            }
        }

        pub fn stop(&mut self) -> Result<()> {
            if self.reaped {
                return Ok(());
            }
            #[cfg(unix)]
            let group_result = self.kill_owned_group();
            #[cfg(unix)]
            {
                if self.reaped {
                    return group_result;
                }
                // Even if group identity was refused, only terminate a verified, unreaped child.
                self.observe_exit()?;
            }
            let killed = self.child.kill();
            if let Err(error) = killed {
                #[cfg(unix)]
                if !self.observe_exit()? {
                    return Err(error).context("Cannot stop conversion process");
                }
                #[cfg(not(unix))]
                if self.child.try_wait()?.is_none() {
                    return Err(error).context("Cannot stop conversion process");
                }
            }
            self.child
                .wait()
                .context("Cannot reap stopped conversion process")?;
            self.reaped = true;
            #[cfg(unix)]
            return group_result;
            #[cfg(not(unix))]
            Ok(())
        }
    }

    impl Drop for OwnedProcess {
        fn drop(&mut self) {
            let _ = self.stop();
        }
    }

    #[cfg(target_os = "macos")]
    fn macos_group_members(group: libc::pid_t) -> Result<Vec<libc::pid_t>> {
        let mut members = [0 as libc::pid_t; 256];
        // libproc returns PID count (not byte count); a zero result can also mean an
        // error. Neither zero nor a full/truncated buffer proves an empty group.
        let count = unsafe {
            libc::proc_listpgrppids(
                group,
                members.as_mut_ptr().cast(),
                std::mem::size_of_val(&members) as libc::c_int,
            )
        };
        if count <= 0 || count as usize >= members.len() {
            bail!("Cannot obtain a complete conversion process group snapshot");
        }
        let mut members = members[..count as usize].to_vec();
        members.sort_unstable();
        if members.iter().any(|&pid| pid <= 1) || members.windows(2).any(|pair| pair[0] == pair[1])
        {
            bail!("Conversion process group snapshot contains invalid identities");
        }
        Ok(members)
    }

    #[cfg(unix)]
    pub trait PipeHandle: std::os::fd::AsRawFd {}
    #[cfg(unix)]
    impl<T: std::os::fd::AsRawFd> PipeHandle for T {}
    #[cfg(windows)]
    pub trait PipeHandle: std::os::windows::io::AsRawHandle {}
    #[cfg(windows)]
    impl<T: std::os::windows::io::AsRawHandle> PipeHandle for T {}
    #[cfg(not(any(unix, windows)))]
    pub trait PipeHandle {}
    #[cfg(not(any(unix, windows)))]
    impl<T> PipeHandle for T {}

    pub struct InterruptiblePipe<R> {
        reader: R,
        stopped: Arc<AtomicBool>,
    }

    impl<R> InterruptiblePipe<R> {
        pub fn new(reader: R, stopped: Arc<AtomicBool>) -> Self {
            Self { reader, stopped }
        }
    }

    impl<R: Read + PipeHandle> Read for InterruptiblePipe<R> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if buffer.is_empty() {
                return Ok(0);
            }
            read_pipe(&mut self.reader, buffer, &self.stopped)
        }
    }

    fn pipe_interrupted() -> std::io::Error {
        // Not ErrorKind::Interrupted: buffered readers may transparently retry that kind.
        std::io::Error::other("Process pipe reader stopped before EOF")
    }

    #[cfg(unix)]
    fn read_pipe(
        reader: &mut (impl Read + PipeHandle),
        buffer: &mut [u8],
        stopped: &AtomicBool,
    ) -> std::io::Result<usize> {
        let mut descriptor = libc::pollfd {
            fd: reader.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            if stopped.load(Ordering::Relaxed) {
                return Err(pipe_interrupted());
            }
            let result = unsafe {
                libc::poll(
                    &mut descriptor,
                    1,
                    PIPE_POLL_INTERVAL.as_millis() as libc::c_int,
                )
            };
            if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if result > 0 {
                if descriptor.revents & libc::POLLNVAL != 0 {
                    return Err(std::io::Error::other("Process pipe handle is invalid"));
                }
                // Only this thread consumes the anonymous pipe. POLLIN/POLLHUP cannot
                // lose readiness to a competing reader before this read.
                return reader.read(buffer);
            }
        }
    }

    #[cfg(windows)]
    fn read_pipe(
        reader: &mut (impl Read + PipeHandle),
        buffer: &mut [u8],
        stopped: &AtomicBool,
    ) -> std::io::Result<usize> {
        // ABI/types/error values match the cached windows-sys 0.59 Windows API bindings.
        #[link(name = "kernel32")]
        extern "system" {
            fn PeekNamedPipe(
                pipe: std::os::windows::io::RawHandle,
                buffer: *mut std::ffi::c_void,
                buffer_size: u32,
                bytes_read: *mut u32,
                bytes_available: *mut u32,
                bytes_left: *mut u32,
            ) -> i32;
        }
        loop {
            if stopped.load(Ordering::Relaxed) {
                return Err(pipe_interrupted());
            }
            let mut available = 0;
            let result = unsafe {
                PeekNamedPipe(
                    reader.as_raw_handle(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut available,
                    std::ptr::null_mut(),
                )
            };
            if result == 0 {
                let error = std::io::Error::last_os_error();
                if matches!(error.raw_os_error(), Some(109 | 232 | 233)) {
                    return Ok(0); // Broken pipe / no data / pipe disconnected: EOF.
                }
                return Err(error);
            }
            if available > 0 {
                let count = buffer.len().min(available as usize);
                return reader.read(&mut buffer[..count]);
            }
            thread::sleep(PIPE_POLL_INTERVAL);
        }
    }

    #[cfg(not(any(unix, windows)))]
    fn read_pipe(
        reader: &mut (impl Read + PipeHandle),
        buffer: &mut [u8],
        stopped: &AtomicBool,
    ) -> std::io::Result<usize> {
        if stopped.load(Ordering::Relaxed) {
            return Err(pipe_interrupted());
        }
        // Unsupported hosts still have a bounded caller-side shutdown, but do not
        // promise interruptible OS pipe reads as Unix and Windows do.
        reader.read(buffer)
    }

    pub fn join_pipe<T>(reader: thread::JoinHandle<std::io::Result<T>>, name: &str) -> Result<T> {
        // Never join an unfinished thread, even if an unsupported host cannot stop its read.
        if !reader.is_finished() {
            bail!("Process {name} reader did not stop within the safety deadline");
        }
        reader
            .join()
            .map_err(|_| anyhow!("Process {name} reader panicked"))?
            .with_context(|| format!("Cannot completely read process {name}"))
    }
}

fn read_stdout(
    reader: impl Read,
    sender: mpsc::SyncSender<String>,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut reader = BufReader::new(reader);
    let mut captured = Vec::new();
    let mut oversized = false;
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        for &byte in available {
            if captured.len() < CAPTURE_LIMIT {
                captured.push(byte);
            } else {
                oversized = true;
            }
            if byte == b'\n' {
                if line.starts_with(b"out_time_us=") {
                    // Progress is advisory. Never block pipe draining or grow an
                    // unbounded queue when the consumer is slow or interrupted.
                    let _ = sender.try_send(String::from_utf8_lossy(&line).into_owned());
                }
                line.clear();
            } else if line.len() < 1024 {
                line.push(byte);
            }
        }
        let consumed = available.len();
        reader.consume(consumed);
    }
    Ok((captured, oversized))
}

fn read_stderr(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut tail = VecDeque::with_capacity(STDERR_LIMIT);
    let mut buffer = [0u8; 4096];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        for &byte in &buffer[..count] {
            if tail.len() == STDERR_LIMIT {
                tail.pop_front();
            }
            tail.push_back(byte);
        }
    }
    Ok(tail.into_iter().collect())
}

fn status_error(status: ExitStatus, stderr: &[u8]) -> anyhow::Error {
    let detail = String::from_utf8_lossy(stderr);
    anyhow!("Process exited with {status}: {}", detail.trim())
}

fn dispatch_progress(
    receiver: &mpsc::Receiver<String>,
    on_line: &mut impl FnMut(&str),
    cancel: &CancelToken,
    start: Instant,
    timeout: Duration,
) {
    for _ in 0..PROGRESS_LINES_PER_TICK {
        if cancel.is_cancelled() || start.elapsed() >= timeout {
            break;
        }
        let Ok(line) = receiver.try_recv() else { break };
        on_line(&line);
    }
}

fn run_process(
    mut command: Command,
    cancel: &CancelToken,
    timeout: Duration,
    mut on_line: impl FnMut(&str),
) -> Result<ProcessOutput> {
    check_cancelled(cancel)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let name = command.get_program().to_string_lossy().into_owned();
    let mut process =
        OwnedProcess::spawn(&mut command).with_context(|| format!("Cannot start {name}"))?;
    let stdout = process
        .take_stdout()
        .context("Process stdout is unavailable")?;
    let stderr = process
        .take_stderr()
        .context("Process stderr is unavailable")?;
    let (sender, receiver) = mpsc::sync_channel(PROGRESS_QUEUE_LIMIT);
    let stop_readers = StopPipeReaders(Arc::new(AtomicBool::new(false)));
    let out_stopped = stop_readers.0.clone();
    let err_stopped = stop_readers.0.clone();
    let stdout_thread =
        thread::spawn(move || read_stdout(InterruptiblePipe::new(stdout, out_stopped), sender));
    let stderr_thread =
        thread::spawn(move || read_stderr(InterruptiblePipe::new(stderr, err_stopped)));
    let start = Instant::now();
    let mut interrupted = None;
    let status = loop {
        dispatch_progress(&receiver, &mut on_line, cancel, start, timeout);
        if cancel.is_cancelled() {
            interrupted = Some("Conversion cancelled");
        } else if start.elapsed() >= timeout {
            interrupted = Some("Conversion timed out");
        }
        if interrupted.is_some() {
            break process.stop().and_then(|()| {
                // Cancellation is returned below; no exit status will be consumed.
                Err(anyhow!("Conversion interrupted"))
            });
        }
        match process.poll_exit() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                let _ = process.stop();
                break Err(error);
            }
        }
    };
    // EOF must be observed on both pipes. Descendants outside our Unix group (or
    // on Windows, where only the direct child is owned) may retain a pipe after
    // leader exit. Allow buffered output to drain, then interrupt instead of
    // waiting indefinitely or returning truncated metadata as success.
    let drain_start = Instant::now();
    while !stdout_thread.is_finished() || !stderr_thread.is_finished() {
        dispatch_progress(&receiver, &mut on_line, cancel, start, timeout);
        if cancel.is_cancelled() {
            interrupted = Some("Conversion cancelled");
        } else if start.elapsed() >= timeout {
            interrupted = Some("Conversion timed out");
        }
        if interrupted.is_some() || status.is_err() || drain_start.elapsed() >= PIPE_DRAIN_GRACE {
            break;
        }
        thread::sleep(PIPE_POLL_INTERVAL);
    }
    stop_readers.0.store(true, Ordering::Relaxed);
    let stop_start = Instant::now();
    while (!stdout_thread.is_finished() || !stderr_thread.is_finished())
        && stop_start.elapsed() < PIPE_STOP_GRACE
    {
        thread::sleep(PIPE_POLL_INTERVAL);
    }
    let stdout = join_pipe(stdout_thread, "output");
    let stderr = join_pipe(stderr_thread, "error");
    if let Some(message) = interrupted {
        bail!("{message}");
    }
    let status = status?;
    let (stdout, oversized) = stdout?;
    let stderr = stderr?;
    if !status.success() {
        return Err(status_error(status, &stderr));
    }
    // FFmpeg progress can exceed this during a long conversion; only metadata
    // probes need the full capture. Prefix capture is intentionally bounded.
    if oversized && command.get_args().any(|arg| arg == "-show_streams") {
        bail!("Media metadata exceeded the 1 MiB safety limit");
    }
    dispatch_progress(&receiver, &mut on_line, cancel, start, timeout);
    check_cancelled(cancel)?;
    if start.elapsed() >= timeout {
        bail!("Conversion timed out");
    }
    Ok(ProcessOutput { stdout })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_roundtrip_and_aliases() {
        for format in TargetFormat::all() {
            assert_eq!(format.to_string().parse::<TargetFormat>().unwrap(), *format);
            assert_eq!(
                serde_json::from_str::<TargetFormat>(&serde_json::to_string(format).unwrap())
                    .unwrap(),
                *format
            );
        }
        assert_eq!("JPEG".parse::<TargetFormat>().unwrap(), TargetFormat::Jpg);
        assert!("pdf".parse::<TargetFormat>().is_err());
    }

    #[test]
    fn heic_and_heif_are_explicit_input_coders_only() {
        assert_eq!(image_coder(Path::new("phone.HEIC")), Some("HEIC"));
        assert_eq!(image_coder(Path::new("phone.HeIf")), Some("HEIC"));
        assert_eq!(image_coder(Path::new("drawing.svg")), None);
        assert!("heic".parse::<TargetFormat>().is_err());
        assert!("heif".parse::<TargetFormat>().is_err());
        assert_eq!(TargetFormat::all().len(), 13);
    }

    #[test]
    fn planning_rejects_existing_and_duplicate_destinations() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("one/same.mp4");
        let second = directory.path().join("two/same.mov");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, b"input").unwrap();
        fs::write(&second, b"input").unwrap();
        let output_dir = directory.path().join("outputs");
        assert!(output_requests(&[first.clone(), second], &output_dir, TargetFormat::Mp3).is_err());
        let request = output_requests(std::slice::from_ref(&first), &output_dir, TargetFormat::Mp3)
            .unwrap()
            .remove(0);
        assert_eq!(request.output.file_name().unwrap(), "same.converted.mp3");
        assert!(!output_dir.exists(), "planning does not create directories");
        fs::create_dir(&output_dir).unwrap();
        fs::write(&request.output, b"keep me").unwrap();
        assert!(output_requests(&[first], &output_dir, TargetFormat::Mp3).is_err());
        assert_eq!(fs::read(request.output).unwrap(), b"keep me");
    }

    #[test]
    fn pre_cancelled_queue_never_creates_output() {
        let directory = tempfile::tempdir().unwrap();
        let request = ConversionRequest {
            input: directory.path().join("missing.mp4"),
            output: directory.path().join("output.mp4"),
            format: TargetFormat::Mp4,
        };
        let cancel = CancelToken::default();
        cancel.cancel();
        let mut events = Vec::new();
        let report = run_queue(&[request], &cancel, |event| events.push(event));
        assert!(!report.success());
        assert_eq!(report.jobs[0].state, JobState::Cancelled);
        assert_eq!(events.first().unwrap().state, JobState::Queued);
        assert!(!directory.path().join("output.mp4").exists());
    }

    #[test]
    fn progress_is_bounded_and_unknown_time_is_ignored() {
        assert_eq!(media_fraction("out_time_us=500000", Some(1.0)), Some(0.5));
        assert_eq!(media_fraction("out_time_us=2000000", Some(1.0)), Some(1.0));
        assert_eq!(media_fraction("out_time_us=N/A", Some(1.0)), None);
        assert_eq!(media_fraction("out_time_us=500000", None), None);
    }

    #[test]
    fn rejects_remote_and_playlist_paths_before_starting_tools() {
        assert!(local_input(Path::new("https://example.com/video.mp4")).is_err());
        assert!(validate_media_extension(Path::new("playlist.m3u8")).is_err());
        assert!(validate_media_extension(Path::new("list.ffconcat")).is_err());
    }

    #[test]
    fn process_timeout_and_cancellation_reap_child_promptly() {
        let Some(sleep) = find_executable("sleep") else {
            return;
        };
        let cancel = CancelToken::default();
        let mut command = Command::new(&sleep);
        command.arg("10");
        let started = Instant::now();
        assert!(
            run_process(command, &cancel, Duration::from_millis(30), |_| {})
                .err()
                .unwrap()
                .to_string()
                .contains("timed out")
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        let other = cancel.clone();
        let thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            other.cancel();
        });
        let mut command = Command::new(sleep);
        command.arg("10");
        assert!(
            run_process(command, &cancel, Duration::from_secs(20), |_| {})
                .err()
                .unwrap()
                .to_string()
                .contains("cancelled")
        );
        thread.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn process_short_lived_leader_drains_complete_output() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "printf 'leader-output'; printf 'leader-error' >&2; exit 0",
        ]);
        let start = Instant::now();
        let output = run_process(
            command,
            &CancelToken::default(),
            Duration::from_secs(2),
            |_| {},
        )
        .unwrap();
        assert_eq!(output.stdout, b"leader-output");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn process_exit_closes_inherited_descendant_pipes_promptly() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "/bin/sleep 4 & printf 'leader-output'; printf 'leader-error' >&2; exit 0",
        ]);
        let started = Instant::now();
        let result = run_process(
            command,
            &CancelToken::default(),
            Duration::from_secs(2),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.stdout, b"leader-output");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn process_descendant_pipes_do_not_block_cancellation_or_timeout() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "/bin/sleep 4 & wait"]);
        let started = Instant::now();
        let error = run_process(
            command,
            &CancelToken::default(),
            Duration::from_millis(50),
            |_| {},
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("timed out"), "{error:#}");
        assert!(started.elapsed() < Duration::from_secs(2));

        let cancel = CancelToken::default();
        let other = cancel.clone();
        let cancelling = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            other.cancel();
        });
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "/bin/sleep 4 & wait"]);
        let started = Instant::now();
        let error = run_process(command, &cancel, Duration::from_secs(2), |_| {})
            .err()
            .unwrap();
        cancelling.join().unwrap();
        assert!(error.to_string().contains("cancelled"), "{error:#}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn interrupted_pipe_reader_does_not_claim_complete_output() {
        let (read, mut write) = std::os::unix::net::UnixStream::pair().unwrap();
        use std::io::Write;
        write.write_all(b"{\"streams\":[]}").unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let reader_stop = stopped.clone();
        let (sender, _receiver) = mpsc::sync_channel(PROGRESS_QUEUE_LIMIT);
        let reader =
            thread::spawn(move || read_stdout(InterruptiblePipe::new(read, reader_stop), sender));
        thread::sleep(PIPE_POLL_INTERVAL);
        stopped.store(true, Ordering::Relaxed);
        let started = Instant::now();
        while !reader.is_finished() && started.elapsed() < Duration::from_secs(1) {
            thread::sleep(PIPE_POLL_INTERVAL);
        }
        let error = join_pipe(reader, "output").unwrap_err();
        assert!(
            format!("{error:#}").contains("stopped before EOF"),
            "{error:#}"
        );
        // The writer remains live: valid-looking JSON is not equivalent to pipe EOF.
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(write);
    }

    #[test]
    fn progress_queue_is_bounded_without_truncating_capture() {
        let output = "out_time_us=123\n"
            .repeat(PROGRESS_QUEUE_LIMIT * 10)
            .into_bytes();
        let (sender, receiver) = mpsc::sync_channel(PROGRESS_QUEUE_LIMIT);
        let (captured, oversized) = read_stdout(std::io::Cursor::new(&output), sender).unwrap();
        assert_eq!(captured, output);
        assert!(!oversized);
        assert_eq!(receiver.try_iter().count(), PROGRESS_QUEUE_LIMIT);
    }

    #[test]
    fn progress_dispatch_has_a_quota_and_rechecks_cancellation() {
        let (sender, receiver) = mpsc::sync_channel(PROGRESS_QUEUE_LIMIT);
        for _ in 0..PROGRESS_QUEUE_LIMIT {
            sender.try_send("out_time_us=1".to_owned()).unwrap();
        }
        let cancel = CancelToken::default();
        let start = Instant::now();
        let mut calls = 0;
        dispatch_progress(
            &receiver,
            &mut |_| calls += 1,
            &cancel,
            start,
            Duration::from_secs(2),
        );
        assert_eq!(calls, PROGRESS_LINES_PER_TICK);
        dispatch_progress(
            &receiver,
            &mut |_| {
                calls += 1;
                cancel.cancel();
            },
            &cancel,
            start,
            Duration::from_secs(2),
        );
        assert_eq!(calls, PROGRESS_LINES_PER_TICK + 1);
        assert_eq!(receiver.try_iter().count(), PROGRESS_QUEUE_LIMIT - calls);
    }

    #[cfg(unix)]
    #[test]
    fn high_volume_progress_does_not_starve_timeout_checks() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "i=0; while [ \"$i\" -lt 5000 ]; do printf 'out_time_us=1\\n'; i=$((i+1)); done; /bin/sleep 4"]);
        let start = Instant::now();
        let error = run_process(
            command,
            &CancelToken::default(),
            Duration::from_millis(80),
            |_| {
                thread::sleep(Duration::from_millis(1));
            },
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("timed out"), "{error:#}");
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
