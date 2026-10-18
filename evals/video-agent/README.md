# Video Agent visual selection evaluation

This small suite separates workflow safety, playable media, and semantic selection quality. A decoded MP4 proves neither visual understanding nor relevance. Genuine development calls, failures and a validated Luna draft are now recorded; formal human semantic review remains pending. [results/status.json](results/status.json) is the authoritative status, not the existence of exported baseline files or the initial freeze label in tasks.json.

Development records retain operator-declared local code snapshot hashes. The public source preview is a single commit on the existing public history; intermediate private development commits are not published. Those historical hashes are not public checkout links, signed build attestations or proof of binary provenance. Original runtime captures remain private; the public summaries retain their hashes and selected numeric facts, not raw task directories.

## Public footage and preparation

The fixed input is the original 2008 **Big Buck Bunny** film from Blender Foundation's official download server. It is computer animation with natural scene content, not live-action footage or a synthetic color-bar test. It is licensed under [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/) according to the [official rights statement](https://peach.blender.org/about/). For excerpts, retain this attribution:

> (c) copyright 2008, Blender Foundation / www.bigbuckbunny.org

Derived excerpts are trimmed, padded to an even frame height and re-encoded; uniform baselines also concatenate segments. Include the attribution, license link and change description beside any shared video or screenshots. Do not remove the credits when sharing the full, unmodified film. Blender logos and trademarks are not licensed by the film's content license. This repository's MIT license does not replace the film's CC BY license. No standalone soundtrack is downloaded or distributed.

Requirements: Python 3.11 or later, curl, FFmpeg and ffprobe with H.264/AAC encoders. Curl uses the system certificate trust store; TLS verification is never disabled. Run from the repository root, using an explicit local directory outside version control:

```sh
python3 evals/video-agent/materials.py fetch --cache-dir /private/tmp/video-agent-public-source
python3 evals/video-agent/materials.py prepare \
  --source /private/tmp/video-agent-public-source/BigBuckBunny_640x360.m4v \
  --output-dir /private/tmp/video-agent-public-inputs
python3 evals/video-agent/materials.py baseline \
  --task dev-rabbit-woodland \
  --prepared-dir /private/tmp/video-agent-public-inputs \
  --output-dir /private/tmp/video-agent-public-baseline
python3 -m unittest discover -s evals/video-agent -p 'test_*.py' -v
```

The fetcher checks both archive and extracted-member SHA256 and sizes from [dataset.json](dataset.json). It never replaces an existing file with a different fingerprint. Preparation verifies the fixed source and exports only the development excerpt by default. Every completed excerpt has a JSON record with the actual derived hash, source interval, FFmpeg version, full-decode check and stream metadata. The prepared source retains the movie's audio as AAC, stereo, 48 kHz. Source time maps as `original_ms = prepared_ms + clip.start_ms`. Encoded hashes can vary with FFmpeg versions; fixed source hashes and the recorded recipe identify the input, and each evaluation pins its actual prepared-file hash.

## Frozen tasks and holdout discipline

[tasks.json](tasks.json) contains two development tasks, including an absent ocean/city negative case, and two heldout tasks on disjoint source intervals. The suite is intentionally small and single-film; it cannot establish general video understanding. It does not assert pre-labeled correct intervals or presume every requested category exists.

Only run `prepare --include-heldout` and `baseline --allow-heldout` after pinning the code commit, model configuration and prompt version. Heldout footage and task intents must not be used to tune prompts, thresholds or sample scheduling. If a heldout failure becomes development material, retire it and freeze a new suite before making further heldout claims. Looking at a task file during harness development does not make it a blinded semantic test; reviewers should disclose such exposure. No semantic model calls or scene-based tuning have been made on these heldout intervals at initial freeze.

The uniform baseline uses four windows centered in equal source strata, ordered chronologically, with total duration equal to the target. For the 75-second excerpts and 20-second target these are exactly `6.875–11.875`, `25.625–30.625`, `44.375–49.375`, and `63.125–68.125` seconds in the prepared input. The script exports a real decodable video and a source-time manifest; it uses no model and no frame-based selection. It is a comparison candidate, not an AI demo. The negative task is expected to expose a weakness of an unconditional baseline, but that expectation is not a completed human score.

