# Video Agent — experimental source preview r2

**Safety correction:** this replaces the withdrawn first Agent preview. Do not run Unix crash tests from earlier Agent snapshots. r2 uses explicit process-group signal operands, checks the owned child's process-group identity and keeps its PID reserved until cleanup. The defect was confined to crash-test code, not the normal video-processing path. [Details](https://github.com/Alaneel/video-toolkit/blob/master/docs/test-process-safety.md). Existing Toolkit v2.1.0 releases do not contain these new Agent tests.

A native Rust desktop workflow for timestamped observations, model-proposed clips, human editing and verified MP4 export. Built on the existing Video Toolkit media engine.

## What is here

- Two bounded vision tools: request interval observations or propose evidence-linked clips. The model cannot approve or export.
- A native review surface for source playback, trimming, rejection, reordering and exact-revision approval.
- Persisted task state, conservative execution accounting, cancellation and verified-stage/shard recovery.
- A real OpenAI Responses backend, now using the user-approved GPT-6 Luna default.
- Frozen public-material recipes, a uniform baseline and failure-preserving evaluation capture tools.

## Actual validation and limits

On the public development excerpt, Luna made two follow-up observation requests and produced a structurally valid five-clip, 25-second draft after a retained citation rejection. One clip lacks the requested visible rabbit. User-directed editing and model-origin export are still awaiting the author's decision; formal human semantic scores remain pending. Three earlier legacy-model proposals also remain visible in the results. A valid schema or playable video is not a relevance score.

Native manual runs separately demonstrate editing, cancellation/reopening and actual playable exports. The first runtime-validated Agent target is macOS Apple Silicon. Other Agent platforms, a continuous real-model demonstration and heldout semantic comparisons are not established by this preview.

The corrected source at `5e5422909ae2cd557fe214796e1e8a1afc3dd3e8` passed the [Linux/macOS/Windows CI checks](https://github.com/Alaneel/video-toolkit/actions/runs/39093650920), including all 29 real-media tests on Linux and the three corrected crash/recovery cases. [Four-target builds](https://github.com/Alaneel/video-toolkit/actions/runs/39093650950) also passed; the license gate correctly left zero binary artifacts. The final r2 follow-up adds documentation only, with the same tested Rust/script source. These checks are not semantic-quality evaluation or cross-platform desktop-runtime acceptance.

## Build locally

Install Rust 1.87+, FFmpeg and ffprobe; ffplay enables interval playback. From this source checkout:

```sh
cargo build --locked --release
./target/release/video-toolkit agent
```

Use **Manual workflow — no AI** without credentials. Real model calls require a privately configured local key and explicit consent to share sampled JPEGs and bounded task text; do not put keys or runtime task directories in Git.

## Distribution status

This is a **source-only prerelease**, not an installer release. Six unresolved dependency license-text questions still block public binaries. Packaging fails closed, with no public package/checksum uploads while target-specific distribution approval is missing. FFmpeg is not bundled. Existing v2.1.0 downloads are the legacy Toolkit, not this Agent.

The preview's public commit contains the final source tree on the existing public history. Operator-declared intermediate development snapshot hashes in evidence records remain local and are not public checkout links or signed build attestations.

[Setup and boundaries](https://github.com/Alaneel/video-toolkit/blob/master/docs/video-agent.md) · [Machine-readable evidence](https://github.com/Alaneel/video-toolkit/blob/master/docs/video-agent-evidence.json) · [Evaluation status](https://github.com/Alaneel/video-toolkit/blob/master/evals/video-agent/results/status.json)
