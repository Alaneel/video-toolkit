use crate::formats::{detect_format, AudioFormat, FormatType, VideoFormat};
use lazy_static::lazy_static;
use regex::Regex;
use std::path::Path;
use std::process::{Command, Output};

use crate::error::{Result, VideoToolkitError};

/// Check if FFmpeg is installed and accessible
pub fn check_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Execute an FFmpeg command with the given arguments
pub fn execute_ffmpeg(args: &[&str]) -> Result<Output> {
    let output = Command::new("ffmpeg")
        .args(args)
        .output()
        .map_err(VideoToolkitError::IoError)?;

    if !output.status.success() {
        let error_message = String::from_utf8_lossy(&output.stderr);
        return Err(VideoToolkitError::FFmpegCommandFailed(
            error_message.to_string(),
        ));
    }

    Ok(output)
}

/// Get video dimensions using FFprobe
pub fn get_video_dimensions(file_path: &str) -> Result<(u32, u32)> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
            file_path,
        ])
        .output()
        .map_err(VideoToolkitError::IoError)?;

    if !output.status.success() {
        let error_message = String::from_utf8_lossy(&output.stderr);
        return Err(VideoToolkitError::FFmpegCommandFailed(
            error_message.to_string(),
        ));
    }

    let dimensions = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = dimensions.trim().split(',').collect();

    if parts.len() != 2 {
        return Err(VideoToolkitError::DimensionsError);
    }

    let width = parts[0]
        .parse::<u32>()
        .map_err(|_| VideoToolkitError::DimensionsError)?;
    let height = parts[1]
        .parse::<u32>()
        .map_err(|_| VideoToolkitError::DimensionsError)?;

    Ok((width, height))
}

/// Verify input file exists
pub fn verify_input_file(file_path: &str) -> Result<()> {
    if !Path::new(file_path).exists() {
        return Err(VideoToolkitError::InputFileNotFound(file_path.to_string()));
    }
    Ok(())
}

// Timestamp validation patterns
lazy_static! {
    pub static ref TIMESTAMP_PATTERNS: Vec<Regex> = vec![
        Regex::new(r"^\d+$").unwrap(),                      // Seconds only
        Regex::new(r"^\d+\.\d+$").unwrap(),                 // Seconds with decimal
        Regex::new(r"^\d+:\d{2}$").unwrap(),                // MM:SS
        Regex::new(r"^\d+:\d{2}\.\d+$").unwrap(),           // MM:SS.mmm
        Regex::new(r"^\d+:\d{2}:\d{2}$").unwrap(),          // HH:MM:SS
        Regex::new(r"^\d+:\d{2}:\d{2}\.\d+$").unwrap(),     // HH:MM:SS.mmm
    ];
}

/// Validate timestamp format (HH:MM:SS or MM:SS or SS or HH:MM:SS.mmm)
pub fn validate_timestamp(timestamp: &str) -> bool {
    TIMESTAMP_PATTERNS
        .iter()
        .any(|pattern| pattern.is_match(timestamp))
        && timestamp_to_seconds(timestamp).is_some()
}

fn timestamp_to_seconds(timestamp: &str) -> Option<f64> {
    let parts: Vec<&str> = timestamp.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [seconds] => (0.0, 0.0, seconds.parse::<f64>().ok()?),
        [minutes, seconds] => (
            0.0,
            minutes.parse::<f64>().ok()?,
            seconds.parse::<f64>().ok()?,
        ),
        [hours, minutes, seconds] => (
            hours.parse::<f64>().ok()?,
            minutes.parse::<f64>().ok()?,
            seconds.parse::<f64>().ok()?,
        ),
        _ => return None,
    };

    if hours < 0.0 || minutes < 0.0 || seconds < 0.0 || minutes >= 60.0 || seconds >= 60.0 {
        return None;
    }

    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

