# Changelog

All notable changes to Video Toolkit are documented here. The project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Reliability hardening

- Isolate MCP stdio behind bounded input/output queues and acknowledge complete output delivery; client backpressure also terminates idle-input sessions without indefinitely holding file-worker cancellation or EOF cleanup. Document the single-process blocked-I/O shutdown boundary.
- Bound conversion/workflow subprocess pipe draining and conversion progress queues, and clean up owned Unix process groups before reaping their leaders; retain explicit non-Unix process-tree limits.
- Preserve fixed local preview-review rejection reason codes without storing raw model arguments, unknown free-form diagnostics or private paths; do not reconstruct missing reasons in historical runs.
- Include the hash-bound embedded DM Sans font notice in collection and local RC packaging, while preserving the public binary gate.
- Recover the original `system-configuration-sys 0.5.0` license texts through a pinned official release-source match, without claiming its unavailable packaged VCS commit was recovered. Five original-text gaps and target-specific distribution approval remain unresolved.
- Preserve complete newer Rust copyright bundles with an 8 MiB limit specific to `THIRD_PARTY_NOTICES.txt`; keep review JSON and ordinary RC documents at 2 MiB, retain full credential scanning and distribution gates, and prepare locked dependency sources before offline provenance tests in CI.
- Distinguish the reference study's historical gap snapshot from the current local implementation in the machine feature inventory.
- Make desktop stroke widths explicit and keep UTF-16 subtitle-prefix scanning compatible with Rust 1.87 and newer compiler checks; cover both byte orders, BOM-only input and an odd trailing byte.

### Desktop redesign

- Rebuild the desktop around a light icon sidebar, rounded workspace, searchable conversion presets, large drop area, compact file cards and a separate queue.
- Group the existing editing tools into a navigable tool gallery and move runtime details into Settings.
- Add folder import, HEIC/HEIF input with ImageMagick/libheif, and a runtime-informed UI/feature inventory.
- Introduce a shared design system (`crates/ui/src/theme.rs`): neutral tokens with one accent, DM Sans SemiBold/Bold static instances for hierarchy, and reusable buttons, chips, segmented controls, cards, form rows, badges and empty states.
- Restyle every Toolkit page on that system: grouped labelled sidebar with an engine-status card, Convert quick-start cards and dashed drop zone, File tools sections, editor breadcrumbs and form cards, Batches/Profiles/Plugins cards, and Settings paths shown relative to `~`.
- Bring the Video Agent window onto the same light theme with a numbered step bar, budget meters and tone-coded banners; all consent, budget and verification wording is unchanged.
- OGG output falls back to FFmpeg's built-in (stereo) Vorbis encoder when the FFmpeg build lacks `libvorbis`, as current Homebrew builds do.
- Add an opt-in `VTK_SCREENSHOT`/`VTK_PAGE` capture hook for refreshing documentation screenshots, and refresh `assets/video-toolkit-ui.png`.

### Local file conversion

- Extend the existing repository with a shared `file_conversion` core, preserving editing tools and the human-reviewed Video Agent.
- Add local video/audio transcoding and audio extraction through FFmpeg, and raster conversion through ImageMagick 7, with 13 output formats.
- Add the default Convert files desktop with multi-file selection, drag-and-drop, background sequential queues, progress, per-file errors and cancellation.
- Add `convert` and `doctor` CLI commands plus a tools-only MCP stdio adapter with explicit allowed folders, progress and request cancellation.
- Validate output streams, dimensions, duration where known and full decode; publish temporary output without replacing existing files.
- Add synthetic real-tool integration tests and a static REA study of the reference application's UI, registry and architecture, with a separate implementation/coverage comparison.

### PDF, subtitle, document and download workflows