## Three separate checks

1. **Workflow correctness:** deterministic tests cover validated ranges and paths, input identity, revision-bound approval, bounded operations, cancellation, retries, stage/shard recovery, stale inputs and output protection. Test fixtures or scripted providers are labeled as such and do not count as real visual selection.
2. **Media correctness:** on the exact candidate file run ffprobe, decode all video/audio packets, verify expected streams and duration, then play it. Test both retained source audio and silent-source behavior in the product. The baseline's audio behavior does not prove the product's export behavior.
3. **Semantic quality:** a human reviews the selected frames and resulting segments for each intent and compares them with the uniform baseline. Neither successful encoding nor a schema-valid model response supplies this judgment.

For a real run, copy [result-template.json](result-template.json) to a new uniquely named record and fill it from the product's original persisted task file. Do not replace pending values with estimates of unrun behavior. Preserve raw task records privately; publish only sanitized records for this public footage.

Required observation entries are `{sample_id, source_id, prepared_time_ms, original_time_ms, image_sha256}`. Each model-call entry must include backend, requested and returned model identifiers when available, prompt version, exact sample IDs sent, response/request ID when provided, validated decision, status/error, input/output token counts, latency and estimated cost. Tool calls must identify the typed tool, validated arguments, resulting frame IDs or plan revision, status and error. Record every additional observation request, rejected call, retry, refusal and budget stop, not only the final proposal. Redact credentials and local private paths, not failure evidence. Configured per-token costs are estimates, never provider invoices; missing usage remains unknown, not zero.

## Read-only evidence capture and pending A/B materials

`evidence.py` supplies the narrow bridge from a retained runtime task to these frozen inputs. It uses only the Python standard library and the local material definitions. It makes **no API request**, runs **no FFmpeg/ffprobe**, never approves or exports a product plan, and does not modify task state, its execution ledger, `results/status.json` or human scores. It hashes existing files; that is not fresh decode verification.

After a genuine run, keep the original private task directory and preparation/baseline records. Use the exact frozen intent and target. Replace the example paths below with those existing files, and run from the pinned clean source checkout:

```sh
python3 evals/video-agent/evidence.py capture \
  --task dev-rabbit-woodland \
  --workspace /absolute/private/task-folder \
  --prepared-record /absolute/private/inputs/bbb-dev-001.json \
  --baseline-record /absolute/private/baseline/dev-rabbit-woodland-uniform.json \
  --code-commit "$(git rev-parse HEAD)" \
  --output-dir /absolute/private/new-run-capture
```

The destination's parent must already exist; the destination must not. Heldout capture/pair requires `--allow-heldout`, checked before reading its media. Do not use that flag during development. Capture checks exact frozen intent/target/input fingerprints, actual-PTS audit mappings and task-owned image hashes, ledger identity and settled state, call accounting, recorded production backend/response identifiers, baseline recipe and existing output ownership/hash/approval/plan digest. Video-stream and container durations are kept separate within the preparation recipe's 150 ms tolerance. Original-time mapping follows the preparation record; codec padding is not proof of extra observed source content.

The resulting `run.json` is always `runtime_record_unattested`, with `publication_ready=false`: local JSON and response identifiers can be forged. A code commit is operator-declared, not a signed binary attestation. Production-looking fields do not authenticate an API call. Manual/zero-model/simulated/mixed-backend records are refused. All attempts stay visible; accounted reservations, known not-sent refunds and provider-reported token usage are separate. Invoice cost, historical unit prices and the original proposal revision remain unknown when the runtime did not persist them. Frame credits can exceed unique model samples because human previews also consume credits. Repeated observation mappings and replay events are not extra API calls or unique frames.

Capture retains the **first typed proposal**, even if rejected, and separately identifies the first validated proposal. A later successful retry must not replace an initial failure in the comparison. Failure text with possible private paths/credentials is withheld while its status and original text hash remain. Other free text still requires a human privacy review before publishing; the scanner is a guardrail, not a proof that arbitrary text is safe. Share neither raw tasks nor private keys/configuration.

