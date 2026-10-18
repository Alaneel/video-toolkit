#!/usr/bin/env python3
"""Create a local, unsigned macOS arm64 Video Agent review candidate.

Package a clean committed checkout (no push/release):
  python3 scripts/agent_rc.py --binary-agent target/release/video-agent \
      --binary-toolkit target/release/video-toolkit --output-dir /private/tmp/rc \
      --commit FULL_LOWERCASE_GIT_SHA

Inspect or safely extract without executing any archived binary:
  python3 scripts/agent_rc.py verify ARCHIVE --commit SHA --extract-to NEW_DIR

Only the explicitly enumerated public files below can enter this archive.
The manifest and sidecar provide integrity, not publisher authentication or
cryptographic proof that a supplied binary was compiled from a particular SHA.
"""
from __future__ import annotations

import argparse
import gzip
import io
import json
import os
from pathlib import Path
import plistlib
import re
import stat
import subprocess
import sys
import tarfile
import tempfile

import release


ROOT = Path(__file__).resolve().parents[1]
TARGET = "aarch64-apple-darwin"
APP = "Video Agent RC.app/Contents"
APP_BINARY = f"{APP}/MacOS/video-agent"
EXECUTABLES = {APP_BINARY, "bin/video-agent", "bin/video-toolkit"}
SOURCE_FILES = (
    "README.md", "LICENSE", "CHANGELOG.md", "THIRD_PARTY_NOTICES.txt",
    "assets/video-agent/hero.svg", "assets/video-agent/ATTRIBUTION.md",
    "docs/video-agent.md", "docs/video-agent-audit.zh-CN.md",
    "docs/video-agent-materials.md", "docs/video-agent-evidence.json",
    "docs/test-process-safety.md",
    "docs/video-agent-review.zh-CN.md",
    "evals/video-agent/README.md", "evals/video-agent/dataset.json",
    "evals/video-agent/tasks.json", "evals/video-agent/result-template.json",
    "evals/video-agent/materials.py", "evals/video-agent/test_materials.py",
    "evals/video-agent/evidence.py", "evals/video-agent/test_evidence.py",
    "evals/video-agent/results/status.json",
    "evals/video-agent/results/dev-rabbit-woodland-20261018-v1.json",
    "evals/video-agent/results/dev-rabbit-woodland-20261018-v2.json",
    "evals/video-agent/results/dev-rabbit-woodland-20261018-v3.json",
    "evals/video-agent/results/dev-rabbit-woodland-20261018-luna.json",
    "evals/video-agent/results/materials-validation.json",
    "scripts/agent_rc.py", "scripts/release.py", "scripts/third_party_notices.py",
)
SCREENSHOTS = tuple(f"assets/video-agent/{name}.jpg" for name in (
    "observed-frames-manual-validation", "human-review-manual-validation",
    "cancelled-manual-export", "verified-manual-output", "actual-output-playback",
    "parallel-observation-manual-validation",
    "real-luna-review", "real-luna-verified-output",
)) + ("assets/video-toolkit-ui.png",)
GENERATED = (f"{APP}/Info.plist", f"{APP}/PkgInfo", "INSTALL.txt")
PAYLOAD_NAMES = set(SOURCE_FILES) | set(SCREENSHOTS) | set(GENERATED) | EXECUTABLES
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
JPEG_SIGNATURE = b"\xff\xd8\xff"
MAX_MEMBER = 128 * 1024 * 1024
MAX_TOTAL = 512 * 1024 * 1024
MAX_TEXT = 2 * 1024 * 1024
MAX_SCREENSHOT = 16 * 1024 * 1024
SECRET_RE = re.compile(rb"\bsk-[A-Za-z0-9_-]{20,}\b")
NOTICES = {
    "real_model_validated": False,
    "semantic_quality_evaluated": False,
    "developer_id_signed": False,
    "notarized": False,
    "real_model_status": "One public development workflow completed real calls, user-directed first-clip removal and verified 20-second export. Formal human semantic comparison remains pending; this is not general model-quality acceptance.",
    "signing_status": "Not Developer ID signed or notarized. A linker ad-hoc signature is not a publisher identity.",
    "platform_status": "Local macOS Apple Silicon review candidate only; other Agent targets unverified.",
    "binary_provenance": "Supplied native binaries; the clean checkout SHA is recorded, not a reproducible-build attestation.",
    "external_dependencies": ["ffmpeg", "ffprobe", "ffplay"],
}
require = release.require
RcError = release.ReleaseError


