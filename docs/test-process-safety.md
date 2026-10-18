# Unix crash-test safety correction — 2026-10-18

Use current master or `agent-source-preview-20261018-r2` for Unix crash tests. The first Agent source preview was withdrawn; do not run ignored crash tests from earlier Agent snapshots on Linux. Existing Toolkit v2.1.0 predates these Agent tests.

## What was unsafe

Three test-only crash/recovery cases started an owned child in its own process group, then passed a negative group ID to `/bin/kill` without an explicit end-of-options separator. The [procps-ng 4.0.4 parser](https://gitlab.com/procps-ng/procps/-/blob/v4.0.4/src/kill.c#L126) can interpret that operand using only its first digit. In particular, an ID beginning with 1 can become the special all-permitted-processes target. This is unsafe even though the intended group belonged to the test.

All three call sites are in Unix test code: one in `crates/video_agent/src/execution.rs` under `cfg(test)`, and one each in `tests/concurrency.rs` and `tests/workflows.rs`. They are not part of normal Agent/Toolkit video execution or release executables.

Two Linux CI attempts stopped reporting during this area and did not promptly respond to cancellation. That is consistent with the parser defect, but the actual runner's executable/version was not recovered, so its exact failure cause is not asserted. Those runs and cancellation requests are retained, not counted as passing tests.

## Correction

- Pass `--` before every negative process-group operand.
- Before signalling, query only the specific positive child/parent PIDs and verify that the child's group equals its owned PID, is greater than 1 and differs from the parent's group.
- In owner-only crash cases, keep the killed child unreaped until the test has verified and cleaned up its media group; do not reuse a stale PID after reaping.
- Preserve the actual crash, independent reopen, conservative accounting, shard reuse and unchanged-input assertions. No affected test is skipped.
- Add an offline source guard for the three call sites, their explicit operands and identity checks. This guard does not replace running the real crash/recovery tests.
- Give Linux media validation a finite step deadline and emit fixed, path-free stage markers for the previously opaque splitter case.

The fixed tests still intentionally kill their own private subprocesses. They require FFmpeg/ffprobe and permission to inspect those exact process identities. A denied identity check must stop before any group signal. That failure path has no unified subprocess-reaping guard and may leave a test child or media orphan temporarily; it is not a zero-orphan guarantee. These tests are workflow/recovery evidence, not human semantic evaluation or an OS-level resource-supervision guarantee.
