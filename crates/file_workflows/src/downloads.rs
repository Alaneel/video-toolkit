//! Explicit HTTP downloads and yt-dlp media jobs, staged before publication.
use crate::{support, valid_filename, CancelToken, Progress};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Options {
    url: String,
    #[serde(default = "direct")]
    mode: String,
    filename: Option<String>,
    #[serde(default = "original")]
    format: String,
    #[serde(default = "default_limit")]
    max_mb: u64,
}
fn direct() -> String {
    "direct".into()
}
fn original() -> String {
    "original".into()
}
fn default_limit() -> u64 {
    2048
}

pub(crate) fn run(
    _inputs: &[PathBuf],
    staging: &Path,
    options: &Value,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(Progress),
) -> Result<Vec<PathBuf>> {
    let options: Options =
        serde_json::from_value(options.clone()).context("Invalid download options")?;
    let url = url::Url::parse(&options.url).context("Enter a complete HTTP or HTTPS URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("Downloads accept HTTP/HTTPS links without embedded login credentials");
    }
    if options.max_mb == 0 || options.max_mb > 102_400 {
        bail!("max_mb must be between 1 and 102400");
    }
    let limit = options.max_mb * 1024 * 1024;
    if let Some(filename) = &options.filename {
        if !valid_filename(filename) {
            bail!("Choose a plain filename without folders or control characters");
        }
    }
    if !matches!(
        options.format.as_str(),
        "original" | "mp4" | "mp3" | "m4a" | "webm"
    ) {
        bail!("Unsupported download format");
    }
    match options.mode.as_str() {
        "direct" => {
            if options.format != "original" {
                bail!("Choose Media link mode to convert downloaded media");
            }
            let filename = options.filename.clone().unwrap_or_else(|| {
                url.path_segments()
                    .and_then(|mut p| p.next_back())
                    .filter(|p| valid_filename(p))
                    .unwrap_or("download.bin")
                    .to_owned()
            });
            let destination = staging.join(filename);
            let metadata = tempfile::Builder::new()
                .prefix("video-toolkit-download-info-")
                .tempdir()?;
            let headers = metadata.path().join("headers");
            let mut command = Command::new(
                support::find_tool("curl").context("curl is required for direct downloads")?,
            );
            command
                .args([
                    "--disable",
                    "--fail",
                    "--location",
                    "--max-redirs",
                    "5",
                    "--proto",
                    "=http,https",
                    "--proto-redir",
                    "=http,https",
                    "--connect-timeout",
                    "15",
                    "--max-time",
                    "1800",
                    "--silent",
                    "--show-error",
                    "--max-filesize",
                ])
                .arg(limit.to_string())
                .arg("--dump-header")
                .arg(&headers)
                .arg("--output")
                .arg(&destination)
                .arg("--")
                .arg(url.as_str());
            let mut last = Instant::now() - Duration::from_secs(1);
            support::run_command_monitored(command, cancel, Duration::from_secs(1810), &mut || {
                let size = destination.metadata().map(|m| m.len()).unwrap_or(0);
                if size > limit {
                    bail!("Download exceeds the configured size limit");
                }
                if last.elapsed() >= Duration::from_millis(150) {
                    let expected = std::fs::read_to_string(&headers)
                        .ok()
                        .and_then(|header| {
                            header.lines().rev().find_map(|line| {
                                line.split_once(':')
                                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                    .and_then(|(_, value)| value.trim().parse::<u64>().ok())
                            })
                        })
                        .filter(|value| *value > 0);
                    let fraction = expected
                        .map(|total| (size as f64 / total as f64).clamp(0.0, 1.0) as f32)
                        .unwrap_or(0.0);
                    progress(Progress {
                        fraction: 0.1 + fraction * 0.75,
                        message: format!(
                            "Downloading · {:.1} MB{}",
                            size as f64 / 1_048_576.0,
                            expected
                                .map(|n| format!(" / {:.1} MB", n as f64 / 1_048_576.0))
                                .unwrap_or_default()
                        ),
                    });
                    last = Instant::now();
                }
                Ok(())
            })
            .context("Direct download failed")?;
            validate_size(&destination, limit)?;
            // A direct download is byte-preserving; no document/media conversion is implied.
            Ok(vec![destination])
        }
        "media" => media(&options, &url, staging, limit, cancel, progress),
        _ => bail!("Download mode must be direct or media"),
    }
}

fn validate_size(path: &Path, limit: u64) -> Result<()> {
    let size = path
        .metadata()
        .context("The server produced no downloadable file")?
        .len();
    if size == 0 {
        bail!("The server returned an empty file");
    }
    if size > limit {
        bail!("Download exceeds the configured size limit");
    }
    Ok(())
}

