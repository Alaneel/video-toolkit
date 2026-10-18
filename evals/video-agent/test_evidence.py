"""Synthetic local-record fixtures only. No API call, real JPEG/video, or human score.

Production-looking field names in these temporary fixtures test validation rules;
they do not constitute genuine provider records and never enter results/.
"""
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import evidence


def hash_bytes(data):
    return hashlib.sha256(data).hexdigest()


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.workspace = self.root / "fixture-task"
        self.workspace.mkdir()
        for name in ("frames", "exports"):
            (self.workspace / name).mkdir()
        self.frozen = evidence.materials.SUITE["tasks"][0]
        self.clip = evidence.materials.DATASET["clips"][0]
        self.prepared_path = self.root / "prepared.json"
        source = b"SYNTHETIC INERT INPUT: not a real movie"
        (self.root / "bbb-dev-001.mp4").write_bytes(source)
        self.input_hash = hash_bytes(source)
        self.prepared = {
            "clip": self.clip, "source_sha256": evidence.materials.DATASET["source"]["member_sha256"],
            "recipe_version": evidence.materials.DATASET["preparation"]["recipe_version"],
            "file": "bbb-dev-001.mp4", "sha256": self.input_hash,
            "media": {"duration_ms": 75000},
        }
        image = b"SYNTHETIC INERT JPEG fixture; no decoding is claimed"
        self.image_path = self.workspace / "frames" / "source-1-1000.jpg"
        self.image_path.write_bytes(image)
        proposal = {"tool": "propose_plan", "clips": [{
            "source_id": "source-1", "start_ms": 0, "end_ms": 2000,
            "reason": "SYNTHETIC selection fixture, not a real model judgment",
            "uncertainty": "No actual visual observation occurred in this test",
            "evidence_ids": ["source-1-1000"],
        }], "summary": "SYNTHETIC proposal fixture only"}
        self.export_path = self.workspace / "exports" / ("3" * 64 + ".mp4")
        self.export_path.write_bytes(b"SYNTHETIC INERT OUTPUT: not playable video")
        self.task = {
            "schema_version": 1, "id": self.workspace.name, "intent": self.frozen["intent"],
            "target_duration_ms": self.frozen["target_duration_ms"], "status": "completed", "last_error": None,
            "assets": [{"id": "source-1", "path": str(self.root / "bbb-dev-001.mp4"),
                        "sha256": self.input_hash, "duration_ms": 75000}],
            "samples": [{"id": "source-1-1000", "source_id": "source-1", "time_ms": 1000,
                         "image_path": str(self.image_path), "sha256": hash_bytes(image)}],
            "model_calls": [{"call_id": "call-1", "backend": "openai_responses",
                "requested_model": "SYNTHETIC-NOT-A-REAL-MODEL", "model": "SYNTHETIC-NOT-A-REAL-MODEL",
                "prompt_version": "SYNTHETIC-NOT-A-PRODUCTION-PROMPT", "sample_ids": ["source-1-1000"],
                "provider_response_id": "SYNTHETIC-NOT-A-PROVIDER-SIGNATURE", "request_id": None,
                "input_tokens": 100, "output_tokens": 20, "estimated_cost_usd": 0.001,
                "elapsed_ms": 12, "status": "validated", "decision": proposal, "error": None}],
            "usage": {"frames_created": 1, "model_calls": 1, "input_tokens": 100, "output_tokens": 20,
                      "estimated_cost_usd": 0.001, "elapsed_ms": 100, "execution_reserved_ms": 0,
                      "conservative_elapsed_ms": 0, "execution_accounting_version": 1},
            "cloud_consent": True,
            "history": [{"kind": "sample_observed", "detail": json.dumps({
                "sample_id": "source-1-1000", "source_id": "source-1", "requested_time_ms": 950,
                "decoded_time_ms": 1000})}],
            "plan": {"revision": 1, "clips": [{"id": "clip-1", **copy.deepcopy(proposal["clips"][0]),
                      "human_modified": False}], "summary": proposal["summary"], "audio_policy": "preserve_with_silence"},
            "approval": {"revision": 1, "plan_digest": "3" * 64},
            "export": {"plan_digest": "3" * 64, "path": str(self.export_path),
                       "sha256": hash_bytes(self.export_path.read_bytes()), "duration_ms": 2000,
                       "verified": True, "completed_at": "SYNTHETIC-NOT-A-REAL-RUN"},
        }
        self.ledger = {"schema_version": 1, "task_id": self.workspace.name,
                       "settled_ms": 100, "conservative_ms": 0, "reservation": None}
        self.baseline_path = self.root / "baseline.json"
        baseline_video = self.root / "dev-rabbit-woodland-uniform.mp4"
        baseline_video.write_bytes(b"SYNTHETIC INERT BASELINE: not playable video")
        windows = evidence.materials.uniform_windows(75000, 20000)
        for window in windows:
            window.update(original_start_ms=30000 + window["start_ms"], original_end_ms=30000 + window["end_ms"])
        self.baseline = {"kind": "nonsemantic_baseline", "model_used": False,
            "task_id": self.frozen["id"], "source": self.clip["id"], "input_sha256": self.input_hash,
            "baseline_id": evidence.materials.SUITE["baseline"]["id"], "clips": windows,
            "file": baseline_video.name, "sha256": hash_bytes(baseline_video.read_bytes())}
        digest = evidence.plan_digest(self.task["plan"], self.task["assets"])
        new_path = self.workspace / "exports" / f"{digest}.mp4"
        self.export_path.rename(new_path)
        self.export_path = new_path
        self.task["export"].update(path=str(new_path), plan_digest=digest)
        self.task["approval"]["plan_digest"] = digest
        self.persist()

    def persist(self):
        for path, value in ((self.workspace / "task.json", self.task),
                            (self.workspace / "execution-ledger.json", self.ledger),
                            (self.prepared_path, self.prepared), (self.baseline_path, self.baseline)):
            path.write_bytes(evidence.encoded(value))

    def capture(self, name="capture"):
        self.persist()
        return evidence.capture(self.frozen["id"], self.workspace, self.prepared_path,
                                self.baseline_path, "a" * 40, self.root / name)

    def test_capture_is_unattested_path_free_pending_and_read_only(self):
        before = (self.workspace / "task.json").read_bytes()
        result = self.capture()
        self.assertEqual(result["attestation"], "runtime_record_unattested")
        self.assertFalse(result["publication_ready"])
        self.assertTrue(result["requires_human_free_text_review"])
        self.assertEqual(result["human_review"], {"status": "pending_human_review", "scores": None})
        self.assertIsNone(result["initial_model_proposal"]["original_plan_revision"])
        self.assertIsNone(result["usage"]["cost_estimation_rates"])
        self.assertNotIn(str(self.root), json.dumps(result))
        self.assertEqual((self.workspace / "task.json").read_bytes(), before)
        self.assertEqual(result["current_export"]["decode_verification"], "task_claim_only_not_reexecuted")

    def test_zero_model_manual_and_simulated_records_are_rejected(self):
        original = copy.deepcopy(self.task)
        for mode in ("manual", "simulated", "mixed"):
            self.task = copy.deepcopy(original)
            if mode == "manual":
                self.task["model_calls"] = []
                self.task["usage"]["model_calls"] = 0
            elif mode == "simulated":
                self.task["model_calls"][0]["backend"] = "SIMULATED-TEST-ONLY"
            else:
                other = copy.deepcopy(self.task["model_calls"][0])
                other.update(call_id="call-2", backend="SIMULATED-TEST-ONLY")
                self.task["model_calls"].append(other)
                self.task["usage"]["model_calls"] = 2
            with self.subTest(mode=mode), self.assertRaises(ValueError):
                self.capture(mode)

    def test_missing_response_trace_and_consent_are_rejected(self):
        self.task["model_calls"][0]["provider_response_id"] = None
        with self.assertRaises(ValueError):
            self.capture()
        self.task["model_calls"][0]["provider_response_id"] = "SYNTHETIC-ID"
        self.task["cloud_consent"] = False
        with self.assertRaises(ValueError):
            self.capture()

    def test_frozen_intent_target_and_input_are_bound(self):
        original = copy.deepcopy(self.task)
        for key, value in (("intent", "different intent"), ("target_duration_ms", 1000)):
            self.task = copy.deepcopy(original)
            self.task[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.capture()
        self.task = copy.deepcopy(original)
        self.task["assets"][0]["sha256"] = "0" * 64
        with self.assertRaises(ValueError):
            self.capture()

    def test_preparation_and_baseline_recipe_mismatches_are_rejected(self):
        self.prepared["recipe_version"] = "changed"
        with self.assertRaises(ValueError):
            self.capture()
        self.prepared["recipe_version"] = evidence.materials.DATASET["preparation"]["recipe_version"]
        self.baseline["clips"][0]["start_ms"] += 1
        with self.assertRaises(ValueError):
            self.capture()

    def test_foreign_missing_and_pending_ledgers_are_not_reset(self):
        self.ledger["task_id"] = "foreign"
        with self.assertRaises(ValueError):
            self.capture()
        self.ledger["task_id"] = self.workspace.name
        self.ledger["reservation"] = {"reserved_ms": 500}
        with self.assertRaises(ValueError):
            self.capture()
        (self.workspace / "execution-ledger.json").unlink()
        with self.assertRaises(OSError):
            evidence.capture(self.frozen["id"], self.workspace, self.prepared_path,
                             self.baseline_path, "a" * 40, self.root / "missing")

    def test_authoritative_ledger_and_frame_credits_are_not_double_counted(self):
        self.ledger.update(settled_ms=500, conservative_ms=200)
        self.task["usage"]["frames_created"] = 3
        result = self.capture()
        self.assertEqual(result["usage"]["effective_accounted_elapsed_ms"], 500)
        self.assertEqual(result["usage"]["conservative_elapsed_ms_included"], 200)
        self.assertEqual(result["usage"]["accounted_frame_credits"], 3)
        self.assertEqual(result["usage"]["unique_sample_count"], 1)

    def test_real_decoded_timestamp_and_image_hash_are_required(self):
        self.task["history"][0]["detail"] = json.dumps({"sample_id": "source-1-1000",
            "source_id": "source-1", "requested_time_ms": 950, "decoded_time_ms": 950})
        with self.assertRaises(ValueError):
            self.capture()
        self.task["history"][0]["detail"] = json.dumps({"sample_id": "source-1-1000",
            "source_id": "source-1", "requested_time_ms": 950, "decoded_time_ms": 1000})
        self.image_path.write_bytes(b"tampered inert fixture")
        with self.assertRaises(ValueError):
            self.capture()

    def test_alias_history_does_not_inflate_observation_count(self):
        self.task["history"].append(copy.deepcopy(self.task["history"][0]))
        self.assertEqual(len(self.capture()["observations"]), 1)

    def test_unknown_and_not_sent_usage_are_distinct_and_failures_retained(self):
        for index, status in enumerate(("failed_unknown_usage", "not_sent", "response_rejected"), 2):
            call = copy.deepcopy(self.task["model_calls"][0])
            call.update(call_id=f"call-{index}", status=status, decision=None, error="Fixture failure")
            if status != "response_rejected":
                call["provider_response_id"] = None
            if status == "not_sent":
                call.update(input_tokens=0, output_tokens=0, estimated_cost_usd=0)
            self.task["model_calls"].append(call)
        self.task["usage"]["model_calls"] = 4
        self.task["usage"].update(input_tokens=300, output_tokens=60, estimated_cost_usd=0.003)
        result = self.capture()
        self.assertEqual([c["usage_kind"] for c in result["model_calls"]],
                         ["provider_reported", "reserved_unknown", "not_sent", "provider_reported"])
        self.assertEqual(len(result["model_calls"]), 4)
        self.assertIsNone(result["model_calls"][1]["reported_model"])
        self.assertIsNone(result["model_calls"][1]["provider_reported_input_tokens"])
        self.assertEqual(result["model_calls"][1]["accounted_input_tokens"], 100)
        self.assertIsNone(result["model_calls"][2]["provider_reported_output_tokens"])

    def test_private_error_is_withheld_but_failed_attempt_is_retained(self):
        self.task["model_calls"][0]["error"] = "Failed reading /Users/private/secret.mov"
        result = self.capture()
        self.assertTrue(result["model_calls"][0]["error"]["text_withheld"])
        self.assertIsNone(result["model_calls"][0]["error"]["text"])
        self.assertEqual(result["model_calls"][0]["error"]["detail_sha256"],
                         hash_bytes(self.task["model_calls"][0]["error"].encode()))
        self.assertNotIn("/Users", json.dumps(result))

    def test_private_reason_is_not_silently_rewritten_and_published(self):
        self.task["model_calls"][0]["decision"]["clips"][0]["reason"] = "From /Users/private/secret.mov"
        with self.assertRaisesRegex(ValueError, "private path"):
            self.capture()

    def test_unknown_submitted_sample_and_duplicate_call_are_rejected(self):
        self.task["model_calls"][0]["sample_ids"] = ["invented"]
        with self.assertRaises(ValueError):
            self.capture()
        self.task["model_calls"][0]["sample_ids"] = ["source-1-1000"]
        self.task["model_calls"].append(copy.deepcopy(self.task["model_calls"][0]))
        self.task["usage"]["model_calls"] = 2
        with self.assertRaises(ValueError):
            self.capture()

    def test_human_modified_current_export_cannot_be_the_initial_candidate(self):
        self.task["history"].append({"kind": "human_edit", "detail": "fixture edit"})
        result = self.capture()
        self.assertEqual(result["current_export"]["kind"], "human_edited_or_other_plan")
        with self.assertRaisesRegex(ValueError, "initial candidate"):
            evidence.pair(self.root / "capture" / "run.json", self.export_path,
                          self.baseline_path, self.root / "pair")

    def test_empty_proposal_is_captured_without_fabricating_video_or_scores(self):
        self.task["model_calls"][0]["decision"]["clips"] = []
        self.task.update(status="review", plan=None, approval=None, export=None)
        result = self.capture()
        self.assertEqual(result["initial_model_proposal"]["decision"]["clips"], [])
        self.assertIsNone(result["current_export"])
        self.assertIsNone(result["human_review"]["scores"])
        with self.assertRaises(ValueError):
            evidence.pair(self.root / "capture" / "run.json", self.export_path,
                          self.baseline_path, self.root / "pair")

    def test_pair_is_pending_blinded_and_has_private_salted_commitment(self):
        self.capture()
        review = evidence.pair(self.root / "capture" / "run.json", self.export_path,
                               self.baseline_path, self.root / "pair")
        mapping_bytes = (self.root / "pair" / "private" / "mapping.json").read_bytes()
        self.assertEqual(review["mapping_commitment_sha256"], hash_bytes(mapping_bytes))
        self.assertEqual(review["status"], "pending_human_review")
        self.assertIsNone(review["A_relevance_0_to_4"])
        self.assertIsNone(review["B_relevance_0_to_4"])
        self.assertIsNone(review["reviewer"])
        self.assertNotIn("baseline", json.dumps(review).lower())
        self.assertNotIn("agent", json.dumps(review).lower())
        mapping = json.loads(mapping_bytes)
        self.assertEqual(len(mapping["salt"]), 64)
        for label in ("A", "B"):
            data = (self.root / "pair" / "review" / f"candidate-{label}.mp4").read_bytes()
            self.assertEqual(hash_bytes(data), mapping["assignment"][label]["sha256"])

    def test_pair_rejects_wrong_media_and_capture_does_not_overwrite(self):
        self.capture()
        with self.assertRaises(FileExistsError):
            self.capture()
        self.export_path.write_bytes(b"wrong inert output")
        with self.assertRaises(ValueError):
            evidence.pair(self.root / "capture" / "run.json", self.export_path,
                          self.baseline_path, self.root / "pair")

    def test_heldout_capture_is_refused_before_opening_any_material(self):
        with mock.patch.object(evidence, "load", side_effect=AssertionError("must not read")):
            with self.assertRaisesRegex(ValueError, "Heldout"):
                evidence.capture("heldout-small-animals", Path("missing"), Path("missing"),
                                 Path("missing"), "a" * 40, Path("missing"))

    def test_https_is_allowed_and_windows_paths_are_private(self):
        evidence.public_text_check({"url": "https://creativecommons.org/licenses/by/3.0/"})
        with self.assertRaises(ValueError):
            evidence.public_text_check({"path": "C:\\private\\movie.mp4"})

    def test_first_rejected_proposal_is_not_replaced_by_successful_retry(self):
        rejected = copy.deepcopy(self.task["model_calls"][0])
        rejected.update(call_id="call-0", status="rejected", error="Fixture invalid proposal")
        self.task["model_calls"].insert(0, rejected)
        self.task["usage"].update(model_calls=2, input_tokens=200, output_tokens=40, estimated_cost_usd=0.002)
        result = self.capture()
        self.assertFalse(result["initial_model_proposal"]["accepted"])
        self.assertEqual(result["initial_model_proposal"]["call_id"], "call-0")
        self.assertEqual(result["first_validated_proposal"]["call_id"], "call-1")
        with self.assertRaises(ValueError):
            evidence.pair(self.root / "capture" / "run.json", self.export_path,
                          self.baseline_path, self.root / "pair")

    def test_aliases_lifecycle_replay_and_private_errors_are_preserved(self):
        alias = json.loads(self.task["history"][0]["detail"])
        alias["requested_time_ms"] = 999
        self.task["history"].extend([
            {"kind": "sample_observed", "detail": json.dumps(alias)},
            {"kind": "model_result_reused", "at": "fixture-time", "detail": "Fixture replay, not new API"},
            {"kind": "operation_stopped", "at": "fixture-time", "detail": "Stopped at /Users/private/input"},
        ])
        result = self.capture()
        self.assertEqual(len(result["observations"]), 1)
        self.assertEqual(len(result["observation_mappings"]), 2)
        self.assertEqual(len(result["model_calls"]), 1)
        self.assertEqual(result["lifecycle_events"][-1]["kind"], "operation_stopped")
        self.assertTrue(result["lifecycle_events"][-1]["detail"]["text_withheld"])

    def test_aggregate_accounting_mismatch_and_nonfinite_cost_are_rejected(self):
        self.task["usage"]["input_tokens"] = 101
        with self.assertRaises(ValueError):
            self.capture()
        self.task["usage"]["input_tokens"] = 100
        self.task["usage"]["estimated_cost_usd"] = -1
        with self.assertRaises(ValueError):
            self.capture()

    def test_current_plan_digest_is_recomputed_not_only_compared(self):
        self.task["plan"]["revision"] += 1
        self.task["approval"]["revision"] += 1
        with self.assertRaisesRegex(ValueError, "digest"):
            self.capture()

    def test_container_padding_duration_uses_materials_recipe_contract(self):
        self.prepared["media"]["duration_ms"] = 75008
        windows = evidence.materials.uniform_windows(75008, 20000)
        for window in windows:
            window.update(original_start_ms=30000 + window["start_ms"], original_end_ms=30000 + window["end_ms"])
        self.baseline["clips"] = windows
        self.assertEqual(self.capture()["input"]["prepared_duration_ms"], 75008)

    def test_rust_golden_digest_from_actual_manual_two_source_plan(self):
        # Recorded production Rust digest from the real manual two-source run,
        # not a vision/model-quality fixture. No private paths are embedded.
        plan = {"revision": 2, "clips": [
            {"id": "human-r1-1", "source_id": "source-2", "start_ms": 0, "end_ms": 4000,
             "reason": "MANUAL: selected by the user; not a model recommendation.",
             "uncertainty": "Human-selected interval. No claim of AI observation outside recorded samples.",
             "evidence_ids": [], "human_modified": True},
            {"id": "clip-1", "source_id": "source-1", "start_ms": 0, "end_ms": 4000,
             "reason": "MANUAL draft: first bounded interval, not semantic AI selection",
             "uncertainty": "No model has judged this interval. Review the source yourself.",
             "evidence_ids": [], "human_modified": True}],
             "summary": "MANUAL workflow draft — no AI selection or semantic-quality claim",
             "audio_policy": "preserve_with_silence"}
        assets = [{"id": "source-1", "sha256": "6e084b652e9c43713e6501ea8abbe0cfcf19321429da86c61c76f75fa4965120"},
                  {"id": "source-2", "sha256": "eeada9984e8f3ec2b218c3b4dfa93ad6534628641716b9a69d10cf2fe043b5ba"}]
        self.assertEqual(evidence.plan_digest(plan, assets),
                         "0bc22014f4f9def76308c3bd08721838aba61916d7e7fb31f7822b64eede7c3c")

    def test_runtime_explanation_uses_eight_thousand_utf8_byte_limit(self):
        candidate = self.task["model_calls"][0]["decision"]["clips"][0]
        candidate["reason"] = "x" * 7000
        candidate["uncertainty"] = "y" * 7000
        self.assertEqual(len(self.capture()["initial_model_proposal"]["decision"]["clips"][0]["reason"]), 7000)
        candidate["reason"] = "字" * 3000  # 9,000 UTF-8 bytes, not 3,000 bytes.
        with self.assertRaisesRegex(ValueError, "UTF-8"):
            self.capture("too-long")
        candidate["reason"] = "   "
        with self.assertRaises(ValueError):
            self.capture("blank")

    def test_source_paths_and_symlinks_do_not_escape_owned_image_paths(self):
        self.task["samples"][0]["image_path"] = str(self.root / "foreign.jpg")
        with self.assertRaises(ValueError):
            self.capture()
        self.task["samples"][0]["image_path"] = str(self.image_path)
        target = self.root / "other"
        self.image_path.rename(target)
        self.image_path.symlink_to(target)
        with self.assertRaises((ValueError, OSError)):
            self.capture()


if __name__ == "__main__":
    unittest.main()
