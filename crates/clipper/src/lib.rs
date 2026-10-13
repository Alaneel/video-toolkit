use std::fs;
use std::path::{Path, PathBuf};

use common::{
    execute_ffmpeg, validate_time_range, verify_input_file, Result, VideoFormat, VideoToolkitError,
};

/// Create a formatted output filename based on the input file and time range
pub fn format_output_filename(
    input_file: &Path,
    start_time: &str,
    end_time: &str,
    output_dir: &Path,
    suffix: Option<&str>,
) -> PathBuf {
    format_output_filename_with_format(
        input_file,
        start_time,
        end_time,
        output_dir,
        suffix,
        VideoFormat::MP4,
    )
}

/// Create a formatted output filename with the requested video extension.
pub fn format_output_filename_with_format(
    input_file: &Path,
    start_time: &str,
    end_time: &str,
    output_dir: &Path,
    suffix: Option<&str>,
    format: VideoFormat,
) -> PathBuf {
    // Get the basename without extension
    let base_name = input_file
        .file_stem()
        .unwrap_or_else(|| std::ffi::OsStr::new("clip"))
        .to_string_lossy();

    // Format timestamps for filename (replace : with _)
    let start_formatted = start_time.replace([':', '.'], "_");
    let end_formatted = end_time.replace([':', '.'], "_");

    // Create the output filename
    let output_name = match suffix {
        Some(s) => format!(
            "{}_{}-{}_{}.{}",
            base_name,
            start_formatted,
            end_formatted,
            s,
            format.extension()
        ),
        None => format!(
            "{}_{}-{}.{}",
            base_name,
            start_formatted,
            end_formatted,
            format.extension()
        ),
    };

    output_dir.join(output_name)
}

/// Extract clips from a video file based on specified time ranges
pub fn clip_video(
    input_file: &str,
    time_ranges: &[(String, String)],
    output_dir: &str,
    copy_codec: bool,
    suffix: Option<&str>,
) -> Result<bool> {
    clip_video_with_format(
        input_file,
        time_ranges,
        output_dir,
        copy_codec,
        suffix,
        VideoFormat::MP4,
    )
}

/// Extract clips into the requested container, validating every range before creating outputs.
pub fn clip_video_with_format(
    input_file: &str,
    time_ranges: &[(String, String)],
    output_dir: &str,
    copy_codec: bool,
    suffix: Option<&str>,
    format: VideoFormat,
) -> Result<bool> {
    if time_ranges.is_empty() {
        return Err(VideoToolkitError::Other(
            "At least one time range is required.".to_string(),
        ));
    }

    let time_ranges: Vec<(String, String)> = time_ranges
        .iter()
        .map(|(start, end)| {
            let range = format!("{start}-{end}");
            validate_time_range(&range).ok_or(VideoToolkitError::InvalidTimeRange(range))
        })
        .collect::<Result<_>>()?;

    // Verify input file exists
    verify_input_file(input_file)?;

    // Create output directory if it doesn't exist
    fs::create_dir_all(output_dir).map_err(VideoToolkitError::IoError)?;

    let input_path = Path::new(input_file);
    let output_path = Path::new(output_dir);

    // Process each time range
    let mut success = true;
    for (i, (start_time, end_time)) in time_ranges.iter().enumerate() {
        println!(
            "Creating clip {}/{} ({} to {})...",
            i + 1,
            time_ranges.len(),
            start_time,
            end_time
        );

        // Create output filename
        let output_file = format_output_filename_with_format(
            input_path,
            start_time,
            end_time,
            output_path,
            suffix,
            format,
        );
        let output_str = output_file.to_string_lossy();

        // Set encoding options
        let mut args = vec!["-y", "-i", input_file, "-ss", start_time, "-to", end_time];

        if copy_codec {
            args.extend_from_slice(&["-c", "copy"]);
        } else {
            args.extend_from_slice(format.encoding_options());
        }

        args.extend_from_slice(&["-f", format.muxer()]);
        args.push(&output_str);

        // Execute FFmpeg command
        if let Err(e) = execute_ffmpeg(&args) {
            eprintln!(
                "Error processing clip {} ({} to {}): {}",
                i + 1,
                start_time,
                end_time,
                e
            );
            success = false;
            continue;
        }

        // Verify output file was created
        if !output_file.exists() {
            eprintln!("Error: Failed to create clip {}", i + 1);
            success = false;
        }
    }

    if success {
        println!("Successfully extracted all {} clip(s).", time_ranges.len());
    }

    Ok(success)
}

/// Parse time range strings into a list of (start_time, end_time) tuples
pub fn parse_time_ranges(time_range_args: &[String]) -> Vec<(String, String)> {
    let mut time_ranges = Vec::new();

    for time_range in time_range_args {
        if let Some(parsed_range) = validate_time_range(time_range) {
            time_ranges.push(parsed_range);
        } else {
            eprintln!(
                "Warning: Invalid time range format: '{}', skipping.",
                time_range
            );
        }
    }

    time_ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_filename_uses_requested_extension_and_sanitized_timestamps() {
        let output = format_output_filename_with_format(
            Path::new("source.mov"),
            "00:00:01.250",
            "00:00:02.500",
            Path::new("clips"),
            Some("highlight"),
            VideoFormat::WebM,
        );
        assert_eq!(
            output,
            Path::new("clips").join("source_00_00_01_250-00_00_02_500_highlight.webm")
        );
    }

    #[test]
    fn existing_output_filename_api_keeps_mp4_default() {
        let output =
            format_output_filename(Path::new("source.mov"), "1", "2", Path::new("clips"), None);
        assert_eq!(output, Path::new("clips").join("source_1-2.mp4"));
    }

    #[test]
    fn invalid_ranges_are_rejected_before_creating_output_directory() {
        let output_dir = std::env::temp_dir().join(format!(
            "video-toolkit-clipper-invalid-ranges-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let output_str = output_dir.to_string_lossy();

        let empty = clip_video_with_format(
            "unused-input.mp4",
            &[],
            &output_str,
            false,
            None,
            VideoFormat::MKV,
        );
        assert!(matches!(empty, Err(VideoToolkitError::Other(_))));

        for invalid in [("2", "1"), ("1", "1"), ("invalid", "2")] {
            let ranges = vec![
                ("0".to_string(), "1".to_string()),
                (invalid.0.to_string(), invalid.1.to_string()),
            ];
            let result = clip_video_with_format(
                "unused-input.mp4",
                &ranges,
                &output_str,
                false,
                None,
                VideoFormat::MKV,
            );
            assert!(matches!(
                result,
                Err(VideoToolkitError::InvalidTimeRange(_))
            ));
        }

        assert!(!output_dir.exists());
    }
}
