"""Offline RC archive tests. Synthetic Mach-O/image headers are NOT runtime evidence."""
import io
import json
import os
from pathlib import Path
import plistlib
import struct
import tarfile
import tempfile
import unittest
from unittest import mock

import agent_rc


VERSION = "2.2.0-rc.1"
COMMIT = "0123456789abcdef0123456789abcdef01234567"


def binary(cpu=0x0100000C, filetype=2, marker=b"agent-fixture"):
    data = bytearray(128)
    data[:4] = b"\xcf\xfa\xed\xfe"
    struct.pack_into("<III", data, 4, cpu, 0, filetype)
    data[64:64 + len(marker)] = marker
    return bytes(data)


class AgentRcTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="agent-rc-offline-test-")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.root = self.directory / "source"
        self.root.mkdir()
        (self.root / "Cargo.toml").write_text(f'[package]\nversion = "{VERSION}"\n')
        for name in agent_rc.SOURCE_FILES:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("{}\n" if name.endswith(".json") else "Public unit fixture. Not an actual screenshot or model result.\n")
        for name in agent_rc.SCREENSHOTS:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            signature = agent_rc.PNG_SIGNATURE if name.endswith(".png") else agent_rc.JPEG_SIGNATURE
            path.write_bytes(signature + b"synthetic header fixture, not a real screenshot" + (b"\xff\xd9" if name.endswith(".jpg") else b""))
        self.agent = self.directory / "agent"
        self.toolkit = self.directory / "toolkit"
        self.agent.write_bytes(binary())
        self.toolkit.write_bytes(binary(marker=b"toolkit-fixture"))
        self.output = self.directory / "output"
        source = mock.patch.object(agent_rc, "verify_source")
        self.source_check = source.start()
        self.addCleanup(source.stop)
        binding = mock.patch.object(agent_rc, "verify_source_payload")
        self.binding_check = binding.start()
        self.addCleanup(binding.stop)

    def package(self):
        return agent_rc.package(self.agent, self.toolkit, self.output, COMMIT, self.root)

    def mutate(self, archive, change):
        with tarfile.open(archive, "r:gz") as source:
            members = [(member, source.extractfile(member).read()) for member in source]
        change(members)
        with tarfile.open(archive, "w:gz") as output:
            for member, data in members:
                member.size = len(data)
                output.addfile(member, io.BytesIO(data))

    def manifest_change(self, members, change):
        for index, (member, data) in enumerate(members):
            if member.name.endswith("/MANIFEST.json"):
                manifest = json.loads(data)
                change(manifest)
                members[index] = member, json.dumps(manifest).encode()
                return
        self.fail("Missing fixture manifest")

    def test_roundtrip_app_cli_manifest_and_checksum(self):
        archive = self.package()
        manifest = agent_rc.verify(archive, COMMIT, VERSION)
        self.assertEqual(manifest["source_commit"], COMMIT)
        self.assertFalse(manifest["validation"]["real_model_validated"])
        self.assertFalse(manifest["validation"]["developer_id_signed"])
        self.assertFalse(manifest["validation"]["notarized"])
        self.assertEqual(Path(f"{archive}.sha256").read_text(), f"{agent_rc.release.digest(archive.read_bytes())}  {archive.name}\n")
        _, archive_modes = agent_rc.members(archive)
        for executable in agent_rc.EXECUTABLES:
            self.assertEqual(archive_modes[f"{agent_rc.folder_name(VERSION, COMMIT)}/{executable}"], 0o755)
        extracted = agent_rc.extract(archive, self.directory / "unpacked", COMMIT)
        app_binary = extracted / agent_rc.APP_BINARY
        self.assertEqual(app_binary.read_bytes(), self.agent.read_bytes())
        # Windows does not expose POSIX executable bits on extracted files.
        # Archive permissions remain verified on every platform above.
        if os.name != "nt":
            self.assertEqual(app_binary.stat().st_mode & 0o777, 0o755)
        self.assertEqual((extracted / "bin/video-toolkit").read_bytes(), self.toolkit.read_bytes())
        metadata = plistlib.loads((extracted / agent_rc.APP / "Info.plist").read_bytes())
        self.assertEqual(metadata["CFBundleExecutable"], "video-agent")
        self.assertEqual(metadata["CFBundlePackageType"], "APPL")
        self.assertEqual(set(manifest["files"]), agent_rc.PAYLOAD_NAMES)

    def test_clean_source_check_receives_exact_commit(self):
        self.package()
        self.assertEqual(self.source_check.call_count, 2)
        self.source_check.assert_called_with(self.root, COMMIT)
        self.binding_check.assert_called_once()
        self.assertEqual(self.binding_check.call_args.args[:2], (self.root, COMMIT))
        self.assertEqual(set(self.binding_check.call_args.args[2]), set(agent_rc.SOURCE_FILES) | set(agent_rc.SCREENSHOTS) | {"Cargo.toml"})

    def test_private_data_and_raw_video_are_never_collected(self):
        for name in (".env", "task.json", "private.mov", "target/cache", ".git/config", "video-agent-private/task.json", "assets/video-agent/private.png"):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("private runtime data MUST NOT be packaged")
        archive = self.package()
        contents, _ = agent_rc.members(archive)
        self.assertEqual(len(contents), len(agent_rc.PAYLOAD_NAMES) + 1)
        self.assertFalse(any(b"private runtime data MUST NOT" in data for data in contents.values()))

    def test_existing_archive_and_checksum_are_not_overwritten(self):
        archive = self.package()
        original = archive.read_bytes()
        with self.assertRaisesRegex(agent_rc.RcError, "overwrite"):
            self.package()
        self.assertEqual(archive.read_bytes(), original)

    def test_preexisting_checksum_alone_blocks_publication(self):
        self.output.mkdir()
        archive = self.output / f"{agent_rc.folder_name(VERSION, COMMIT)}.tar.gz"
        checksum = Path(f"{archive}.sha256")
        checksum.write_text("keep this existing file")
        with self.assertRaisesRegex(agent_rc.RcError, "overwrite"):
            self.package()
        self.assertFalse(archive.exists())
        self.assertEqual(checksum.read_text(), "keep this existing file")

    def test_existing_extraction_directory_is_never_changed(self):
        archive = self.package()
        destination = self.directory / "existing"
        destination.mkdir()
        marker = destination / "private"
        marker.write_text("unchanged")
        with self.assertRaisesRegex(agent_rc.RcError, "already exist"):
            agent_rc.extract(archive, destination, COMMIT)
        self.assertEqual(marker.read_text(), "unchanged")

    def test_wrong_architecture_in_either_binary_is_rejected(self):
        for path in (self.agent, self.toolkit):
            with self.subTest(path=path):
                original = path.read_bytes()
                path.write_bytes(binary(cpu=0x01000007))
                with self.assertRaisesRegex(agent_rc.RcError, "does not match"):
                    self.package()
                path.write_bytes(original)
        self.assertFalse(self.output.exists())

    def test_non_executable_macho_is_rejected(self):
        self.agent.write_bytes(binary(filetype=6))
        with self.assertRaisesRegex(agent_rc.RcError, "not an executable"):
            self.package()

    def test_symlink_source_and_symlink_ancestor_are_rejected(self):
        readme = self.root / "README.md"
        readme.unlink()
        readme.symlink_to(self.agent)
        with self.assertRaisesRegex(agent_rc.RcError, "Symlink"):
            self.package()
        readme.unlink()
        readme.write_text("public")
        docs = self.root / "docs"
        docs.rename(self.root / "other-docs")
        docs.symlink_to(self.root / "other-docs", target_is_directory=True)
        with self.assertRaisesRegex(agent_rc.RcError, "Symlink"):
            self.package()

    def test_possible_credential_in_public_text_is_rejected(self):
        (self.root / "README.md").write_text("sk-proj-" + "a" * 48)
        with self.assertRaisesRegex(agent_rc.RcError, "credential"):
            self.package()

    def test_missing_misnamed_or_non_image_screenshot_is_not_silently_skipped(self):
        path = self.root / agent_rc.SCREENSHOTS[0]
        path.write_text("not an image")
        with self.assertRaisesRegex(agent_rc.RcError, "must match"):
            self.package()
        path.write_bytes(agent_rc.PNG_SIGNATURE + b"wrong encoding for jpg filename")
        with self.assertRaisesRegex(agent_rc.RcError, "must match"):
            self.package()
        path.unlink()
        with self.assertRaisesRegex(agent_rc.RcError, "regular file"):
            self.package()

    def test_hash_corruption_is_detected(self):
        archive = self.package()
        def corrupt(members):
            for index, (member, data) in enumerate(members):
                if member.name.endswith("/bin/video-agent"):
                    members[index] = member, data[:-1] + b"!"
        self.mutate(archive, corrupt)
        with self.assertRaisesRegex(agent_rc.RcError, "SHA256"):
            agent_rc.verify(archive, COMMIT)

    def test_extraction_verifies_the_exact_snapshot_it_writes(self):
        archive = self.package()
        snapshot = agent_rc.members(archive)
        with mock.patch.object(agent_rc, "members", return_value=snapshot) as read:
            extracted = agent_rc.extract(archive, self.directory / "once-only", COMMIT)
        read.assert_called_once_with(archive)
        self.assertEqual((extracted / "bin/video-agent").read_bytes(), self.agent.read_bytes())

    def test_rehashed_app_copy_must_match_portable_agent(self):
        archive = self.package()
        replacement = binary(marker=b"a different native arm64 executable")
        def replace(members):
            for index, (member, data) in enumerate(members):
                if member.name.endswith("/" + agent_rc.APP_BINARY):
                    members[index] = member, replacement
            self.manifest_change(members, lambda m: m["files"][agent_rc.APP_BINARY].update(
                sha256=agent_rc.release.digest(replacement), size=len(replacement)))
        self.mutate(archive, replace)
        with self.assertRaisesRegex(agent_rc.RcError, "must be identical"):
            agent_rc.verify(archive, COMMIT)

    def test_rehashed_plist_cannot_point_to_another_executable(self):
        archive = self.package()
        metadata = plistlib.loads(agent_rc.plist(VERSION))
        metadata["CFBundleExecutable"] = "video-toolkit"
        replacement = plistlib.dumps(metadata)
        def replace(members):
            for index, (member, data) in enumerate(members):
                if member.name.endswith("/Contents/Info.plist"):
                    members[index] = member, replacement
            self.manifest_change(members, lambda m: m["files"][f"{agent_rc.APP}/Info.plist"].update(
                sha256=agent_rc.release.digest(replacement), size=len(replacement)))
        self.mutate(archive, replace)
        with self.assertRaisesRegex(agent_rc.RcError, "bundle metadata"):
            agent_rc.verify(archive, COMMIT)

    def test_limitation_flags_are_real_booleans_not_equal_numbers(self):
        archive = self.package()
        self.mutate(archive, lambda members: self.manifest_change(members, lambda m: m["validation"].update(real_model_validated=0)))
        with self.assertRaisesRegex(agent_rc.RcError, "JSON booleans"):
            agent_rc.verify(archive, COMMIT)

    def test_unexpected_member_is_not_accepted_even_with_safe_path(self):
        archive = self.package()
        def append(members):
            extra = tarfile.TarInfo(f"{agent_rc.folder_name(VERSION, COMMIT)}/task.json")
            members.append((extra, b"private task state"))
        self.mutate(archive, append)
        with self.assertRaises(agent_rc.RcError):
            agent_rc.verify(archive, COMMIT)

    def test_fake_verified_or_signed_claims_are_rejected(self):
        archive = self.package()
        self.mutate(archive, lambda members: self.manifest_change(members, lambda m: m["validation"].update(real_model_validated=True, notarized=True)))
        with self.assertRaisesRegex(agent_rc.RcError, "limitations"):
            agent_rc.verify(archive, COMMIT)

    def test_wrong_expected_source_commit_is_rejected(self):
        archive = self.package()
        with self.assertRaisesRegex(agent_rc.RcError, "source identity"):
            agent_rc.verify(archive, "f" * 40)

    def test_manifest_cannot_omit_a_payload_hash(self):
        archive = self.package()
        self.mutate(archive, lambda members: self.manifest_change(members, lambda m: m["files"].pop("LICENSE")))
        with self.assertRaisesRegex(agent_rc.RcError, "hash every"):
            agent_rc.verify(archive, COMMIT)

    def test_traversal_and_links_are_rejected_before_extraction(self):
        for name in ("../outside", "/absolute", "folder/../escape", "folder\\escape", "C:/escape"):
            archive = self.package()
            self.mutate(archive, lambda members: setattr(members[0][0], "name", name))
            with self.subTest(name=name), self.assertRaisesRegex(agent_rc.RcError, "unsafe archive"):
                agent_rc.verify(archive, COMMIT)
            archive.unlink()
            Path(f"{archive}.sha256").unlink()
        archive = self.package()
        self.mutate(archive, lambda members: setattr(members[0][0], "type", tarfile.SYMTYPE))
        with self.assertRaisesRegex(agent_rc.RcError, "links and special"):
            agent_rc.verify(archive, COMMIT)

    def test_binary_permissions_cannot_be_nonexecutable_or_privileged(self):
        for mode in (0o644, 0o4755):
            archive = self.package()
            def chmod(members):
                for member, _ in members:
                    if member.name.endswith("/bin/video-agent"):
                        member.mode = mode
            self.mutate(archive, chmod)
            with self.subTest(mode=mode), self.assertRaisesRegex(agent_rc.RcError, "permissions"):
                agent_rc.verify(archive, COMMIT)
            archive.unlink()
            Path(f"{archive}.sha256").unlink()

    def test_identical_inputs_produce_deterministic_archives(self):
        first = self.package()
        data = first.read_bytes()
        first.unlink()
        Path(f"{first}.sha256").unlink()
        self.assertEqual(self.package().read_bytes(), data)

    def test_source_git_identity_rejects_dirty_or_different_head(self):
        # Test the real guard, bypassing the fixture's packaging-only mock.
        original = self.source_check.side_effect
        self.source_check.side_effect = None
        import importlib.util
        spec = importlib.util.spec_from_file_location("agent_rc_guard", Path(agent_rc.__file__))
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        for head, status in (("f" * 40, ""), (COMMIT, " M README.md\n")):
            with mock.patch.object(module.subprocess, "run", side_effect=[mock.Mock(returncode=0, stdout=head), mock.Mock(returncode=0, stdout=status)]):
                with self.assertRaises(agent_rc.RcError):
                    module.verify_source(self.root, COMMIT)
        self.source_check.side_effect = original

    def test_public_payload_must_match_committed_git_blobs(self):
        import importlib.util
        spec = importlib.util.spec_from_file_location("agent_rc_blob_guard", Path(agent_rc.__file__))
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        data = b"committed public README\n"
        output = b"a" * 40 + f" blob {len(data)}\n".encode() + data + b"\n"
        with mock.patch.object(module.subprocess, "run", return_value=mock.Mock(returncode=0, stdout=output)):
            module.verify_source_payload(self.root, COMMIT, {"README.md": data})
            with self.assertRaisesRegex(agent_rc.RcError, "differs from Git HEAD"):
                module.verify_source_payload(self.root, COMMIT, {"README.md": b"!" + data[1:]})


if __name__ == "__main__":
    unittest.main()