- Add a shared `file_workflows` engine with 15 registered operations, used by desktop forms, `workflow`/`workflows` CLI commands and MCP discovery/execution.
- Implement PDF merge, split, selected-page rotation, compression, text watermark, metadata, PNG/JPEG page rendering, searchable text extraction and ordered images-to-PDF, including HEIC/HEIF input.
- Add subtitle conversion, timing shifts, FPS retiming and cleaning for eight timed formats, with TXT/CSV/JSON export and explicit text/timing preservation limits.
- Convert TXT, Markdown, HTML, DOCX, RTF, ODT, EPUB and searchable PDF into TXT/Markdown/HTML/DOCX/PDF/EPUB through a sanitized semantic document pipeline; rebuild layout rather than promising exact pagination.
- Add direct HTTP(S) downloads and media-link retrieval/extraction, with byte limits, cancellable subprocess cleanup, completed-transfer checks and full decode for downloaded media.
- Validate staged output structure before no-clobber publication; reject collisions and remove unpublished files on errors or cancellation. Include source stems in single-input PDF output names so different-source jobs can share a batch destination. Install optional dependencies through `scripts/setup-file-workflows.sh`.
- Pass the three real PDF/document integration tests, including the 48 input/output document combinations, source preservation, resource rejection and public publication/no-overwrite behavior; render and inspect representative PDF/DOCX pages.
- Extend CI to install the PDF/document runtime and run loopback download tests; explicitly run the HEIC-dependent PDF test on macOS alongside the existing media/image real-tool checks. General data conversion, persistent queues, more image editing/formats and natural-language file planning remain future work.

### Bounded preview-driven feedback

- Add opt-in, agent-task-scoped versioned JSON responses and offline capability discovery while preserving legacy command behavior.
- Render bounded, unapproved whole-plan FFmpeg previews with plan/media fingerprints, objective checks, published bundle metadata and a persistent local trajectory.
- Add offline `iterate` for the current draft and explicit candidate plans within three attempts per invocation and eight previews per task; this mode never calls a model, rewrites the task plan or approves/exports automatically. Semantic quality remains unevaluated.
- Add a separate, explicitly consented `smart-iterate` loop: extract actual rendered-result JPEGs, obtain typed OpenAI reviews, validate source-evidenced revisions and re-render within three rounds and the existing shared task budgets. Model verdicts are sparse-image self-assessments, not formal semantic scores.
- Add native feedback controls and `smart-status` / `smart-adopt`; explicit adoption preserves the actual model proposal provenance, creates a new revision and revokes approval. No feedback command automatically approves or exports.
- Journal received review results before accounting settlement so recovery can reuse them without another request; keep interrupted unknown usage charged. Recover the preview-start/history write window conservatively, without refunding attempts or claiming partial-encoder recovery.
- Keep strict source/output evidence binding, distinct source versus nominally mapped preview timestamps, known-unsent refunds, bounded no-progress termination and simulation-versus-production labeling.
- Document the pinned CLI-Anything source study, independent design implementation, security differences and unchanged public-binary license gate.
- Prepare source preview r3 and refresh README/Profile links. Preserve two real Luna feedback development runs (five calls): both rendered revisions, neither reached acceptance; no candidate was automatically adopted or exported.

### Experimental source preview: 2.2.0-rc.1

- Add a native Video Agent workflow for local inspection, actual-PTS samples, bounded OpenAI vision proposals, human editing, digest-bound approval and verified MP4 output.
- Add private persisted tasks, source/output integrity checks, cancellation, deadlines, model-budget reservations and stage/shard recovery without silent overwrite.
- Keep the original toolkit GUI and CLI; add `video-toolkit agent`, diagnostic `agent-task` commands and a dedicated `video-agent` app entry point.
- Add real media/audio/low-FPS/VFR recovery tests, a frozen public-material evaluation protocol, truthful manual GUI evidence and a local Mac candidate packer.
- Collect dependency and embedded-font notices, with six original-text gaps explicitly unresolved before public binary distribution.
- Add genuine two-source manual desktop export/playback evidence, including silent-to-audio ordering; preserve an additional exact upstream copyright/reference header without treating it as complete license terms.
- Reserve bounded execution steps durably, charge abandoned bounds once on reopening, keep exhausted tasks viewable without verification authority, and test real process-kill recovery. This is accounting/admission control, not an OS quota.
- Add configurable 1–4 local frame workers to initial and follow-up sampling, with shared batch deadlines, single-writer publication, cancellation/draining, actual-PTS deduplication and real child-process overlap records. Cloud calls and ordered export remain sequential; no proportional speedup is claimed.
- Add read-only frozen-task evidence capture and randomized pending A/B review material, separating private role mappings, initial proposals, human edits, unknown usage and failure records. No model request or semantic score is generated; local records remain unattested.
- Migrate the user-approved default to GPT-6 Luna and retain seven genuine development calls: three legacy-model rejections, two Luna observation requests, a Luna citation rejection and a structurally validated 25-second draft. The user's subsequent first-clip rejection, exact-revision approval and verified 20-second export are recorded separately; formal semantic comparison remains pending.
- Publish the actual user-edited demonstration MP4 and native verified-output screenshot with CC BY 3.0 attribution. Preserve the original draft and failures; do not treat the edited result as an initial-draft quality score or continuous screen recording.
- Withdraw the first Agent source preview and correct three test-only Unix process-group signals with explicit operands, exact owned-group identity checks and cleanup before PID reaping. See [test-process safety](docs/test-process-safety.md); no affected test is skipped.
- Publish source-only preview r2 with refreshed README/Profile presentation. Public Agent binaries remain blocked by unresolved dependency license texts; this is not a completed AI-quality or license-compliance acceptance.

