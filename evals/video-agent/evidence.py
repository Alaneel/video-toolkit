#!/usr/bin/env python3
"""Read-only local-record capture and pending A/B assembly; no API/media execution.

Runtime records are locally editable, not provider-signed evidence. This tool
does not establish genuine API execution, decoding success, or semantic quality.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import secrets
import stat
import sys

import materials

MAX_JSON = 8 * 1024 * 1024
MAX_MEDIA = 512 * 1024 * 1024
HEX = re.compile(r"[0-9a-f]{64}\Z")
IDENT = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]{0,127}\Z")
PRIVATE_TEXT = re.compile(
    r"(?<![:/\w])/(?!/)[A-Za-z0-9_.-]+|(?<!\w)[A-Za-z]:[\\/]|~/|file://|"
    r"\bsk-[A-Za-z0-9_-]{20,}|\bBearer\s+\S+|(?:OPENAI_API_KEY|VIDEO_AGENT_API_KEY)\s*[=:]"
)
STATUSES = {
    "reserved", "interrupted_unknown_usage", "failed_unknown_usage", "invalid_usage_record",
    "not_sent", "response_rejected", "usage_exceeded_reservation", "received",
    "observations_applied", "validated", "rejected", "completed",
}
UNKNOWN_USAGE = {"reserved", "interrupted_unknown_usage", "failed_unknown_usage", "invalid_usage_record"}
TASK_STATUSES = {"created", "sampling", "awaiting_consent", "analyzing", "review", "exporting",
                 "completed", "cancelled", "failed", "stale"}
LIFECYCLE = {"created", "cloud_consent", "model_call_reserved", "model_response_rejected", "model_result_reused",
             "model_observation", "model_plan", "human_edit", "human_approval", "manual_plan",
             "operation_stopped", "preflight_stopped", "source_invalidated", "output_invalidated",
             "execution_accounting_started", "execution_reservation_recovered", "export_complete",
             "export_reused", "sampling_complete"}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def integer(value: object, label: str) -> int:
    require(type(value) is int and value >= 0, f"Invalid {label}")
    return value


def text(value: object, label: str, limit: int = 8000) -> str:
    require(isinstance(value, str) and 0 < len(value) <= limit, f"Invalid {label}")
    return value


def explanation(value: object) -> None:
    require(isinstance(value, str) and bool(value.strip()) and len(value.encode()) <= 8000,
            "Invalid selection explanation (expected 1–8000 UTF-8 bytes)")


def sha(value: object) -> str:
    require(isinstance(value, str) and HEX.fullmatch(value) is not None, "Invalid SHA256")
    return value


def regular(path: Path, cap: int):
    # O_NOFOLLOW also closes the final-component symlink race on the tested Mac.
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    stream = os.fdopen(fd, "rb")
    info = os.fstat(fd)
    if path.is_symlink() or not stat.S_ISREG(info.st_mode) or info.st_size > cap:
        stream.close()
        raise ValueError("Expected a bounded regular, non-symlink file")
    return stream


def no_duplicates(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "Duplicate JSON key")
        result[key] = value
    return result


def load(path: Path) -> tuple[dict, bytes]:
    with regular(path, MAX_JSON) as stream:
        data = stream.read(MAX_JSON + 1)
    require(len(data) <= MAX_JSON, "JSON record exceeds size bound")
    value = json.loads(data, object_pairs_hook=no_duplicates,
                       parse_constant=lambda _: (_ for _ in ()).throw(ValueError("Nonfinite JSON")))
    require(isinstance(value, dict), "Expected a JSON object")
    return value, data


def file_hash(path: Path, expected: str | None = None) -> str:
    with regular(path, MAX_MEDIA) as stream:
        before = os.fstat(stream.fileno())
        digest = hashlib.sha256()
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
        after = os.fstat(stream.fileno())
    require((before.st_size, before.st_mtime_ns) == (after.st_size, after.st_mtime_ns),
            "File changed while being hashed")
    value = digest.hexdigest()
    require(expected is None or value == sha(expected), "File fingerprint mismatch")
    return value


def encoded(value: dict) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False, allow_nan=False) + "\n").encode()


def write_new(path: Path, data: bytes) -> None:
    with path.open("xb") as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def public_text_check(value: object) -> None:
    if isinstance(value, str):
        require(PRIVATE_TEXT.search(value) is None,
                "Potential private path or credential in exported text; review the private record")
    elif isinstance(value, dict):
        for item in value.values():
            public_text_check(item)
    elif isinstance(value, list):
        for item in value:
            public_text_check(item)


def public_error(value: object) -> dict:
    if value is None:
        return {"has_error": False, "text": None, "text_withheld": False, "detail_sha256": None}
    value = text(value, "error message", 32000)
    withheld = PRIVATE_TEXT.search(value) is not None
    return {"has_error": True, "text": None if withheld else value, "text_withheld": withheld,
            "detail_sha256": hashlib.sha256(value.encode()).hexdigest()}


def frozen(task_id: str, allow_heldout: bool) -> tuple[dict, dict]:
    tasks = [t for t in materials.SUITE["tasks"] if t["id"] == task_id]
    require(len(tasks) == 1, "Unknown frozen task")
    task = tasks[0]
    # Must happen before opening any run, preparation, baseline, image or video.
    require(task["split"] != "heldout" or allow_heldout,
            "Heldout records require --allow-heldout before reading material")
    clip = next(c for c in materials.DATASET["clips"] if c["id"] == task["clip_id"])
    return task, clip


def baseline_record(path: Path, task: dict, clip: dict, input_hash: str, duration_ms: int) -> tuple[dict, bytes]:
    record, raw = load(path)
    require(record.get("kind") == "nonsemantic_baseline" and record.get("model_used") is False,
            "Not a nonsemantic baseline record")
    require(record.get("task_id") == task["id"] and record.get("source") == clip["id"]
            and record.get("baseline_id") == materials.SUITE["baseline"]["id"]
            and record.get("input_sha256") == input_hash, "Baseline binding mismatch")
    expected = materials.uniform_windows(duration_ms, task["target_duration_ms"])
    for window in expected:
        window.update(original_start_ms=clip["start_ms"] + window["start_ms"],
                      original_end_ms=clip["start_ms"] + window["end_ms"])
    require(record.get("clips") == expected, "Baseline windows differ from frozen recipe")
    require(record.get("file") == f"{task['id']}-uniform.mp4", "Unexpected baseline filename")
    file_hash(path.parent / record["file"], sha(record.get("sha256")))
    return {"baseline_id": record["baseline_id"], "sha256": record["sha256"],
            "input_sha256": input_hash, "clips": expected,
            "record_sha256": hashlib.sha256(raw).hexdigest()}, raw


def decision_copy(decision: object) -> dict | None:
    if decision is None:
        return None
    require(isinstance(decision, dict), "Invalid typed decision")
    if decision.get("tool") == "observe_interval":
        keys = ("tool", "source_id", "start_ms", "end_ms", "count", "reason")
    else:
        require(decision.get("tool") == "propose_plan", "Unknown typed tool")
        keys = ("tool", "clips", "summary")
        require(isinstance(decision.get("clips"), list), "Invalid proposal clips")
        require(len(decision["clips"]) <= 240, "Too many proposal clips")
        clip_keys = ("source_id", "start_ms", "end_ms", "reason", "uncertainty", "evidence_ids")
        decision = dict(decision)
        decision["clips"] = [{key: item[key] for key in clip_keys} for item in decision["clips"]]
    return {key: decision[key] for key in keys}


def initial_matches(task: dict, proposal: dict | None) -> bool:
    # No inferred historical revision/audio policy: compare the actual current
    # plan, and reject any task that already contains a manual editing stage.
    if proposal is None or any(e.get("kind") in {"human_edit", "manual_plan"} for e in task["history"]):
        return False
    plan = task.get("plan")
    if not isinstance(plan, dict) or plan.get("summary") != proposal["summary"]:
        return False
    if plan.get("audio_policy") != "preserve_with_silence":
        return False
    keys = ("source_id", "start_ms", "end_ms", "reason", "uncertainty", "evidence_ids")
    return (all(c.get("human_modified") is False for c in plan["clips"])
            and [{k: c[k] for k in keys} for c in plan["clips"]] == proposal["clips"])


def plan_digest(plan: dict, assets: list[dict]) -> str:
    """Rust's declared struct order, compact UTF-8 tuple; never sort JSON keys."""
    clip_keys = ("id", "source_id", "start_ms", "end_ms", "reason", "uncertainty", "evidence_ids", "human_modified")
    ordered = {"revision": plan["revision"],
               "clips": [{key: item[key] for key in clip_keys} for item in plan["clips"]],
               "summary": plan["summary"], "audio_policy": plan["audio_policy"]}
    value = [1, ordered, [[asset["id"], asset["sha256"]] for asset in assets]]
    return hashlib.sha256(json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode()).hexdigest()


