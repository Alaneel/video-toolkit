use std::fs;
use std::path::Path;

use common::{execute_ffmpeg, get_video_dimensions, verify_input_file, Result, VideoToolkitError};

/// Verify that the input video has the expected 1920x1080 dimensions
pub fn verify_video_dimensions(input_file: &str) -> Result<(u32, u32)> {
    let dimensions = get_video_dimensions(input_file)?;
    let (width, height) = dimensions;

    if width == 1920 && height == 1080 {
        Ok(dimensions)
    } else {
        Err(VideoToolkitError::InvalidDimensions(width, height))
    }
}

/// Split a 1920x1080 video into 5 equal vertical slices of 384x1080 each
pub fn split_video(
    input_file: &str,
    output_dir: &str,
    output_prefix: &str,
    encode_options: Option<&str>,
    force: bool,
) -> Result<bool> {
    // Verify input file exists
    verify_input_file(input_file)?;

    // Verify video dimensions if not forced
    if !force {
        verify_video_dimensions(input_file)?;
    }

    // Create output directory if it doesn't exist
    fs::create_dir_all(output_dir).map_err(VideoToolkitError::IoError)?;

    // Set default encoding options if none provided
    let encode_options = encode_options.unwrap_or("-c:v libx264 -preset medium -crf 22 -c:a copy");

    // Define slice parameters (x position, width)
    let slices = [
        (0, 384),    // Slice 1: 0-383
        (384, 384),  // Slice 2: 384-767
        (768, 384),  // Slice 3: 768-1151
        (1152, 384), // Slice 4: 1152-1535
        (1536, 384), // Slice 5: 1536-1919
    ];

    // Process each slice
    let mut success = true;
    for (i, (x_pos, width)) in slices.iter().enumerate() {
        let output_file = format!("{}/{}_{}.mp4", output_dir, output_prefix, i + 1);

        println!(
            "Creating slice {}/5 (x={}, width={})...",
            i + 1,
            x_pos,
            width
        );

        // Build FFmpeg command
        let filter_value = format!("crop={}:1080:{}:0", width, x_pos);
        let mut args = vec!["-y", "-i", input_file, "-filter:v", &filter_value];

        // Add encoding options
        args.extend(encode_options.split_whitespace());

        args.push(&output_file);

        // Execute FFmpeg command
        if let Err(e) = execute_ffmpeg(&args) {
            eprintln!("Error while processing slice {}: {}", i + 1, e);
            success = false;
            continue;
        }

        // Verify output file was created
        if !Path::new(&output_file).exists() {
            eprintln!("Error: Failed to create slice {}", i + 1);
            success = false;
        }
    }

    if success {
        println!(
            "Successfully split video into 5 slices. Files saved in: {}",
            output_dir
        );
    }

    Ok(success)
}
