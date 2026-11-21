"""Offline source/claim consistency; not runtime conversion or reference-app QA."""
import hashlib
import json
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]


class FeatureInventoryTests(unittest.TestCase):
    def setUp(self):
        self.inventory = json.loads((ROOT / "docs/feature-inventory.json").read_text("utf-8"))
        self.current = self.inventory["current_repository_reconciliation"]

    def test_static_reference_and_historical_gap_snapshot_are_not_rewritten(self):
        self.assertEqual(len(self.inventory["literal_tools"]), 108)
        self.assertEqual(len(self.inventory["generated_tools"]), 75)
        self.assertEqual(len(self.inventory["source_manifest"]), 64)
        canonical = (json.dumps(self.inventory["gaps"], sort_keys=True, separators=(",", ":")) + "\n").encode()
        self.assertEqual(hashlib.sha256(canonical).hexdigest(),
                         "7e890d36f2eb9571d5de1fc955c29d29691838bbebade05bc43958c896b851fc")
        self.assertEqual(self.inventory["historical_gap_baseline"]["status"],
                         "historical_conversion_only_comparison_not_current_repository_coverage")
        self.assertIsNone(self.inventory["historical_gap_baseline"]["source_commit"])

    def test_current_claims_match_the_typed_conversion_and_workflow_registries(self):
        conversion = (ROOT / "crates/file_conversion/src/lib.rs").read_text("utf-8")
        extensions = conversion.split("pub fn extension(self)", 1)[1].split("pub fn is_image", 1)[0]
        targets = re.findall(r'Self::\w+\s*=>\s*"([a-z0-9_]+)"', extensions)
        self.assertEqual(self.current["file_conversion_targets"], targets)
        self.assertEqual(len(targets), 13)
        workflows = (ROOT / "crates/file_workflows/src/lib.rs").read_text("utf-8")
        kinds = workflows.split("pub fn as_str(self)", 1)[1].split("impl std::fmt::Display", 1)[0]
        operations = re.findall(r'Self::\w+\s*=>\s*"([a-z_]+)"', kinds)
        self.assertEqual(self.current["file_workflow_operations"], operations)
        self.assertEqual(len(operations), 15)
        self.assertEqual(self.current["shared_adapters"], ["desktop", "cli", "mcp"])
        for name in self.current["source_paths"]:
            self.assertTrue((ROOT / name).is_file(), name)

    def test_newly_implemented_families_are_distinguished_from_remaining_gaps(self):
        implemented = self.current["implemented_additions_after_historical_gaps"]
        self.assertEqual(implemented["pdf_operations"], 9)
        self.assertTrue(implemented["images_to_pdf"])
        self.assertEqual(implemented["subtitle_operations"], 4)
        self.assertEqual(len(implemented["timed_subtitle_formats"]), 8)
        self.assertEqual(len(implemented["document_input_families"]), 8)
        self.assertEqual(len(implemented["document_output_targets"]), 6)
        remaining = self.current["remaining_scope_gaps"]
        self.assertNotIn("PDF", remaining)
        self.assertNotIn("subtitle", remaining)
        self.assertNotIn("EPUB", remaining)
        self.assertIn("general JSON/YAML/XML/CSV/LOG data-file conversion", remaining)
        self.assertIn("unified images to GIF/video/DOCX", remaining)

    def test_source_reconciliation_does_not_create_runtime_or_release_acceptance(self):
        self.assertEqual(self.current["basis"], "local_uncommitted_working_tree_source_inspection")
        self.assertIsNone(self.current["source_commit"])
        self.assertIs(self.current["new_runtime_validation_performed"], False)
        self.assertIs(self.current["public_release_acceptance"], False)


if __name__ == "__main__":
    unittest.main()
