use std::cmp;
use std::fs;
use std::path::Path;

use common::{
    execute_ffmpeg, get_file_size_mb, get_video_dimensions, verify_input_file, Result,
    VideoToolkitError,
};

/// Convert any video format (MP4, WebM, etc.) to GIF using FFmpeg with size optimization
pub fn convert_video_to_gif(
    input_file: &str,
    output_file: &str,
    width: Option<u32>,
    fps: u32,
    max_size_mb: f64,
) -> Result<bool> {
    // Check if input file exists
    verify_input_file(input_file)?;

    // Determine width if not provided
    let width = match width {
        Some(w) => w,
        None => {
            match get_video_dimensions(input_file) {
                Ok((orig_width, _)) => cmp::min(480, orig_width),
                Err(_) => {
                    eprintln!("Warning: Could not determine video dimensions. Using default width of 480px.");
                    480
                }
            }
        }
    };

    // Create output directory if it doesn't exist
    if let Some(parent) = Path::new(output_file).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(VideoToolkitError::IoError)?;
        }
    }

    // Create a temporary palette file
    let palette_file = format!("{}.png", output_file);

    // Calculate palette first (improved quality)
    let palette_filter = format!("fps={},scale={}:-1:flags=lanczos,palettegen", fps, width);
    let palette_args = vec![
        "-y",
        "-i",
        input_file,
        "-vf",
        &palette_filter,
        &palette_file,
    ];

    if let Err(e) = execute_ffmpeg(&palette_args) {
        // Clean up palette file if it exists
        let _ = fs::remove_file(&palette_file);
        return Err(e);
    }

    // Convert using the palette
    let filter_complex = format!(
        "fps={},scale={}:-1:flags=lanczos[x];[x][1:v]paletteuse",
        fps, width
    );
    let convert_args = vec![
        "-y",
        "-i",
        input_file,
        "-i",
        &palette_file,
        "-filter_complex",
        &filter_complex,
        output_file,
    ];

    let conversion_result = execute_ffmpeg(&convert_args);

    // Clean up palette file
    let _ = fs::remove_file(&palette_file);

    // Check if the conversion was successful
    conversion_result?;

    // Check if the output file exists and is under size limit
    let output_path = Path::new(output_file);
    if output_path.exists() {
        let size_mb = get_file_size_mb(output_path);
        if size_mb <= max_size_mb {
            println!("Conversion successful! Output size: {:.2}MB", size_mb);
            Ok(true)
        } else {
            println!(
                "Output file exceeds size limit ({:.2}MB > {:.2}MB).",
                size_mb, max_size_mb
            );
            println!("Consider reducing width or FPS for smaller file size.");
            Ok(false)
        }
    } else {
        Err(VideoToolkitError::OutputFileNotCreated)
    }
}

/// Iteratively attempt conversion with decreasing quality until size requirements are met
pub fn optimize_conversion(
    input_file: &str,
    output_file: &str,
    max_size_mb: f64,
    initial_width: Option<u32>,
) -> Result<bool> {
    // Try with different quality settings
    let width_options = vec![initial_width.unwrap_or(480), 360, 320, 240, 160];
    let fps_options = vec![10, 8, 5];

    for width in width_options {
        for &fps in &fps_options {
            println!(
                "Attempting conversion with width={}px, fps={}...",
                width, fps
            );

            match convert_video_to_gif(input_file, output_file, Some(width), fps, max_size_mb) {
                Ok(true) => return Ok(true),
                Ok(false) => {
                    // If file exists but is too large, remove it before the next attempt
                    let output_path = Path::new(output_file);
                    if output_path.exists() && get_file_size_mb(output_path) > max_size_mb {
                        let _ = fs::remove_file(output_path);
                    }
                }
                Err(e) => {
                    eprintln!("Error during conversion attempt: {}", e);
                    // Continue to the next attempt
                }
            }
        }
    }

    // If we tried all options and still couldn't meet size requirements
    println!("Could not achieve target file size with any optimization settings.");

    // As a last resort, try with the lowest settings
    convert_video_to_gif(input_file, output_file, Some(120), 3, max_size_mb)
}

// For backward compatibility - will be deprecated
#[deprecated(since = "0.2.0", note = "Use convert_video_to_gif instead")]
pub fn convert_mp4_to_gif(
    input_file: &str,
    output_file: &str,
    width: Option<u32>,
    fps: u32,
    max_size_mb: f64,
) -> Result<bool> {
    convert_video_to_gif(input_file, output_file, width, fps, max_size_mb)
}
