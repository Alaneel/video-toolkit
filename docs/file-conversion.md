# Local file conversion

This unreleased extension lives inside the existing Video Toolkit Rust workspace. Start the desktop with `cargo run --locked`, or use its `convert`, `doctor` and `mcp` commands. macOS Apple Silicon is the local validation target. Rust 1.87+, FFmpeg/ffprobe and ImageMagick **7** (`magick`) are required for the corresponding conversion types. No dependencies are bundled or downloaded by the app. `doctor --json` reports executable discovery, not a guarantee that every codec is installed.

## Desktop

The default **Convert files** page accepts native multi-selection or dropped local files. Choose a format and output folder, then start the queue. The form is fixed during execution. Queued, converting, validating, completed, failed and cancelled states are shown separately. Cancellation stops the active child, cleans its unpublished output and cancels remaining items. Existing toolkit tasks share the desktop processing gate. Normal window closure cancels and waits for worker cleanup; forced process termination is not resumable.

For a local Finder-launchable development app, run `./scripts/build-local-macos-app.sh`, then open `target/local/Video Toolkit.app`. This helper builds locally without installing, signing, notarizing or publishing the app. A Finder launch uses `~/Downloads/converted_media` as its fallback output folder; choose any intended destination in the form.

## Formats and output behavior

| Family | Input | Output |
| --- | --- | --- |
| Video | MP4, MKV, MOV, WebM, AVI, FLV, WMV, TS, M4V, MPEG/MPG, VOB, OGV; must contain video | MP4, MKV, MOV (H.264/AAC), WebM (VP9/Opus) |
| Audio | The media formats above and MP3, M4A, AAC, WAV, FLAC, OGG, AIF/AIFF; must contain audio | MP3, M4A/AAC, WAV/PCM, FLAC, OGG/Vorbis |
| Raster images | PNG, JPG/JPEG, WebP, TIFF/TIF, GIF, BMP, HEIC/HEIF (libheif required) | PNG, JPG, WebP, TIFF |

The `convert` command accepts local regular media/image files. PDF, subtitle, document and URL jobs use the separate [workflow entry point](file-workflows.md). SVG, playlists and video-to-PNG are not part of the media converter. Video output uses the first video stream and optional first audio stream; audio output uses the first audio stream. Embedded subtitles, chapters, attachments and metadata are omitted. Images are auto-oriented and stripped of metadata; animation/multipage inputs use their first frame/page. JPEG uses a white background for transparent pixels. These are conversion defaults, not archival preservation settings.

An explicit `--output` requires one input and a matching extension. Batch output is `<stem>.converted.<format>` inside `--output-dir` (default `./converted`). A missing destination is created for CLI/GUI. Existing destinations, input/output identity and colliding batch names are rejected. CLI/GUI batch planning fails before execution for missing inputs or conflicting destinations; if an existing but damaged file fails during processing, the remaining queue continues. There is one conversion at a time and no queue persistence yet.

```sh
cargo run --locked -- convert recording.mov --format mp4 --output recording.mp4
cargo run --locked -- convert recording.mp4 --format mp3 --output soundtrack.mp3
cargo run --locked -- convert first.png second.jpg --format webp --output-dir converted
cargo run --locked -- convert recording.mov --format mkv --output out.mkv --json
```

For `--json`, stdout contains one `file-conversion/v1` summary and stderr contains JSON progress events. A nonzero exit status means a rejected request or at least one failed/cancelled job. Completed items remain published when another item fails. Read `result.jobs` for each state and error. GUI and MCP expose cooperative cancellation; killing the CLI process externally can leave an unpublished temporary file.

Success requires a nonempty output, expected streams/dimensions and duration tolerance where the source duration is known, followed by a full FFmpeg decode. Image output is identified and decoded by ImageMagick. Validation checks structural media integrity; it does not judge subjective quality or visual/semantic fidelity. Conversion and verification have per-process deadlines (media up to two hours each, probe 30 seconds, image stages five minutes each). Unknown media duration gives stage progress rather than an invented percentage.

