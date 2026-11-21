"""Offline packaging of existing public bytes; no model/media execution or scores."""
import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import final_review


@unittest.skipUnless(os.name == "posix", "Secure directory-relative packager is POSIX-only")
class FinalReviewTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="final-review-test-")
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name).resolve()
        self.root = self.base / "source"
        for name in final_review.SOURCES:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes((final_review.ROOT / name).read_bytes())
        self.status = self.root / "evals/video-agent/results/status.json"
        self.status.write_bytes(b"PRIVATE SYNTHETIC STATUS MUST NOT BE OPENED OR CHANGED\n")
        self.output = self.base / "new-bundle"

    def create(self):
        return final_review.create(self.output, self.root)

    def test_real_public_bytes_are_bound_and_only_four_review_files_are_copied(self):
        before = {name: (self.root / name).read_bytes() for name in final_review.SOURCES}
        status = self.status.read_bytes()
        record = self.create()
        self.assertEqual({path.name for path in self.output.iterdir()},
                         {"final-output.mp4", "ATTRIBUTION.md", "README.md", "review.json"})
        self.assertEqual((self.output / "final-output.mp4").read_bytes(), before[final_review.VIDEO])
        self.assertEqual((self.output / "ATTRIBUTION.md").read_bytes(), before[final_review.ATTRIBUTION])
        for name, (_, digest) in final_review.SOURCES.items():
            self.assertEqual(hashlib.sha256(before[name]).hexdigest(), digest)
            self.assertEqual((self.root / name).read_bytes(), before[name])
        self.assertEqual(self.status.read_bytes(), status)
        self.assertEqual(record, json.loads((self.output / "review.json").read_bytes()))
        self.assertNotIn(str(self.base), json.dumps(record))

    def test_all_human_answers_are_null_and_scope_cannot_be_initial_blind_or_heldout(self):
        record = self.create()
        self.assertEqual(record["kind"], "human_edited_final_output_review")
        self.assertEqual(record["status"], "pending_human_review")
        self.assertTrue(all(value is None for value in record["human_review"].values()))
        self.assertTrue(all(value is False for value in record["scope"].values()))
        for field in ("source_omissions", "reason_faithfulness_0_to_4", "baseline_preference"):
            self.assertIsNone(record["human_review"][field])
            self.assertIn(field, record["unsupported_assessments"])
        instructions = (self.output / "README.md").read_text()
        for phrase in ("NOT 初稿", "NOT A/B", "NOT blind", "NOT heldout", "null"):
            self.assertIn(phrase, instructions)

    def test_frozen_intent_and_public_edit_recipe_are_retained_without_reexecution_claim(self):
        record = self.create()
        suite = json.loads((self.root / final_review.TASKS).read_bytes())
        self.assertEqual(record["intent"], suite["tasks"][0]["intent"])
        recipe = record["recorded_recipe_not_reexecuted"]
        self.assertEqual(recipe["prepared_source_ranges_ms"],
                         [[13125, 18125], [38125, 43125], [50625, 55625], [58167, 63167]])
        self.assertEqual(recipe["original_source_ranges_ms"],
                         [[43125, 48125], [68125, 73125], [80625, 85625], [88167, 93167]])
        self.assertEqual(recipe["persisted_duration_ms"], 20000)
        self.assertIn("no fresh probe, decode, playback", record["evidence_limit"])

    def test_each_missing_or_changed_source_fails_before_output_creation(self):
        for name in final_review.SOURCES:
            path = self.root / name
            original = path.read_bytes()
            with self.subTest(name=name, mode="missing"):
                path.unlink()
                with self.assertRaises((ValueError, OSError)):
                    self.create()
                self.assertFalse(self.output.exists())
            with self.subTest(name=name, mode="same-size tamper"):
                path.write_bytes(bytes([original[0] ^ 1]) + original[1:])
                with self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
                    self.create()
                self.assertFalse(self.output.exists())
            path.write_bytes(original)

    def test_source_file_and_parent_symlinks_are_refused(self):
        path = self.root / final_review.VIDEO
        path.unlink()
        path.symlink_to(final_review.ROOT / final_review.VIDEO)
        with self.assertRaises(OSError):
            self.create()
        self.assertFalse(self.output.exists())
        path.unlink()
        path.write_bytes((final_review.ROOT / final_review.VIDEO).read_bytes())
        parent = path.parent
        moved = self.root / "moved-media"
        parent.rename(moved)
        parent.symlink_to(moved, target_is_directory=True)
        with self.assertRaises(OSError):
            self.create()
        self.assertFalse(self.output.exists())

    def test_existing_outputs_and_dangling_links_are_never_replaced(self):
        for mode in ("file", "directory", "link"):
            with self.subTest(mode=mode):
                if mode == "file":
                    self.output.write_bytes(b"keep")
                elif mode == "directory":
                    self.output.mkdir()
                    (self.output / "keep").write_bytes(b"keep")
                else:
                    self.output.symlink_to(self.base / "absent")
                with self.assertRaises(FileExistsError):
                    self.create()
                if mode == "directory":
                    self.assertEqual((self.output / "keep").read_bytes(), b"keep")
                    (self.output / "keep").unlink()
                    self.output.rmdir()
                else:
                    self.output.unlink()

    def test_output_parent_symlink_traversal_and_missing_parent_are_refused(self):
        link = self.base / "linked-parent"
        link.symlink_to(self.base, target_is_directory=True)
        with self.assertRaises(OSError):
            final_review.create(link / "escaped", self.root)
        self.assertFalse((self.base / "escaped").exists())
        with self.assertRaisesRegex(ValueError, "traversal"):
            final_review.create(self.base / "missing/../escaped", self.root)
        with self.assertRaises(OSError):
            final_review.create(self.base / "missing/new", self.root)

    def test_relative_creation_uses_exclusive_no_follow_writes(self):
        self.output.mkdir()
        directory = final_review.directory_fd(self.output)
        try:
            (self.output / "occupied").symlink_to(self.base / "absent")
            with self.assertRaises(FileExistsError):
                final_review.write_new(directory, "occupied", b"must not write")
        finally:
            os.close(directory)
        self.assertFalse((self.base / "absent").exists())

    def test_command_errors_do_not_echo_private_output_paths(self):
        with mock.patch("sys.argv", ["final_review.py", "--output-dir", str(self.output)]), \
                mock.patch.object(final_review, "create", side_effect=OSError("/private/secret")), \
                mock.patch("sys.stderr") as error:
            self.assertEqual(final_review.main(), 1)
        self.assertNotIn("/private/secret", "".join(call.args[0] for call in error.write.call_args_list))


if __name__ == "__main__":
    unittest.main()
