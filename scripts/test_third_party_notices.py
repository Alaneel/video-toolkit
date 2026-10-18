"""Offline notice-provenance checks; not legal/compliance approval."""
import hashlib
import json
from pathlib import Path
import tomllib
import unittest

import third_party_notices as notices


class SourceHeaderNoticeTests(unittest.TestCase):
    def test_original_header_hash_provenance_and_reference_only_role(self):
        manifest = json.loads((notices.SUPPLEMENT_ROOT / "manifest.json").read_text("utf-8"))
        review = manifest["cached_source_review"]
        self.assertEqual(review["full_license_gaps_remaining"], 6)
        self.assertEqual(len(review["items"]), 6)
        self.assertTrue(all(item["cargo_lock_checksum_matches"]
                            and item["installed_sources_match_cached_archive"]
                            for item in review["items"]))
        descriptor, = review["source_header_notices"]
        self.assertEqual((descriptor["crate"], descriptor["version"]),
                         ("system-configuration-sys", "0.5.0"))
        original = (
            b"// Copyright 2017 Amagicom AB.\n"
            b"//\n"
            b"// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or\n"
            b"// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license\n"
            b"// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your\n"
            b"// option. This file may not be copied, modified, or distributed\n"
            b"// except according to those terms.\n"
        )
        stored = (notices.SUPPLEMENT_ROOT / descriptor["path"]).read_bytes()
        self.assertEqual(stored, original)
        self.assertEqual(hashlib.sha256(stored).hexdigest(), descriptor["sha256"])
        self.assertEqual(notices.original_comment_header(
            original + b"\n//! program docs must not be copied\nfn code_must_not_be_copied() {}\n"), original)
        locked = tomllib.loads((notices.ROOT / "Cargo.lock").read_text("utf-8"))
        package = next(package for package in locked["package"]
                       if package["name"] == descriptor["crate"]
                       and package["version"] == descriptor["version"])
        self.assertEqual(descriptor["package_sha256"], package["checksum"])
        source_review = next(item for item in review["items"]
                             if item["crate"] == descriptor["crate"])
        source = next(source for source in source_review["source_files"]
                      if source["path"] == descriptor["source_path"])
        self.assertEqual(descriptor["source_sha256"], source["sha256"])
        self.assertEqual((descriptor["line_start"], descriptor["line_end"]), (1, 7))
        self.assertEqual(descriptor["role"], "copyright_and_license_reference_only")
        self.assertFalse(descriptor["resolves_full_license_gap"])
        for invalid in (
            dict(descriptor, role="full_license_text"),
            dict(descriptor, resolves_full_license_gap=True),
        ):
            with self.assertRaisesRegex(notices.NoticeError, "must not masquerade"):
                notices.verify_source_notice(Path("."), package["checksum"], invalid)


if __name__ == "__main__":
    unittest.main()
