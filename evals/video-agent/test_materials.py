"""Offline checks for the evaluation harness, not tests of visual intelligence."""

import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import materials


class FrozenSuiteTests(unittest.TestCase):
    def test_frozen_tasks_and_clips_are_consistent(self):
        clips = {clip["id"]: clip for clip in materials.DATASET["clips"]}
        tasks = materials.SUITE["tasks"]
        self.assertEqual(len({task["id"] for task in tasks}), len(tasks))
        self.assertGreaterEqual(sum(task["split"] == "heldout" for task in tasks), 2)
        for task in tasks:
            clip = clips[task["clip_id"]]
            self.assertEqual(task["split"], clip["split"])
            self.assertGreater(task["target_duration_ms"], 0)
            self.assertLessEqual(task["target_duration_ms"], clip["end_ms"] - clip["start_ms"])

    def test_source_intervals_do_not_leak_between_splits(self):
        clips = materials.DATASET["clips"]
        for left in clips:
            for right in clips:
                if left["split"] != right["split"]:
                    self.assertFalse(left["start_ms"] < right["end_ms"] and right["start_ms"] < left["end_ms"])

    def test_real_run_status_has_records_and_no_prefilled_semantic_success(self):
        status = json.loads((materials.HERE / "results" / "status.json").read_text())
        tasks = {task["id"] for task in materials.SUITE["tasks"]}
        for run in status["real_model_runs"]:
            self.assertIn(run["task_id"], tasks)
            record = materials.HERE / "results" / run["record"]
            self.assertEqual(record.parent, materials.HERE / "results")
            evidence = json.loads(record.read_text())
            self.assertEqual(evidence["task_id"], run["task_id"])
            self.assertEqual(evidence["outcome"], run["outcome"])
            self.assertIsNone(evidence["semantic_scores"])
        self.assertEqual(status["human_reviews"], [])
        self.assertNotEqual(status["status"], "semantic_quality_validated")

    def test_baseline_exact_windows(self):
        self.assertEqual(materials.uniform_windows(75000, 20000), [
            {"start_ms": 6875, "end_ms": 11875},
            {"start_ms": 25625, "end_ms": 30625},
            {"start_ms": 44375, "end_ms": 49375},
            {"start_ms": 63125, "end_ms": 68125},
        ])

    def test_baseline_rounding_bounds_and_duration(self):
        for duration in (4, 7, 8, 13, 999, 75001):
            for target in (4, duration):
                windows = materials.uniform_windows(duration, target)
                self.assertEqual(sum(w["end_ms"] - w["start_ms"] for w in windows), target)
                for index, window in enumerate(windows):
                    self.assertGreaterEqual(window["start_ms"], duration * index // 4)
                    self.assertLessEqual(window["end_ms"], duration * (index + 1) // 4)

    def test_invalid_baseline_is_rejected(self):
        for args in ((0, 20), (10, 0), (10, 11), (10, 2), (10, 5, 0)):
            with self.assertRaises(ValueError):
                materials.uniform_windows(*args)

    def test_fixed_fingerprint_rejects_wrong_file_and_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "source"
            path.write_bytes(b"fixture only")
            expected = hashlib.sha256(b"fixture only").hexdigest()
            self.assertEqual(materials.checked(path, expected, 12), path)
            with self.assertRaises(ValueError):
                materials.checked(path, "0" * 64)
            with self.assertRaises(ValueError):
                materials.checked(path, expected, 13)
            link = Path(directory) / "symlink"
            link.symlink_to(path)
            with self.assertRaises(ValueError):
                materials.checked(link, expected)

    def test_json_evidence_and_final_publish_never_overwrite(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            record = root / "record.json"
            materials.write_new_json(record, {"status": "pending"})
            with self.assertRaises(FileExistsError):
                materials.write_new_json(record, {"status": "invented_success"})
            self.assertEqual(json.loads(record.read_text())["status"], "pending")
            partial = root / "partial"
            partial.write_bytes(b"new")
            existing = root / "final"
            existing.write_bytes(b"old")
            with self.assertRaises(FileExistsError):
                materials.publish_new(partial, existing)
            self.assertEqual(existing.read_bytes(), b"old")
            self.assertTrue(partial.exists())

    def test_heldout_baseline_needs_explicit_flag(self):
        with self.assertRaisesRegex(ValueError, "Heldout export requires"):
            materials.baseline("heldout-small-animals", Path("missing"), Path("missing"))


if __name__ == "__main__":
    unittest.main()
