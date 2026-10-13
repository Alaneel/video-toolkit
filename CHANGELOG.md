# Changelog

All notable changes to Video Toolkit are documented here. The project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Shared output-container selection for desktop and CLI clip, split, and batch workflows.
- Real FFmpeg CLI integration tests that probe generated containers, codecs, and audio sources in CI.

### Fixed

- All six CLI `--format` options now select output extensions, compatible codecs, and an explicit muxer.
- Audio/video merging always uses the specified audio source instead of FFmpeg's automatic stream selection.
- Batch splitting includes the source filename to prevent different sources overwriting the same five slices.
- Batch media discovery accepts every supported video container, deduplicates repeated sources, and rejects conflicting output names before processing.
- Merger output extensions are inferred or validated before processing, and input files cannot be overwritten by the merger.
- Clipping validates every requested range before creating outputs.
- Batch commands no longer have colliding short flags, and custom encoding arguments accept codec flags beginning with a hyphen.

## [2.1.0] - 2026-10-13

### Added

- Cross-platform release packaging triggered by version tags.

- A redesigned desktop shell with grouped navigation and FFmpeg status.
- Semantic timestamp and forward-range validation with regression tests.
- Cross-platform CI, issue templates, contribution guidance, and security policy.
- A project screenshot and complete quick-start documentation.

### Changed

- Restored the project version to the existing v2 release line after the repository polish work.
- Updated CI and release checkout steps to the current Node.js 24-based action runtime.
- FFmpeg availability is checked once at application startup instead of once per frame.
- GIF background transparency now uses valid GIF89a graphic control extensions.
- The package metadata and binary name are normalized to `video-toolkit`.

### Fixed

- Profile-system tests use isolated temporary directories and no longer race in parallel CI runs.
- Invalid or reversed clip ranges are rejected before invoking FFmpeg.
- GIF transparency processing preserves the required `0x3B` file trailer.
- The workspace builds cleanly with Clippy warnings treated as errors.

[Unreleased]: https://github.com/Alaneel/video-toolkit/compare/v2.1.0...HEAD
[2.1.0]: https://github.com/Alaneel/video-toolkit/compare/v2.0.0...v2.1.0