def screenshot_matches(name: str, data: bytes) -> bool:
    """Keep native capture bytes; require the filename to match its encoding."""
    if name.endswith(".png"):
        return data.startswith(PNG_SIGNATURE)
    if name.endswith(".jpg"):
        return data.startswith(JPEG_SIGNATURE) and data.endswith(b"\xff\xd9")
    return False


def folder_name(version: str, commit: str) -> str:
    release.validate_identity(version, commit)
    return f"video-agent-{version}-macos-arm64-{commit[:12]}"


def verify_source(root: Path, commit: str) -> None:
    """Read-only Git checks. Never change a checkout, credential or remote."""
    try:
        options = dict(cwd=root, stdin=subprocess.DEVNULL, capture_output=True,
                       text=True, timeout=15, check=False)
        head = subprocess.run(["git", "rev-parse", "HEAD"], **options)
        status = subprocess.run(["git", "status", "--porcelain", "--untracked-files=all"], **options)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RcError("Cannot verify local Git source identity with the installed toolchain") from error
    require(head.returncode == 0 and status.returncode == 0,
            "Cannot verify local Git source identity; use a usable Git toolchain")
    require(head.stdout.strip() == commit, "--commit must match the current source HEAD")
    require(not status.stdout.strip(), "Commit candidate source changes before packaging; checkout is not clean")