## [2.1.0] - 2026-10-14

### Added

- Architecture-specific release archives for Linux x86_64, macOS Intel, macOS Apple Silicon, and Windows x64, with installation notes, build manifests, and SHA-256 checksums.
- Release archive verification, executable smoke checks, and version-tag validation before publication.
- Prebuilt installation instructions, platform runtime prerequisites, and checksum verification examples.
- Validated profile-driven batch execution for all five built-in operations in the CLI and desktop app.
- Portable profile storage via `--profiles-dir`, editable desktop defaults, and per-run CLI overrides.
- Profile workflow tests covering storage round-trips without FFmpeg and real media processing.
- Shared output-container selection for desktop and CLI clip, split, and batch workflows.
- Real FFmpeg CLI integration tests that probe generated containers, codecs, and audio sources in CI.
- A redesigned desktop shell with grouped navigation and FFmpeg status.
- Semantic timestamp and forward-range validation with regression tests.
- Cross-platform CI, issue templates, contribution guidance, and security policy.
- A project screenshot and complete quick-start documentation.

### Changed

- Restored the project version to the existing v2 release line after the repository polish work.
- Updated CI, artifact, and release actions to the current Node.js 24-based runtime.
- FFmpeg availability is checked once at application startup instead of once per frame.
- GIF background transparency now uses valid GIF89a graphic control extensions.
- The package metadata and binary name are normalized to `video-toolkit`.

### Fixed

- Unix release archives preserve executable permissions by packaging binaries before artifact upload.
- Release builds use supported macOS runners and explicit Rust targets instead of the retired macOS 13 runner or implicit host architectures.
- Windows release packages use ZIP archives instead of requiring users to extract a Unix-style tarball.
- `batch with-profile` processes its inputs instead of printing a placeholder and reporting success.
- Profile saves, edits, and imports validate runnable parameters before storage; regex whitespace and saved settings are preserved.
- Profile names cannot traverse directories, JSON identity is checked on load, and updates/renames preserve creation dates and existing profiles on failure.
- GIF optimization starts from the requested FPS and width; GIF batch transparency honors backup settings and preserves the first original backup.
- Missing inputs and discovery errors fail before processing instead of being silently skipped.
- All six CLI `--format` options now select output extensions, compatible codecs, and an explicit muxer.
- Audio/video merging always uses the specified audio source instead of FFmpeg's automatic stream selection.
- Batch splitting includes the source filename to prevent different sources overwriting the same five slices.
- Batch media discovery accepts every supported video container, deduplicates repeated sources, and rejects conflicting output names before processing.
- Merger output extensions are inferred or validated before processing, and input files cannot be overwritten by the merger.
- Clipping validates every requested range before creating outputs.
- Batch commands no longer have colliding short flags, and custom encoding arguments accept codec flags beginning with a hyphen.
- Profile-system tests use isolated temporary directories and no longer race in parallel CI runs.
- Invalid or reversed clip ranges are rejected before invoking FFmpeg.
- GIF transparency processing preserves the required `0x3B` file trailer.
- The workspace builds cleanly with Clippy warnings treated as errors.

[Unreleased]: https://github.com/Alaneel/video-toolkit/compare/v2.1.0...HEAD
[2.1.0]: https://github.com/Alaneel/video-toolkit/compare/v2.0.0...v2.1.0
