"""Frozen public smart-run evidence checks; no API, private task or media access."""

from decimal import Decimal
import hashlib
import json
from pathlib import Path
import unittest


REPOSITORY = Path(__file__).resolve().parents[2]
SUMMARY = REPOSITORY / "docs/smart-feedback-evidence.json"
SUMMARY_SHA256 = "07ff75ca84162d3de1d35702b21a3f2cce295518cb03517b593b6a238359f4e3"
BASE_PLAN = "d404dc5c3efa8f554cbb8928426243e11a673f6c3fce508cda90cf7010903850"


class SmartFeedbackSummaryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.raw = SUMMARY.read_bytes()
        cls.record = json.loads(cls.raw, parse_float=Decimal)
        cls.v1, cls.v2 = cls.record["runs"]

    def test_frozen_capture_and_schema(self):
        self.assertEqual(hashlib.sha256(self.raw).hexdigest(), SUMMARY_SHA256)
        self.assertEqual(self.record["schema"], "video-agent-smart-development-evidence/v1")
        self.assertEqual(self.record["recorded_date"], "2026-10-18")
        self.assertEqual(self.record["date_timezone"], "Asia/Singapore")
        self.assertEqual(len(self.record["runs"]), 2)

    def test_both_unsuccessful_outcomes_remain_visible(self):
        self.assertEqual(self.v1["stopped_reason"], "revision_has_no_edit_changes")
        self.assertIs(self.v1["command_ok"], True)
        self.assertEqual(self.v2["stopped_reason"], "model_response_rejected")
        self.assertIs(self.v2["command_ok"], False)
        self.assertEqual(self.v2["command_error_code"], "Invalid")
        for run in self.record["runs"]:
            self.assertEqual(run["phase"], "completed")
            self.assertIsNone(run["selected_plan"])
            self.assertIs(run["needs_human_review"], True)
            self.assertFalse(any(p["review_verdict"] == "accept" for p in run["previews"]))
            self.assertEqual(run["maximum_rounds"], 3)
            self.assertEqual(run["rendered_rounds"], len(run["previews"]))

    def test_real_calls_and_prompt_versions_do_not_publish_identifiers(self):
        for version, run in enumerate(self.record["runs"], 1):
            self.assertEqual(run["prompt_version"], f"video-agent-preview-review-v{version}")
            self.assertEqual(run["backend"], "openai_responses")
            self.assertEqual(run["requested_model"], "gpt-6-luna")
            self.assertEqual(run["reported_model"], "gpt-6-luna")
            for sequence, call in enumerate(run["model_calls"], 1):
                self.assertEqual(call["sequence"], sequence)
                self.assertIs(call["provider_response_id_present"], True)
                self.assertIs(call["request_id_present"], True)
                self.assertGreater(call["elapsed_ms"], 0)
                self.assertEqual(call["rendered_image_count"], 3)
                self.assertEqual(len(call["source_actual_pts_ms"]), 8)

    def test_exact_token_and_cost_accounting_with_shared_frame_credits(self):
        expected = [(16414, 1142, Decimal("0.00262275"), 21),
                    (12636, 512, Decimal("0.0018355"), 18)]
        for run, values in zip(self.record["runs"], expected):
            usage = run["usage_at_run_completion"]
            self.assertEqual((usage["input_tokens"], usage["output_tokens"],
                              usage["estimated_cost_usd"], usage["frame_credits"]), values)
            calls = run["model_calls"]
            self.assertEqual(usage["real_model_calls"], len(calls))
            for key in ("input_tokens", "output_tokens", "estimated_cost_usd"):
                self.assertEqual(usage[key], sum(call[key] for call in calls))
            self.assertEqual(usage["source_sample_count"], 12)
            self.assertEqual(usage["frame_credits"], 12 + 3 * run["rendered_rounds"])
            self.assertIsNone(usage["provider_invoiced_cost_usd"])
        totals = self.record["totals_for_these_runs_only"]
        self.assertEqual((totals["real_model_calls"], totals["input_tokens"], totals["output_tokens"]),
                         (5, 29050, 1654))
        self.assertEqual(totals["estimated_cost_usd"], Decimal("0.00445825"))
        for key in ("accepted_runs", "selected_plans", "adopted_plans", "approved_plans", "final_exports"):
            self.assertEqual(totals[key], 0)

    def test_v1_revision_chain_retains_duration_failures_and_no_progress(self):
        previews = self.v1["previews"]
        self.assertEqual([p["source_ranges_ms"] for p in previews],
                         [[[0, 2000]], [[3267, 5768]], [[4767, 5768]]])
        self.assertEqual([p["review_verdict"] for p in previews], ["revise"] * 3)
        for current, following in zip(previews, previews[1:]):
            self.assertEqual(current["proposed_next_ranges_ms"], following["source_ranges_ms"])
        self.assertEqual(previews[-1]["proposed_next_ranges_ms"], previews[-1]["source_ranges_ms"])
        self.assertEqual([p["plan_duration_ms"] for p in previews], [2000, 2501, 1001])
        self.assertEqual([p["observed_duration_ms"] for p in previews], [2000, 2500, 1000])
        self.assertEqual([p["objective_pass"] for p in previews], [True, False, False])
        for preview in previews[1:]:
            self.assertEqual(preview["failed_objective_checks"], ["target_duration_alignment"])

    def test_v2_rendered_candidate_is_not_an_accepted_decision(self):
        first, candidate = self.v2["previews"]
        self.assertEqual(first["proposed_next_ranges_ms"], [[4000, 6000]])
        self.assertEqual(candidate["source_ranges_ms"], first["proposed_next_ranges_ms"])
        self.assertEqual(candidate["plan_duration_ms"], 2000)
        self.assertEqual(candidate["observed_duration_ms"], 2000)
        self.assertIs(candidate["objective_pass"], True)
        self.assertIsNone(candidate["review_verdict"])
        self.assertIsNone(candidate["proposed_next_ranges_ms"])
        self.assertEqual(candidate["review_status"], "response_rejected_no_accepted_decision")
        self.assertEqual(candidate["observation_record_basis"],
                         "retained_request_journal_not_a_validated_round_review")
        self.assertEqual(self.v2["model_calls"][-1]["status"], "feedback_response_rejected")

    def test_source_anchors_and_output_pts_keep_their_distinct_meanings(self):
        known = set(self.record["fixture"]["source_sample_actual_pts_ms"])
        for run in self.record["runs"]:
            for call, preview in zip(run["model_calls"], run["previews"]):
                sent = set(call["source_actual_pts_ms"])
                self.assertTrue(sent <= known)
                start, end = preview["source_ranges_ms"][0]
                self.assertEqual(preview["plan_duration_ms"], end - start)
                self.assertEqual(preview["nominal_source_positions_ms"],
                                 [start + pts for pts in preview["actual_output_pts_ms"]])
                self.assertTrue(all(0 <= pts < preview["observed_duration_ms"]
                                    for pts in preview["actual_output_pts_ms"]))
                if preview["proposed_next_ranges_ms"] is not None:
                    for bounds, anchors in zip(preview["proposed_next_ranges_ms"],
                                               preview["proposed_source_evidence_pts_ms"]):
                        self.assertTrue(set(anchors) <= sent)
                        self.assertTrue(all(bounds[0] <= pts < bounds[1] for pts in anchors))

    def test_media_hashes_and_objective_scope_are_frozen_not_a_semantic_score(self):
        self.assertEqual([p["mp4_sha256"] for p in self.v1["previews"]], [
            "b71ae533ac210b82cc82a3b7f9be3f80275081624a8c967647542220bb67804e",
            "42c6bd5ec92f9df964ebd965649c8f8407a51951fd83a8168b6022d831a6f769",
            "73634867f695e4f52a31e01b2b810b5ce42e5e1e8b4f0cff76340a0909efdc4e",
        ])
        self.assertEqual(self.v2["previews"][0]["mp4_sha256"], self.v1["previews"][0]["mp4_sha256"])
        self.assertEqual(self.v2["previews"][1]["mp4_sha256"],
                         "e50f892a9ace70a7776def215dd66d0aff75235f058a6ad511f546fbb711f512")
        for run in self.record["runs"]:
            for preview in run["previews"]:
                self.assertEqual(preview["semantic_evaluation"], "not_evaluated")
                self.assertEqual(preview["full_decode"], "runtime_reported_pass")
                self.assertEqual(len(preview["plan_digest"]), 64)
            for digest in run["private_capture_sha256"].values():
                self.assertEqual(len(digest), 64)

    def test_saved_task_stays_manual_unapproved_and_unexported(self):
        for run in self.record["runs"]:
            task = run["final_task"]
            self.assertEqual(task["saved_revision"], 1)
            self.assertEqual(task["saved_plan_digest"], BASE_PLAN)
            self.assertEqual(task["saved_source_ranges_ms"], [[0, 2000]])
            self.assertEqual(task["origin"], "manual")
            self.assertIs(task["plan_unchanged"], True)
            for key in ("adopted", "approved", "exported", "source_selection_cloud_consent"):
                self.assertIs(task[key], False)
            self.assertEqual(task["preview_upload_consent"], "explicit_cli_flag")

    def test_uncommitted_runtime_hashes_are_not_a_fabricated_new_commit(self):
        implementation = self.record["implementation"]
        self.assertEqual(implementation["baseline_commit"], "41f866fe0afb4d0f989dc0a5311c6665014f8da6")
        self.assertIsNone(implementation["runtime_commit"])
        self.assertIs(implementation["uncommitted_working_changes"], True)
        self.assertEqual(implementation["v2_runtime_sha256"], {
            "video_toolkit_debug_binary": "f57a8cb6776945fbf6f1b904d255fff8572f148fc70fb7a8b7ff35911871e9bf",
            "video_agent_debug_binary": "dcdc7f62b78a4e45d4f79d9e226d6ab44ea9022e3b9f97a81f8be8a1f1ae43c0",
            "review_rs": "a2a05eb7f9efc95293e9d3ee287cf03f14a371699be6a315d9b4473a24d893be",
            "smart_feedback_rs": "d5b0156057d5aac859412613cddb698f068029d213a921f5997a46cc61cdf64f",
        })

    def test_native_walkthrough_does_not_claim_accepted_adoption_flow(self):
        gui = self.record["native_walkthrough"]
        self.assertEqual(gui["run_label"], self.v2["label"])
        self.assertEqual(gui["displayed_round_bound"], 3)
        self.assertEqual(gui["displayed_stop_reason"], self.v2["stopped_reason"])
        self.assertEqual(gui["evidence_basis"], "runtime_operator_report_and_matching_saved_task_metadata")
        self.assertEqual(gui["fresh_recheck_and_preview_playback"], "operator_verified_pass")
        self.assertEqual(gui["independent_vlc_playback"], "operator_observed_blue_frame_at_one_second")
        self.assertIs(gui["player_complete_exit_observed"], False)
        for key in ("new_app_opened_actual_task", "saved_status_loaded", "original_manual_plan_unchanged"):
            self.assertIs(gui[key], True)
        for key in ("preview_upload_checkbox_checked", "adopt_button_enabled",
                    "accepted_candidate_gui_adoption_exercised", "human_semantic_review", "continuous_screen_recording"):
            self.assertIs(gui[key], False)
        self.assertEqual(gui["original_manual_revision"], 1)
        self.assertEqual(gui["original_manual_ranges_ms"], [[0, 2000]])

    def test_unsigned_development_scope_excludes_benchmark_and_prior_rabbit_runs(self):
        scope = self.record["scope"]
        self.assertEqual(scope["kind"], "local_unsigned_development_evidence")
        for key in ("signed", "independently_audited", "simulated_provider_used_in_these_runs",
                    "automated_simulation_tests_are_semantic_scores", "heldout_used"):
            self.assertIs(scope[key], False)
        self.assertIsNone(scope["formal_human_semantic_review"])
        self.assertIsNone(scope["semantic_scores"])
        for key in ("synthetic_source_real_openai_calls", "prior_rabbit_runs_are_separate",
                    "preview_mp4_bytes_rehashed_by_summary_author", "media_decode_not_reexecuted_by_summary_author",
                    "runtime_operator_fresh_full_decode_pass"):
            self.assertIs(scope[key], True)

    def test_public_record_omits_private_paths_credentials_ids_and_model_free_text(self):
        forbidden = {"provider_response_id", "request_id", "api_key", "authorization", "path", "image_path",
                     "source_path", "workspace", "reason", "uncertainty", "summary", "raw_response"}

        def check(value):
            if isinstance(value, dict):
                self.assertFalse(forbidden.intersection(value))
                for item in value.values():
                    check(item)
            elif isinstance(value, list):
                for item in value:
                    check(item)
            elif isinstance(value, str):
                for marker in ("/private/", "/Users/", "resp_", "req_", "sk-"):
                    self.assertNotIn(marker, value)

        check(self.record)


if __name__ == "__main__":
    unittest.main()
