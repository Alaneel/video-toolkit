use std::fs;
use std::path::Path;

use common::{execute_ffmpeg, verify_input_file, Result, VideoFormat, VideoToolkitError};

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
    let extension = Path::new(output_file)
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| {
            VideoToolkitError::Other(format!(
                "Output file '{}' must have a supported video extension",
                output_file
            ))
        })?;
    let format = extension.parse::<VideoFormat>().map_err(|error| {
        VideoToolkitError::Other(format!(
            "Unsupported output video format for '{}': {}",
            output_file, error
        ))
    })?;

    merge_audio_video_with_format(
        video_file,
        audio_file,
        output_file,
        use_shortest,
        copy_codec,
        format,
    )
}

/// Merge the first video stream with the first audio stream from the selected
/// audio source, using an explicit output container.
pub fn merge_audio_video_with_format(
    video_file: &str,
    audio_file: &str,
    output_file: &str,
    use_shortest: bool,
    copy_codec: bool,
    format: VideoFormat,
) -> Result<()> {
    verify_input_file(video_file)?;
    verify_input_file(audio_file)?;
    verify_output_is_not_input(video_file, audio_file, output_file)?;

    // Create output directory if it doesn't exist
    if let Some(parent) = Path::new(output_file).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(VideoToolkitError::IoError)?;
        }
    }

    // Build merge command
    let mut args = vec![
        "-y", "-i", video_file, "-i", audio_file, "-map", "0:v:0", "-map", "1:a:0",
    ];

    if copy_codec {
        args.extend_from_slice(&["-c", "copy"]);
    } else {
        args.extend_from_slice(format.encoding_options());
    }

    if use_shortest {
        args.push("-shortest");
    }

    args.extend_from_slice(&["-f", format.muxer(), output_file]);

    // Execute FFmpeg command
    execute_ffmpeg(&args)?;

    if !Path::new(output_file).exists() {
        return Err(VideoToolkitError::OutputFileNotCreated);
    }

    Ok(())
}

fn verify_output_is_not_input(video_file: &str, audio_file: &str, output_file: &str) -> Result<()> {
    if Path::new(output_file).exists() {
        let output_path = fs::canonicalize(output_file)?;
        for input_file in [video_file, audio_file] {
            if output_path == fs::canonicalize(input_file)? {
                return Err(VideoToolkitError::Other(format!(
                    "Output file '{}' must be different from both input files",
                    output_file
                )));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "video_toolkit_merger_{}_{}",
                std::process::id(),
                NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn legacy_merge_rejects_unsupported_output_extensions() {
        let error =
            merge_audio_video("video.mp4", "audio.wav", "output.unknown", true, false).unwrap_err();

        assert!(error
            .to_string()
            .contains("Unsupported output video format"));
    }

    #[test]
    fn legacy_merge_requires_an_output_extension() {
        let error = merge_audio_video("video.mp4", "audio.wav", "output", true, false).unwrap_err();

        assert!(error.to_string().contains("supported video extension"));
    }

    #[test]
    fn merge_rejects_overwriting_either_input_before_running_ffmpeg() {
        let directory = TestDirectory::new();
        let video_file = directory.0.join("video.mp4");
        let audio_file = directory.0.join("audio.wav");
        fs::write(&video_file, b"original video").unwrap();
        fs::write(&audio_file, b"original audio").unwrap();
        let alias_directory = directory.0.join("aliases");
        fs::create_dir(&alias_directory).unwrap();

        for output_file in [
            alias_directory.join("..").join("video.mp4"),
            audio_file.clone(),
        ] {
            let error = merge_audio_video_with_format(
                video_file.to_str().unwrap(),
                audio_file.to_str().unwrap(),
                output_file.to_str().unwrap(),
                true,
                false,
                VideoFormat::MP4,
            )
            .unwrap_err();

            assert!(error
                .to_string()
                .contains("different from both input files"));
        }

        assert_eq!(fs::read(video_file).unwrap(), b"original video");
        assert_eq!(fs::read(audio_file).unwrap(), b"original audio");
    }
}
