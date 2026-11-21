//! Explicit end-to-end checks against installed FFmpeg / ImageMagick.
//! Run with: cargo test -p file_conversion --test real_tools -- --ignored
use file_conversion::{
    convert, dependencies, output_requests, run_queue, CancelToken, ConversionRequest, JobState,
    TargetFormat,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn tool(name: &str) -> PathBuf {
    dependencies()
        .into_iter()
        .find(|entry| entry.name == name)
        .unwrap()
        .path
        .unwrap_or_else(|| panic!("Required test dependency {name} is missing"))
}

fn fixture_video(directory: &Path) -> PathBuf {
    let input = directory.join("source video with spaces 视频.mp4");
    let result = Command::new(tool("FFmpeg"))
        .args([
            "-hide_banner",
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=32x24:rate=6",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "0.4",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
        ])
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "fixture generation: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    input
}

fn fixture_image(directory: &Path) -> PathBuf {
    let input = directory.join("source [1] image 图片.png");
    // PPM is a simple fixture format: no encoder or external fixture needed.
    let ppm = directory.join("fixture.ppm");
    fs::write(
        &ppm,
        b"P3\n2 2\n255\n255 0 0  0 255 0\n0 0 255  255 255 255\n",
    )
    .unwrap();
    let result = Command::new(tool("ImageMagick"))
        .arg(&ppm)
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "fixture generation: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    input
}

fn request(input: &Path, directory: &Path, format: TargetFormat) -> ConversionRequest {
    ConversionRequest {
        input: input.to_path_buf(),
        output: directory.join(format!("output.{format}")),
        format,
    }
}

fn assert_no_temporary_files(directory: &Path) {
    assert!(!fs::read_dir(directory).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".video-toolkit-")));
}

#[test]
#[ignore = "requires installed FFmpeg with libx264/libvpx-vp9/audio encoders"]
fn converts_every_media_target_and_validates_output() {
    let directory = tempfile::tempdir().unwrap();
    let input = fixture_video(directory.path());
    for format in TargetFormat::all()
        .iter()
        .copied()
        .filter(|format| !format.is_image())
    {
        let request = request(&input, directory.path(), format);
        let mut progress = Vec::new();
        let result = convert(&request, &CancelToken::default(), |event| {
            progress.push(event)
        })
        .unwrap_or_else(|error| panic!("{format}: {error:#}"));
        assert_eq!(result.output, fs::canonicalize(&request.output).unwrap());
        assert!(result.bytes > 0);
        assert_eq!(progress.last().unwrap().fraction, 1.0);
        assert!(progress
            .iter()
            .any(|event| event.message.starts_with("Validating")));
        assert!(progress
            .windows(2)
            .all(|events| events[1].fraction >= events[0].fraction));
        // Output inspection is also done independently of the engine's return.
        let inspection = Command::new(tool("ffprobe"))
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type",
                "-of",
                "csv=p=0",
            ])
            .arg(&result.output)
            .output()
            .unwrap();
        assert!(inspection.status.success());
        let streams = String::from_utf8_lossy(&inspection.stdout);
        if format.is_audio() {
            assert!(streams.contains("audio") && !streams.contains("video"));
        } else {
            assert!(streams.contains("video") && streams.contains("audio"));
        }
    }
    assert_no_temporary_files(directory.path());
}

#[test]
#[ignore = "requires installed ImageMagick with PNG/JPEG/WebP/TIFF coders"]
fn imagemagick_converts_every_image_target_and_handles_literal_filename_syntax() {
    let directory = tempfile::tempdir().unwrap();
    let input = fixture_image(directory.path());
    for format in TargetFormat::all()
        .iter()
        .copied()
        .filter(|format| format.is_image())
    {
        let request = request(&input, directory.path(), format);
        let result = convert(&request, &CancelToken::default(), |_| {})
            .unwrap_or_else(|error| panic!("{format}: {error:#}"));
        assert!(result.bytes > 0);
        let inspection = Command::new(tool("ImageMagick"))
            .args(["identify", "-format", "%w %h"])
            .arg(result.output)
            .output()
            .unwrap();
        assert!(inspection.status.success());
        assert_eq!(String::from_utf8_lossy(&inspection.stdout), "2 2");
    }
    assert_no_temporary_files(directory.path());
}

#[test]
#[ignore = "requires installed FFmpeg and ffprobe"]
fn a_failed_job_does_not_block_the_rest_of_the_queue() {
    let directory = tempfile::tempdir().unwrap();
    let valid = fixture_video(directory.path());
    let broken = directory.path().join("broken.mp4");
    fs::write(&broken, b"this is not a video").unwrap();
    let requests = output_requests(
        &[broken, valid],
        &directory.path().join("out"),
        TargetFormat::Wav,
    )
    .unwrap();
    let mut events = Vec::new();
    let report = run_queue(&requests, &CancelToken::default(), |event| {
        events.push(event)
    });
    assert!(!report.success());
    assert_eq!(report.jobs[0].state, JobState::Failed);
    assert_eq!(report.jobs[1].state, JobState::Completed);
    assert!(!requests[0].output.exists());
    assert!(requests[1].output.exists());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.state == JobState::Queued)
            .count(),
        2
    );
    assert!(events
        .iter()
        .any(|event| event.index == 1 && event.state == JobState::Validating));
    assert_no_temporary_files(requests[0].output.parent().unwrap());
}

