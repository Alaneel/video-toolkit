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
| ☆ Profiles | Save reusable operation settings as portable profiles |
| ⬡ Plugins | Discover and run dynamic extensions; a watermark plugin is included as an example |

The app supports common video formats including MP4, WebM, MKV, AVI, MOV, FLV, MPEG, and OGV, plus widely used audio and image formats.

## Quick start

### Prerequisites

- The latest stable [Rust toolchain](https://www.rust-lang.org/tools/install)
- [FFmpeg](https://ffmpeg.org/download.html), including `ffprobe`, available in your `PATH`

Verify FFmpeg before building:

```bash
ffmpeg -version
ffprobe -version
```

### Build and launch

```bash
git clone https://github.com/Alaneel/video-toolkit.git
cd video-toolkit
cargo run --release
```

No subcommand opens the native desktop app. To build a standalone binary:

```bash
cargo build --release
./target/release/video-toolkit --help
```

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
cargo test --test media_workflows -- --ignored
```

CI checks formatting and Clippy on Linux and runs workspace tests on Linux, macOS, and Windows. Linux also executes real FFmpeg integration tests that verify containers, codecs, replacement audio, and batch outputs. See [CONTRIBUTING.md](CONTRIBUTING.md) for the contribution workflow, and [CHANGELOG.md](CHANGELOG.md) for release history.

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
