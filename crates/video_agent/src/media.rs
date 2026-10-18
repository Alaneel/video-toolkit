//! Narrow, fixed FFmpeg operations. No command text, shell, or model-selected paths.
use crate::domain::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
}

pub fn hash_file(path: &Path) -> Result<String> {
    hash_file_with_cancel(path, &CancelToken::default())
}

pub fn hash_file_with_cancel(path: &Path, cancel: &CancelToken) -> Result<String> {
    hash_file_bounded(path, cancel, Duration::from_secs(3600))
}

pub fn hash_file_bounded(path: &Path, cancel: &CancelToken, timeout: Duration) -> Result<String> {
    let mut input = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    let started = Instant::now();
    loop {
        cancel.check()?;
        if started.elapsed() >= timeout {
            return Err(AgentError::TimedOut);
        }
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn reader(mut pipe: impl Read + Send + 'static, cap: usize) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut out = Vec::new();
        let mut buffer = [0; 8192];
        while let Ok(count) = pipe.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let remaining = cap.saturating_sub(out.len());
            out.extend_from_slice(&buffer[..count.min(remaining)]);
        }
        out
    })
}

/// Waits for a real child, drains both pipes, and always reaps it on cancel/timeout.
pub(crate) fn run(
    command: &mut Command,
    cancel: &CancelToken,
    timeout: Duration,
) -> Result<Vec<u8>> {
    run_capture(command, cancel, timeout).map(|(stdout, _)| stdout)
}

fn run_capture(
    command: &mut Command,
    cancel: &CancelToken,
    timeout: Duration,
) -> Result<(Vec<u8>, Vec<u8>)> {
    run_capture_traced(command, cancel, timeout, None).map(|(stdout, stderr, _)| (stdout, stderr))
}

#[derive(Debug, Clone)]
pub(crate) struct ProcessSpan {
    pub pid: u32,
    pub started: Instant,
    pub completed: Instant,
}

fn run_capture_traced(
    command: &mut Command,
    cancel: &CancelToken,
    timeout: Duration,
    trace: Option<&Mutex<Option<ProcessSpan>>>,
) -> Result<(Vec<u8>, Vec<u8>, ProcessSpan)> {
    cancel.check()?;
    if timeout.is_zero() {
        return Err(AgentError::TimedOut);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            AgentError::Invalid(format!(
                "Cannot start media tool (install FFmpeg and ffprobe): {e}"
            ))
        })?;
    let process_started = Instant::now();
    let pid = child.id();
    let stdout = reader(child.stdout.take().unwrap(), 2 * 1024 * 1024);
    let stderr = reader(child.stderr.take().unwrap(), 32 * 1024);
    let started = Instant::now();
    let result = loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            break Err(AgentError::Cancelled);
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            break Err(AgentError::TimedOut);
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(15)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(e.into());
            }
        }
    };
    let process_completed = Instant::now();
    let span = ProcessSpan {
        pid,
        started: process_started,
        completed: process_completed,
    };
    if let Some(trace) = trace {
        *trace
            .lock()
            .map_err(|_| AgentError::Invalid("Media trace lock poisoned".into()))? =
            Some(span.clone());
    }
    let output = stdout.join().unwrap_or_default();
    let error = stderr.join().unwrap_or_default();
    let status = result?;
    if !status.success() {
        return Err(AgentError::Invalid(format!(
            "Media tool failed: {}",
            String::from_utf8_lossy(&error)
        )));
    }
    Ok((output, error, span))
}

pub fn probe(path: &Path, cancel: &CancelToken, timeout: Duration) -> Result<MediaInfo> {
    let output = run(
        Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                "file,pipe",
                "-format_whitelist",
                "mov,matroska,webm,avi,mpeg,mpegts,asf,flv,ogg",
                "-show_entries",
                "format=duration:stream=codec_type,width,height,duration",
                "-of",
                "json",
            ])
            .arg(path),
        cancel,
        timeout,
    )?;
    let data: Value = serde_json::from_slice(&output)?;
    let streams = data["streams"]
        .as_array()
        .ok_or_else(|| AgentError::Invalid("No media streams found".into()))?;
    let video = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .ok_or_else(|| AgentError::Invalid("Input must contain a video stream".into()))?;
    let duration = video["duration"]
        .as_str()
        .or_else(|| data["format"]["duration"].as_str())
        .and_then(|n| n.parse::<f64>().ok())
        .unwrap_or(0.0);
    let width = video["width"].as_u64().unwrap_or(0);
    let height = video["height"].as_u64().unwrap_or(0);
    if !duration.is_finite()
        || duration <= 0.0
        || duration > 604800.0
        || width == 0
        || height == 0
        || width > 16384
        || height > 16384
    {
        return Err(AgentError::Invalid(
            "Unsupported or unknown video duration/dimensions".into(),
        ));
    }
    Ok(MediaInfo {
        duration_ms: (duration * 1000.0).round() as u64,
        width: width as u32,
        height: height as u32,
        has_audio: streams.iter().any(|s| s["codec_type"] == "audio"),
    })
}

