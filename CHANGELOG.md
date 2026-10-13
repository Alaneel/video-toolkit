# Changelog

All notable changes to Video Toolkit are documented here. The project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [2.1.0] - 2026-10-13

### Added

- Cross-platform release packaging triggered by version tags.

- A redesigned desktop shell with grouped navigation and FFmpeg status.
- Semantic timestamp and forward-range validation with regression tests.
- Cross-platform CI, issue templates, contribution guidance, and security policy.
- A project screenshot and complete quick-start documentation.

### Changed

- Restored the project version to the existing v2 release line after the repository polish work.
- FFmpeg availability is checked once at application startup instead of once per frame.
- GIF background transparency now uses valid GIF89a graphic control extensions.
- The package metadata and binary name are normalized to `video-toolkit`.

### Fixed

- Invalid or reversed clip ranges are rejected before invoking FFmpeg.
- GIF transparency processing preserves the required `0x3B` file trailer.
- The workspace builds cleanly with Clippy warnings treated as errors.

[Unreleased]: https://github.com/Alaneel/video-toolkit/compare/v2.1.0...HEAD
[2.1.0]: https://github.com/Alaneel/video-toolkit/compare/v2.0.0...v2.1.0
