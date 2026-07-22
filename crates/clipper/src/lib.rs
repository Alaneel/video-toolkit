use std::fs;
use std::path::{Path, PathBuf};

use common::{execute_ffmpeg, validate_time_range, verify_input_file, Result, VideoToolkitError};

/// Create a formatted output filename based on the input file and time range
pub fn format_output_filename(
    input_file: &Path,
    start_time: &str,
    end_time: &str,
    output_dir: &Path,
    suffix: Option<&str>,
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
            "{}_{}-{}_{}.mp4",
            base_name, start_formatted, end_formatted, s
        ),
        None => format!("{}_{}-{}.mp4", base_name, start_formatted, end_formatted),
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
        let output_file =
            format_output_filename(input_path, start_time, end_time, output_path, suffix);
        let output_str = output_file.to_string_lossy();

        // Set encoding options
        let mut args = vec!["-y", "-i", input_file, "-ss", start_time, "-to", end_time];

        if copy_codec {
            args.extend_from_slice(&["-c", "copy"]);
        } else {
            args.extend_from_slice(&[
                "-c:v", "libx264", "-preset", "medium", "-crf", "22", "-c:a", "aac",
            ]);
        }

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
