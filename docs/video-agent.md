# Video Agent: setup, workflow and boundaries

This experimental `2.2.0-rc.1` source preview adds a human-reviewed visual rough-cut workflow to the existing Rust workspace. Source publication and GitHub showcase updates are authorized; public Agent binaries remain blocked by the unresolved dependency notice review. No new Agent binary tag is released. The stable public v2.1.0 binary remains the legacy toolkit.

## Run the desktop application

The first tested target is macOS Apple Silicon. Install Rust 1.87 or newer and FFmpeg/ffprobe; install ffplay for source-interval and result playback. On macOS, `brew install ffmpeg` supplies these media tools. FFmpeg is a separate dependency, not bundled or relicensed by this repository. Build from this candidate checkout:

```sh
cargo build --locked --release
./target/release/video-toolkit agent
./target/release/video-toolkit agent --workspace /absolute/path/to/task-directory
# Dedicated app entry point (same native interface):
./target/release/video-agent
```

The desktop's **Observation and execution budgets** panel sets limits for a new task, including **Simultaneous frame extractions** (1–4, default 1). A diagnostic CLI task can use the same limit with `agent-task create ... --frame-workers 4` (`--max-concurrency` is an alias). Limits are persisted at creation; changing controls for a new task does not mutate a previously opened task.

The local Mac RC also has a double-clickable `Video Agent RC.app`. Its executable is the dedicated `video-agent` entry point; the legacy `video-toolkit` binary is included separately. On macOS this app appends existing `/opt/homebrew/bin` and `/usr/local/bin` to its own PATH so an ordinary Homebrew FFmpeg installation can be found without inheriting a shell. It does not modify system settings or install tools.

If the machine's selected Xcode requires an unaccepted license, select an already installed, licensed toolchain yourself. In this environment the existing Command Line Tools worked with `DEVELOPER_DIR=/Library/Developer/CommandLineTools`; no license was accepted automatically.

Without a subcommand the application still opens the original Video Toolkit interface. Existing CLI commands retain their behavior. The new `agent-task --help` commands are diagnostic/reproducibility helpers, not a substitute for the native interface.

## Configure the one real provider

OpenAI Responses is the only production vision backend. Credentials are read from `OPENAI_API_KEY`, then `VIDEO_AGENT_API_KEY`, or a private JSON file at `$HOME/.config/video-agent/config.json`. `VIDEO_AGENT_CONFIG` can name another private file outside any Git checkout. Do not paste a key into chat, a command recorded in shell history, task JSON, screenshots or repository files. Configure it locally and launch/relaunch the application from that environment. GUI applications launched from Finder may not inherit shell variables.

Example structure for the private file, **not a file to commit**:

```json
{
  "api_key": "YOUR_LOCAL_SECRET",
  "model": "gpt-6-luna"
}
```

`VIDEO_AGENT_MODEL` overrides the model name. GPT-6 Luna and the explicitly configured legacy GPT-4.1-mini snapshot/alias have separate built-in budget settings. Other models require `input_usd_per_million`, `output_usd_per_million` and `image_tokens_upper_bound` in the private JSON. Choose a text+image model supporting Responses function calling, check its current schema and pricing, and rerun the real evaluation; arbitrary model IDs are not automatically compatible. The endpoint is fixed to verified-TLS `https://api.openai.com/v1/responses`, with redirects disabled.

