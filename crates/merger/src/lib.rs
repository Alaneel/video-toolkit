use std::fs;
use std::path::Path;

use common::{execute_ffmpeg, verify_input_file, Result, VideoToolkitError};

/// Extract audio from a video file
pub fn extract_audio(video_file: &str, audio_file: &str) -> Result<()> {
    verify_input_file(video_file)?;

    // Create output directory if it doesn't exist
    if let Some(parent) = Path::new(audio_file).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(VideoToolkitError::IoError)?;
        }
    }

    // Extract audio command
    let args = vec!["-y", "-i", video_file, "-acodec", "copy", audio_file];

    execute_ffmpeg(&args)?;

    if !Path::new(audio_file).exists() {
        return Err(VideoToolkitError::OutputFileNotCreated);
    }

    Ok(())
}

/// Merge audio and video files
pub fn merge_audio_video(
    video_file: &str,
    audio_file: &str,
    output_file: &str,
    use_shortest: bool,
    copy_codec: bool,
) -> Result<()> {
    verify_input_file(video_file)?;
    verify_input_file(audio_file)?;

    // Create output directory if it doesn't exist
    if let Some(parent) = Path::new(output_file).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(VideoToolkitError::IoError)?;
        }
    }

    // Build merge command
    let mut args = vec!["-y", "-i", video_file, "-i", audio_file];

    if copy_codec {
        args.extend_from_slice(&["-c", "copy"]);
    }

    if use_shortest {
        args.push("-shortest");
    }

    args.push(output_file);

    // Execute FFmpeg command
    execute_ffmpeg(&args)?;

    if !Path::new(output_file).exists() {
        return Err(VideoToolkitError::OutputFileNotCreated);
    }

    Ok(())
}
