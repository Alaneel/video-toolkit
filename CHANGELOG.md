# Changelog

All notable changes to Video ToolKit are documented here. The project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Cross-platform release packaging triggered by version tags.

## [0.2.0] - 2026-07-23

### Added

- A redesigned desktop shell with grouped navigation and FFmpeg status.
- Semantic timestamp and forward-range validation with regression tests.
- Cross-platform CI, issue templates, contribution guidance, and security policy.
- A project screenshot and complete quick-start documentation.

### Changed

- FFmpeg availability is checked once at application startup instead of once per frame.
- GIF background transparency now uses valid GIF89a graphic control extensions.
- The package metadata and binary name are normalized to `video-toolkit`.

### Fixed

- Invalid or reversed clip ranges are rejected before invoking FFmpeg.
- GIF transparency processing preserves the required `0x3B` file trailer.
- The workspace builds cleanly with Clippy warnings treated as errors.

[Unreleased]: https://github.com/Alaneel/Video-ToolKit/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/Alaneel/Video-ToolKit/compare/v0.1.0...v0.2.0
