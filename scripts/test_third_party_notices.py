"""Offline notice-provenance checks; not legal/compliance approval."""
import hashlib
import copy
import io
import json
import os
from pathlib import Path
import tempfile
import tarfile
import tomllib
import unittest
from unittest import mock

import third_party_notices as notices


class SourceHeaderNoticeTests(unittest.TestCase):
    def test_original_header_hash_provenance_and_reference_only_role(self):
        manifest = json.loads((notices.SUPPLEMENT_ROOT / "manifest.json").read_text("utf-8"))
        review = manifest["cached_source_review"]
        self.assertEqual(review["full_license_gaps_remaining"], 5)
        self.assertEqual(review["historical_full_license_gaps_at_review"], 6)
        self.assertEqual(manifest["historical_cached_source_review"]["full_license_gaps_remaining"], 6)
        self.assertEqual(review["items"], manifest["historical_cached_source_review"]["items"])
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


class WorkspaceFontNoticeTests(unittest.TestCase):
    def fixture(self):
        temporary = tempfile.TemporaryDirectory(prefix="workspace-font-notice-test-")
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        for font in notices.WORKSPACE_FONTS:
            for field in ("asset", "license", "source_record"):
                path = root / font[field]
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes((notices.ROOT / font[field]).read_bytes())
        return root

    def test_workspace_font_assets_license_and_source_are_bound_to_real_bytes(self):
        font, *derived = notices.workspace_font_records(notices.ROOT)
        self.assertEqual(font["name"], "DM Sans")
        for field in ("asset", "license", "source_record"):
            self.assertEqual(hashlib.sha256(font[f"{field}_bytes"]).hexdigest(), font[f"{field}_sha256"])
        self.assertEqual(len(font["asset_bytes"]), 240164)
        self.assertIn(b"Copyright 2014 The DM Sans Project Authors", font["license_bytes"])
        self.assertIn(b"SIL OPEN FONT LICENSE Version 1.1", font["license_bytes"])
        self.assertEqual([f["asset"] for f in derived],
                         ["assets/fonts/DM-Sans-SemiBold.ttf", "assets/fonts/DM-Sans-Bold.ttf"])
        for record in derived:
            self.assertEqual(hashlib.sha256(record["asset_bytes"]).hexdigest(), record["asset_sha256"])

    def test_missing_or_changed_font_license_and_source_are_not_silently_omitted(self):
        font = notices.WORKSPACE_FONTS[0]
        for field in ("asset", "license", "source_record"):
            root = self.fixture()
            path = root / font[field]
            original = path.read_bytes()
            with self.subTest(field=field, failure="missing"):
                path.unlink()
                with self.assertRaisesRegex(notices.NoticeError, "missing"):
                    notices.workspace_font_records(root)
            with self.subTest(field=field, failure="changed"):
                path.write_bytes(original + b"unexpected change")
                with self.assertRaisesRegex(notices.NoticeError, "SHA256 changed"):
                    notices.workspace_font_records(root)

    def test_font_symlink_and_symlinked_parent_are_rejected(self):
        font = notices.WORKSPACE_FONTS[0]
        root = self.fixture()
        path = root / font["asset"]
        path.unlink()
        path.symlink_to(notices.ROOT / font["asset"])
        with self.assertRaisesRegex(notices.NoticeError, "symlinks"):
            notices.workspace_font_records(root)
        root = self.fixture()
        original = root / "assets/fonts"
        moved = root / "original-fonts"
        original.rename(moved)
        original.symlink_to(moved, target_is_directory=True)
        with self.assertRaisesRegex(notices.NoticeError, "symlinks"):
            notices.workspace_font_records(root)

    def test_generated_notice_preserves_font_license_and_current_workspace_inventory(self):
        font, *derived = notices.workspace_font_records(notices.ROOT)
        notice = (notices.ROOT / "THIRD_PARTY_NOTICES.txt").read_bytes()
        self.assertIn(b"WORKSPACE EMBEDDED FONT DM Sans", notice)
        for record in derived:
            self.assertIn(record["asset_sha256"].encode(), notice)
        self.assertIn(font["asset_sha256"].encode(), notice)
        self.assertIn(font["source_record_sha256"].encode(), notice)
        marker = f"===== BEGIN TEXT SHA256 {font['license_sha256']} =====\n".encode()
        self.assertIn(marker + font["license_bytes"] + b"\n===== END TEXT", notice)
        for package in ("file_conversion", "file_workflows"):
            self.assertIn(f"  {package} 0.1.0 (covered by separately shipped project LICENSE)".encode(), notice)
        self.assertIn(b"UNRESOLVED ITEMS (5)", notice)