/// Validate time range format (start-end)
pub fn validate_time_range(time_range: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = time_range.split('-').collect();
    if parts.len() != 2 {
        return None;
    }

    let start_time = parts[0].trim();
    let end_time = parts[1].trim();

    if !validate_timestamp(start_time) || !validate_timestamp(end_time) {
        return None;
    }

    let start_seconds = timestamp_to_seconds(start_time)?;
    let end_seconds = timestamp_to_seconds(end_time)?;

    (start_seconds < end_seconds).then(|| (start_time.to_string(), end_time.to_string()))
}

/// Get file size in megabytes
pub fn get_file_size_mb(file_path: &Path) -> f64 {
    match std::fs::metadata(file_path) {
        Ok(metadata) => metadata.len() as f64 / (1024.0 * 1024.0),
        Err(_) => 0.0,
    }
}

/// Get output container format extension based on input format and conversion type
pub fn get_output_format(input_path: &Path, target_format: Option<&str>) -> Result<String> {
    // If target format is explicitly specified, use that
    if let Some(format) = target_format {
        return Ok(format.to_string());
    }

    // Otherwise, infer from input file
    if let Some(format_type) = detect_format(input_path) {
        match format_type {
            FormatType::Video(_) => Ok("mp4".to_string()),
            FormatType::Audio(_) => Ok("mp3".to_string()),
            FormatType::Image(_) => Ok("png".to_string()),
        }
    } else {
        // Default to MP4 if unable to determine
        Ok("mp4".to_string())
    }
}

/// Get FFmpeg codec options for a specific format
pub fn get_codec_options(format: &str) -> Vec<String> {
    if let Ok(video_format) = format.parse::<crate::formats::VideoFormat>() {
        return video_format
            .encoding_options()
            .iter()
            .map(|option| (*option).to_string())
            .collect();
    }
    match format.to_lowercase().as_str() {
        // Audio formats
        "mp3" => vec!["-c:a", "libmp3lame"],
        "aac" => vec!["-c:a", "aac"],
        "wav" => vec!["-c:a", "pcm_s16le"],
        "flac" => vec!["-c:a", "flac"],
        "ogg" => vec!["-c:a", "libvorbis"],
        "m4a" => vec!["-c:a", "aac"],

        // Image formats (animation)
        "gif" => vec!["-c:v", "gif"],
        "apng" => vec!["-c:v", "apng"],

        // Default to H.264 + AAC
        _ => vec!["-c:v", "libx264", "-c:a", "aac"],
    }
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Check if a format is supported for a specific operation
pub fn is_format_supported_for_operation(format: &str, operation: &str) -> bool {
    match operation {
        "clipper" | "gif_converter" | "splitter" => format.parse::<VideoFormat>().is_ok(),
        "gif_transparency" => format.to_lowercase() == "gif",
        "merger" => format.parse::<VideoFormat>().is_ok() || format.parse::<AudioFormat>().is_ok(),
        _ => false,
    }
}

/// Get all supported formats for a specific operation
pub fn get_supported_formats(operation: &str) -> Vec<String> {
    match operation {
        "clipper" | "gif_converter" | "splitter" => VideoFormat::all()
            .into_iter()
            .map(|format| format.extension().to_string())
            .collect(),
        "gif_transparency" => ["gif"].iter().map(|s| s.to_string()).collect(),
        "merger" => AudioFormat::all()
            .into_iter()
            .map(|format| format.extension().to_string())
            .chain(
                VideoFormat::all()
                    .into_iter()
                    .map(|format| format.extension().to_string()),
            )
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_time_range, validate_timestamp};

    #[test]
    fn accepts_supported_timestamp_forms() {
        for timestamp in ["12", "12.5", "02:12", "01:02:03", "01:02:03.250"] {
            assert!(validate_timestamp(timestamp), "{timestamp} should be valid");
        }
    }

    #[test]
    fn rejects_out_of_range_components() {
        for timestamp in ["", "1:60", "00:00:60", "1:2:03", "-1"] {
            assert!(
                !validate_timestamp(timestamp),
                "{timestamp} should be invalid"
            );
        }
    }

    #[test]
    fn requires_a_forward_time_range() {
        assert!(validate_time_range("00:10-00:20").is_some());
        assert!(validate_time_range("00:20-00:10").is_none());
        assert!(validate_time_range("00:10-00:10").is_none());
    }
}