def capture(task_id: str, workspace: Path, prepared_path: Path, baseline_path: Path,
            code_commit: str, output: Path, allow_heldout: bool = False) -> dict:
    frozen_task, clip = frozen(task_id, allow_heldout)
    require(re.fullmatch(r"[0-9a-f]{40}", code_commit) is not None, "Expected full code commit")
    workspace = workspace.resolve(strict=True)
    task_path, ledger_path = workspace / "task.json", workspace / "execution-ledger.json"
    task, task_bytes = load(task_path)
    ledger, ledger_bytes = load(ledger_path)
    prepared, prepared_bytes = load(prepared_path)
    require(task.get("id") == workspace.name and task.get("schema_version") == 1, "Task identity mismatch")
    require(task.get("intent") == frozen_task["intent"]
            and task.get("target_duration_ms") == frozen_task["target_duration_ms"], "Frozen intent/target mismatch")
    require(prepared.get("clip") == clip and prepared.get("source_sha256") == materials.DATASET["source"]["member_sha256"]
            and prepared.get("recipe_version") == materials.DATASET["preparation"]["recipe_version"]
            and prepared.get("file") == f"{clip['id']}.mp4", "Preparation record mismatch")
    input_hash = sha(prepared.get("sha256"))
    prepared_duration = integer(prepared.get("media", {}).get("duration_ms"), "prepared duration")
    require(abs(prepared_duration - (clip["end_ms"] - clip["start_ms"])) <= 150,
            "Prepared duration differs from frozen interval")
    file_hash(prepared_path.parent / prepared["file"], input_hash)
    assets = task.get("assets", [])
    require(len(assets) == 1 and assets[0].get("sha256") == input_hash, "Evaluation requires the exact single frozen input")
    asset = assets[0]
    source_id = text(asset.get("id"), "source ID", 128)
    require(IDENT.fullmatch(source_id) is not None, "Invalid source ID")
    video_duration = integer(asset.get("duration_ms"), "source video duration")
    require(abs(video_duration - (clip["end_ms"] - clip["start_ms"])) <= 150
            and abs(video_duration - prepared_duration) <= 150, "Input duration mismatch")
    usage = task["usage"]
    require(usage.get("execution_accounting_version") == 1 and ledger.get("schema_version") == 1
            and ledger.get("task_id") == task["id"], "Missing or foreign execution ledger")
    require(ledger.get("reservation") is None and usage.get("execution_reserved_ms") == 0,
            "In-flight execution must finish before capture")
    settled = max(integer(usage.get("elapsed_ms"), "elapsed time"), integer(ledger.get("settled_ms"), "settled time"))
    conservative = integer(ledger.get("conservative_ms"), "conservative time")
    require(conservative <= settled, "Invalid conservative execution accounting")
    require(task.get("status") in TASK_STATUSES, "Unknown task status")
    require(task.get("status") not in {"sampling", "analyzing", "exporting"}, "Task is still active")
    observations, samples, mappings, events = [], {}, [], []
    audited = set()
    for event in task.get("history", []):
        if event.get("kind") == "sample_observed":
            mapping = json.loads(event["detail"])
            integer(mapping.get("requested_time_ms"), "requested seek")
            integer(mapping.get("decoded_time_ms"), "decoded timestamp")
            mappings.append({key: mapping[key] for key in (
                "sample_id", "source_id", "requested_time_ms", "decoded_time_ms")})
            audited.add((mapping.get("sample_id"), mapping.get("source_id"), mapping.get("decoded_time_ms")))
        elif event.get("kind") in LIFECYCLE:
            detail = public_error(event.get("detail"))
            detail["original_text_present"] = detail.pop("has_error")
            events.append({"kind": event["kind"], "at": event.get("at"),
                           "detail": detail})
    require(not (workspace / "frames").is_symlink(), "Foreign frames directory")
    for sample in task.get("samples", []):
        time = integer(sample.get("time_ms"), "decoded timestamp")
        sample_id = sample.get("id")
        require(sample_id == f"{source_id}-{time}" and sample_id not in samples
                and sample.get("source_id") == source_id and time < asset["duration_ms"], "Invalid or duplicate sample")
        require((sample_id, source_id, time) in audited, "Sample lacks actual decoded PTS audit")
        image = workspace / "frames" / f"{sample_id}.jpg"
        require(sample.get("image_path") == str(image), "Sample path is not task-owned")
        file_hash(image, sha(sample.get("sha256")))
        item = {"sample_id": sample_id, "source_id": source_id, "prepared_time_ms": time,
                "original_time_ms": clip["start_ms"] + time, "image_sha256": sample["sha256"]}
        samples[sample_id] = item
        observations.append(item)
    calls, initial, first_validated, call_ids = [], None, None, set()
    raw_calls = task.get("model_calls", [])
    require(raw_calls and usage.get("model_calls") == len(raw_calls), "No model calls or inconsistent attempt counter")
    for call in raw_calls:
        call_id = text(call.get("call_id"), "call ID", 128)
        require(call_id not in call_ids, "Duplicate model-call ID")
        call_ids.add(call_id)
        require(call.get("backend") == "openai_responses", "Simulated or mixed backend is not a production run record")
        require(call.get("status") in STATUSES, "Unknown model-call status")
        sent = call.get("sample_ids")
        require(isinstance(sent, list) and sent and len(set(sent)) == len(sent)
                and all(s in samples for s in sent), "Call cites unknown submitted samples")
        trace = call.get("provider_response_id")
        if trace is not None:
            text(trace, "response ID", 256)
        require(not trace or call["status"] not in UNKNOWN_USAGE | {"not_sent"},
                "Reserved or not-sent usage cannot become provider-reported through a trace field")
        item = {key: call.get(key) for key in (
            "call_id", "backend", "requested_model", "prompt_version", "sample_ids", "provider_response_id",
            "request_id", "status", "error")}
        text(item["prompt_version"], "prompt version", 256)
        item["error"] = public_error(item["error"])
        item["reported_model"] = text(call.get("model"), "reported model", 128) if trace else None
        item["decision"] = decision_copy(call.get("decision"))
        item["usage_kind"] = "provider_reported" if trace else ("not_sent" if call["status"] == "not_sent" else "reserved_unknown")
        for key in ("input_tokens", "output_tokens"):
            item[f"accounted_{key}"] = integer(call.get(key), key)
            item[f"provider_reported_{key}"] = call[key] if trace else None
        item["local_elapsed_ms"] = integer(call.get("elapsed_ms"), "call elapsed")
        cost = call.get("estimated_cost_usd")
        require(type(cost) in (int, float) and math.isfinite(cost) and cost >= 0, "Invalid estimated cost")
        item["accounted_estimated_cost_usd"] = cost
        item["reported_usage_estimated_cost_usd"] = cost if trace else None
        item["provider_reported_cost_usd"] = None  # Responses reports tokens, not an invoice.
        require(call["status"] != "not_sent" or (call["input_tokens"] == call["output_tokens"] == cost == 0),
                "Not-sent requests must have refunded token/cost reservations")
        if initial is None and item["decision"] and item["decision"]["tool"] == "propose_plan":
            initial = {"call_id": item["call_id"], "decision": item["decision"],
                       "status": item["status"], "accepted": item["status"] == "validated",
                       "original_plan_revision": None}
        if item["status"] == "validated":
            require(trace and item["decision"] and item["decision"]["tool"] == "propose_plan", "Validated proposal lacks production response trace")
            for candidate in item["decision"]["clips"]:
                explanation(candidate["reason"])
                explanation(candidate["uncertainty"])
                start, end = integer(candidate["start_ms"], "clip start"), integer(candidate["end_ms"], "clip end")
                evidence = candidate["evidence_ids"]
                require(candidate["source_id"] == source_id and start < end <= asset["duration_ms"]
                        and evidence and all(s in sent and start <= samples[s]["prepared_time_ms"] < end for s in evidence),
                        "Validated proposal evidence is not supported by submitted frames")
            if first_validated is None:
                first_validated = {"call_id": item["call_id"], "decision": item["decision"],
                                   "original_plan_revision": None}
        calls.append(item)
    require(any(c["provider_response_id"] for c in calls), "No recorded production response; real API execution remains unestablished")
    require(task.get("cloud_consent") is True, "Run record has no cloud consent")
    for key in ("frames_created", "input_tokens", "output_tokens"):
        integer(usage.get(key), key)
    require(usage["frames_created"] >= len(samples), "Frame credit counter is below unique samples")
    total_cost = usage.get("estimated_cost_usd")
    require(type(total_cost) in (int, float) and math.isfinite(total_cost) and total_cost >= 0,
            "Invalid total estimated cost")
    require(usage["input_tokens"] == sum(c["accounted_input_tokens"] for c in calls)
            and usage["output_tokens"] == sum(c["accounted_output_tokens"] for c in calls)
            and math.isclose(total_cost, sum(c["accounted_estimated_cost_usd"] for c in calls), abs_tol=1e-8),
            "Aggregate token/cost accounting differs from model-call records")
    baseline, _ = baseline_record(baseline_path, frozen_task, clip, input_hash, prepared_duration)
    matches = bool(initial and initial["accepted"] and initial_matches(task, initial["decision"]))
    artifact = None
    export = task.get("export")
    if export and export.get("verified") is True and task.get("status") == "completed":
        approval, plan = task.get("approval"), task.get("plan")
        require(approval and plan and approval.get("plan_digest") == export.get("plan_digest")
                and approval.get("revision") == plan.get("revision") and export.get("completed_at"), "Export/approval binding mismatch")
        digest = sha(export.get("plan_digest"))
        require(plan_digest(plan, assets) == digest, "Current export plan digest does not match the full current plan")
        require(not (workspace / "exports").is_symlink(), "Foreign exports directory")
        video = workspace / "exports" / f"{digest}.mp4"
        require(export.get("path") == str(video), "Output path is not task-owned")
        file_hash(video, sha(export.get("sha256")))
        artifact = {"kind": "unmodified_model_selection" if matches else "human_edited_or_other_plan",
                    "sha256": export["sha256"], "duration_ms": export["duration_ms"],
                    "persisted_plan_digest": digest, "persisted_revision": plan["revision"],
                    "plan_digest_recomputed": True, "decode_verification": "task_claim_only_not_reexecuted"}
    result = {
        "schema_version": 1, "kind": "local_video_agent_evidence", "attestation": "runtime_record_unattested",
        "publication_ready": False, "requires_human_free_text_review": True,
        "suite_id": materials.SUITE["suite_id"], "task_id": task_id, "split": frozen_task["split"],
        "intent": frozen_task["intent"], "code_commit": code_commit, "code_commit_source": "operator_declared",
        "raw_task_sha256": hashlib.sha256(task_bytes).hexdigest(), "raw_ledger_sha256": hashlib.sha256(ledger_bytes).hexdigest(),
        "prepared_record_sha256": hashlib.sha256(prepared_bytes).hexdigest(),
        "input": {"clip_id": clip["id"], "prepared_sha256": input_hash, "prepared_duration_ms": prepared_duration,
                  "source_video_duration_ms": video_duration,
                  "original_source_sha256": materials.DATASET["source"]["member_sha256"]},
        "task_status": task["status"], "task_error": public_error(task.get("last_error")),
        "observations": observations, "observation_mappings": mappings, "lifecycle_events": events,
        "model_calls": calls, "initial_model_proposal": initial, "first_validated_proposal": first_validated,
        "current_export": artifact, "baseline": baseline,
        "usage": {"accounted_frame_credits": usage["frames_created"], "unique_sample_count": len(samples),
                  "local_model_attempts": len(calls), "effective_accounted_elapsed_ms": settled,
                  "conservative_elapsed_ms_included": conservative,
                  "accounted_input_tokens": usage["input_tokens"], "accounted_output_tokens": usage["output_tokens"],
                  "accounted_estimated_cost_usd": usage["estimated_cost_usd"],
                  "cost_estimation_rates": None, "provider_invoiced_cost_usd": None},
        "human_review": {"status": "pending_human_review", "scores": None},
        "limitations": ["Local records and response identifiers can be forged; no provider signature is verified.",
                        "Hash checks do not establish decoding or semantic quality.",
                        "Original proposal revision and historical price rates are not persisted; they remain unknown."]}
    public_text_check(result)
    require(load(task_path)[1] == task_bytes and load(ledger_path)[1] == ledger_bytes,
            "Task or ledger changed during capture; retry on an inactive task")
    output.mkdir(parents=False, exist_ok=False, mode=0o700)
    write_new(output / "run.json", encoded(result))
    return result