class NoticeCapacityTests(unittest.TestCase):
    def test_only_notice_capacity_is_shared_across_collector_and_both_packagers(self):
        import agent_rc
        import release
        self.assertEqual(notices.MAX_NOTICE_BYTES, 8 * 1024 * 1024)
        self.assertEqual(notices.MAX_NOTICE_BYTES, release.MAX_NOTICE_BYTES)
        self.assertEqual(notices.MAX_NOTICE_BYTES, agent_rc.MAX_NOTICE_BYTES)
        self.assertEqual(notices.MAX_INPUT_BYTES, 8 * 1024 * 1024)
        self.assertEqual(agent_rc.MAX_TEXT, 2 * 1024 * 1024)
        self.assertEqual(release.MAX_LICENSE_TEXT, 2 * 1024 * 1024)
        self.assertEqual(agent_rc.MAX_SCREENSHOT, 16 * 1024 * 1024)
        self.assertEqual(agent_rc.MAX_MEMBER, 128 * 1024 * 1024)
        self.assertEqual(agent_rc.MAX_TOTAL, 512 * 1024 * 1024)

    @unittest.skipUnless(os.name == "posix", "Actual macOS-target source proof uses POSIX no-follow access")
    def test_complete_large_original_text_is_preserved_and_total_over_eight_mib_rejected(self):
        read_notice = notices.read_notice
        # Inert in-memory replacement for one copyright input. Actual committed
        # NOTICE, dependency sources and toolchain files are never edited.
        for size, accepted in ((2 * 1024 * 1024 + 777, True), (notices.MAX_NOTICE_BYTES - 1, False)):
            original = b"INERT FULL COPYRIGHT BYTE FIXTURE\n" + b"x" * (size - 64) + b"\nEND ORIGINAL COPYRIGHT BYTES\n"
            def controlled(path, base):
                return original if path.name == "COPYRIGHT-library.html" else read_notice(path, base)
            with self.subTest(size=size), mock.patch.object(notices, "read_notice", side_effect=controlled):
                if accepted:
                    output, summary = notices.collect("aarch64-apple-darwin")
                    self.assertGreater(len(output), 2 * 1024 * 1024)
                    self.assertLess(len(output), notices.MAX_NOTICE_BYTES)
                    digest = hashlib.sha256(original).hexdigest()
                    self.assertIn(f"===== BEGIN TEXT SHA256 {digest} =====\n".encode()
                                  + original + f"\n===== END TEXT SHA256 {digest} =====".encode(), output)
                    self.assertEqual(summary["unresolved"], 5)
                else:
                    with self.assertRaisesRegex(notices.NoticeError, "8 MiB notice limit"):
                        notices.collect("aarch64-apple-darwin")