For initial-proposal media comparison, first review, approve and export that unchanged draft through the normal product UI, then capture it **before** editing. This does not waive human approval. Later human-edited output is distinct; it cannot be scored as the initial model proposal. Existing edited tasks can still yield their original typed proposal and failure/usage evidence, but this tool will not invent an earlier export or revision. An empty/refused proposal is captured without fabricating a video; its human evaluation remains pending, and no two-video pair is generated.

If an actual unchanged initial export exists, create pending review materials:

```sh
python3 evals/video-agent/evidence.py pair \
  --run-record /absolute/private/new-run-capture/run.json \
  --initial-video /absolute/private/task-folder/exports/PLAN_DIGEST.mp4 \
  --baseline-record /absolute/private/baseline/dev-rabbit-woodland-uniform.json \
  --output-dir /absolute/private/new-review-pair
```

Only share the new **`review/` directory** with the first-stage reviewer. It contains exact-byte `candidate-A.mp4`/`candidate-B.mp4` copies and an A/B form with all answers null, no explanations and no condition labels. Keep `private/mapping.json` separate; the review form commits to its salted SHA256. Candidate assignment is randomized. Original encoding properties, durations and the publicly reproducible baseline can still expose identity, so this is label blinding, not guaranteed technical anonymity. No score or preference is generated.

Retain the reviewer's original completed A/B form before revealing explanations. Then give the reviewer the cited samples/reasons from the captured first proposal for the separate faithfulness assessment below. The legacy `result-template.json` remains an organizer's summary template, **not** the blinded first-stage form. This tool deliberately does not infer reviewer identity, verify that a respondent is human, import scores, compute semantic rankings, or claim that an unrun evaluation passed. Pending forms do not change the authoritative suite status.

Offline tests use deliberately inert temporary files and synthetic production-shaped records to check rejection, accounting and packaging rules. They make no genuine model or playable-media claim and are never written to `results/`:

```sh
python3 -m unittest discover -s evals/video-agent -p 'test_*.py' -v
```

## Human comparison rubric

Randomize candidate names and order before review. The reviewer first watches both exports without model explanations. Rate relevance from 0 (no requested content) to 4 (nearly all selected content clearly matches the intent), noting approximate irrelevant seconds. Then review the actual input and list missed relevant intervals; report omissions as timestamped observations, not an unsupported recall percentage. Finally expose the model's cited samples and rate reason faithfulness from 0 (fabricated/contradicted) to 4 (all material claims supported by the observed frames and uncertainty stated). Treat inferred audio, unseen actions and uncited event continuity as unsupported claims.

A reviewer may prefer a short/refused plan over fabricated content. Record actual selection duration, category order, requested but unavailable content, the chosen candidate and free-text rationale. Evaluate the initial model proposal separately from the human-edited result: a user's correction must not retroactively improve the model's score. Store reviewer identity or pseudonym, date, any prior exposure, and both candidate hashes. Keep failures in the results, and report the small sample size instead of presenting this suite as a benchmark leaderboard.

## Three minute demonstration outline

- **0:00–0:25:** Open the real desktop application, choose `bbb-dev-001.mp4`, show the public-material attribution, enter the development intent and visible budgets.
- **0:25–1:00:** Show local timecoded samples, grant explicit consent for those JPEG samples and task text, then run the real configured OpenAI backend. Display actual observations, selected clips, reasons and uncertainty. If credentials are unavailable, stop this part and label it blocked; do not substitute the baseline or a scripted provider.
- **1:00–1:45:** Play candidate previews, delete or trim one clip and reorder another. Show the changed revision and fresh confirmation before export.
- **1:45–2:30:** Begin a real export, cancel or close at a stage boundary, reopen the persisted task and resume. Show completed stages/shards reused; do not claim arbitrary-position encoding resume.
- **2:30–3:00:** Play the final output with the chosen audio policy. Show probe/decode results, elapsed usage, cost estimate and output location. Distinguish this single demo from the later frozen semantic comparison.

This is an outline, not a claim that a demonstration has been recorded. Never expose API keys, private videos, local runtime directories or unrelated file paths in a public recording.
