# Contributing to Video Toolkit

Thanks for helping make local media tooling better. Bug fixes, documentation improvements, and focused features are welcome.

## Before opening a pull request

1. Open an issue first for large features or behavior changes.
2. Keep processing logic in its focused crate and UI code in `crates/ui`.
3. Add tests for parsing, validation, and other behavior that does not require FFmpeg fixtures.
4. Run the local quality gate:

   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace --all-targets
   ```

5. Run the packaging and evaluation checks with Python 3.11+:

   ```bash
   # One-time online preparation; fetch all locked target sources, including
   # the macOS crate archive used by the offline notice-provenance checks.
   cargo fetch --locked
   python3 -m unittest discover -s scripts -p 'test_*.py'
   python3 -m unittest discover -s evals/video-agent -p 'test_*.py'
   ```

   The scripts use local Cargo/rustc metadata and cached dependency bytes, not
   model API calls. The fixed release-source proof uses POSIX no-follow file
   access; its tests run on Linux/macOS and are explicitly skipped on Windows.
   Notice collection is not legal approval for public binary distribution.

## Pull requests

Keep each pull request focused. Explain the user-facing change, how you tested it, and any FFmpeg or platform assumptions. Add a screenshot or short recording for visible UI changes.

By contributing, you agree that your contribution is licensed under the project's MIT License.
