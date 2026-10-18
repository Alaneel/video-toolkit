"""Offline source guard for test-only Unix group signals; not runtime proof."""
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]
EXPECTED = {
    "crates/video_agent/src/execution.rs",
    "crates/video_agent/tests/concurrency.rs",
    "crates/video_agent/tests/workflows.rs",
}
CALL = re.compile(r'Command::new\(\s*"/bin/kill"\s*\)')
SAFE_ARGS = re.compile(
    r'\s*\.args\(\[\s*"-KILL",\s*"--",\s*'
    r'&format!\("-\{worker_pid\}"\)\s*,?\s*\]\)'
)


class ProcessSignalSourceTests(unittest.TestCase):
    def test_crash_tests_keep_owned_group_signal_guards(self):
        # Inspect only repository Rust sources, never build output or dependencies.
        calls = {}
        for directory in ("src", "tests", "crates"):
            for path in (ROOT / directory).rglob("*.rs"):
                source = path.read_text("utf-8")
                matches = list(CALL.finditer(source))
                if matches:
                    calls[path.relative_to(ROOT).as_posix()] = (source, matches)
        self.assertEqual(set(calls), EXPECTED,
                         "Review any added/removed group-signal call site explicitly")
        for name, (source, matches) in calls.items():
            with self.subTest(file=name):
                self.assertEqual(len(matches), 1)
                call, = matches
                self.assertIsNotNone(SAFE_ARGS.match(source[call.end():]),
                                     "Negative group IDs must follow an explicit --")
                start = source.rfind("let mut worker =", 0, call.start())
                self.assertGreaterEqual(start, 0)
                before_signal = source[start:call.start()]
                for guard in (
                    ".process_group(0)",
                    "let worker_pid = worker.id();",
                    "assert!(worker_pid > 1);",
                    "assert_ne!(worker_pid, std::process::id());",
                    "assert_eq!(process_group_of(worker_pid), worker_pid);",
                    "assert_ne!(process_group_of(std::process::id()), worker_pid);",
                ):
                    self.assertIn(guard, before_signal)
                self.assertNotIn("worker.wait(", before_signal,
                                 "Keep the owned PID unreaped until group cleanup")


if __name__ == "__main__":
    unittest.main()
