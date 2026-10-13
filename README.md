<div align="center">

# Video Toolkit

**A local-first desktop app and CLI for the FFmpeg jobs you do every day.**

Clip, convert, split, merge, and batch-process media without uploading files to a cloud service.

[![CI](https://github.com/Alaneel/video-toolkit/actions/workflows/ci.yml/badge.svg)](https://github.com/Alaneel/video-toolkit/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/Rust-stable-dea584?logo=rust)](https://www.rust-lang.org/)
[![FFmpeg](https://img.shields.io/badge/FFmpeg-required-007808?logo=ffmpeg)](https://ffmpeg.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

</div>

![Video Toolkit desktop interface](assets/video-toolkit-ui.png)

## Why Video Toolkit?

Most video utilities either hide FFmpeg behind a subscription or make simple work feel like shell scripting. Video Toolkit gives you both: a friendly native GUI for quick jobs and a composable CLI for repeatable workflows. Processing stays on your machine.

## Highlights

| Tool | What it does |
| --- | --- |
| ✂️ Video clipper | Extract one or many timestamp ranges, with fast codec copy or precise re-encoding |
| ◈ GIF converter | Generate palette-optimized GIFs and automatically search for a target file size |
| ▥ Video splitter | Turn a 1920×1080 video into five equal vertical slices |
| ⇄ Media merger | Replace a video's audio using an audio file or another video, optionally stopping at the shortest stream |
| ▦ Batch processing | Run clip, GIF, split, merge, and GIF operations across folders in parallel |
| ☆ Profiles | Save reusable settings, load them into desktop batch processing, or run them directly from the CLI |
| ⬡ Plugins | Discover and run dynamic extensions; a watermark plugin is included as an example |

The app supports common video formats including MP4, WebM, MKV, AVI, MOV, FLV, MPEG, and OGV, plus widely used audio and image formats.

## Quick start

### Download a prebuilt binary

Prebuilt packages do **not** require Rust. Get the package for your operating system and CPU from [Releases](https://github.com/Alaneel/video-toolkit/releases), together with that release's `SHA256SUMS` file.

Tagged releases built by the current workflow use the archive/checksum layout below. Older releases such as v2.0.0 contain bare binaries instead. To try an unreleased build, use artifacts from a successful [release-packaging preview](https://github.com/Alaneel/video-toolkit/actions/workflows/packages.yml) or build from source. For a preview, download your `package-…` artifact and `release-checksums`, then extract GitHub's outer ZIP files to access the archive and `SHA256SUMS`.

| Your machine | Archive name for 2.1.0 |
| --- | --- |
| Linux, Ubuntu 22.04+ / compatible x86_64 desktop | `video-toolkit-2.1.0-x86_64-unknown-linux-gnu.tar.gz` |
| macOS, Apple Silicon (M1 or newer, `arm64`) | `video-toolkit-2.1.0-aarch64-apple-darwin.tar.gz` |
| macOS, Intel (`x86_64`) | `video-toolkit-2.1.0-x86_64-apple-darwin.tar.gz` |
| Windows, x64 | `video-toolkit-2.1.0-x86_64-pc-windows-msvc.zip` |

On macOS, check **About This Mac** to choose the native package; `uname -m` also reports the current terminal's architecture. Archives contain the executable, license, README, installation notes, and a build manifest. FFmpeg is not bundled.

#### Install the runtime dependencies

Media conversion needs both [FFmpeg and ffprobe](https://ffmpeg.org/download.html) in your `PATH`. Profile management and native GIF transparency can run without them.

On Ubuntu 22.04, install the desktop runtime and FFmpeg:

```bash
sudo apt update
sudo apt install libgtk-3-0 libxcb-shape0 libxcb-xfixes0 ffmpeg
```

Ubuntu 24.04 and newer use [`libgtk-3-0t64`](https://packages.ubuntu.com/noble/libgtk-3-0t64) in place of `libgtk-3-0`; use the package provided by your distribution. A graphical desktop is needed to open the GUI, but the CLI can run without a display.

On macOS with [Homebrew](https://formulae.brew.sh/formula/ffmpeg), install FFmpeg with `brew install ffmpeg`. On Windows, use a build linked from the [FFmpeg download page](https://ffmpeg.org/download.html#build-windows), then add its `bin` folder, containing both `ffmpeg.exe` and `ffprobe.exe`, to `PATH`. Open a new terminal after changing `PATH`.

Check both programs before processing media:

```bash
ffmpeg -version
ffprobe -version
```

#### Verify and launch

In the folder where you downloaded the archive and `SHA256SUMS`, calculate the archive's SHA-256 hash and compare it with the matching filename in `SHA256SUMS`. Proceed only if the entire hash matches. Checksums detect damaged or mismatched downloads; these packages are not signed installers.

```bash
# Linux
sha256sum video-toolkit-2.1.0-x86_64-unknown-linux-gnu.tar.gz

# macOS (Apple Silicon example; use x86_64-apple-darwin for Intel)
shasum -a 256 video-toolkit-2.1.0-aarch64-apple-darwin.tar.gz
```

```powershell
# Windows PowerShell
Get-FileHash .\video-toolkit-2.1.0-x86_64-pc-windows-msvc.zip -Algorithm SHA256
```

Extract the package, enter its folder, and run the executable. On Linux:

```bash
tar -xzf video-toolkit-2.1.0-x86_64-unknown-linux-gnu.tar.gz
cd video-toolkit-2.1.0-x86_64-unknown-linux-gnu
./video-toolkit --version
./video-toolkit --help
./video-toolkit
```

On macOS (Apple Silicon example):

```bash
tar -xzf video-toolkit-2.1.0-aarch64-apple-darwin.tar.gz
cd video-toolkit-2.1.0-aarch64-apple-darwin
./video-toolkit --version
./video-toolkit
```

macOS packages are unsigned command-line executables, not notarized `.app` bundles. Launch them from Terminal. macOS may ask you to approve a downloaded executable; review the package's source and checksum before using Apple's [**Privacy & Security** approval controls](https://support.apple.com/en-us/102445). Do not disable Gatekeeper globally. If your security policy forbids unsigned downloads, build from source instead.

On Windows PowerShell:

```powershell
Expand-Archive .\video-toolkit-2.1.0-x86_64-pc-windows-msvc.zip -DestinationPath .
Set-Location .\video-toolkit-2.1.0-x86_64-pc-windows-msvc
.\video-toolkit.exe --version
.\video-toolkit.exe --help
.\video-toolkit.exe
```

No subcommand opens the native desktop app. To use the CLI examples below from the extracted folder, replace `video-toolkit` with `./video-toolkit` on Linux/macOS or `.\video-toolkit.exe` in PowerShell. Alternatively, add the executable's folder to `PATH`.

### Build from source

Install the latest stable [Rust toolchain](https://www.rust-lang.org/tools/install) and the runtime dependencies above. Source builds also need your platform's native compiler/linker. On Ubuntu, install the development libraries used by the desktop UI:

```bash
sudo apt update
sudo apt install build-essential pkg-config libgtk-3-dev libxcb-shape0-dev libxcb-xfixes0-dev ffmpeg
```

```bash
git clone https://github.com/Alaneel/video-toolkit.git
cd video-toolkit
cargo run --release --locked
```

To build a standalone binary:

```bash
cargo build --release --locked
./target/release/video-toolkit --help
```

On Windows, the built executable is `target\release\video-toolkit.exe`. Prebuilt releases are the simpler choice if you do not already have a Rust development environment.

## CLI examples

```bash
# Extract two clips
video-toolkit clipper video.mp4 \
  --ranges 00:01:00-00:02:00 00:05:30-00:06:15 \
  --output-dir clips

# Export clips as WebM with compatible VP9/Opus encoding
video-toolkit clipper video.mp4 --ranges 00:00-00:15 \
  --output-dir web-clips --format webm

# Convert a WebM to a palette-optimized GIF under 5 MB
video-toolkit gif-converter demo.webm \
  --output demo.gif --width 480 --fps 10 --max-size 5 --optimize

# Merge a video with a replacement audio track
video-toolkit merger video.mp4 soundtrack.mp3 \
  --output final.mp4 --shortest

# Process a folder recursively
video-toolkit batch clipper ./recordings --recursive \
  --ranges 00:00-00:15 --output-dir highlights

# Explore the complete command surface
video-toolkit --help
video-toolkit formats
```

Every command is also available through Cargo during development by replacing `video-toolkit` with `cargo run --release --`.

Choose an output container with `--format` for clip, split, and their batch commands: `mp4` (default), `mkv`, `avi`, `mov`, `webm`, `flv`, `wmv`, `ts`, `m4v`, `mpeg` (or `mpg`), `vob`, and `ogv`. The desktop clip, split, and batch tools offer the same selection. FFmpeg must include the corresponding encoders. Defaults select compatible video/audio codecs; AVI, WMV, MPEG, and VOB use stereo audio, and MPEG/VOB use 25 fps. `--copy-codec` preserves the source codecs, which must already be supported by the chosen container. Custom split encoding options replace the default codecs.

The merger infers its container from `--output final.webm`, or accepts `--format webm` with a matching extension. Without an extension, it adds the requested extension (MP4 by default). Conflicting extensions and unknown formats are rejected before processing. The output always uses the video from the first input and the audio from the selected audio source. Batch slices include the source filename, for example `recording_slice_1.mkv`, so different source filenames keep separate outputs. Repeated inputs are processed once; different inputs with conflicting output names are rejected before any output is created. Rename those sources or run them into separate output directories.

## Reusable profiles

Save a preset once, then apply it to different inputs:

```bash
video-toolkit profile create web-preview --profile-type clipper \
  --params 'ranges=0-00:15,00:30-00:45' 'format=webm' \
  'output_dir=previews' 'recursive=true' 'parallel=false'

video-toolkit batch with-profile ./recordings \
  --profile web-preview --profile-type clipper

# Override this run without changing the saved preset
video-toolkit batch with-profile ./recordings \
  --profile web-preview --profile-type clipper \
  --output-dir review-clips --recursive=false --parallel=true \
  --pattern '\.mp4$'

# Export and import portable JSON presets
video-toolkit profile export web-preview --profile-type clipper --output web-preview.json
video-toolkit profile import web-preview.json
```

In the desktop app, open **Profiles**, select or create a preset, and choose **Use in Batch**. The app loads its settings into Batch Processing; choose inputs, review or adjust the settings, and start the job. Loading a preset does not start processing or change the saved preset.

Profiles use the following parameters and defaults. Numbers must be positive (`max_size` must also be finite), booleans are `true`/`false`, and unknown keys or conflicting aliases are rejected before processing.

| Profile type | Required parameters | Optional parameters and defaults |
| --- | --- | --- |
| `clipper` | `ranges`: comma- or space-separated `START-END` ranges | `output_dir=output_clips`, `format=mp4`, `copy_codec=false`, `suffix` |
| `gif_converter` | None | `width` (up to 480 px by default), `fps=10`, `max_size=5.0` MB, `optimize=false`, `output_dir=output_gifs` |
| `gif_transparency` | None | `backup=false`; modifies the input GIFs in place |
| `splitter` | None | `output_dir=output_slices`, `prefix=slice`, `format=mp4`, `custom_encode`, `force=false` |
| `merger` | `audio`: audio file or source-video path | `output_dir=output_merged`, `format=mp4`, `shortest=false`, `copy_codec=false` |

Every profile also accepts `recursive=false`, `parallel=true`, and `pattern` (an optional filename regex). Regex whitespace is preserved; an empty value disables filtering. Relative audio and output paths are resolved from the working directory when the job runs. With GIF optimization enabled, conversion starts at the requested width/FPS and may reduce them to meet the size limit. GIF transparency with `backup=true` preserves the original in a `.gif.bak` file and keeps that first backup on repeated runs.

The aliases `time_ranges`, `max_size_mb`, `create_backup`, `audio_file`, `use_shortest`, `output_format`, and `encode_options` correspond to `ranges`, `max_size`, `backup`, `audio`, `shortest`, `format`, and `custom_encode`. Supply one key from each pair. Empty optional `width`, `suffix`, `custom_encode`, and `pattern` values are allowed; required fields cannot be blank. Custom profile types can be stored and exported, but cannot be run by the built-in batch tools.

Profiles normally use the platform's application configuration directory. Use the global `--profiles-dir ./presets` option to select a portable storage directory for CLI commands or the desktop app. Profile names and custom type names must be portable filename components; imported metadata must match its stored name and type. Profile management and native GIF transparency remain available without FFmpeg. A missing input, invalid preset, empty match set, or failed media job returns a nonzero CLI exit status.

## Architecture

Video Toolkit is a Rust workspace with small, focused crates:

```text
video-toolkit
├── src/                    GUI/CLI entry point
├── crates/
│   ├── common             FFmpeg process, formats, validation, errors
│   ├── clipper            timestamp-based extraction
│   ├── gif_converter      palette generation and size optimization
│   ├── gif_transparency   batch GIF processing
│   ├── splitter           vertical video slicing
│   ├── merger             audio extraction and media merging
│   ├── batch_processing   discovery, filtering, and parallel execution
│   ├── profile_system     persistent reusable settings
│   ├── plugin_system      dynamic plugin API and loader
│   └── ui                 egui desktop interface
└── plugins/
    └── watermark_plugin   reference plugin implementation
```

The processing crates depend on the shared `common` layer and remain independent of the GUI, so the same behavior is available from desktop and command-line workflows.

## Plugins

Plugins are dynamic libraries implementing the `Plugin` trait from `plugin_system`. Start with [`plugins/watermark_plugin`](plugins/watermark_plugin) as a working reference, build it in release mode, and place the resulting library in the platform plugin directory shown by the app.

Because loading a dynamic library executes native code, only install plugins you trust.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
# Requires FFmpeg/ffprobe: encode real fixtures, exercise the CLI, and probe outputs
cargo test --tests -- --ignored
```

CI checks formatting and Clippy on Linux and runs workspace tests on Linux, macOS, and Windows. Linux also executes real FFmpeg integration tests that verify containers, codecs, replacement audio, batch outputs, and saved-profile processing. Dependency-free CLI tests verify profile storage and native GIF workflows with FFmpeg removed from `PATH`. See [CONTRIBUTING.md](CONTRIBUTING.md) for the contribution workflow, and [CHANGELOG.md](CHANGELOG.md) for release history.

The release workflow builds explicit Linux x86_64, macOS Intel, macOS Apple Silicon, and Windows x64 targets. It packages executables on their build machines, verifies extracted archives before upload, and checks version tags against the Cargo package version. Preview runs do not publish releases; a matching version tag produces the archives and `SHA256SUMS` on the Releases page.

## Roadmap

- Drag-and-drop inputs and richer progress reporting
- Configurable split layouts instead of a fixed five-column preset
- Hardware-accelerated encoding presets
- Signed desktop release artifacts for macOS, Windows, and Linux
- Preview thumbnails and before/after media metadata

Have an idea? [Open an issue](https://github.com/Alaneel/video-toolkit/issues/new/choose) or send a pull request.

## License

Released under the [MIT License](LICENSE).

## Acknowledgements

Built with [FFmpeg](https://ffmpeg.org/), [egui](https://github.com/emilk/egui), and the Rust ecosystem.