def verify_source_payload(root: Path, commit: str, payload: dict[str, bytes]) -> None:
    """Bind captured public bytes to Git blobs, even with assume-unchanged flags."""
    names = sorted(payload)
    request = "".join(f"{commit}:{name}\n" for name in names).encode("utf-8")
    try:
        result = subprocess.run(["git", "cat-file", "--batch"], cwd=root, input=request,
                                capture_output=True, timeout=15, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RcError("Cannot bind public candidate files to the committed Git blobs") from error
    require(result.returncode == 0, "Cannot read committed public Git blobs")
    cursor = 0
    for name in names:
        end = result.stdout.find(b"\n", cursor)
        require(end >= cursor, f"Missing committed public file: {name}")
        fields = result.stdout[cursor:end].split()
        require(len(fields) == 3 and fields[1] == b"blob" and fields[2].isdigit(),
                f"Expected a committed public blob: {name}")
        size = int(fields[2])
        require(size == len(payload[name]), f"Public file differs from Git HEAD: {name}")
        cursor = end + 1
        require(result.stdout[cursor:cursor + size] == payload[name]
                and result.stdout[cursor + size:cursor + size + 1] == b"\n",
                f"Public file differs from Git HEAD: {name}")
        cursor += size + 1
    require(cursor == len(result.stdout), "Unexpected extra committed Git blob data")


def public_file(root: Path, name: str) -> bytes:
    """Fixed allowlist plus no symlink ancestors; never recursively collect a tree."""
    require(name in set(SOURCE_FILES) | set(SCREENSHOTS), "File is outside the public allowlist")
    root = root.resolve(strict=True)
    path = root / name
    for part in Path(name).parts:
        root = root / part
        require(not root.is_symlink(), f"Symlinked public source is forbidden: {name}")
    data = release.read_regular(path)
    limit = MAX_SCREENSHOT if name in SCREENSHOTS else MAX_TEXT
    require(len(data) <= limit, f"Public file exceeds its size limit: {name}")
    if name in SCREENSHOTS:
        require(screenshot_matches(name, data), f"Screenshot must match its PNG/JPEG filename: {name}")
    else:
        require(not SECRET_RE.search(data), f"Possible API credential in public file: {name}")
        try:
            data.decode("utf-8")
        except UnicodeDecodeError as error:
            raise RcError(f"Expected UTF-8 public text: {name}") from error
    return data


def plist(version: str) -> bytes:
    return plistlib.dumps({
        "CFBundleName": "Video Agent RC", "CFBundleDisplayName": "Video Agent RC",
        "CFBundleIdentifier": "io.github.alaneel.video-agent.rc",
        "CFBundleExecutable": "video-agent", "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version.split("-", 1)[0].split("+", 1)[0],
        "CFBundleVersion": version.split("-", 1)[0].split("+", 1)[0],
        "CFBundleSupportedPlatforms": ["MacOSX"], "NSHighResolutionCapable": True,
        "LSApplicationCategoryType": "public.app-category.video",
        "NSHumanReadableCopyright": "MIT License; see the accompanying LICENSE.",
    }, sort_keys=True)


def installation(version: str) -> bytes:
    return f"""Video Agent {version} — local macOS Apple Silicon review candidate

This is NOT a public release and NOT a completed AI-quality evaluation.
Real API-backed development calls produced a draft; a user-directed first-clip
removal and verified 20-second export have now completed in the native UI.
Formal human semantic comparison remains pending, not inferred from playback.
Manual workflow screenshots are explicitly separate non-AI validation.
The default model is GPT-6 Luna. The configurable legacy GPT-4.1-mini snapshot
is deprecated, with shutdown scheduled 2027-01-14. See the provider guide for
separate estimates, known failures and actual validation status.

1. Read README.md, docs/video-agent.md and the Chinese audit/delivery notes.
2. Double-click 'Video Agent RC.app' to launch the real native Agent interface.
   bin/video-agent is the same executable for Terminal use. It accepts --workspace
   /absolute/private/task-folder to reopen a saved job.
3. bin/video-toolkit keeps the original toolkit GUI/CLI. Its 'agent' subcommand
   also opens the new Agent. No Rust toolchain is needed for these binaries.
4. Install ffmpeg, ffprobe and ffplay separately. They are not bundled. They must
   be discoverable by the launched app; Finder may not inherit shell PATH or API
   environment variables. Official guidance: https://ffmpeg.org/download.html
   THIRD_PARTY_NOTICES.txt includes mechanically collected dependency and default
   font notices. This collection is not a completed legal/license-compliance audit;
   unresolved audit status must not be presented as verified compliance.
5. Configure credentials locally, preferably the private config described in the
   Agent guide. Never put credentials, private footage or runtime task directories
   into this archive or Git. Only sampled JPEGs are sent after explicit consent.

This app is NOT Developer ID signed or notarized; macOS may block launch.
A compiler/linker ad-hoc signature does not identify a trusted publisher.
Verify provenance and checksums before following Apple's normal app approval
guidance: https://support.apple.com/102445 . Do not disable system-wide security.
Only macOS Apple Silicon is the current Agent runtime target. No compatibility
claim is made for other systems or older macOS versions.

MANIFEST.json records source commit, file sizes, hashes and permissions. The
archive's .sha256 sidecar covers the archive itself. These are integrity checks,
not a signature or proof of source-to-binary equivalence. Review/extract without
executing code using Python 3.11+ and the included verifier:
  python3 scripts/agent_rc.py verify /absolute/path/archive.tar.gz --commit FULL_SHA
  python3 scripts/agent_rc.py verify /absolute/path/archive.tar.gz --commit FULL_SHA --extract-to /new/private/directory

No task state, raw video, API config, dependency cache or Git metadata is bundled.
Completed output remains under the private task folder; never overwrite sources.
""".encode()


def package(binary_agent: Path, binary_toolkit: Path, output_dir: Path,
            commit: str, root: Path = ROOT) -> Path:
    version = release.root_version(root)
    release.validate_identity(version, commit)
    verify_source(root, commit)
    agent, toolkit = release.read_regular(binary_agent), release.read_regular(binary_toolkit)
    for data in (agent, toolkit):
        require(len(data) <= MAX_MEMBER, "Native binary exceeds the candidate size limit")
        release.check_architecture(data, TARGET)
    payload = {name: public_file(root, name) for name in (*SOURCE_FILES, *SCREENSHOTS)}
    verify_source_payload(root, commit, {"Cargo.toml": release.read_regular(root / "Cargo.toml"), **payload})
    verify_source(root, commit)
    payload.update({APP_BINARY: agent, "bin/video-agent": agent, "bin/video-toolkit": toolkit,
                    f"{APP}/Info.plist": plist(version), f"{APP}/PkgInfo": b"APPL????",
                    "INSTALL.txt": installation(version)})
    require(sum(map(len, payload.values())) <= MAX_TOTAL, "Candidate contents exceed the total size limit")
    manifest = {
        "schema_version": 1, "product": "Video Agent local RC", "version": version,
        "target": TARGET, "source_commit": commit, "source_checkout": "clean_git_head",
        "validation": NOTICES,
        "files": {name: {"sha256": release.digest(data), "size": len(data),
                         "mode": 0o755 if name in EXECUTABLES else 0o644}
                  for name, data in sorted(payload.items())},
    }
    payload["MANIFEST.json"] = (json.dumps(manifest, sort_keys=True, indent=2) + "\n").encode()
    folder = folder_name(version, commit)
    output_dir.mkdir(parents=True, exist_ok=True)
    require(not output_dir.is_symlink(), "Output directory must not be a symlink")
    output = output_dir / f"{folder}.tar.gz"
    checksum = Path(f"{output}.sha256")
    require(not os.path.lexists(output) and not os.path.lexists(checksum),
            "Refusing to overwrite an existing candidate archive or checksum")
    with tempfile.TemporaryDirectory(prefix="agent-rc-build-", dir=output_dir) as staging:
        temporary = Path(staging) / "candidate.tar.gz"
        with temporary.open("xb") as destination, gzip.GzipFile(
            filename="", fileobj=destination, mode="wb", mtime=0
        ) as compressed, tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
            for name, data in sorted(payload.items()):
                member = tarfile.TarInfo(f"{folder}/{name}")
                member.size, member.mtime = len(data), 0
                member.mode = 0o755 if name in EXECUTABLES else 0o644
                archive.addfile(member, io.BytesIO(data))
        verify(temporary, commit, version, check_filename=False)
        temporary_sum = Path(staging) / "candidate.sha256"
        temporary_sum.write_text(f"{release.digest(temporary.read_bytes())}  {output.name}\n", encoding="ascii")
        # Hard links are same-filesystem, atomic and no-clobber, unlike replace().
        try:
            os.link(temporary, output)
            os.link(temporary_sum, checksum)
        except FileExistsError as error:
            raise RcError("Candidate publication raced an existing file; nothing was overwritten") from error
    return output


def members(path: Path) -> tuple[dict[str, bytes], dict[str, int]]:
    require(path.is_file() and not path.is_symlink(), "Archive must be a regular file")
    require(path.stat().st_size <= MAX_TOTAL, "Compressed archive exceeds the size limit")
    contents, modes = {}, {}
    total = 0
    with tarfile.open(path, "r:gz") as archive:
        for index, member in enumerate(archive):
            require(index < len(PAYLOAD_NAMES) + 1, "Too many archive members")
            release.safe_name(member.name)
            require(member.isreg() and not member.issparse(), "Archive links and special files are forbidden")
            require(member.name not in contents, "Duplicate archive member")
            require(0 <= member.size <= MAX_MEMBER, "Archive member exceeds the size limit")
            total += member.size
            require(total <= MAX_TOTAL, "Archive exceeds the total size limit")
            stream = archive.extractfile(member)
            require(stream is not None, "Cannot read archive member")
            with stream:
                data = stream.read(MAX_MEMBER + 1)
            require(len(data) == member.size, "Archive member size mismatch")
            contents[member.name], modes[member.name] = data, member.mode & 0o7777
    return contents, modes


def verify(path: Path, expected_commit: str, expected_version: str | None = None,
           *, check_filename: bool = True) -> dict:
    release.validate_identity(expected_version or "0.0.0", expected_commit)
    contents, modes = members(path)
    return _verify_members(path, contents, modes, expected_commit, expected_version,
                           check_filename=check_filename)


def _verify_members(path: Path, contents: dict[str, bytes], modes: dict[str, int],
                    expected_commit: str, expected_version: str | None = None,
                    *, check_filename: bool = True) -> dict:
    """Verify one immutable in-memory snapshot; extraction uses these same bytes."""
    release.validate_identity(expected_version or "0.0.0", expected_commit)
    manifest_names = [name for name in contents if name.endswith("/MANIFEST.json")]
    require(len(manifest_names) == 1, "Expected exactly one candidate manifest")
    manifest_path = manifest_names[0]
    require(len(contents[manifest_path]) <= MAX_TEXT, "Manifest is too large")
    manifest = json.loads(contents[manifest_path], object_pairs_hook=release.json_object)
    require(isinstance(manifest, dict) and set(manifest) == {
        "schema_version", "product", "version", "target", "source_commit", "source_checkout", "validation", "files"
    }, "Unexpected candidate manifest fields")
    version = manifest["version"]
    release.validate_identity(version, expected_commit)
    require(type(manifest["schema_version"]) is int and manifest["schema_version"] == 1, "Unsupported candidate schema")
    require(manifest["product"] == "Video Agent local RC" and manifest["target"] == TARGET,
            "Candidate product/target mismatch")
    require(manifest["source_commit"] == expected_commit and manifest["source_checkout"] == "clean_git_head", "Candidate source identity mismatch")
    require(expected_version is None or version == expected_version, "Candidate version mismatch")
    require(manifest["validation"] == NOTICES, "Candidate must retain explicit unverified/unsigned limitations")
    require(all(type(manifest["validation"][key]) is bool for key in (
        "real_model_validated", "semantic_quality_evaluated", "developer_id_signed", "notarized"
    )), "Candidate limitation flags must be JSON booleans")
    folder = folder_name(version, expected_commit)
    require(not check_filename or path.name == f"{folder}.tar.gz", "Candidate filename mismatch")
    expected_names = {f"{folder}/{name}" for name in PAYLOAD_NAMES | {"MANIFEST.json"}}
    require(set(contents) == expected_names, "Archive must contain exactly the fixed public allowlist")
    hashes = manifest["files"]
    require(isinstance(hashes, dict) and set(hashes) == PAYLOAD_NAMES, "Manifest must hash every allowed payload")
    for name, record in hashes.items():
        require(isinstance(record, dict) and set(record) == {"sha256", "size", "mode"}, "Invalid manifest file record")
        data = contents[f"{folder}/{name}"]
        expected_mode = 0o755 if name in EXECUTABLES else 0o644
        require(type(record["size"]) is int and record["size"] == len(data), f"Payload size mismatch: {name}")
        require(record["sha256"] == release.digest(data), f"Payload SHA256 mismatch: {name}")
        require(type(record["mode"]) is int and record["mode"] == modes[f"{folder}/{name}"] == expected_mode, f"Incorrect payload permissions: {name}")
        if name in EXECUTABLES:
            release.check_architecture(data, TARGET)
        elif name in SCREENSHOTS:
            require(len(data) <= MAX_SCREENSHOT and screenshot_matches(name, data), "Invalid screenshot payload")
        else:
            require(len(data) <= MAX_TEXT and not SECRET_RE.search(data), "Oversized or potentially secret text payload")
    require(modes[manifest_path] == 0o644, "Incorrect manifest permissions")
    require(contents[f"{folder}/{APP_BINARY}"] == contents[f"{folder}/bin/video-agent"], "App and portable Agent must be identical")
    require(contents[f"{folder}/{APP}/Info.plist"] == plist(version), "Unexpected application bundle metadata")
    require(contents[f"{folder}/{APP}/PkgInfo"] == b"APPL????", "Invalid application bundle type")
    require(contents[f"{folder}/INSTALL.txt"] == installation(version), "Candidate installation/limitations text mismatch")
    return manifest


def extract(path: Path, destination: Path, expected_commit: str,
            expected_version: str | None = None) -> Path:
    contents, modes = members(path)
    manifest = _verify_members(path, contents, modes, expected_commit, expected_version)
    require(not os.path.lexists(destination), "Extraction destination must not already exist")
    destination.parent.mkdir(parents=True, exist_ok=True)
    # Build in a private sibling and publish the complete, verified directory only.
    with tempfile.TemporaryDirectory(prefix="agent-rc-extract-", dir=destination.parent) as staging:
        root = Path(staging) / "unpacked"
        root.mkdir(mode=0o755)
        for name, data in sorted(contents.items()):
            output = root / name
            output.parent.mkdir(parents=True, exist_ok=True)
            with output.open("xb") as stream:
                stream.write(data)
            output.chmod(modes[name])
        require(not os.path.lexists(destination), "Extraction destination appeared concurrently")
        # rename cannot replace a populated directory; mkdir reserves ownership.
        destination.mkdir(mode=0o755)
        try:
            (root / folder_name(manifest["version"], expected_commit)).rename(
                destination / folder_name(manifest["version"], expected_commit))
        except BaseException:
            destination.rmdir()
            raise
    return destination / folder_name(manifest["version"], expected_commit)


def main(argv: list[str] | None = None) -> int:
    arguments = list(sys.argv[1:] if argv is None else argv)
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    if arguments and arguments[0] == "verify":
        parser.add_argument("archive", type=Path)
        parser.add_argument("--commit", required=True)
        parser.add_argument("--expected-version")
        parser.add_argument("--extract-to", type=Path)
        args = parser.parse_args(arguments[1:])
        try:
            manifest = verify(args.archive, args.commit, args.expected_version)
            unpacked = extract(args.archive, args.extract_to, args.commit, args.expected_version) if args.extract_to else None
            print(json.dumps({"verified": True, "source_commit": manifest["source_commit"],
                              "version": manifest["version"], "extracted_to": str(unpacked) if unpacked else None}))
            return 0
        except (OSError, ValueError, tarfile.TarError) as error:
            parser.exit(1, f"Candidate verification failed: {error}\n")
    parser.add_argument("--binary-agent", type=Path, required=True)
    parser.add_argument("--binary-toolkit", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--commit", required=True)
    args = parser.parse_args(arguments)
    try:
        require(release.host_target() == TARGET, "Packaging CLI is limited to local macOS Apple Silicon")
        archive = package(args.binary_agent, args.binary_toolkit, args.output_dir, args.commit)
        print(archive.resolve())
        print(Path(f"{archive}.sha256").resolve())
        return 0
    except (OSError, ValueError, tarfile.TarError) as error:
        parser.exit(1, f"Candidate packaging failed: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