The default is now [GPT-6 Luna](https://developers.openai.com/api/docs/models/gpt-6-luna), switched with the user's approval after three retained development failures from the old model. Requests explicitly use Responses, `reasoning.effort=low` and standard `service_tier=default`; the existing 2,500-token output cap includes reasoning tokens. The legacy `gpt-4.1-mini-2025-04-14` remains configurable but is [scheduled for shutdown on January 14, 2027](https://developers.openai.com/api/docs/deprecations#2026-10-14-gpt-4-series-model-deprecations). A supported API contract is not semantic acceptance; see the actual evaluation status.

Current [standard short-context rates](https://developers.openai.com/api/docs/pricing) for Luna are $0.10 per million input tokens, $0.125 for cache writes and $0.50 for output. Local estimates use the higher $0.125 input rate and $0.50 output rate, without assuming cache hits, plus a configured 2,048-token allowance for each product thumbnail of at most 640×360 pixels. Larger Luna images require a higher explicit local image allowance and still obey the global 2,048-pixel dimension limit. The allowance is a local safety estimate, **not an official Luna image-token formula or guaranteed upper billing bound**; the image guide does not currently specify Luna's exact multiplier. Requests whose local input reservation exceeds 272,000 tokens are refused rather than silently applying short-context prices. The legacy model retains its separate $0.40/$1.60 rates and patch estimate. The local guard reserves input/image estimates plus the configured maximum output tokens before sending; completed responses reconcile reported usage, and reported overages are retained and rejected. Failed/interrupted requests retain reservations because billing may be unknown. These are not provider invoices or an account spending guarantee. Set provider-side project/account controls too. Price changes and model migration require configuration review.

Prompt `video-agent-vision-v4` explicitly supplies per-clip and total duration bounds, interval/evidence rules and explanation limits. It follows retained v1/v2/v3 development failures. Every supplied sample also has an optional arithmetic-only legal time-window example; these are not relevance labels and do not choose the plan. The model still selects, requests observations, proposes another valid range or refuses. A rejected proposal stops immediately; only another explicit user action sends a new request, with fixed rules and the prior proposal's numeric timing facts and known source/sample IDs. Raw errors, explanations and paths are not copied into this feedback. It does not expand budgets, approve a plan or erase failed attempts.

All locally observed frames stay in the task, but each new model request uses at most **16 images**. When there are more, a deterministic, semantic-free policy allocates slots across sources with observations and selects evenly spaced actual timestamps, including each source's endpoints. Smaller sets retain their original order. This bounds repeated context without lowering the image allowance or enlarging the task's token/cost budget. The per-call `sample_ids` record identifies the actual subset; saved but unsent frames cannot support that call's proposal. This is not a memory of an unseen image or complete-video understanding.

## Use the native workflow

1. **Sources & intent:** choose one local video initially, or up to three with the default limit. Choose an independent task-parent directory outside the repository. Enter the desired visual content, approximate duration and budgets. Create a task to probe streams and fingerprint the original without modifying it.
2. **Observed frames:** generate the local timestamped storyboard. Review what was actually sampled. Grant explicit consent only for footage you may send to OpenAI; this includes bounded follow-up JPEG observations, intent, source IDs, timestamps and dimensions. No original paths, source video or audio are included in the model request. Intent and images can still contain sensitive information.
3. **Ask the vision model:** the real backend can request a bounded interval observation or propose evidence-linked clips. The interface records reasons and uncertainty, requested and returned model IDs, prompt version, sample identities, usage and failure states. It does not claim complete-video or audio understanding. A missing key disables this action; an HTTP/schema/budget error is shown, not replaced by fabricated candidates.
4. **Human review:** play each interval with source audio, inspect additional local frames, change in/out points, reject clips, change order, or add explicitly human-selected intervals. Save edits, then confirm that exact saved plan. A dirty or subsequently changed plan is not exportable under an old confirmation.
5. **Result:** export the confirmed plan. Open/play the completed file and use re-check to verify it again. Success requires expected streams/duration and a full decode, not merely a zero exit code or a file on disk.

The expandable **Manual workflow — no AI** option lets you test human editing and local export without credentials. Its first bounded interval is explicitly labeled manual; it is neither semantic selection nor a model demonstration.

## Media policy

The first version deliberately exports a single format: H.264 MP4, 640×360 letterboxed, 30 fps. The `PreserveWithSilence` policy uses the first audio stream, re-encodes to stereo AAC at 48 kHz, and supplies silence when a clip has no audio. It is not source-quality passthrough. `Mute` exports no audio stream. Clips are hard cuts without transitions, subtitles, voiceover or a full timeline editor. Preview uses a separately managed ffplay process; if unavailable, install it before claiming playback validation.

Inputs must be ordinary local media files, not network URLs or playlists. The model never supplies a filesystem path, executable or codec argument. Fixed FFmpeg/ffprobe calls disable overwrite and use bounded timeouts. Output is stored separately under the task; no original is changed or silently replaced.

## State, cancellation and recovery

Each task stores `task.json`, `execution-ledger.json`, `frames/`, `shards/` and `exports/`. State includes source SHA256/size/mtime, samples and hashes, consent, model decisions and usage, plan revisions, human provenance, approval digest and export verification. Keep this directory private: it contains source paths, intent, images and records even though it contains no API key.

Task writes are atomic and process-locked. Frame/shard publication is journaled before a no-clobber filesystem operation. Reopening validates inputs and reuses completed stages/shards; a changed or missing source makes the task stale and revokes approval and cloud consent. A changed plan needs a fresh confirmation. An already verified export for that plan is rechecked and reused, not duplicated.

Cancel stops local child processes and retains completed work. Cancellation during a blocking cloud request may wait until its bounded HTTP timeout; it does not guarantee the provider stopped billing. Unknown usage stays charged against the local budget. If local admission stops before the provider is entered, the record is `not_sent`: token/cost reservations are refunded, but the local attempt still counts toward the call limit. No automatic network retry is sent. Retry is an explicit human action within the remaining call/frame/token/cost/time budget. Incomplete temporary files are never presented as finished results. Recovery is at stages or verified clip shards, not at an arbitrary frame inside an interrupted encode.

Default limits: 12 initial / 36 total frames, 4 model attempts, 2,500 output tokens per call, 100,000 total token accounting, $0.25 estimated cost, 600 seconds accounted execution time, 120 seconds per local media command, 45 seconds per model request, three sources, one frame-extraction worker, 15 seconds per clip, 120 seconds total output. Limits are validated and shown in the interface. Increasing a supported bound is an explicit local choice, not a model action. Time spent waiting for human review is not execution time.

`max_concurrency` now controls 1–4 simultaneous local JPEG extraction workers for both the initial storyboard and bounded follow-up observation. It is not a number of cloud requests, independent Agent tasks or FFmpeg CPU threads. Model requests, source checking, human confirmation and ordered export remain sequential. A coordinator alone holds the task lock and publishes records; workers only extract into independent task-owned temporary directories. The coordinator admits unknown seeks against their worst-case remaining frame allowance before starting any worker, joins/drains every worker before proceeding, verifies sources, then records and publishes results in deterministic request order with actual-PTS deduplication. A failed/cancelled batch publishes no incomplete new observations; earlier completed batches remain reusable.

The frame-credit counter covers journaled unique observation records and charged human-only preview attempts, not every failed decode attempt. Observation metadata is journaled before image publication; after interruption a missing image must be regenerated and verified before use. A human preview is charged before extraction, and a failed/cancelled preview can retain that credit without a saved JPEG. The storyboard list and each request's recorded sample IDs, not this counter, identify model-visible evidence. Discarded temporary work is still subject to execution deadlines and durable wall-time accounting. `peak_media_workers` is a recorded extraction-worker peak, while `media_worker_ms` sums returned workers' wall durations and may exceed elapsed execution wall time under parallelism; it does not consume the execution budget a second time. History records actual FFmpeg child PID/start/end spans against a shared monotonic batch clock for auditing overlap. Work lost before a hard kill is not reconstructed as measured worker telemetry.

Before each bounded hash/media/model step, an atomic, synced execution ledger reserves at least 5 ms and at most the smaller step deadline or remaining budget. Normally completed steps settle measured time (rounded up to milliseconds) and refund unused time. If the owning process dies, the next exclusive owner charges that abandoned upper bound exactly once; unknown interrupted time is never silently reset to zero. `elapsed_ms` includes measured execution and recovered conservative charges, `conservative_elapsed_ms` identifies the latter, and `execution_reserved_ms` shows the current in-flight upper bound. Fresh source/output checks on reopening are also budgeted. Exhausted tasks remain viewable as saved metadata, without fresh fingerprint verification or export/playback authorization; further processing is refused.

This is crash-conservative **accounting and admission control**, not an OS-enforced CPU/runtime quota. File-system stalls, process scheduling and bounded watchdog polling can add overhead. If only the owning app is killed, an already spawned FFmpeg process may outlive its watchdog; the application cannot promise a wall-clock stop for that orphan. Real tests separately kill a worker/media process group to verify one-time budget reconciliation, and kill only a production export owner after a verified shard checkpoint, clean up any orphan media in the test harness, then resume in another process and verify shard reuse and final output. These are not power-loss tests or real-model semantic evaluations.

Existing pre-ledger tasks migrate from their saved execution counter and explicitly record that older uncheckpointed work cannot be reconstructed. Once journaling is enabled, a missing, foreign, symlinked or malformed ledger fails closed instead of silently migrating again. These local files are not tamper-proof or signed. Initial source inspection now reserves before hashing/probing; interruption before inspection finishes leaves an incomplete directory without a valid `task.json`, which cannot be opened as a completed inspection.

## Architecture and reuse

`crates/video_agent` contains typed domain data, private provider configuration, bounded media operations, atomic task storage and orchestration. `crates/ui/src/agent_app.rs` is the native interface. The existing `common::VideoFormat::MP4` codec/muxer policy is reused. The old clipping APIs do not offer cancellation, write-ahead recovery or no-clobber publication and use overwrite flags; the small new execution adapter supplies those missing guarantees without replacing FFmpeg or changing legacy workflows. The model's entire tool surface is `observe_interval` and `propose_plan`. Export and approval exist only in deterministic human-facing code.

## Validation and known gaps

Run the workflow tests and separate real media suite:

```sh
cargo test --locked --workspace
cargo test --locked -p video_agent -- --ignored --test-threads=1 --nocapture
python3 -m unittest discover -s evals/video-agent -p 'test_*.py' -v
```

Scripted providers are labeled simulations. The public-material harness, frozen tasks, interval baseline and human rubric are in [the evaluation guide](../evals/video-agent/README.md). Legal source and attribution are documented there and in [the materials notes](video-agent-materials.md).

Genuine calls on consented public development footage began on 2026-10-18. Three legacy-model proposals failed deterministic validation and remain in the results. User-approved GPT-6 Luna then made two bounded observation requests, retained an initial citation rejection and produced a validated five-clip, 25-second draft. The draft still included a burrow-only shot without the requested visible rabbit; structural acceptance did not make that selection relevant. Author-directed editing/export and formal human semantic comparison remain pending. See [the current status and all retained failures](../evals/video-agent/results/status.json). No claim of better relevance, recall or faithful reasoning is justified by the offline tests. Windows, Linux and Intel Mac are not runtime-validated for the new Agent; source portability and legacy release matrices do not establish Agent support. The current Mac binary candidate is unsigned/unnotarized, has external FFmpeg/ffplay dependencies and is not a public binary release.

## Actual local evidence

[The evidence record](video-agent-evidence.json) distinguishes the native manual walkthrough from the pending real-model evaluation. Five actual screenshots are under `assets/video-agent/`: local storyboard, edited manual plan, cancelled export, verified result and actual QuickTime playback. The walkthrough produced an ordered 27-second MP4 from prepared source ranges 30–45s and 10–22s, with zero model calls and no cloud consent. Video duration is 27.000s; the container is 27.021333s because AAC has padding. The original SHA256 remained unchanged. The GUI run demonstrates cancel/resume from retained stages; reuse of an already encoded shard is separately covered by the live-encoder cancellation test.

The local companion `manual-ui-snapshot-walkthrough.mp4` is a 25-second **still-screenshot walkthrough**, not a continuous screen recording or a real-model demonstration. `manual-workflow-output.mp4` is the actual exported video. Keep the included media attribution with either file: (c) copyright 2008, Blender Foundation / www.bigbuckbunny.org, [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/); trimmed, reordered and re-encoded. The unmodified source is acquired through the licensed-material harness, not shipped as private task data.

A follow-up using the extracted `4c05e71` app completed the real two-source desktop path: an audio-bearing development source and a silent derivative, 12 timestamped samples, human preview, trim and cross-source reordering, revision-bound confirmation, export, reload and re-check. The 8-second MP4 plays in QuickTime; measured audio is silent in the first interval and non-silent in the second. Source hashes stayed unchanged. This is additional **manual, zero-model-call** evidence, not a semantic evaluation, and it does not retroactively test later execution-accounting changes. Details and file hashes are in the evidence record; the actual media and screenshots are retained in the separate local follow-up delivery.

## Dependency notices

[THIRD_PARTY_NOTICES.txt](../THIRD_PARTY_NOTICES.txt) mechanically inventories the locked Mac-target normal/build dependency graph, original license/notice texts, four embedded font notices and the Rust standard-library notices. Supplemental texts are tied to exact upstream commits with URL/SHA256 provenance under `third_party/licenses/`; they are not copied from a moving branch. No dependency source cache, font binaries or external FFmpeg distribution is bundled.

This is not a completed legal/compliance audit. Six old packages still lack a recoverable original code-license text in their published crate and the checked exact upstream tree: `block 0.1.6`, `dispatch 0.2.0`, `malloc_buf 0.0.6`, `objc-foundation 0.1.1`, `objc_id 0.1.1`, `system-configuration-sys 0.5.0`. Their declared SPDX/authors/repository metadata is preserved; missing original text is explicitly unresolved, not replaced with a fabricated license. An exact cached-package/source review additionally preserved the seven-line `system-configuration-sys` copyright and license-reference header; it is not the full license text and does not close any of those six gaps. The current binary is for local private review. Resolve the remaining notice questions before public binary distribution.

Public packaging fails closed: source builds still run, but package/checksum uploads are skipped while distribution is unapproved, and tagged binary releases fail the gate. Clearance requires resolved notice inventories and an explicit review bound to the target, notice hash and Cargo.lock hash. The current inventory covers Mac ARM64 only; restoring the full legacy release matrix also requires reviewed inventories for each other target. A source-only preview is not a binary-distribution approval.

Recheck the collection from a full source checkout with the same installed Rust toolchain and cached dependencies: `python3 scripts/third_party_notices.py --check`. Acquisition of supplemental public license text is an explicit opt-in operation; normal generation/checking is offline. See the script's help for that separate workflow.