fn seconds(ms: u64) -> String {
    format!("{:.3}", ms as f64 / 1000.0)
}

pub(crate) fn frame(
    input: &Path,
    time_ms: u64,
    output: &Path,
    cancel: &CancelToken,
    timeout: Duration,
) -> Result<u64> {
    frame_traced(input, time_ms, output, cancel, timeout, None).map(|(time, _)| time)
}

pub(crate) fn frame_traced(
    input: &Path,
    time_ms: u64,
    output: &Path,
    cancel: &CancelToken,
    timeout: Duration,
    trace: Option<&Mutex<Option<ProcessSpan>>>,
) -> Result<(u64, ProcessSpan)> {
    let (_, log, span) = run_capture_traced(
        Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "info", "-nostdin", "-n", "-ss"])
            .arg(seconds(time_ms))
            .args(["-copyts", "-start_at_zero"])
            .args([
                "-protocol_whitelist",
                "file,pipe",
                "-format_whitelist",
                "mov,matroska,webm,avi,mpeg,mpegts,asf,flv,ogg",
            ])
            .arg("-i")
            .arg(input)
            .args([
                "-map",
                "0:v:0",
                "-frames:v",
                "1",
                "-vf",
                "showinfo,scale=640:360:force_original_aspect_ratio=decrease",
                "-fps_mode",
                "passthrough",
                "-q:v",
                "3",
                "-threads",
                "1",
            ])
            .arg(output),
        cancel,
        timeout,
        trace,
    )?;
    if !output.is_file() || output.metadata()?.len() == 0 {
        return Err(AgentError::Invalid(
            "Frame extraction produced no image".into(),
        ));
    }
    Ok((decoded_timestamp_ms(&log)?, span))
}

fn decoded_timestamp_ms(log: &[u8]) -> Result<u64> {
    let text = String::from_utf8_lossy(log);
    let mut time_base = None;
    for line in text
        .lines()
        .filter(|line| line.contains("Parsed_showinfo_"))
    {
        if let Some(value) = line.split("config in time_base:").nth(1) {
            if let Some((num, den)) = value
                .split(',')
                .next()
                .and_then(|part| part.trim().split_once('/'))
            {
                time_base = num
                    .parse::<u128>()
                    .ok()
                    .zip(den.parse::<u128>().ok())
                    .filter(|(n, d)| *n > 0 && *d > 0);
            }
        }
        if let Some(pts) = line
            .split(" pts:")
            .nth(1)
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<u128>().ok())
        {
            if let Some((num, den)) = time_base {
                // Integer PTS plus exact rational timebase, not the rounded
                // human-readable pts_time log string (which loses precision on long inputs).
                let ms = pts
                    .checked_mul(num)
                    .and_then(|n| n.checked_mul(1000))
                    .and_then(|n| n.checked_add(den / 2))
                    .map(|n| n / den)
                    .and_then(|n| u64::try_from(n).ok());
                if let Some(ms) = ms {
                    return Ok(ms);
                }
            }
        }
    }
    Err(AgentError::Invalid(
        "Frame did not report a valid integer PTS and rational source timebase".into(),
    ))
}

pub(crate) fn shard(
    asset: &Asset,
    clip: &Clip,
    audio: AudioPolicy,
    output: &Path,
    cancel: &CancelToken,
    timeout: Duration,
) -> Result<()> {
    let duration = seconds(clip.end_ms - clip.start_ms);
    let mut command = Command::new("ffmpeg");
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-n",
            "-ss",
        ])
        .arg(seconds(clip.start_ms))
        .args([
            "-protocol_whitelist",
            "file,pipe",
            "-format_whitelist",
            "mov,matroska,webm,avi,mpeg,mpegts,asf,flv,ogg",
        ])
        .arg("-i")
        .arg(&asset.path);
    if audio == AudioPolicy::PreserveWithSilence && !asset.has_audio {
        command.args(["-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo"]);
    }
    command.args(["-map", "0:v:0", "-vf", "scale=640:360:force_original_aspect_ratio=decrease,pad=640:360:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=30", "-t"])
        .arg(&duration).args(common::VideoFormat::MP4.encoding_options());
    match audio {
        AudioPolicy::Mute => {
            command.arg("-an");
        }
        AudioPolicy::PreserveWithSilence => {
            command.args([
                "-map",
                if asset.has_audio { "0:a:0" } else { "1:a:0" },
                "-af",
                "aresample=48000,apad",
                "-ac",
                "2",
                "-ar",
                "48000",
            ]);
        }
    }
    command
        .args([
            "-threads",
            "1",
            "-movflags",
            "+faststart",
            "-f",
            common::VideoFormat::MP4.muxer(),
        ])
        .arg(output);
    run(&mut command, cancel, timeout)?;
    Ok(())
}

