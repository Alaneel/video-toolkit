//! Saved-profile workflows through the real CLI, with isolated profile storage.
//!
//! The default suite needs no media tools. Run the ignored cases after installing
//! FFmpeg and ffprobe: `cargo test --test profile_workflows -- --ignored`.

use profile_system::{Profile, ProfileManager, ProfileType};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct Workspace {
    path: PathBuf,
    profiles: PathBuf,
    empty_path: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "video-toolkit-profile-workflows-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create isolated profile test workspace");
        let empty_path = path.join("no media executables");
        fs::create_dir(&empty_path).expect("create an empty executable search path");
        Self {
            profiles: path.join("saved profiles"),
            path,
            empty_path,
        }
    }

    fn cli(&self) -> Command {
        self.cli_with_profiles(&self.profiles)
    }

    fn cli_with_profiles(&self, profiles: &Path) -> Command {
        let mut command = self.bare_cli();
        command.arg("--profiles-dir").arg(profiles);
        command
    }

    fn bare_cli(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_video-toolkit"));
        command.current_dir(&self.path);
        command
    }

    fn without_ffmpeg(&self) -> Command {
        let mut command = self.cli();
        command.env("PATH", &self.empty_path);
        command
    }

    fn create(&self, name: &str, kind: &str, parameters: &[(&str, &str)]) {
        let mut command = self.without_ffmpeg();
        command
            .args(["profile", "create", name, "--profile-type", kind])
            .args(["--description", "Reusable test settings with spaces"]);
        if !parameters.is_empty() {
            command.arg("--params");
            for (key, value) in parameters {
                command.arg(format!("{key}={value}"));
            }
        }
        run_success(&mut command);
    }

    fn batch(&self, name: &str, kind: &str, inputs: &[&Path]) -> Command {
        let mut command = self.cli();
        command.args([
            "batch",
            "with-profile",
            "--profile",
            name,
            "--profile-type",
            kind,
        ]);
        command.args(inputs);
        command
    }

    fn profile_file(&self, name: &str, kind: &str) -> PathBuf {
        let kind = match kind {
            "gif_converter" => "gifconverter",
            "gif_transparency" => "giftransparency",
            other => other,
        };
        self.profiles.join(kind).join(format!("{name}.json"))
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run(command: &mut Command) -> Output {
    command.stdin(Stdio::null());
    command
        .output()
        .unwrap_or_else(|error| panic!("could not run {command:?}: {error}"))
}

fn diagnostics(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn run_success(command: &mut Command) -> Output {
    let output = run(command);
    assert!(
        output.status.success(),
        "command failed: {command:?}\n{}",
        diagnostics(&output)
    );
    output
}

fn run_failure(command: &mut Command, expected: &str) {
    let output = run(command);
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded: {command:?}\n{}",
        diagnostics(&output)
    );
    assert!(
        diagnostics(&output).contains(expected),
        "missing error '{expected}' from {command:?}\n{}",
        diagnostics(&output)
    );
}

fn parameters(values: &[(&str, &str)]) -> HashMap<String, String> {
    values
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[test]
fn profile_metadata_round_trips_without_ffmpeg() {
    let workspace = Workspace::new();
    let output_dir = workspace.path.join("future clips with spaces");
    workspace.create(
        "travel-preview",
        "clipper",
        &[
            ("ranges", "0-1, 2-3"),
            ("output_dir", &output_dir.to_string_lossy()),
            ("format", "webm"),
            ("pattern", r"^holiday .*\.mov$"),
        ],
    );
    assert!(
        !output_dir.exists(),
        "saving settings must not create media outputs"
    );
    let saved: Value = serde_json::from_slice(
        &fs::read(workspace.profile_file("travel-preview", "clipper")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["parameters"]["ranges"], "0-1, 2-3");
    assert_eq!(saved["parameters"]["pattern"], r"^holiday .*\.mov$");
    assert_eq!(saved["description"], "Reusable test settings with spaces");

    for args in [
        vec!["profile", "list"],
        vec!["profile", "list", "--profile-type", "clipper"],
        vec![
            "profile",
            "show",
            "travel-preview",
            "--profile-type",
            "clipper",
        ],
    ] {
        let mut command = workspace.without_ffmpeg();
        command.args(args);
        let output = run_success(&mut command);
        assert!(String::from_utf8_lossy(&output.stdout).contains("travel-preview"));
    }

    let exported = workspace.path.join("portable profile with spaces.json");
    let mut export = workspace.without_ffmpeg();
    export
        .args([
            "profile",
            "export",
            "travel-preview",
            "--profile-type",
            "clipper",
            "--output",
        ])
        .arg(&exported);
    run_success(&mut export);
    let exported_data: Value = serde_json::from_slice(&fs::read(&exported).unwrap()).unwrap();
    assert_eq!(exported_data, saved);

    let imported_profiles = workspace.path.join("imported profiles");
    let mut import = workspace.cli_with_profiles(&imported_profiles);
    import
        .env("PATH", &workspace.empty_path)
        .args(["profile", "import"])
        .arg(&exported);
    run_success(&mut import);
    let imported: Value = serde_json::from_slice(
        &fs::read(imported_profiles.join("clipper/travel-preview.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(imported, saved);

    // Global storage selection also works after the subcommand.
    let mut list_imported = workspace.bare_cli();
    list_imported
        .env("PATH", &workspace.empty_path)
        .args(["profile", "list", "--profiles-dir"])
        .arg(&imported_profiles);
    let output = run_success(&mut list_imported);
    assert!(String::from_utf8_lossy(&output.stdout).contains("travel-preview"));

    let mut delete = workspace.cli_with_profiles(&imported_profiles);
    delete.env("PATH", &workspace.empty_path).args([
        "profile",
        "delete",
        "travel-preview",
        "--profile-type",
        "clipper",
    ]);
    run_success(&mut delete);
    assert!(!imported_profiles
        .join("clipper/travel-preview.json")
        .exists());
    assert!(workspace.profile_file("travel-preview", "clipper").exists());
}

#[test]
fn invalid_profiles_are_rejected_before_storage_or_media_outputs() {
    let workspace = Workspace::new();
    let output_dir = workspace.path.join("must not be created");
    let output_parameter = format!("output_dir={}", output_dir.display());
    let cases: &[(&str, &[&str], &str)] = &[
        ("clipper", &[], "required parameter"),
        (
            "clipper",
            &["ranges=0-1", "ranges=2-3"],
            "Duplicate profile parameter",
        ),
        ("clipper", &["ranges=0-1", "parallel=yes"], "parallel"),
        ("clipper", &["ranges=0-1", "typo=true"], "typo"),
        ("clipper", &["ranges=0-1", "time_ranges=2-3"], "ambiguous"),
        ("clipper", &["ranges=1-0"], "ranges"),
        ("gif_converter", &["fps=0"], "fps"),
        ("gif_converter", &["max_size=NaN"], "max_size"),
        ("splitter", &["pattern=["], "pattern"),
        ("splitter", &["prefix=../escape"], "prefix"),
        ("merger", &[], "required parameter"),
        ("merger", &["audio="], "audio"),
        ("gif_transparency", &["format=mp4"], "format"),
    ];
    for (index, (kind, values, expected)) in cases.iter().enumerate() {
        let name = format!("invalid-{index}");
        let mut command = workspace.without_ffmpeg();
        command.args(["profile", "create", &name, "--profile-type", kind]);
        if !values.is_empty() {
            command.arg("--params").args(*values);
        }
        // Output paths in invalid profiles are never materialized.
        if *kind != "gif_transparency" {
            if values.is_empty() {
                command.arg("--params");
            }
            command.arg(&output_parameter);
        }
        run_failure(&mut command, expected);
        assert!(!workspace.profile_file(&name, kind).exists());
        assert!(!output_dir.exists());
    }
}

#[test]
fn imported_invalid_execution_schema_is_not_saved() {
    let workspace = Workspace::new();
    let output_dir = workspace.path.join("invalid import output");
    let imported = Profile::new(
        "invalid-import",
        ProfileType::GifConverter,
        parameters(&[("fps", "0"), ("output_dir", &output_dir.to_string_lossy())]),
    );
    let source = workspace.path.join("invalid imported settings.json");
    fs::write(&source, serde_json::to_vec_pretty(&imported).unwrap()).unwrap();
    let mut command = workspace.without_ffmpeg();
    command.args(["profile", "import"]).arg(&source);
    run_failure(&mut command, "fps");
    assert!(!workspace
        .profile_file("invalid-import", "gifconverter")
        .exists());
    assert!(!output_dir.exists());
}

#[test]
fn existing_invalid_and_custom_profiles_fail_execution_without_media_tools() {
    let workspace = Workspace::new();
    let invalid_output = workspace.path.join("invalid saved-profile output");
    let manager = ProfileManager::with_directory(&workspace.profiles).unwrap();
    manager
        .save_profile(&Profile::new(
            "old-invalid",
            ProfileType::Clipper,
            parameters(&[
                ("ranges", "0-1"),
                ("copy_codec", "yes"),
                ("output_dir", &invalid_output.to_string_lossy()),
            ]),
        ))
        .unwrap();
    let nonexistent = workspace.path.join("source does not exist.mp4");
    let mut command = workspace.batch("old-invalid", "clipper", &[&nonexistent]);
    command.env("PATH", &workspace.empty_path);
    run_failure(&mut command, "copy_codec");
    assert!(!invalid_output.exists());

    workspace.create(
        "effect-template",
        "example_effect",
        &[("arbitrary_plugin_setting", "saved")],
    );
    assert!(workspace
        .profiles
        .join("custom/example_effect/effect-template.json")
        .exists());
    let custom_output = workspace.path.join("unsupported custom output");
    let mut custom = workspace.batch("effect-template", "example_effect", &[&nonexistent]);
    custom
        .env("PATH", &workspace.empty_path)
        .arg("--output-dir")
        .arg(&custom_output);
    run_failure(&mut custom, "custom profile types cannot be executed");
    assert!(!custom_output.exists());
}

// A complete 1x1 opaque GIF89a with a two-entry global color table.
const OPAQUE_GIF: &[u8] = &[
    71, 73, 70, 56, 57, 97, 1, 0, 1, 0, 128, 0, 0, 0, 0, 0, 255, 255, 255, 44, 0, 0, 0, 0, 1, 0, 1,
    0, 0, 2, 2, 68, 1, 0, 59,
];

#[test]
fn native_transparency_profile_runs_without_ffmpeg_and_preserves_backup() {
    let workspace = Workspace::new();
    let source = workspace.path.join("opaque background with spaces.GIF");
    fs::write(&source, OPAQUE_GIF).unwrap();
    workspace.create("transparent", "gif_transparency", &[("backup", "true")]);
    let saved_file = workspace.profile_file("transparent", "giftransparency");
    let before = fs::read(&saved_file).unwrap();
    let mut command = workspace.batch("transparent", "gif_transparency", &[&source]);
    command
        .env("PATH", &workspace.empty_path)
        .args(["--parallel=false", "--recursive=false"]);
    let output = run_success(&mut command);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Successfully processed 1/1"));
    let changed = fs::read(&source).unwrap();
    assert_ne!(changed, OPAQUE_GIF);
    assert!(changed
        .windows(8)
        .any(|block| block == [0x21, 0xF9, 0x04, 0x01, 0, 0, 0, 0]));
    assert!(changed.starts_with(b"GIF89a"));
    assert_eq!(changed.last(), Some(&0x3B));
    assert_eq!(
        fs::read(source.with_extension("gif.bak")).unwrap(),
        OPAQUE_GIF
    );
    let mut repeated = workspace.batch("transparent", "gif_transparency", &[&source]);
    repeated
        .env("PATH", &workspace.empty_path)
        .args(["--parallel=false", "--recursive=false"]);
    run_success(&mut repeated);
    assert_eq!(fs::read(&source).unwrap(), changed);
    assert_eq!(
        fs::read(source.with_extension("gif.bak")).unwrap(),
        OPAQUE_GIF,
        "re-running transparency must retain the original opaque backup"
    );
    assert_eq!(fs::read(saved_file).unwrap(), before);
}

#[test]
fn missing_explicit_input_prevents_all_native_gif_changes_and_backups() {
    let workspace = Workspace::new();
    let valid = workspace.path.join("valid.gif");
    let missing = workspace.path.join("missing.gif");
    fs::write(&valid, OPAQUE_GIF).unwrap();
    workspace.create(
        "all-or-nothing-inputs",
        "gif_transparency",
        &[("backup", "true")],
    );
    let mut command = workspace.batch(
        "all-or-nothing-inputs",
        "gif_transparency",
        &[&valid, &missing],
    );
    command
        .env("PATH", &workspace.empty_path)
        .arg("--parallel=false");
    run_failure(&mut command, "missing.gif");
    assert_eq!(fs::read(&valid).unwrap(), OPAQUE_GIF);
    assert!(!valid.with_extension("gif.bak").exists());
    assert!(!missing.exists());
    assert!(!missing.with_extension("gif.bak").exists());
}

#[test]
fn native_profile_and_cli_override_preserve_literal_regex_whitespace() {
    for pattern in [" one", " "] {
        let workspace = Workspace::new();
        workspace.create(
            "saved-whitespace",
            "gif_transparency",
            &[("backup", "true"), ("pattern", pattern)],
        );
        workspace.create(
            "override-whitespace",
            "gif_transparency",
            &[("backup", "true"), ("pattern", r"^one\.gif$")],
        );
        let stored: Value = serde_json::from_slice(
            &fs::read(workspace.profile_file("saved-whitespace", "gif_transparency")).unwrap(),
        )
        .unwrap();
        assert_eq!(stored["parameters"]["pattern"], pattern);

        for (profile_name, override_pattern) in
            [("saved-whitespace", false), ("override-whitespace", true)]
        {
            let directory = workspace.path.join(profile_name);
            fs::create_dir(&directory).unwrap();
            let excluded = directory.join("one.gif");
            let selected = directory.join("prefix one.gif");
            fs::write(&excluded, OPAQUE_GIF).unwrap();
            fs::write(&selected, OPAQUE_GIF).unwrap();
            let mut command = workspace.batch(profile_name, "gif_transparency", &[&directory]);
            command.env("PATH", &workspace.empty_path);
            if override_pattern {
                command.args(["--pattern", pattern]);
            }
            let output = run_success(&mut command);
            assert!(String::from_utf8_lossy(&output.stdout).contains("Successfully processed 1/1"));
            assert_ne!(fs::read(&selected).unwrap(), OPAQUE_GIF);
            assert_eq!(
                fs::read(selected.with_extension("gif.bak")).unwrap(),
                OPAQUE_GIF
            );
            assert_eq!(fs::read(&excluded).unwrap(), OPAQUE_GIF);
            assert!(!excluded.with_extension("gif.bak").exists());
        }
    }
}

fn media_command(binary: &str) -> Command {
    Command::new(binary)
}

fn generate_video(path: &Path, source: &str, duration: &str) {
    let mut command = media_command("ffmpeg");
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        source,
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000",
        "-t",
        duration,
        "-map",
        "0:v:0",
        "-map",
        "1:a:0",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-threads",
        "1",
        "-c:a",
        "aac",
        "-ac",
        "6",
    ]);
    command.arg(path);
    run_success(&mut command);
}

fn probe(path: &Path) -> Value {
    let mut command = media_command("ffprobe");
    command.args([
        "-v", "error", "-count_frames", "-show_entries",
        "stream=codec_type,codec_name,width,height,channels,r_frame_rate,avg_frame_rate,nb_read_frames:format=format_name,duration",
        "-of", "json",
    ]);
    command.arg(path);
    serde_json::from_slice(&run_success(&mut command).stdout).expect("valid ffprobe JSON")
}

fn stream<'a>(metadata: &'a Value, kind: &str) -> &'a Value {
    metadata["streams"]
        .as_array()
        .expect("ffprobe stream list")
        .iter()
        .find(|stream| stream["codec_type"] == kind)
        .unwrap_or_else(|| panic!("missing {kind} stream in {metadata}"))
}

fn output_files(directory: &Path, extension: &str, count: usize) -> Vec<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read outputs {}: {error}", directory.display()))
        .map(|entry| entry.unwrap().path())
        .collect();
    files.sort();
    assert_eq!(files.len(), count, "unexpected outputs: {files:?}");
    for file in &files {
        assert_eq!(
            file.extension().and_then(|value| value.to_str()),
            Some(extension)
        );
        assert!(fs::metadata(file).unwrap().len() > 0);
    }
    files
}

fn assert_decodable(path: &Path) {
    let mut command = media_command("ffmpeg");
    command
        .args(["-hide_banner", "-loglevel", "error", "-xerror", "-i"])
        .arg(path)
        .args(["-f", "null", "-"]);
    run_success(&mut command);
}

fn assert_webm(path: &Path, expected_channels: Option<u64>) {
    let metadata = probe(path);
    assert_eq!(stream(&metadata, "video")["codec_name"], "vp9");
    assert_eq!(stream(&metadata, "audio")["codec_name"], "opus");
    assert!(metadata["format"]["format_name"]
        .as_str()
        .unwrap()
        .contains("webm"));
    if let Some(channels) = expected_channels {
        assert_eq!(stream(&metadata, "audio")["channels"], channels);
    }
    assert_decodable(path);
}

#[test]
#[ignore = "requires FFmpeg and ffprobe"]
fn clipper_profile_applies_traversal_filters_and_cli_overrides_without_mutating_saved_settings() {
    let workspace = Workspace::new();
    let sources = workspace.path.join("sources with spaces");
    let nested = sources.join("nested sources");
    fs::create_dir_all(&nested).unwrap();
    let first = sources.join("selected one.mp4");
    generate_video(&first, "color=c=navy:s=320x192:r=10", "0.4");
    fs::copy(&first, nested.join("selected two.mp4")).unwrap();
    fs::copy(&first, sources.join("override root.mp4")).unwrap();
    fs::copy(&first, nested.join("override nested.mp4")).unwrap();
    fs::write(
        sources.join("unselected invalid media.mp4"),
        b"must be filtered out",
    )
    .unwrap();
    let saved_output = workspace.path.join("saved profile output");
    workspace.create(
        "clip-preview",
        "clipper",
        &[
            ("ranges", "0-0.2"),
            ("output_dir", &saved_output.to_string_lossy()),
            ("format", "webm"),
            ("recursive", "true"),
            ("parallel", "false"),
            ("pattern", r"^selected .*\.mp4$"),
            ("suffix", "preview"),
        ],
    );
    let saved_file = workspace.profile_file("clip-preview", "clipper");
    let saved_before = fs::read(&saved_file).unwrap();
    let mut from_profile = workspace.batch("clip-preview", "clipper", &[&sources]);
    let output = run_success(&mut from_profile);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Successfully processed 2/2"));
    for path in output_files(&saved_output, "webm", 2) {
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("preview"));
        assert_webm(&path, None);
    }

    let overridden_output = workspace.path.join("CLI override output");
    let mut overridden = workspace.batch("clip-preview", "clipper", &[&sources]);
    overridden
        .arg("--output-dir")
        .arg(&overridden_output)
        .args([
            "--recursive=false",
            "--parallel=true",
            "--pattern",
            r"^override .*\.mp4$",
        ]);
    let output = run_success(&mut overridden);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Successfully processed 1/1"));
    let files = output_files(&overridden_output, "webm", 1);
    assert!(files[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("override root"));
    assert_webm(&files[0], None);
    assert_eq!(fs::read(&saved_file).unwrap(), saved_before);
    assert_eq!(output_files(&saved_output, "webm", 2).len(), 2);

    let recursive_output = workspace.path.join("recursive CLI override output");
    let mut recursive = workspace.batch("clip-preview", "clipper", &[&sources]);
    recursive.arg("--output-dir").arg(&recursive_output).args([
        "--recursive=true",
        "--parallel=false",
        "--pattern",
        r"^override .*\.mp4$",
    ]);
    run_success(&mut recursive);
    for file in output_files(&recursive_output, "webm", 2) {
        assert_webm(&file, None);
    }
    assert_eq!(fs::read(saved_file).unwrap(), saved_before);
}

#[test]
#[ignore = "requires FFmpeg and ffprobe"]
fn fixed_and_optimized_gif_profiles_honor_width_and_requested_low_frame_rate() {
    let workspace = Workspace::new();
    let input = workspace.path.join("GIF source with spaces.mp4");
    generate_video(&input, "color=c=orange:s=320x192:r=12", "1.0");
    for optimize in ["false", "true"] {
        let name = format!("gif-{optimize}");
        let output_dir = workspace.path.join(&name);
        workspace.create(
            &name,
            "gif_converter",
            &[
                ("width", "160"),
                ("fps", "3"),
                ("max_size", "5.0"),
                ("optimize", optimize),
                ("output_dir", &output_dir.to_string_lossy()),
            ],
        );
        let mut command = workspace.batch(&name, "gif_converter", &[&input]);
        run_success(&mut command);
        let files = output_files(&output_dir, "gif", 1);
        let metadata = probe(&files[0]);
        let video = stream(&metadata, "video");
        assert_eq!(video["codec_name"], "gif");
        assert_eq!(video["width"], 160);
        assert_eq!(video["height"], 96);
        // Three frames from a one-second input proves optimize did not replace
        // the requested FPS with its former hard-coded 10/8/5 FPS settings.
        assert_eq!(video["nb_read_frames"], "3");
        assert_decodable(&files[0]);
    }
}

#[test]
#[ignore = "requires FFmpeg and ffprobe"]
fn splitter_profile_produces_five_mkv_tiles_with_custom_encoding_and_prefix() {
    let workspace = Workspace::new();
    let input = workspace.path.join("1920 input with spaces.mp4");
    eprintln!("splitter profile stage: generate input start");
    generate_video(&input, "color=c=navy:s=1920x1080:r=10", "0.3");
    eprintln!("splitter profile stage: generate input complete");
    let output_dir = workspace.path.join("profile slices");
    workspace.create(
        "wall-slices",
        "splitter",
        &[
            ("output_dir", &output_dir.to_string_lossy()),
            ("format", "mkv"),
            ("prefix", "panel"),
            (
                "custom_encode",
                "-c:v mpeg4 -q:v 5 -c:a pcm_s16le -threads 1",
            ),
        ],
    );
    let mut command = workspace.batch("wall-slices", "splitter", &[&input]);
    eprintln!("splitter profile stage: batch start");
    run_success(&mut command);
    eprintln!("splitter profile stage: batch complete");
    for (index, file) in output_files(&output_dir, "mkv", 5).iter().enumerate() {
        assert_eq!(
            file.file_name().unwrap().to_string_lossy(),
            format!("1920 input with spaces_panel_{}.mkv", index + 1)
        );
        eprintln!("splitter profile stage: tile {} probe start", index + 1);
        let metadata = probe(file);
        eprintln!("splitter profile stage: tile {} probe complete", index + 1);
        assert_eq!(stream(&metadata, "video")["width"], 384);
        assert_eq!(stream(&metadata, "video")["height"], 1080);
        assert_eq!(stream(&metadata, "video")["codec_name"], "mpeg4");
        assert_eq!(stream(&metadata, "audio")["codec_name"], "pcm_s16le");
        assert!(metadata["format"]["format_name"]
            .as_str()
            .unwrap()
            .contains("matroska"));
        eprintln!("splitter profile stage: tile {} decode start", index + 1);
        assert_decodable(file);
        eprintln!("splitter profile stage: tile {} decode complete", index + 1);
    }
}

#[test]
#[ignore = "requires FFmpeg and ffprobe"]
fn merger_profile_selects_replacement_stereo_audio_and_exports_webm() {
    let workspace = Workspace::new();
    let input = workspace.path.join("original surround video.mp4");
    generate_video(&input, "color=c=navy:s=160x96:r=10", "0.4");
    let audio = workspace.path.join("replacement stereo audio.wav");
    let mut generate_audio = media_command("ffmpeg");
    generate_audio.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=880:sample_rate=48000:duration=0.4",
        "-ac",
        "2",
        "-c:a",
        "pcm_s16le",
    ]);
    generate_audio.arg(&audio);
    run_success(&mut generate_audio);
    assert_eq!(stream(&probe(&input), "audio")["channels"], 6);
    assert_eq!(stream(&probe(&audio), "audio")["channels"], 2);
    let output_dir = workspace.path.join("profile merged output");
    workspace.create(
        "stereo-webm",
        "merger",
        &[
            ("audio", &audio.to_string_lossy()),
            ("output_dir", &output_dir.to_string_lossy()),
            ("format", "webm"),
            ("shortest", "true"),
        ],
    );
    let mut command = workspace.batch("stereo-webm", "merger", &[&input]);
    run_success(&mut command);
    for file in output_files(&output_dir, "webm", 1) {
        assert_webm(&file, Some(2));
    }
}