#[test]
#[ignore = "requires installed FFmpeg and ffprobe"]
fn cancellation_after_encode_discards_unpublished_output() {
    let directory = tempfile::tempdir().unwrap();
    let input = fixture_video(directory.path());
    let request = request(&input, directory.path(), TargetFormat::Wav);
    let cancel = CancelToken::default();
    let result = convert(&request, &cancel, |event| {
        if event.message.starts_with("Validating") {
            cancel.cancel();
        }
    });
    assert!(result.is_err());
    assert!(!request.output.exists());
    assert_no_temporary_files(directory.path());
}

#[test]
#[ignore = "requires installed FFmpeg and ffprobe"]
fn destination_created_during_encode_is_never_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let input = fixture_video(directory.path());
    let request = request(&input, directory.path(), TargetFormat::Wav);
    let mut planted = false;
    let result = convert(&request, &CancelToken::default(), |event| {
        if !planted && event.message.starts_with("Validating") {
            fs::write(&request.output, b"existing user output").unwrap();
            planted = true;
        }
    });
    assert!(result.is_err());
    assert!(planted);
    assert_eq!(fs::read(&request.output).unwrap(), b"existing user output");
    assert_no_temporary_files(directory.path());
}

#[test]
#[ignore = "requires installed FFmpeg and ffprobe"]
fn rejects_a_playlist_disguised_as_media() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("playlist.mp4");
    fs::write(&input, b"#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10,\nhttp://127.0.0.1:1/never-request-this.ts\n#EXT-X-ENDLIST\n").unwrap();
    let request = request(&input, directory.path(), TargetFormat::Mp4);
    let error = convert(&request, &CancelToken::default(), |_| {}).unwrap_err();
    assert!(format!("{error:#}").contains("Cannot inspect local media input"));
    assert!(!request.output.exists());
    assert_no_temporary_files(directory.path());
}

#[test]
#[ignore = "requires installed ImageMagick"]
fn imagemagick_rejects_image_scripts_disguised_as_a_raster() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("script.png");
    fs::write(&input, b"push graphic-context\nviewbox 0 0 10 10\nimage over 0,0 0,0 'https://127.0.0.1:1/no'\npop graphic-context\n").unwrap();
    let request = request(&input, directory.path(), TargetFormat::Jpg);
    assert!(convert(&request, &CancelToken::default(), |_| {}).is_err());
    assert!(!request.output.exists());
    assert_no_temporary_files(directory.path());
}

#[test]
#[ignore = "requires ImageMagick with working HEIC/libheif encode and decode support"]
fn imagemagick_heic_and_heif_inputs_convert_to_jpg_and_png() {
    let directory = tempfile::tempdir().unwrap();
    let png = fixture_image(directory.path());
    let heic = directory.path().join("phone photo 图片.HEIC");
    let fixture = Command::new(tool("ImageMagick"))
        .arg(&png)
        .args(["-resize", "32x32!", "-quality", "75"])
        .arg(&heic)
        .output()
        .unwrap();
    assert!(
        fixture.status.success(),
        "HEIC fixture generation requires an installed ImageMagick HEIC/libheif encoder: {}",
        String::from_utf8_lossy(&fixture.stderr)
    );
    // Exercise the .heif alias with exactly the same real encoded payload.
    let heif = directory.path().join("phone photo alias.heif");
    fs::copy(&heic, &heif).unwrap();
    for (input_index, input) in [&heic, &heif].into_iter().enumerate() {
        for format in [TargetFormat::Jpg, TargetFormat::Png] {
            let output_dir = directory
                .path()
                .join(format!("converted-{input_index}-{format}"));
            let requests =
                output_requests(std::slice::from_ref(input), &output_dir, format).unwrap();
            let result = convert(&requests[0], &CancelToken::default(), |_| {})
                .unwrap_or_else(|error| panic!("{} -> {format}: {error:#}", input.display()));
            assert!(result.bytes > 0);
            let inspection = Command::new(tool("ImageMagick"))
                .args(["identify", "-format", "%w %h"])
                .arg(&result.output)
                .output()
                .unwrap();
            assert!(inspection.status.success());
            assert_eq!(String::from_utf8_lossy(&inspection.stdout), "32 32");
            assert_no_temporary_files(&output_dir);
        }
    }
    assert_eq!(TargetFormat::all().len(), 13);
    assert!("heic".parse::<TargetFormat>().is_err());
    assert!("heif".parse::<TargetFormat>().is_err());
}