pub(crate) fn concat(
    paths: &[std::path::PathBuf],
    durations_ms: &[u64],
    output: &Path,
    cancel: &CancelToken,
    timeout: Duration,
) -> Result<()> {
    let parent = paths
        .first()
        .and_then(|p| p.parent())
        .ok_or_else(|| AgentError::Invalid("No clips to export".into()))?;
    let mut list = tempfile::Builder::new()
        .prefix("concat-")
        .suffix(".txt")
        .tempfile_in(parent)?;
    if paths.len() != durations_ms.len() {
        return Err(AgentError::Invalid("Missing clip durations".into()));
    }
    for (path, duration_ms) in paths.iter().zip(durations_ms) {
        // Only internally generated hex digest filenames are permitted in the demuxer list.
        if path.parent() != Some(parent) {
            return Err(AgentError::Invalid("Shard must belong to the task".into()));
        }
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.len() != 68
            || !name.ends_with(".mp4")
            || !name[..64].bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(AgentError::Invalid("Invalid shard name".into()));
        }
        writeln!(list, "file '{name}'")?;
        // Ignore AAC encoder-padding container duration so it cannot accumulate
        // into growing video gaps across a sequence of short clips.
        writeln!(list, "duration {}", seconds(*duration_ms))?;
    }
    list.flush()?;
    run(
        Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "concat",
                "-safe",
                "1",
                "-i",
            ])
            .arg(list.path())
            .args([
                "-map",
                "0:v:0",
                "-map",
                "0:a:0?",
                "-c",
                "copy",
                "-movflags",
                "+faststart",
                "-f",
                common::VideoFormat::MP4.muxer(),
            ])
            .arg(output),
        cancel,
        timeout,
    )?;
    Ok(())
}

pub fn verify_output(
    path: &Path,
    expected_ms: u64,
    expect_audio: bool,
    cancel: &CancelToken,
    timeout: Duration,
) -> Result<MediaInfo> {
    let started = Instant::now();
    let info = probe(path, cancel, timeout)?;
    if info.duration_ms.abs_diff(expected_ms) > 150.max(expected_ms / 100)
        || info.has_audio != expect_audio
    {
        return Err(AgentError::Invalid(format!("Output verification mismatch: expected {expected_ms} ms, audio={expect_audio}; found {} ms, audio={}", info.duration_ms, info.has_audio)));
    }
    run(
        Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-xerror",
                "-i",
            ])
            .arg(path)
            .args(["-map", "0:v:0", "-map", "0:a:0?", "-f", "null", "-"]),
        cancel,
        timeout.saturating_sub(started.elapsed()),
    )?;
    Ok(info)
}

pub(crate) fn preview(asset: &Asset, clip: &Clip) -> Result<Child> {
    Command::new("ffplay")
        .args(["-autoexit", "-loglevel", "error", "-ss"])
        .arg(seconds(clip.start_ms))
        .arg("-t")
        .arg(seconds(clip.end_ms - clip.start_ms))
        .args([
            "-protocol_whitelist",
            "file,pipe",
            "-format_whitelist",
            "mov,matroska,webm,avi,mpeg,mpegts,asf,flv,ogg",
        ])
        .arg(&asset.path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            AgentError::Invalid(format!("Cannot start preview player; install ffplay: {e}"))
        })
}

#[cfg(test)]
mod tests {
    use super::decoded_timestamp_ms;
    #[test]
    fn decoded_timecode_uses_integer_pts_not_rounded_display_time() {
        let log = b"[Parsed_showinfo_0] config in time_base: 1/1000, frame_rate: 30/1\n[Parsed_showinfo_0] n:0 pts: 100000123 pts_time:100000\n";
        assert_eq!(decoded_timestamp_ms(log).unwrap(), 100000123);
        assert!(decoded_timestamp_ms(b"[Parsed_showinfo_0] n:0 pts: -1 pts_time:-0.01").is_err());
    }
}
