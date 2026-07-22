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

## Pull requests

Keep each pull request focused. Explain the user-facing change, how you tested it, and any FFmpeg or platform assumptions. Add a screenshot or short recording for visible UI changes.

By contributing, you agree that your contribution is licensed under the project's MIT License.