def copy_new(source: Path, destination: Path, expected: str) -> None:
    with regular(source, MAX_MEDIA) as incoming, destination.open("xb") as outgoing:
        digest = hashlib.sha256()
        for block in iter(lambda: incoming.read(1024 * 1024), b""):
            digest.update(block)
            outgoing.write(block)
    require(digest.hexdigest() == expected, "Candidate changed while copied; incomplete pair must not be used")


def pair(run_path: Path, initial_video: Path, baseline_path: Path, output: Path,
         allow_heldout: bool = False) -> dict:
    # First read only the sanitized JSON, never heldout images/media before guard.
    run, raw = load(run_path)
    task, clip = frozen(run.get("task_id"), allow_heldout)
    require(run.get("kind") == "local_video_agent_evidence" and run.get("attestation") == "runtime_record_unattested",
            "Expected a local evidence capture")
    calls = run.get("model_calls", [])
    require(calls and all(c.get("backend") == "openai_responses" for c in calls)
            and any(c.get("provider_response_id") for c in calls),
            "Pair requires captured production-shaped response records, not zero-model or simulated data")
    artifact = run.get("current_export")
    require(artifact and artifact.get("kind") == "unmodified_model_selection"
            and artifact.get("plan_digest_recomputed") is True
            and isinstance(run.get("initial_model_proposal"), dict)
            and run["initial_model_proposal"].get("accepted") is True,
            "No captured unmodified initial candidate export; no A/B media pair can be created")
    file_hash(initial_video, sha(artifact.get("sha256")))
    baseline, _ = baseline_record(baseline_path, task, clip, run["input"]["prepared_sha256"],
                                  integer(run["input"].get("prepared_duration_ms"), "prepared duration"))
    require(baseline == run.get("baseline"), "Baseline changed since capture")
    candidates = [("initial_model_selection", initial_video, artifact["sha256"]),
                  ("uniform_baseline", baseline_path.parent / f"{task['id']}-uniform.mp4", baseline["sha256"])]
    if secrets.randbelow(2):
        candidates.reverse()
    mapping = {"schema_version": 1, "salt": secrets.token_hex(32),
               "run_record_sha256": hashlib.sha256(raw).hexdigest(),
               "assignment": {label: {"role": item[0], "sha256": item[2]} for label, item in zip(("A", "B"), candidates)}}
    mapping_bytes = encoded(mapping)
    review = {"schema_version": 1, "status": "pending_human_review", "task_id": task["id"],
              "intent": task["intent"], "mapping_commitment_sha256": hashlib.sha256(mapping_bytes).hexdigest(),
              "candidate_order": ["A", "B"],
              "candidate_sha256": {label: item[2] for label, item in zip(("A", "B"), candidates)},
              "reviewer": None, "reviewed_at": None, "prior_task_or_footage_exposure": None,
              "A_relevance_0_to_4": None, "B_relevance_0_to_4": None,
              "A_irrelevant_seconds": None, "B_irrelevant_seconds": None,
              "A_omissions": None, "B_omissions": None, "preferred_candidate": None, "notes": None,
              "instruction": "Watch both candidates before seeing any selection explanations. Complete relevance and timestamped omissions first; rationale review happens separately after these answers are retained.",
              "blinding_limit": "Role labels are removed, but original file properties may still distinguish candidates.",
              "attribution": materials.DATASET["source"]["attribution"],
              "license_url": materials.DATASET["source"]["license_url"],
              "changes": "Excerpts trimmed, reordered and re-encoded; review copies preserve their exact bytes."}
    public_text_check(review)
    output.mkdir(parents=False, exist_ok=False, mode=0o700)
    (output / "private").mkdir(mode=0o700)
    (output / "review").mkdir(mode=0o700)
    for label, item in zip(("A", "B"), candidates):
        copy_new(item[1], output / "review" / f"candidate-{label}.mp4", item[2])
    write_new(output / "private" / "mapping.json", mapping_bytes)
    write_new(output / "review" / "review.json", encoded(review))
    return review


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    cap = commands.add_parser("capture", help="Capture sanitized, unattested local runtime evidence")
    for name in ("workspace", "prepared-record", "baseline-record", "output-dir"):
        cap.add_argument(f"--{name}", required=True, type=Path)
    cap.add_argument("--task", required=True)
    cap.add_argument("--code-commit", required=True)
    cap.add_argument("--allow-heldout", action="store_true")
    compare = commands.add_parser("pair", help="Copy two existing candidates and create a blank A/B review form")
    for name in ("run-record", "initial-video", "baseline-record", "output-dir"):
        compare.add_argument(f"--{name}", required=True, type=Path)
    compare.add_argument("--allow-heldout", action="store_true")
    args = parser.parse_args()
    try:
        if args.command == "capture":
            result = capture(args.task, args.workspace, args.prepared_record, args.baseline_record,
                             args.code_commit, args.output_dir, args.allow_heldout)
        else:
            result = pair(args.run_record, args.initial_video, args.baseline_record, args.output_dir,
                          args.allow_heldout)
        print(json.dumps({"status": result.get("status", "captured_unattested"), "semantic_scores_generated": False}))
        return 0
    except (ValueError, KeyError, TypeError, OSError, StopIteration) as error:
        # OSError strings can contain private absolute paths. No raw JSON/config
        # or provider response is echoed on failure.
        message = str(error) if isinstance(error, ValueError) else type(error).__name__
        print(f"ERROR: {message}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
