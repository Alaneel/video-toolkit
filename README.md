# Video Agent

![Video Agent — from sampled frames to a cut you control](assets/video-agent/hero.svg)

A native Rust desktop workflow for timestamped observations, model-proposed clips, human editing and verified video export. Built on the existing **Video Toolkit** media engine.

`Rust` · `egui` · `FFmpeg` · `OpenAI Responses`

[Try the Agent](#try-the-agent) · [How it works](#how-it-works) · [Evidence & limits](#evidence-not-hype) · [Toolkit downloads](https://github.com/Alaneel/video-toolkit/releases/tag/v2.1.0)

**[Source preview](https://github.com/Alaneel/video-toolkit/releases/tag/agent-source-preview-20261018) · `2.2.0-rc.1`.** A real GPT-6 Luna draft has passed structural validation and awaits user-directed editing and export. Human semantic evaluation is pending; [public Agent binaries remain gated](docs/video-agent.md#dependency-notices).

![Real GPT-6 Luna proposal in Video Agent's native human-review interface](assets/video-agent/real-luna-review.jpg)

<sub>Actual 25-second model draft across five clips—not an accepted final cut. Its first clip is explicitly described as lacking a clearly visible rabbit; the user can reject it. This is not a semantic-evaluation result. Footage: (c) copyright 2008, Blender Foundation / www.bigbuckbunny.org, [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/); trimmed and re-encoded. [Evidence & limitations](docs/video-agent-evidence.json).</sub>

## Try the Agent

From this candidate checkout, with **Rust 1.87+**, FFmpeg and ffprobe installed; ffplay enables interval playback:

```sh
cargo build --locked --release
./target/release/video-toolkit agent
```

Choose local footage, describe the desired cut and generate a storyboard. The **Manual workflow — no AI** path lets you explore editing and export without credentials. For model proposals, configure a private local API key and explicitly consent to sharing the sampled frames. Never put a key in this repository or a task file.

[Setup, credentials and platform requirements](docs/video-agent.md#run-the-desktop-application) · [Cloud-data boundary](docs/video-agent.md#use-the-native-workflow)

The first runtime-validated Agent target is **macOS Apple Silicon**. The legacy Toolkit's release matrix does not establish Agent support on other platforms. The default vision model is **GPT-6 Luna**; the [provider guide](docs/video-agent.md#configure-the-one-real-provider) documents its local estimates and the deprecated legacy-model option.

## How it works

```text
Local footage → Timestamped frames → Draft clip plan → Human review → Verified MP4
                    ↑                    │                 │
                    └─ Bounded follow-up ┘          Approve exact revision
```

- **Observe what is there.** The storyboard records actual decoded timestamps and image identities. Only consented JPEG samples and bounded task metadata go to OpenAI; source video, audio and local paths stay local.
- **Keep the model's authority small.** It can request an interval observation or propose clips. It cannot approve a plan, export media, select filesystem paths or execute shell commands.
- **Make the cut yours.** Play source intervals, trim, delete, reorder or add human-selected clips. Approval is tied to the saved plan's digest; an edit requires fresh approval.
- **Finish with actual media.** Export H.264 MP4 at 640×360/30 fps, with source audio normalized to AAC or explicitly muted. Verify streams, duration and a full decode before treating the file as complete.

Sparse frames do not establish unseen events or audio understanding. This is a bounded rough-cut workflow, not a full timeline editor or source-quality passthrough.

## Engineering worth inspecting

| Concern | Concrete design | Start here |
| --- | --- | --- |
| Model-to-action boundary | Two typed tools; deterministic validation; human-only export approval | [Vision backend](crates/video_agent/src/vision.rs) · [Domain](crates/video_agent/src/domain.rs) |
| Recoverable execution | Atomic task state, process locks, write-ahead records and verified shard reuse | [Store](crates/video_agent/src/store.rs) · [Execution](crates/video_agent/src/execution.rs) |
| Bounded local work | 1–4 frame-extraction workers, shared batch deadline and conservative interrupted-work accounting | [Batch coordinator](crates/video_agent/src/batch.rs) · [Budget semantics](docs/video-agent.md#state-cancellation-and-recovery) |
| Inspectable evaluation | Frozen public inputs, uniform baseline, failure-preserving captures and randomized review copies | [Evaluation protocol](evals/video-agent/README.md) |
| Native product surface | One Rust workspace shares the GUI, CLI and FFmpeg policy | [Agent UI](crates/ui/src/agent_app.rs) · [Common media layer](crates/common) |

Recovery is at task stages and verified clip shards, not arbitrary encoder positions. Runtime limits are accounting/admission controls—not OS resource quotas. The [workflow guide](docs/video-agent.md) documents the exact failure and recovery boundaries.

## Evidence, not hype

| Question | Current evidence |
| --- | --- |
| Does the local media workflow run? | Native manual walkthroughs, actual playable exports, real FFmpeg tests and recorded source/output hashes. |
| What ran on the real backend? | **A structurally validated draft, awaiting user review.** GPT-6 Luna requested two additional observations, then proposed five clips. Human-directed editing and model-origin export remain pending; rejected attempts are retained. |
| Is it better than uniform sampling? | **Not established.** Frozen tasks and a human-review rubric exist; semantic scores remain pending. |
| Can I download a public Agent release? | **Source preview only.** Build locally for review; unresolved dependency notice questions block public binary distribution. |

[Machine-readable evidence](docs/video-agent-evidence.json) · [Authoritative evaluation status](evals/video-agent/results/status.json) · [Chinese audit & handoff](docs/video-agent-audit.zh-CN.md) · [Dependency notices](THIRD_PARTY_NOTICES.txt)

<details>
<summary>See the manual review and verified-output interfaces</summary>

These are genuine local manual runs, with **zero model calls**. They demonstrate the interface and media path, not semantic selection quality.

![Manual clip review, with explicit human-selection labels](assets/video-agent/human-review-manual-validation.jpg)

![Verified output from the manual workflow](assets/video-agent/verified-manual-output.jpg)

Footage: (c) copyright 2008, Blender Foundation / www.bigbuckbunny.org, [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/); trimmed, reordered and re-encoded.

</details>

## Video Toolkit

The existing desktop app and CLI remain available: clip recordings, replace audio, split video, create GIFs, run batches and reuse saved profiles. **Video Agent extends this repository; it does not rename it or replace the existing commands.**

[v2.1.0 downloads](https://github.com/Alaneel/video-toolkit/releases/tag/v2.1.0) · [Full installation & usage guide](#full-installation-and-usage-guide) · [Changes](CHANGELOG.md)

```sh
# Open the original desktop app
./target/release/video-toolkit

# Or use a deterministic command directly
./target/release/video-toolkit clipper recording.mp4 \
  --ranges 00:01:00-00:02:00 --output-dir clips
```

<details>
<summary>See the existing Toolkit interface and processing crates</summary>

![Video Toolkit desktop interface](assets/video-toolkit-ui.png)

| Workflow | Implementation |
| --- | --- |
| Extract timestamp ranges | [Clipper](crates/clipper) |
| Make palette-optimized GIFs with a size target | [GIF converter](crates/gif_converter) |
| Split video or replace its audio | [Splitter](crates/splitter) / [Merger](crates/merger) |
| Discover inputs and process batches | [Batch processing](crates/batch_processing) |
| Save and reuse settings | [Profile system](crates/profile_system) |

FFmpeg and ffprobe are separate runtime dependencies. GIF transparency can modify inputs in place; review its backup option. Dynamically loaded plugins execute native code.

</details>

## Develop

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
# Real media integration tests; FFmpeg/ffprobe required
cargo test --locked --workspace --tests -- --ignored --test-threads=1
python3 -m unittest discover -s evals/video-agent -p 'test_*.py' -v
```

[Contributing](CONTRIBUTING.md) · [CI](.github/workflows/ci.yml) · [Packaging](.github/workflows/packages.yml) · [Agent architecture](docs/video-agent.md#architecture-and-reuse)

Repository code is [MIT](LICENSE). Dependencies and sample footage retain their own licenses; FFmpeg is not bundled. See the [open notice questions](docs/video-agent.md#dependency-notices) before redistributing the Agent candidate.

## Full installation and usage guide

<details>
<summary>Open platform setup, checksums, CLI options, profiles and plugin notes</summary>

<!-- The existing detailed guide is retained below during the documentation refresh. -->

## Quick start

### Download a prebuilt binary

Prebuilt packages do **not** require Rust. Get the package for your operating system and CPU from [Releases](https://github.com/Alaneel/video-toolkit/releases), together with that release's `SHA256SUMS` file.

Tagged releases use the archive/checksum layout below. Older releases such as v2.0.0 contain bare binaries instead. Current source checks still compile the project, but the [packaging workflow](https://github.com/Alaneel/video-toolkit/actions/workflows/packages.yml) skips binary uploads while the public-distribution license gate is unresolved. Build this Agent candidate from source for local review; do not expect downloadable preview artifacts. Once approved, package artifacts and `release-checksums` contain the archive and `SHA256SUMS` inside GitHub's outer ZIP files.

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

Running without a subcommand opens the native desktop app. To use the CLI examples below from the extracted folder, replace `video-toolkit` with `./video-toolkit` on Linux/macOS or `.\video-toolkit.exe` in PowerShell. Alternatively, add the executable's folder to `PATH`.

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

# Convert a WebM to a palette-optimized GIF targeting 5 MB (may fail if unattainable)
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
│   ├── video_agent        typed observations, plans, approval, durable execution
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

</details>
