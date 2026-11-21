"""Static prerequisite ordering, not evidence of a remote CI run."""
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]


class NoticeTestPrerequisiteTests(unittest.TestCase):
    def test_locked_all_target_sources_are_fetched_before_offline_notice_tests(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text("utf-8")
        quality = workflow.split("  quality:\n", 1)[1].split("  test-platforms:\n", 1)[0]
        rust = quality.index("uses: dtolnay/rust-toolchain@stable")
        cache = quality.index("uses: Swatinem/rust-cache@v2")
        fetch = quality.index("run: cargo fetch --locked")
        tests = quality.index("run: python -m unittest discover -s scripts")
        self.assertLess(rust, cache)
        self.assertLess(cache, fetch)
        self.assertLess(fetch, tests)
        # A host-only fetch would omit the macOS package required by the
        # archive/source match even when this test job itself runs on Linux.
        fetch_command = quality[fetch:].splitlines()[0]
        self.assertEqual(fetch_command, "run: cargo fetch --locked")


if __name__ == "__main__":
    unittest.main()
