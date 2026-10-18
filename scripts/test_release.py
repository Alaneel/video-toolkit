"""Release integrity checks; no Rust compiler, native executables or FFmpeg needed."""

import contextlib
import io
import json
import os
from pathlib import Path
import shutil
import stat
import struct
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile

import release


VERSION = "2.1.0"
COMMIT = "0123456789abcdef0123456789abcdef01234567"
LINUX = "x86_64-unknown-linux-gnu"
WINDOWS = "x86_64-pc-windows-msvc"
INTEL_MAC = "x86_64-apple-darwin"
ARM_MAC = "aarch64-apple-darwin"


def executable(target):
    kind, machine, _, _ = release.target_info(target)
    if kind == "ELF":
        data = bytearray(128)
        data[:7] = b"\x7fELF\x02\x01\x01"
        struct.pack_into("<HH", data, 16, 3, machine)
    elif kind == "PE":
        data = bytearray(256)
        data[:2] = b"MZ"
        struct.pack_into("<I", data, 60, 128)
        data[128:132] = b"PE\x00\x00"
        struct.pack_into("<H", data, 132, machine)
        struct.pack_into("<H", data, 150, 0x0002)
        struct.pack_into("<H", data, 152, 0x020B)
    else:
        data = bytearray(128)
        data[:4] = b"\xcf\xfa\xed\xfe"
        struct.pack_into("<III", data, 4, machine, 0, 2)
    return bytes(data)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="release-unit-tests-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.root = self.directory / "source"
        self.root.mkdir()
        (self.root / "Cargo.toml").write_text(f'[package]\nname = "video-toolkit"\nversion = "{VERSION}"\n')
        (self.root / "README.md").write_text("Video Toolkit documentation\n", encoding="utf-8")
        (self.root / "LICENSE").write_text("MIT\n", encoding="utf-8")
        (self.root / "Cargo.lock").write_text("# Inert unit-test lock fixture; no third-party packages.\n")
        self.license_fixture(LINUX)
        self.output = self.directory / "archives"

    def license_fixture(self, target):
        """Synthetic approved inventory; never a real dependency clearance."""
        notices = ("VIDEO TOOLKIT / VIDEO AGENT — MECHANICALLY COLLECTED THIRD-PARTY NOTICES\n"
                   f"Target inventory: {target}\n\nUNRESOLVED ITEMS (0)\n"
                   "None detected by these mechanical checks; this is not proof of legal completeness.\n\n"
                   "FIRST-PARTY WORKSPACE\n  Synthetic unit fixture only.\n").encode()
        (self.root / "THIRD_PARTY_NOTICES.txt").write_bytes(notices)
        inventory = {
            "schema_version": 1, "target": target, "entries": [],
            "unresolved_review": {"items": [], "status": "Inert unit-test fixture only"},
            "cached_source_review": {"full_license_gaps_remaining": 0},
            "public_distribution_review": {
                "status": "approved", "target": target,
                "notices_sha256": release.digest(notices),
                "cargo_lock_sha256": release.digest((self.root / "Cargo.lock").read_bytes()),
                "fixture": "Synthetic test input; not a real publication approval",
            },
        }
        path = self.root / "third_party/licenses/manifest.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(inventory))
        self.license_target = target

    def change_license_inventory(self, change):
        path = self.root / "third_party/licenses/manifest.json"
        inventory = json.loads(path.read_bytes())
        change(inventory)
        path.write_text(json.dumps(inventory))

    def package(self, target=LINUX, commit=COMMIT, tag=None):
        if target != self.license_target:
            self.license_fixture(target)
        binary = self.directory / f"binary-{target}"
        binary.write_bytes(executable(target))
        return release.package(binary, target, self.output, commit, tag, root=self.root)

    def verify(self, archive, target=LINUX, commit=COMMIT, **kwargs):
        return release.verify(archive, target, VERSION, commit, **kwargs)

    def mutate_tar(self, archive, change):
        with tarfile.open(archive, "r:gz") as source:
            members = [(member, source.extractfile(member).read()) for member in source]
        change(members)
        with tarfile.open(archive, "w:gz") as output:
            for member, data in members:
                member.size = len(data)
                output.addfile(member, io.BytesIO(data))

    def mutate_zip(self, archive, change):
        with zipfile.ZipFile(archive) as source:
            members = [(member, source.read(member)) for member in source.infolist()]
        change(members)
        with zipfile.ZipFile(archive, "w") as output:
            for member, data in members:
                output.writestr(member, data)

    def alter_manifest(self, members, change):
        for index, (member, data) in enumerate(members):
            name = member.name if isinstance(member, tarfile.TarInfo) else member.filename
            if name.endswith("/RELEASE.json"):
                manifest = json.loads(data)
                change(manifest)
                members[index] = member, json.dumps(manifest).encode()
                return
        self.fail("fixture has no manifest")

    def test_all_four_platform_archives_round_trip(self):
        for target in release.TARGETS:
            with self.subTest(target=target):
                archive = self.package(target)
                manifest = self.verify(archive, target)
                self.assertEqual(manifest["target"], target)
                self.assertEqual(manifest["version"], VERSION)
                self.assertEqual(manifest["commit"], COMMIT)
                self.assertEqual(manifest["binary_sha256"], release.digest(executable(target)))

    def test_unix_permissions_are_inside_the_tar(self):
        archive = self.package()
        # Artifact transport may reset the archive file's own permissions; the
        # permissions inside it must still survive intact.
        archive.chmod(0o644)
        with tarfile.open(archive) as source:
            for member in source:
                expected = 0o755 if member.name.endswith("/video-toolkit") else 0o644
                self.assertEqual(member.mode, expected)
        self.verify(archive)

    def test_zip_contains_an_exe_and_exactly_the_portable_files(self):
        archive = self.package(WINDOWS)
        self.assertTrue(archive.name.endswith(".zip"))
        with zipfile.ZipFile(archive) as source:
            self.assertEqual(len(source.infolist()), 8)
            self.assertTrue(any(name.endswith("/video-toolkit.exe") for name in source.namelist()))
        self.verify(archive, WINDOWS)

    def test_wrong_tag_is_rejected_before_creating_an_archive(self):
        for tag in ("v9.0.0", VERSION, f"v{VERSION}-extra", f"v{VERSION}/oops"):
            with self.subTest(tag=tag), self.assertRaisesRegex(release.ReleaseError, "tag must exactly"):
                self.package(tag=tag)
        self.assertFalse(self.output.exists())
        self.verify(self.package(tag=f"v{VERSION}"))

    def test_existing_archive_is_not_overwritten(self):
        archive = self.package()
        original = archive.read_bytes()
        with self.assertRaisesRegex(release.ReleaseError, "refusing to overwrite"):
            self.package()
        self.assertEqual(archive.read_bytes(), original)

    def test_public_packages_contain_exact_original_notice_review_and_lock_bytes(self):
        archive = self.package()
        contents, _ = release.archive_members(archive, LINUX)
        prefix = f"video-toolkit-{VERSION}-{LINUX}/"
        self.assertEqual(contents[prefix + "THIRD_PARTY_NOTICES.txt"],
                         (self.root / "THIRD_PARTY_NOTICES.txt").read_bytes())
        self.assertEqual(contents[prefix + "LICENSE-REVIEW.json"],
                         (self.root / "third_party/licenses/manifest.json").read_bytes())
        self.assertEqual(contents[prefix + "Cargo.lock"], (self.root / "Cargo.lock").read_bytes())

    def test_recorded_license_gaps_block_before_archive_or_output_creation(self):
        self.change_license_inventory(lambda inventory: inventory["unresolved_review"]["items"].extend(
            {"crate": f"unresolved-{number}"} for number in range(6)))
        with self.assertRaisesRegex(release.ReleaseError, "6 unresolved"):
            self.package()
        self.assertFalse(self.output.exists())

    def test_notice_and_cached_gap_counts_cannot_be_hidden_by_an_empty_review_list(self):
        for source in ("notices", "cached"):
            with self.subTest(source=source):
                self.license_fixture(LINUX)
                if source == "notices":
                    path = self.root / "THIRD_PARTY_NOTICES.txt"
                    path.write_bytes(path.read_bytes().replace(b"ITEMS (0)", b"ITEMS (6)"))
                else:
                    self.change_license_inventory(lambda inventory: inventory["cached_source_review"].update(
                        full_license_gaps_remaining=6))
                with self.assertRaisesRegex(release.ReleaseError, "6 unresolved"):
                    self.package()
                self.assertFalse(self.output.exists())

    def test_zero_mechanical_gaps_do_not_replace_explicit_publication_approval(self):
        self.change_license_inventory(lambda inventory: inventory.pop("public_distribution_review"))
        with self.assertRaisesRegex(release.ReleaseError, "no explicit approved review"):
            self.package()
        self.assertFalse(self.output.exists())

    def test_zero_count_cannot_conceal_unresolved_notice_summary(self):
        path = self.root / "THIRD_PARTY_NOTICES.txt"
        notices = path.read_bytes().replace(
            b"None detected by these mechanical checks; this is not proof of legal completeness.",
            b"- A license is still missing despite the forged zero count.")
        path.write_bytes(notices)
        self.change_license_inventory(lambda inventory: inventory["public_distribution_review"].update(
            notices_sha256=release.digest(notices)))
        with self.assertRaisesRegex(release.ReleaseError, "still lists unresolved"):
            self.package()
        self.assertFalse(self.output.exists())

    def test_missing_notice_or_inventory_fails_closed(self):
        for name in ("THIRD_PARTY_NOTICES.txt", "third_party/licenses/manifest.json", "Cargo.lock"):
            with self.subTest(name=name):
                path = self.root / name
                data = path.read_bytes()
                path.unlink()
                with self.assertRaisesRegex(release.ReleaseError, "regular file"):
                    self.package()
                path.write_bytes(data)
                self.assertFalse(self.output.exists())

    def test_stale_approval_cannot_follow_changed_notices_or_lock(self):
        for name in ("THIRD_PARTY_NOTICES.txt", "Cargo.lock"):
            with self.subTest(name=name):
                self.license_fixture(LINUX)
                path = self.root / name
                data = path.read_bytes()
                path.write_bytes(data + b"\nChanged fixture bytes\n")
                with self.assertRaisesRegex(release.ReleaseError, "review is stale"):
                    self.package()
                path.write_bytes(data)
                self.assertFalse(self.output.exists())

    def test_target_specific_clearance_cannot_be_reused_for_other_platforms(self):
        binary = self.directory / "arm64-binary"
        binary.write_bytes(executable(ARM_MAC))
        with self.assertRaisesRegex(release.ReleaseError, "does not cover"):
            release.package(binary, ARM_MAC, self.output, COMMIT, root=self.root)
        release.publication_check([LINUX], self.root)
        with self.assertRaisesRegex(release.ReleaseError, "does not cover"):
            release.publication_check(root=self.root)

    def test_malformed_duplicate_or_incomplete_inventory_fails_closed(self):
        path = self.root / "third_party/licenses/manifest.json"
        for data in (b"[]", b"{}", b'{"schema_version":1,"schema_version":1}'):
            with self.subTest(data=data):
                path.write_bytes(data)
                with self.assertRaises(release.ReleaseError):
                    self.package()
                self.assertFalse(self.output.exists())

    def test_real_checkout_is_not_cleared_by_synthetic_unit_fixtures(self):
        with self.assertRaisesRegex(release.ReleaseError, "6 unresolved"):
            release.publication_check([ARM_MAC])

    def test_rehashed_archive_cannot_remove_a_license_gap(self):
        archive = self.package()
        def alter(members):
            for index, (member, data) in enumerate(members):
                if member.name.endswith("/LICENSE-REVIEW.json"):
                    inventory = json.loads(data)
                    inventory["cached_source_review"]["full_license_gaps_remaining"] = 6
                    data = json.dumps(inventory).encode()
                    members[index] = member, data
                    self.alter_manifest(members, lambda manifest: manifest["files"]["LICENSE-REVIEW.json"].update(
                        sha256=release.digest(data), size=len(data)))
                    return
        self.mutate_tar(archive, alter)
        with self.assertRaisesRegex(release.ReleaseError, "6 unresolved"):
            self.verify(archive)

    def test_checksums_refuse_rehashed_uncleared_packages(self):
        for target in release.TARGETS:
            archive = self.package(target)
            if target == LINUX:
                def alter(members):
                    for index, (member, data) in enumerate(members):
                        if member.name.endswith("/LICENSE-REVIEW.json"):
                            inventory = json.loads(data)
                            inventory.pop("public_distribution_review")
                            data = json.dumps(inventory).encode()
                            members[index] = member, data
                            self.alter_manifest(members, lambda manifest: manifest["files"]["LICENSE-REVIEW.json"].update(
                                sha256=release.digest(data), size=len(data)))
                            return
                self.mutate_tar(archive, alter)
        with self.assertRaisesRegex(release.ReleaseError, "no explicit approved review"):
            release.checksums(self.output, VERSION, COMMIT)
        self.assertFalse((self.output / "SHA256SUMS").exists())

    def test_archives_are_deterministic_for_identical_inputs(self):
        for target in (LINUX, WINDOWS):
            with self.subTest(target=target):
                archive = self.package(target)
                original = archive.read_bytes()
                archive.unlink()
                self.assertEqual(self.package(target).read_bytes(), original)

    def test_package_rejects_mismatched_architecture(self):
        self.license_fixture(INTEL_MAC)
        binary = self.directory / "wrong binary"
        binary.write_bytes(executable(ARM_MAC))
        with self.assertRaisesRegex(release.ReleaseError, "does not match"):
            release.package(binary, INTEL_MAC, self.output, COMMIT, root=self.root)

    def test_manifest_identity_mismatch_is_rejected(self):
        for key, value in [("version", "3.0.0"), ("target", ARM_MAC),
                           ("commit", "f" * 40), ("binary", "other")]:
            with self.subTest(key=key):
                archive = self.package()
                self.mutate_tar(archive, lambda members: self.alter_manifest(
                    members, lambda manifest: manifest.__setitem__(key, value)))
                with self.assertRaisesRegex(release.ReleaseError, f"manifest {key}"):
                    self.verify(archive)
                archive.unlink()

    def test_payload_corruption_is_rejected(self):
        archive = self.package()
        def corrupt(members):
            for index, (member, data) in enumerate(members):
                if member.name.endswith("/video-toolkit"):
                    members[index] = member, data[:-1] + b"!"
        self.mutate_tar(archive, corrupt)
        with self.assertRaisesRegex(release.ReleaseError, "SHA256 mismatch"):
            self.verify(archive)

    def test_documentation_corruption_is_also_rejected(self):
        archive = self.package()
        def corrupt(members):
            for index, (member, data) in enumerate(members):
                if member.name.endswith("/README.md"):
                    members[index] = member, data + b"malicious instructions"
        self.mutate_tar(archive, corrupt)
        with self.assertRaisesRegex(release.ReleaseError, "size mismatch"):
            self.verify(archive)

    def test_binary_hash_cannot_disagree_with_payload_hash(self):
        archive = self.package()
        self.mutate_tar(archive, lambda members: self.alter_manifest(
            members, lambda manifest: manifest.__setitem__("binary_sha256", "0" * 64)))
        with self.assertRaisesRegex(release.ReleaseError, "binary SHA256 mismatch"):
            self.verify(archive)

    def test_rehashed_wrong_architecture_is_still_rejected(self):
        archive = self.package()
        wrong = bytearray(executable(LINUX))
        struct.pack_into("<H", wrong, 18, 183)  # AArch64 ELF, not x86_64.
        def replace(members):
            for index, (member, data) in enumerate(members):
                if member.name.endswith("/video-toolkit"):
                    members[index] = member, bytes(wrong)
            def rehash(manifest):
                manifest["binary_sha256"] = release.digest(wrong)
                manifest["files"]["video-toolkit"]["sha256"] = release.digest(wrong)
            self.alter_manifest(members, rehash)
        self.mutate_tar(archive, replace)
        with self.assertRaisesRegex(release.ReleaseError, "does not match"):
            self.verify(archive)

    def test_non_executable_unix_permissions_are_rejected(self):
        archive = self.package()
        def chmod(members):
            for member, _ in members:
                if member.name.endswith("/video-toolkit"):
                    member.mode = 0o644
        self.mutate_tar(archive, chmod)
        with self.assertRaisesRegex(release.ReleaseError, "incorrect UNIX permissions"):
            self.verify(archive)

    def test_privileged_unix_permissions_are_rejected(self):
        archive = self.package()
        self.mutate_tar(archive, lambda members: setattr(members[0][0], "mode", 0o4644))
        with self.assertRaisesRegex(release.ReleaseError, "incorrect UNIX permissions"):
            self.verify(archive)

    def test_traversal_absolute_backslash_and_drive_paths_are_rejected(self):
        for name in ("../outside", "/outside", "folder/../outside", "folder\\outside", "C:/outside"):
            with self.subTest(name=name):
                archive = self.package()
                self.mutate_tar(archive, lambda members: setattr(members[0][0], "name", name))
                with self.assertRaisesRegex(release.ReleaseError, "unsafe archive member"):
                    self.verify(archive)
                archive.unlink()

    def test_tar_symbolic_and_hard_links_are_rejected(self):
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE):
            with self.subTest(kind=kind):
                archive = self.package()
                def link(members):
                    member, _ = members[0]
                    member.type = kind
                    member.linkname = "/outside"
                self.mutate_tar(archive, link)
                with self.assertRaisesRegex(release.ReleaseError, "link or special file"):
                    self.verify(archive)
                archive.unlink()

    def test_zip_symlinks_and_traversal_are_rejected(self):
        archive = self.package(WINDOWS)
        self.mutate_zip(archive, lambda members: setattr(
            members[0][0], "external_attr", (stat.S_IFLNK | 0o777) << 16))
        with self.assertRaisesRegex(release.ReleaseError, "link or special file"):
            self.verify(archive, WINDOWS)
        archive.unlink()
        archive = self.package(WINDOWS)
        self.mutate_zip(archive, lambda members: setattr(members[0][0], "filename", "../outside"))
        with self.assertRaisesRegex(release.ReleaseError, "unsafe archive member"):
            self.verify(archive, WINDOWS)

    def test_duplicate_or_missing_members_are_rejected(self):
        archive = self.package()
        self.mutate_tar(archive, lambda members: members.append(members[0]))
        with self.assertRaisesRegex(release.ReleaseError, "duplicate archive member"):
            self.verify(archive)
        archive.unlink()
        archive = self.package()
        self.mutate_tar(archive, lambda members: members.pop())
        with self.assertRaisesRegex(release.ReleaseError, "exactly the expected files"):
            self.verify(archive)

    def test_extra_member_is_rejected(self):
        archive = self.package()
        def extra(members):
            member = tarfile.TarInfo("unexpected.txt")
            member.mode = 0o644
            members.append((member, b"unexpected"))
        self.mutate_tar(archive, extra)
        with self.assertRaisesRegex(release.ReleaseError, "exactly the expected files"):
            self.verify(archive)

    def test_renaming_archive_does_not_change_its_identity(self):
        archive = self.package()
        wrong = archive.with_name(release.archive_name(VERSION, ARM_MAC))
        archive.rename(wrong)
        with self.assertRaisesRegex(release.ReleaseError, "filename"):
            self.verify(wrong)

    def test_duplicate_json_keys_are_rejected(self):
        archive = self.package()
        def duplicate(members):
            for index, (member, data) in enumerate(members):
                if member.name.endswith("/RELEASE.json"):
                    members[index] = member, b'{"version":"2.1.0","version":"9.0.0"}'
        self.mutate_tar(archive, duplicate)
        with self.assertRaisesRegex(release.ReleaseError, "duplicate JSON key"):
            self.verify(archive)

    def test_oversized_archive_member_is_rejected_before_reading(self):
        archive = self.package()
        with mock.patch.object(release, "MAX_MEMBER_SIZE", 32):
            with self.assertRaisesRegex(release.ReleaseError, "too large"):
                release.archive_members(archive, LINUX)

    def test_all_architectures_have_distinct_headers(self):
        for target in release.TARGETS:
            release.check_architecture(executable(target), target)
            for other in release.TARGETS:
                if other != target:
                    with self.subTest(target=target, other=other), self.assertRaises(release.ReleaseError):
                        release.check_architecture(executable(target), other)

    def test_truncated_and_non_native_headers_are_rejected(self):
        for data in (b"hello", b"\x7fELF", b"MZ", b"\xcf\xfa\xed\xfe"):
            with self.subTest(data=data), self.assertRaises(release.ReleaseError):
                release.architecture(data)

    def test_32bit_elf_and_pe_or_dll_are_rejected(self):
        elf = bytearray(executable(LINUX))
        elf[4] = 1
        with self.assertRaisesRegex(release.ReleaseError, "64-bit"):
            release.architecture(elf)
        pe = bytearray(executable(WINDOWS))
        struct.pack_into("<H", pe, 152, 0x010B)
        with self.assertRaisesRegex(release.ReleaseError, "64-bit"):
            release.architecture(pe)
        pe = bytearray(executable(WINDOWS))
        struct.pack_into("<H", pe, 150, 0x2002)
        with self.assertRaisesRegex(release.ReleaseError, "DLLs"):
            release.architecture(pe)

    def test_bad_pe_offsets_are_rejected(self):
        for offset in (0, 32, 255, 0xFFFFFFFF):
            pe = bytearray(executable(WINDOWS))
            struct.pack_into("<I", pe, 60, offset)
            with self.subTest(offset=offset), self.assertRaisesRegex(release.ReleaseError, "offset"):
                release.architecture(pe)

    def test_checksums_require_all_four_archives(self):
        self.package()
        with self.assertRaisesRegex(release.ReleaseError, "four expected"):
            release.checksums(self.output, VERSION)
        self.assertFalse((self.output / "SHA256SUMS").exists())

    def test_checksums_cover_every_archive_and_can_be_regenerated(self):
        for target in release.TARGETS:
            self.package(target)
        path = release.checksums(self.output, VERSION, COMMIT)
        lines = path.read_text().splitlines()
        self.assertEqual(len(lines), 4)
        for line in lines:
            checksum, name = line.split("  ")
            self.assertEqual(checksum, release.digest((self.output / name).read_bytes()))
        self.assertEqual(release.checksums(self.output, VERSION, COMMIT).read_text(), path.read_text())

    def test_checksums_reject_mixed_commits(self):
        for target in release.TARGETS:
            self.package(target, "f" * 40 if target == WINDOWS else COMMIT)
        with self.assertRaisesRegex(release.ReleaseError, "same source commit"):
            release.checksums(self.output, VERSION)
        with self.assertRaisesRegex(release.ReleaseError, "manifest commit"):
            release.checksums(self.output, VERSION, COMMIT)

    def test_checksums_reject_unexpected_files(self):
        for target in release.TARGETS:
            self.package(target)
        (self.output / "extra.zip").write_bytes(b"not part of this release")
        with self.assertRaisesRegex(release.ReleaseError, "four expected"):
            release.checksums(self.output, VERSION)

    def test_checksums_validate_payloads_instead_of_only_hashing_archives(self):
        for target in release.TARGETS:
            archive = self.package(target)
            if target == LINUX:
                self.mutate_tar(archive, lambda members: self.alter_manifest(
                    members, lambda manifest: manifest.__setitem__("binary_sha256", "0" * 64)))
        with self.assertRaisesRegex(release.ReleaseError, "binary SHA256 mismatch"):
            release.checksums(self.output, VERSION, COMMIT)

    def test_smoke_runs_only_after_integrity_verification(self):
        archive = self.package()
        self.mutate_tar(archive, lambda members: self.alter_manifest(
            members, lambda manifest: manifest.__setitem__("commit", "f" * 40)))
        with mock.patch.object(release, "smoke") as smoke, self.assertRaises(release.ReleaseError):
            self.verify(archive, run_smoke=True)
        smoke.assert_not_called()

    def test_smoke_uses_unpacked_binary_and_removes_temporary_files(self):
        archive = self.package()
        unpacked = []
        def inspect(binary, target, version, directory, media):
            self.assertEqual(binary.read_bytes(), executable(LINUX))
            if os.name != "nt":
                self.assertEqual(binary.stat().st_mode & 0o777, 0o755)
            self.assertEqual(target, LINUX)
            self.assertEqual(version, VERSION)
            self.assertTrue(media)
            unpacked.append(directory)
        with mock.patch.object(release, "smoke", side_effect=inspect):
            self.verify(archive, media_smoke=True)
        self.assertTrue(unpacked)
        self.assertFalse(unpacked[0].exists())

    def test_invalid_commits_are_rejected(self):
        for commit in ("main", "abc123", "F" * 40, "a" * 39, "a" * 41):
            with self.subTest(commit=commit), self.assertRaisesRegex(release.ReleaseError, "40-character"):
                self.package(commit=commit)

    def test_semver_validation_accepts_prereleases_but_rejects_invalid_identifiers(self):
        for version in ("2.1.0", "2.1.0-rc.1", "2.1.0-01a+build.004", "0.1.0"):
            release.validate_identity(version, COMMIT)
        for version in ("02.1.0", "2.1", "2.1.0-01", "2.1.0-..", "2.1.0+a..b", "2.1.0/evil"):
            with self.subTest(version=version), self.assertRaises(release.ReleaseError):
                release.validate_identity(version, COMMIT)

    def test_version_command_prints_plain_version(self):
        with mock.patch.object(release, "root_version", return_value=VERSION):
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(release.main(["version", "--tag", "v2.1.0"]), 0)
            self.assertEqual(output.getvalue(), "2.1.0\n")
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(release.main(["version", "--tag", "v99.0.0"]), 1)

    def test_publication_command_has_no_approval_bypass_and_checks_requested_targets(self):
        with mock.patch.object(release, "publication_check") as check:
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(release.main(["publication-check", "--target", ARM_MAC]), 0)
            check.assert_called_once_with([ARM_MAC])
        with mock.patch.object(release, "publication_check", side_effect=release.ReleaseError("unresolved")):
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(release.main(["publication-check"]), 1)
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            release.main(["publication-check", "--skip-license-review"])

    @unittest.skipUnless(shutil.which("bash"), "workflow branch simulation requires bash")
    def test_workflow_gate_allows_source_checks_but_fails_tagged_distribution(self):
        workflow = (release.ROOT / ".github/workflows/packages.yml").read_text()
        start = workflow.index("          if python scripts/release.py publication-check; then\n")
        stop = workflow.index("\n\n  build:", start)
        gate_script = "\n".join(line[10:] for line in workflow[start:stop].splitlines())
        cases = (
            (False, "refs/heads/master", "", 0, "false"),
            (False, "refs/pull/1/merge", "", 0, "false"),
            (False, "refs/tags/v2.2.0-rc.1", "", 1, "false"),
            (False, "refs/heads/master", "v2.2.0-rc.1", 1, "false"),
            (True, "refs/heads/master", "", 0, "true"),
            (True, "refs/tags/v2.2.0-rc.1", "v2.2.0-rc.1", 0, "true"),
        )
        for number, (allowed, ref, tag, exit_code, expected_output) in enumerate(cases):
            with self.subTest(ref=ref, tag=tag, allowed=allowed):
                output = self.directory / f"gate-{number}.out"
                summary = self.directory / f"gate-{number}.summary"
                env = {**os.environ, "GITHUB_OUTPUT": str(output), "GITHUB_STEP_SUMMARY": str(summary),
                       "GITHUB_REF": ref, "RELEASE_TAG": tag}
                # Inert shell stub exercises the checked-in workflow branches;
                # it neither grants actual approval nor runs publication code.
                script = f"python() {{ return {0 if allowed else 1}; }}\n" + gate_script
                result = subprocess.run([shutil.which("bash"), "-e", "-o", "pipefail", "-c", script],
                                        env=env, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, exit_code, result.stderr)
                self.assertEqual(output.read_text(), f"allowed={expected_output}\n")
                if not allowed:
                    self.assertIn("No package or checksum artifacts will be uploaded", summary.read_text())

    def test_workflow_artifact_paths_and_tag_publication_are_explicitly_gated(self):
        workflow = (release.ROOT / ".github/workflows/packages.yml").read_text()
        for name in ("Archive executable with installation notes and manifest",
                     "Verify and run the extracted package", "Record package verification"):
            self.assertIn(f"- name: {name}\n        if: needs.publication.outputs.allowed == 'true'", workflow)
        self.assertIn("- uses: actions/upload-artifact@v7\n        if: needs.publication.outputs.allowed == 'true'", workflow)
        self.assertIn("needs: [publication, build]\n    if: needs.publication.outputs.allowed == 'true'", workflow)
        release_workflow = (release.ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("python scripts/release.py publication-check", release_workflow)


if __name__ == "__main__":
    unittest.main()