@unittest.skipUnless(os.name == "posix", "Official source matching requires POSIX no-follow file access")
class OfficialReleaseSourceMatchTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.metadata = json.loads(notices.command([
            "cargo", "metadata", "--format-version", "1", "--locked", "--offline",
            "--filter-platform", "aarch64-apple-darwin"]))
        cls.actual_package = next(package for package in notices.dependency_closure(cls.metadata)
                                  if (package["name"], package["version"]) == notices.RELEASE_MATCH_KEY)
        base = Path(cls.actual_package["manifest_path"]).parent
        cls.archive_bytes = (base.parent.parent.parent / "cache" / base.parent.name
                             / "system-configuration-sys-0.5.0.crate").read_bytes()
        with tarfile.open(fileobj=io.BytesIO(cls.archive_bytes), mode="r:gz") as bundle:
            cls.members = [(member.name, bundle.extractfile(member).read())
                           for member in bundle.getmembers()]
        cls.manifest_bytes = (notices.SUPPLEMENT_ROOT / "manifest.json").read_bytes()
        cls.original_manifest = json.loads(cls.manifest_bytes)
        cls.original_entry = next(entry for entry in cls.original_manifest["entries"]
                                  if (entry["crate"], entry["version"]) == notices.RELEASE_MATCH_KEY)
        cls.proof_bytes = (notices.SUPPLEMENT_ROOT / notices.RELEASE_MATCH_PROOF["path"]).read_bytes()
        cls.proof = json.loads(cls.proof_bytes)
        cls.license_bytes = {item["path"]: (notices.SUPPLEMENT_ROOT / item["path"]).read_bytes()
                             for item in cls.proof["license_files"]}

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="release-source-match-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.supplement = self.root / "supplement"
        for name, data in {"manifest.json": self.manifest_bytes,
                           notices.RELEASE_MATCH_PROOF["path"]: self.proof_bytes,
                           **self.license_bytes}.items():
            path = self.supplement / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        self.base = self.root / "registry/src/test-index/system-configuration-sys-0.5.0"
        for name, data in self.members:
            relative = name.removeprefix("system-configuration-sys-0.5.0/")
            path = self.base / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        self.archive = self.root / "registry/cache/test-index/system-configuration-sys-0.5.0.crate"
        self.archive.parent.mkdir(parents=True)
        self.archive.write_bytes(self.archive_bytes)
        self.package = dict(self.actual_package, manifest_path=str(self.base / "Cargo.toml"))
        self.entry = copy.deepcopy(self.original_entry)
        self.checksum = self.proof["package_sha256"]
        self.patch = mock.patch.object(notices, "SUPPLEMENT_ROOT", self.supplement)
        self.patch.start()
        self.addCleanup(self.patch.stop)

    def verify(self):
        return notices.verify_release_source_match(self.package, self.checksum, self.entry)

    def repack(self, members):
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w:gz") as bundle:
            for name, data, kind in members:
                member = tarfile.TarInfo(name)
                member.type = kind
                member.size = len(data)
                if kind == tarfile.SYMTYPE:
                    member.linkname = "outside"
                bundle.addfile(member, io.BytesIO(data))
        return output.getvalue()

    def test_exact_locked_archive_nine_author_files_and_original_texts_pass_offline(self):
        with mock.patch.object(notices, "command", side_effect=AssertionError("No process/network probe allowed")):
            proof, licenses = self.verify()
        self.assertEqual(proof["kind"], "official_release_source_match")
        self.assertEqual(len(proof["author_files"]), 9)
        self.assertEqual(proof["pinned_official_commit"], "d464f4fee68328353d2bcee27e9422ff3cf7a1fd")
        self.assertEqual(proof["packaged_vcs_commit"], "592a485494be9d63a60e54944dd2fcc4f077ec58")
        self.assertNotEqual(proof["official_author_date"], proof["official_committer_date"])
        self.assertEqual(proof["official_commit_signature"], "unsigned")
        self.assertEqual(len(licenses), 2)
        for descriptor, data in licenses:
            self.assertEqual(hashlib.sha256(data).hexdigest(), descriptor["sha256"])
            self.assertEqual(notices.git_blob_sha(data), descriptor["git_blob_sha"])

    def test_wrong_archive_or_lock_checksum_is_rejected(self):
        for mode in ("archive", "lock"):
            with self.subTest(mode=mode):
                if mode == "archive":
                    self.archive.write_bytes(self.archive_bytes[:-1] + b"x")
                else:
                    self.archive.write_bytes(self.archive_bytes)
                    self.checksum = "0" * 64
                with self.assertRaisesRegex(notices.NoticeError, "Cargo.lock checksum"):
                    self.verify()

    def test_archive_author_set_rejects_missing_extra_duplicate_link_and_escape(self):
        original = [(name, data, tarfile.REGTYPE) for name, data in self.members]
        variants = {
            "missing": original[:-1],
            "extra": original + [("system-configuration-sys-0.5.0/src/extra.rs", b"", tarfile.REGTYPE)],
            "duplicate": original[:-1] + [original[0]],
            "symlink": [(original[0][0], b"", tarfile.SYMTYPE)] + original[1:],
            "escape": [("system-configuration-sys-0.5.0/../outside", b"", tarfile.REGTYPE)] + original[1:],
        }
        for mode, members in variants.items():
            with self.subTest(mode=mode):
                with self.assertRaises(notices.NoticeError):
                    notices.release_archive_members(self.repack(members), self.proof)

    def test_archive_source_original_manifest_and_each_hash_type_are_checked(self):
        for item in self.proof["author_files"]:
            with self.subTest(path=item["archive_path"]):
                name = "system-configuration-sys-0.5.0/" + item["archive_path"]
                modified = [(path, data + b"x" if path == name else data, tarfile.REGTYPE)
                            for path, data in self.members]
                with self.assertRaisesRegex(notices.NoticeError, "author-file fingerprint"):
                    notices.release_archive_members(self.repack(modified), self.proof)
        for field in ("sha256", "git_blob_sha"):
            proof = copy.deepcopy(self.proof)
            proof["author_files"][0][field] = "0" * len(proof["author_files"][0][field])
            with self.assertRaisesRegex(notices.NoticeError, "author-file fingerprint"):
                notices.release_archive_members(self.archive_bytes, proof)

    def test_installed_author_and_generated_manifest_bytes_are_checked(self):
        for name, original in self.members:
            relative = name.removeprefix("system-configuration-sys-0.5.0/")
            with self.subTest(path=relative):
                path = self.base / relative
                addition = b'\n[lib]\npath="elsewhere.rs"\n' if relative == "Cargo.toml" else b"x"
                path.write_bytes(original + addition)
                with self.assertRaisesRegex(notices.NoticeError, "differs from the locked archive"):
                    self.verify()
                path.write_bytes(original)

    def test_installed_file_set_and_symlinks_are_rejected(self):
        for relative in ("src/extra.rs", "alternate.rs", "alternate-build.rs"):
            path = self.base / relative
            path.write_bytes(b"fn unexpected() {}")
            with self.assertRaisesRegex(notices.NoticeError, "file set|author-file set"):
                self.verify()
            path.unlink()
        path = self.base / "src/lib.rs"
        data = path.read_bytes()
        path.unlink()
        path.symlink_to(self.base / "build.rs")
        with self.assertRaisesRegex(notices.NoticeError, "symlinks"):
            self.verify()
        path.unlink()
        path.write_bytes(data)
        source = self.base / "src"
        moved = self.base / "original-src"
        source.rename(moved)
        source.symlink_to(moved, target_is_directory=True)
        with self.assertRaises(notices.NoticeError):
            self.verify()

    def test_entry_identity_commit_url_and_fixed_proof_cannot_be_retargeted(self):
        for field, value in (("commit", "0" * 40), ("repository", "https://github.com/other/fork"),
                             ("spdx", "MIT"), ("version", "0.6.0"), ("provenance", "packaged_commit")):
            with self.subTest(field=field):
                self.entry = dict(self.original_entry, **{field: value})
                with self.assertRaises(notices.NoticeError):
                    self.verify()
        self.entry = copy.deepcopy(self.original_entry)
        self.entry["files"][0]["source_url"] = self.entry["files"][0]["source_url"].replace(
            self.proof["pinned_official_commit"], "main")
        with self.assertRaises(notices.NoticeError):
            self.verify()
        for field, value in (("pinned_official_commit", "0" * 40),
                             ("upstream_directory", "sys"), ("official_tree_sha", "0" * 40)):
            self.entry = copy.deepcopy(self.original_entry)
            changed = dict(self.proof, **{field: value})
            data = json.dumps(changed).encode()
            (self.supplement / notices.RELEASE_MATCH_PROOF["path"]).write_bytes(data)
            self.entry["source_match_proof"]["sha256"] = hashlib.sha256(data).hexdigest()
            with self.assertRaisesRegex(notices.NoticeError, "proof path or SHA256"):
                self.verify()

    def test_license_bytes_and_synchronized_descriptor_tampering_fail_closed(self):
        for descriptor in self.proof["license_files"]:
            path = self.supplement / descriptor["path"]
            original = path.read_bytes()
            path.write_bytes(original + b"changed terms")
            with self.assertRaisesRegex(notices.NoticeError, "full license text fingerprint"):
                self.verify()
            changed = copy.deepcopy(self.original_entry)
            changed["files"][0]["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
            self.entry = changed
            with self.assertRaises(notices.NoticeError):
                self.verify()
            self.entry = copy.deepcopy(self.original_entry)
            path.write_bytes(original)

    def test_package_metadata_and_posix_support_fail_closed(self):
        for field, value in (("name", "other"), ("version", "0.6.0"), ("license", "MIT"),
                             ("repository", "https://github.com/other/fork"), ("authors", ["other"])):
            package = dict(self.package, **{field: value})
            with self.assertRaisesRegex(notices.NoticeError, "package identity"):
                notices.verify_release_source_match(package, self.checksum, self.entry)
        with mock.patch.object(notices.os, "name", "nt"):
            with self.assertRaisesRegex(notices.NoticeError, "requires POSIX"):
                self.verify()

    def test_history_is_preserved_and_current_five_are_derived_from_verified_completion(self):
        manifest = copy.deepcopy(self.original_manifest)
        self.assertEqual(notices.verify_current_gap_views(manifest, {notices.RELEASE_MATCH_KEY}), 5)
        self.assertEqual(len(manifest["historical_unresolved_review"]["items"]), 6)
        self.assertEqual(manifest["historical_cached_source_review"]["full_license_gaps_remaining"], 6)
        old, = manifest["historical_exact_commit_entries"]
        self.assertEqual(old["commit"], self.proof["packaged_vcs_commit"])
        self.assertEqual(old["files"], [])
        self.assertIn("unresolved", old)
        with self.assertRaises(notices.NoticeError):
            notices.verify_current_gap_views(manifest, set())
        manifest["cached_source_review"]["full_license_gaps_remaining"] = 0
        with self.assertRaises(notices.NoticeError):
            notices.verify_current_gap_views(manifest, {notices.RELEASE_MATCH_KEY})

    def test_fetch_rebuild_retains_explicit_source_match_and_historical_records(self):
        source = {key: self.entry[key] for key in ("crate", "version", "spdx", "repository", "commit")}
        with mock.patch.object(notices, "missing_upstream_sources", return_value=[source]), \
                mock.patch.object(notices, "command", side_effect=notices.NoticeError("Unavailable exact commit")):
            notices.fetch_upstream("aarch64-apple-darwin")
        rebuilt = json.loads((self.supplement / "manifest.json").read_bytes())
        self.assertEqual(rebuilt["entries"], [self.original_entry])
        for field in ("historical_unresolved_review", "historical_cached_source_review", "historical_exact_commit_entries",
                      "unresolved_review", "cached_source_review"):
            self.assertEqual(rebuilt[field], self.original_manifest[field])

    def test_repeated_six_source_header_review_does_not_restore_six_current_gaps(self):
        for descriptor in self.original_manifest["cached_source_review"]["source_header_notices"]:
            original = notices.original_comment_header((Path(self.actual_package["manifest_path"]).parent / "src/lib.rs").read_bytes())
            path = self.supplement / descriptor["path"]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(original)
        result = notices.inspect_cached_source_headers("aarch64-apple-darwin")
        self.assertEqual(result["reviewed_crates"], 6)
        self.assertEqual(result["full_license_gaps_remaining"], 5)
        rebuilt = json.loads((self.supplement / "manifest.json").read_bytes())
        self.assertEqual(rebuilt["historical_cached_source_review"], self.original_manifest["historical_cached_source_review"])

    def test_normal_collect_is_offline_and_reports_only_five_remaining_gaps(self):
        # Use actual local sources and original supplement; output stays in memory.
        self.patch.stop()
        calls = []
        command = notices.command
        def checked(args):
            calls.append(args)
            self.assertIn(args[0], ("cargo", "rustc"))
            if args[0] == "cargo":
                self.assertIn("--offline", args)
                self.assertIn("--locked", args)
            return command(args)
        with mock.patch.object(notices, "command", side_effect=checked):
            output, summary = notices.collect("aarch64-apple-darwin")
        self.assertTrue(calls)
        self.assertEqual(summary["unresolved"], 5)
        self.assertIn(b"UNRESOLVED ITEMS (5)", output)
        self.assertIn(b"official_release_source_match", output)
        self.assertIn(b"NOT the packaged VCS commit; unsigned", output)


if __name__ == "__main__":
    unittest.main()
