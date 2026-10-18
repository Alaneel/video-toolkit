"""Offline consistency checks for published development evidence, not new media/model runs."""

import hashlib
import json
from pathlib import Path
import unittest


HERE = Path(__file__).resolve().parent
REPOSITORY = HERE.parents[1]
RUNTIME_COMMIT = "42f1909a87016c9ceb1c8e530dd25dcb131b4fda"
PLAN_DIGEST = "fb35f66c7e3766de45db03194c1fc8133fa099a445dc287b6fc4ff775bb641b0"
OUTPUT_SHA256 = "4543e1b941fa964fe8487cad30e1ace70ae45e847c88d7537ed32bebb5201417"
SOURCE_SHA256 = "6e084b652e9c43713e6501ea8abbe0cfcf19321429da86c61c76f75fa4965120"
FINAL_CAPTURE_SHA256 = "770ec911ca2d14c3cff5853ff42fca042163e490e98c5a1d8432bb8fbb6f9faf"


class PublishedRealRunSummaryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.record = json.loads((HERE / "results/dev-rabbit-woodland-20261018-luna.json").read_text(encoding="utf-8"))
        cls.status_record = json.loads((HERE / "results/status.json").read_text(encoding="utf-8"))
        cls.evidence = json.loads((REPOSITORY / "docs/video-agent-evidence.json").read_text(encoding="utf-8"))

    def test_first_rejected_proposal_and_failure_remain_preserved(self):
        initial = self.record["initial_typed_proposal"]
        self.assertEqual(initial["call_id"], "call-3")
        self.assertEqual(initial["status"], "rejected")
        call = self.record["model_calls"][2]
        self.assertEqual(call["status"], "rejected")
        self.assertEqual(initial["decision"], call["decision"])
        last_clip = initial["decision"]["clips"][-1]
        self.assertEqual(last_clip["start_ms"], 59500)
        self.assertIn("source-1-59375", last_clip["evidence_ids"])
        self.assertGreaterEqual(len(self.record["retained_failures"]), 3)

    def test_original_validated_five_clip_draft_is_not_replaced_by_export(self):
        draft = self.record["first_validated_proposal"]
        self.assertEqual(draft["call_id"], "call-4")
        self.assertEqual(draft["decision"], self.record["model_calls"][3]["decision"])
        clips = draft["decision"]["clips"]
        self.assertEqual(len(clips), 5)
        self.assertEqual(sum(clip["end_ms"] - clip["start_ms"] for clip in clips), 25000)
        self.assertEqual(draft["duration_ms"], 25000)
        self.assertEqual([clips[0]["start_ms"], clips[0]["end_ms"]], [625, 5625])
        self.assertEqual(self.record["verified_export"]["kind"], "human_edited_or_other_plan")

    def test_user_authorized_edit_only_removes_first_clip(self):
        edit = self.record["author_edit"]
        self.assertEqual(edit["authorization"], "direct_user_confirmation")
        self.assertEqual(edit["operator"], "development_agent_using_native_desktop_ui")
        self.assertEqual(edit["action"], "reject_first_clip_of_call_4")
        self.assertEqual(edit["removed_interval_ms"], [625, 5625])
        self.assertEqual(edit["final_clips"], self.record["first_validated_proposal"]["decision"]["clips"][1:])
        ranges = [[clip["start_ms"], clip["end_ms"]] for clip in edit["final_clips"]]
        self.assertEqual(ranges, [[13125, 18125], [38125, 43125], [50625, 55625], [58167, 63167]])
        self.assertEqual(sum(end - start for start, end in ranges), 20000)
        self.assertEqual(edit["duration_ms"], 20000)
        self.assertEqual(self.evidence["real_model"]["author_edit"]["final_source_ranges_ms"], ranges)

    def test_revision_approval_and_export_digests_agree(self):
        edit, export = self.record["author_edit"], self.record["verified_export"]
        self.assertEqual(edit["saved_revision"], 2)
        self.assertEqual(edit["approved_revision"], 2)
        self.assertEqual(export["persisted_revision"], 2)
        self.assertEqual(edit["approved_plan_digest"], PLAN_DIGEST)
        self.assertEqual(export["persisted_plan_digest"], PLAN_DIGEST)
        self.assertIs(export["plan_digest_recomputed"], True)
        public_export = self.evidence["real_model"]["verified_export"]
        self.assertEqual(public_export["persisted_plan_digest"], PLAN_DIGEST)
        self.assertEqual(self.evidence["real_model"]["author_edit"]["approved_plan_digest"], PLAN_DIGEST)

    def test_public_mp4_bytes_match_the_recorded_export_hash(self):
        export = self.record["verified_export"]
        asset = REPOSITORY / export["asset"]
        self.assertEqual(asset.parent, REPOSITORY / "assets/video-agent")
        self.assertEqual(asset.suffix, ".mp4")
        self.assertEqual(hashlib.sha256(asset.read_bytes()).hexdigest(), OUTPUT_SHA256)
        self.assertEqual(export["sha256"], OUTPUT_SHA256)
        self.assertEqual(export["probe"]["size_bytes"], asset.stat().st_size)
        self.assertEqual(export["task_owned_mp4_count"], 1)
        self.assertEqual(self.evidence["real_model"]["verified_export"]["sha256"], OUTPUT_SHA256)
        self.assertIs(export["source_sha256_unchanged"], True)
        self.assertEqual(export["source_sha256"], SOURCE_SHA256)
        self.assertEqual(self.record["input"]["prepared_sha256"], SOURCE_SHA256)

    def test_probe_keeps_actual_frames_and_audio_padding_separate(self):
        export = self.record["verified_export"]
        probe = export["probe"]
        self.assertEqual(export["persisted_duration_ms"], 20000)
        self.assertEqual(probe["video_duration_seconds"], 19.999674)
        self.assertEqual(probe["container_duration_seconds"], 20.021333)
        self.assertEqual(probe["decoded_video_frames"], 599)
        self.assertEqual(probe["nominal_frame_rate"], "30/1")
        self.assertEqual(probe["average_frame_rate"], "1840128/61439")
        self.assertEqual((probe["video_codec"], probe["width"], probe["height"]), ("h264", 640, 360))
        self.assertEqual((probe["audio_codec"], probe["audio_sample_rate_hz"], probe["audio_channels"]), ("aac", 48000, 2))
        self.assertEqual(probe["audio_duration_seconds"], 20.021333)
        self.assertEqual(export["capture_decode_verification"], "task_claim_only_not_reexecuted")
        self.assertEqual(export["independent_full_decode"], "passed_ffmpeg_xerror_video_and_audio")

    def test_user_edit_does_not_become_semantic_or_heldout_acceptance(self):
        self.assertIsNone(self.record["semantic_scores"])
        self.assertEqual(self.record["split"], "dev")
        self.assertEqual(self.record["human_review"], "user_directed_edit_completed_formal_semantic_review_pending")
        self.assertEqual(self.status_record["human_reviews"], [])
        self.assertEqual(self.status_record["baseline_quality"], "pending_human_review")
        self.assertEqual(self.status_record["heldout_state"], "frozen_not_used_for_semantic_debugging")
        self.assertNotEqual(self.status_record["status"], "semantic_quality_validated")
        self.assertIs(self.evidence["real_model"]["end_to_end_validated"], False)
        self.assertIsNone(self.evidence["real_model"]["semantic_scores"])
        self.assertIs(self.record["verified_export"]["native_playback"]["semantic_review"], False)

    def test_export_did_not_add_model_calls_or_erase_legacy_attempts(self):
        self.assertEqual([call["call_id"] for call in self.record["model_calls"]], ["call-1", "call-2", "call-3", "call-4"])
        total = 0
        for item in self.status_record["real_model_runs"]:
            record = json.loads((HERE / "results" / item["record"]).read_text(encoding="utf-8"))
            calls = record["model_calls"] if "model_calls" in record else record["model_calls_in_this_attempt"]
            total += len(calls) if isinstance(calls, list) else calls
            self.assertEqual(record["outcome"], item["outcome"])
            self.assertIsNone(record["semantic_scores"])
        self.assertEqual(total, 7)
        self.assertEqual(self.status_record["real_model_calls_total"], 7)
        self.assertEqual(self.evidence["real_model"]["calls"], 7)
        self.assertEqual(self.evidence["real_model"]["legacy_model_calls"], 3)
        self.assertEqual(self.evidence["real_model"]["luna_calls"], 4)

    def test_capture_usage_is_frozen_and_runtime_is_not_public_release(self):
        usage = self.record["usage"]
        self.assertEqual(self.record["code_commit"], RUNTIME_COMMIT)
        self.assertEqual(self.record["verified_export"]["runtime_code_commit"], RUNTIME_COMMIT)
        self.assertEqual(self.evidence["real_model"]["runtime_code_commit"], RUNTIME_COMMIT)
        self.assertEqual(usage["effective_accounted_elapsed_ms"], 36344)
        self.assertEqual(usage["local_model_attempts"], 4)
        self.assertEqual((usage["accounted_input_tokens"], usage["accounted_output_tokens"]), (30004, 1222))
        self.assertEqual(usage["accounted_frame_credits"], 35)
        self.assertEqual(usage["unique_sample_count"], 35)
        self.assertEqual(self.record["unique_samples_submitted"], 28)
        self.assertEqual([len(call["sample_ids"]) for call in self.record["model_calls"]], [12, 24, 16, 16])
        self.assertIsNone(usage["provider_invoiced_cost_usd"])

    def test_final_capture_is_appended_without_replacing_earlier_hashes(self):
        self.assertEqual(self.record["private_capture_sha256"], {
            "initial_observations": "440c96c92319d8e57c14945720cb02845886990c0dcf8e110cc327ae366c2fba",
            "second_observations": "75b9b4b606703ea92ebe59feb614aff1f8c0e95ff60c1f3d7f65645135b0b1cb",
            "validated_proposal": "caa8bcd4b9089c11ba4af7275ce204dad754a8c10485cda2bfb47627558f9d66",
            "user_edited_verified_export": FINAL_CAPTURE_SHA256,
        })
        self.assertEqual(self.evidence["real_model"]["verified_export"]["private_capture_sha256"], FINAL_CAPTURE_SHA256)

    def test_public_record_has_no_raw_paths_traces_or_model_free_text_fields(self):
        forbidden = {"provider_response_id", "request_id", "api_key", "path", "image_path", "reason", "uncertainty", "summary"}

        def check(value):
            if isinstance(value, dict):
                self.assertFalse(forbidden.intersection(value))
                for item in value.values():
                    check(item)
            elif isinstance(value, list):
                for item in value:
                    check(item)
            elif isinstance(value, str):
                for marker in ("/private/", "/Users/", "resp_", "sk-proj-"):
                    self.assertNotIn(marker, value)

        check(self.record)

    def test_status_and_evidence_scope_match_the_sanitized_outcome(self):
        self.assertEqual(self.record["outcome"], "user_directed_edited_export_verified")
        self.assertEqual(self.evidence["real_model"]["status"], self.record["outcome"])
        self.assertIs(self.record["export_produced"], True)
        self.assertIs(self.record["workflow_smoke_verified"], True)
        self.assertIs(self.status_record["workflow_smoke_verified"], True)
        self.assertIs(self.evidence["real_model"]["workflow_smoke_verified"], True)
        self.assertEqual(len(self.status_record["release_gates_remaining"]), 2)
        self.assertEqual(self.evidence["dependency_notices"]["unresolved_original_code_license_texts"], 6)


if __name__ == "__main__":
    unittest.main()
