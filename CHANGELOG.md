# Changelog

All notable changes to Video Toolkit are documented here. The project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

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