fn media(
    options: &Options,
    url: &url::Url,
    staging: &Path,
    limit: u64,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(Progress),
) -> Result<Vec<PathBuf>> {
    let executable = support::find_tool("yt-dlp")
        .context("Media downloads require yt-dlp. Run scripts/setup-file-workflows.sh")?;
    let mut command = Command::new(executable);
    let stem = options.filename.as_deref().unwrap_or("download");
    // yt-dlp interprets percent sequences in templates; filenames are always literal.
    if stem.contains('%') {
        bail!("Media filenames cannot contain %");
    }
    let stem = Path::new(stem)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| valid_filename(s))
        .context("Choose a valid media filename")?;
    let template = staging.join(format!("{stem}.%(ext)s"));
    command
        .args([
            "--ignore-config",
            "--no-plugin-dirs",
            "--no-playlist",
            "--no-exec",
            "--no-cache-dir",
            "--no-cookies",
            "--no-warnings",
            "--newline",
            "--no-simulate",
            "--socket-timeout",
            "15",
            "--retries",
            "2",
            "--fragment-retries",
            "2",
            "--restrict-filenames",
            "--max-filesize",
        ])
        .arg(limit.to_string())
        .arg("--output")
        .arg(template)
        .arg("--print")
        .arg("after_move:filepath");
    if let Some(ffmpeg) = support::find_tool("ffmpeg") {
        command.arg("--ffmpeg-location").arg(ffmpeg);
    }
    match options.format.as_str() {
        "mp3" | "m4a" => {
            command.args(["--extract-audio", "--audio-format", &options.format]);
        }
        "mp4" | "webm" => {
            command.args(["--recode-video", &options.format]);
        }
        _ => {}
    }
    command.arg("--").arg(url.as_str());
    let mut last = Instant::now() - Duration::from_secs(1);
    let result =
        support::run_command_monitored(command, cancel, Duration::from_secs(1800), &mut || {
            let size = std::fs::read_dir(staging)?
                .filter_map(|e| e.ok())
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum::<u64>();
            // Allow one source plus one transcoded temporary file during FFmpeg postprocessing.
            if size > limit.saturating_mul(2) {
                bail!("Media download exceeds the configured size limit");
            }
            if last.elapsed() >= Duration::from_millis(200) {
                progress(Progress {
                    fraction: 0.4,
                    message: format!(
                        "Downloading / processing media · {:.1} MB",
                        size as f64 / 1_048_576.0
                    ),
                });
                last = Instant::now();
            }
            Ok(())
        })
        .context("Media download failed")?;
    let paths = String::from_utf8(result.stdout)
        .context("Media downloader returned a non-UTF-8 filename")?;
    let output = paths
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .context("The link produced no media output (it may exceed max_mb)")?;
    let output = PathBuf::from(output);
    if output.parent() != Some(staging) {
        bail!("Media downloader returned an unexpected output path");
    }
    validate_size(&output, limit)?;
    let probe =
        support::find_tool("ffprobe").context("ffprobe is required to verify downloaded media")?;
    let mut command = Command::new(probe);
    command
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type",
            "-of",
            "json",
            "-protocol_whitelist",
            "file",
            "-format_whitelist",
            "mov,matroska,avi,flv,asf,mpegts,mpeg,mpegvideo,m4v,ogg,mp3,aac,wav,flac,aiff",
            "-i",
        ])
        .arg(&output);
    let result = support::run_command(command, cancel, Duration::from_secs(30))?;
    let streams: Value = serde_json::from_slice(&result.stdout)?;
    if !streams["streams"].as_array().is_some_and(|streams| {
        streams
            .iter()
            .any(|s| matches!(s["codec_type"].as_str(), Some("audio" | "video")))
    }) {
        bail!("The downloaded file has no audio or video stream");
    }
    progress(Progress {
        fraction: 0.88,
        message: "Decoding downloaded media".into(),
    });
    let mut command = Command::new(
        support::find_tool("ffmpeg").context("FFmpeg is required to verify media downloads")?,
    );
    command
        .args([
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-xerror",
            "-protocol_whitelist",
            "file",
            "-format_whitelist",
            "mov,matroska,avi,flv,asf,mpegts,mpeg,mpegvideo,m4v,ogg,mp3,aac,wav,flac,aiff",
            "-i",
        ])
        .arg(&output)
        .args(["-map", "0:v?", "-map", "0:a?", "-f", "null", "-"]);
    support::run_command(command, cancel, Duration::from_secs(1800))
        .context("Downloaded media did not pass full decode verification")?;
    Ok(vec![output])
}