On Unix, each conversion command has a verified private process group; ordinary descendants in that group are stopped before its leader is reaped. This also handles a wrapper that exits while a descendant still holds stdout/stderr open. Pipe draining is bounded, and interrupted or incomplete captures fail rather than being accepted as complete metadata. Intermediate subprocess progress is best effort through a bounded queue with a per-tick dispatch limit, so excess progress cannot grow memory without bound or starve deadline checks. Windows uses interruptible pipe polling and stops the direct child, but does not promise descendant-tree cleanup. A descendant that deliberately leaves its Unix group is also outside that cleanup guarantee; this is process management, not an OS sandbox.

## MCP stdio

The same executable provides a tools-only, newline-delimited JSON-RPC server. It advertises protocol versions `2025-06-18` and `2024-11-05`. No HTTP listener, cloud model or account is involved. Configure a client with an absolute executable path and explicitly allowed existing folders:

```json
{
  "mcpServers": {
    "video-toolkit": {
      "command": "/absolute/path/to/video-toolkit/target/debug/video-toolkit",
      "args": ["mcp", "--root", "/absolute/path/to/media"]
    }
  }
}
```

Media tools are `list_formats`, `dependency_status` and `convert_files`. New `workflow_tools` and `run_workflow` expose PDF/subtitle/document/download operations through the same server. `workflow_tools` returns the typed registry and options schema; `run_workflow` accepts `{kind, inputs, output_dir, options}`. `convert_files` accepts:

```json
{
  "inputs": ["/absolute/path/to/media/photo.png"],
  "output_dir": "/absolute/path/to/media/converted",
  "format": "webp"
}
```

Create the output directory before an MCP request. Inputs and output directories must resolve inside configured roots; static symlinks resolving outside are rejected. Roots are application path checks, not an OS sandbox against another local process concurrently replacing files/directories. Give the server only folders intended for conversion.

After initialization, the client may attach `params._meta.progressToken` to `tools/call` for strictly increasing `notifications/progress`. Only one queue runs per server process. `notifications/cancelled` with that request's `requestId` cooperatively cancels it and suppresses an undispatched final result; an already-started stdout write cannot be retracted and may race cancellation. EOF/read failure cancels active work and waits for child cleanup. Results include text JSON plus `structuredContent`; failed conversions set `isError`. Logs never share stdout with protocol messages.

Protocol input and output each have one dedicated I/O owner and a bounded queue, so a client that stops reading stdout does not block the file-operation worker inside a progress callback. Progress is best effort under backpressure; delivered progress remains increasing. A final response is not considered delivered merely because it was queued: the complete write/flush must be acknowledged within the output deadline. A failed or timed-out transport stops the session from admitting new file work and cancels its active queue, even if the client keeps stdin open without sending another line.

Transport failure does not roll back already published outputs. Inspect the destination before retrying; existing files will still be refused rather than overwritten.

The production MCP entry point is a single-session stdio process. Portable `Read`/`Write` cannot forcibly interrupt an OS-level blocked stdio operation; shutdown therefore waits only a bounded grace period for each of its two I/O threads, then lets process exit release a blocked thread if necessary. File-operation workers are still cancelled and joined. This is not a guarantee of zero detached I/O threads when repeatedly embedding the private stream server in a long-lived host process. It does not make application path checks into an OS sandbox.

## Verification

```sh
cargo fmt --all -- --check
cargo clippy --offline --locked --workspace --all-targets -- -D warnings
cargo test --offline --locked --workspace --all-targets
# Real tools; no network or model requests, synthetic fixtures only:
cargo test --offline --locked -p file_conversion --test real_tools -- --ignored
cargo test --offline --locked --test file_conversion_cli -- --ignored
```

The real-tool tests are deliberately opt-in and fail if required tools/codecs are missing. Linux CI runs the FFmpeg cases; macOS CI installs ImageMagick 7 and runs the new media/image and MCP cases. Existing Agent network behavior and consent rules are documented separately in [video-agent.md](video-agent.md).
