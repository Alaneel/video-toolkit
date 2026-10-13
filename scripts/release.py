#!/usr/bin/env python3
"""Build and verify portable release archives using only Python 3.11+ stdlib.

Run ``python scripts/release.py --help`` for the CI-facing command interface.
Verification never trusts archive paths or uses extractall. Smoke tests run the
verified, unpacked executable, not a separate binary in the build directory.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile


ROOT = Path(__file__).resolve().parents[1]
TARGETS = {
    "x86_64-unknown-linux-gnu": ("ELF", 62, ".tar.gz", "video-toolkit"),
    "aarch64-apple-darwin": ("Mach-O", 0x0100000C, ".tar.gz", "video-toolkit"),
    "x86_64-apple-darwin": ("Mach-O", 0x01000007, ".tar.gz", "video-toolkit"),
    "x86_64-pc-windows-msvc": ("PE", 0x8664, ".zip", "video-toolkit.exe"),
}
MAX_MEMBER_SIZE = 512 * 1024 * 1024
PRERELEASE_IDENTIFIER = r"(?:0|[1-9][0-9]*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)"
VERSION_RE = re.compile(
    r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
    rf"(?:-{PRERELEASE_IDENTIFIER}(?:\.{PRERELEASE_IDENTIFIER})*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?\Z"
)
COMMIT_RE = re.compile(r"[0-9a-f]{40}\Z")


class ReleaseError(ValueError):
    """Invalid release inputs or archive contents."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ReleaseError(message)


def root_version(root: Path = ROOT) -> str:
    with (root / "Cargo.toml").open("rb") as source:
        version = tomllib.load(source)["package"]["version"]
    validate_identity(version, "0" * 40)
    return version


def validate_identity(version: str, commit: str) -> None:
    require(isinstance(version, str) and VERSION_RE.fullmatch(version) is not None,
            "version must be a semantic version from Cargo.toml")
    require(isinstance(commit, str) and COMMIT_RE.fullmatch(commit) is not None,
            "commit must be a full lowercase 40-character Git SHA")


def validate_tag(tag: str | None, version: str) -> None:
    if tag is not None:
        require(tag == f"v{version}", f"tag must exactly match Cargo.toml: v{version}")


def target_info(target: str) -> tuple[str, int, str, str]:
    require(target in TARGETS, f"unsupported release target: {target}")
    return TARGETS[target]


def archive_name(version: str, target: str) -> str:
    return f"video-toolkit-{version}-{target}{target_info(target)[2]}"


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def architecture(data: bytes) -> tuple[str, int]:
    """Read the executable format, 64-bit ABI and CPU from native file headers."""
    if data.startswith(b"\x7fELF"):
        require(len(data) >= 64, "truncated ELF header")
        require(data[4:7] == b"\x02\x01\x01", "ELF must be 64-bit little-endian version 1")
        kind, machine = struct.unpack_from("<HH", data, 16)
        require(kind in (2, 3), "ELF is not an executable or position-independent executable")
        return "ELF", machine
    if data.startswith(b"MZ"):
        require(len(data) >= 64, "truncated PE DOS header")
        offset = struct.unpack_from("<I", data, 60)[0]
        require(64 <= offset <= len(data) - 26, "invalid PE header offset")
        require(data[offset:offset + 4] == b"PE\x00\x00", "missing PE signature")
        machine = struct.unpack_from("<H", data, offset + 4)[0]
        characteristics = struct.unpack_from("<H", data, offset + 22)[0]
        require(characteristics & 0x0002 and not characteristics & 0x2000,
                "PE is not an executable (DLLs are not release binaries)")
        require(struct.unpack_from("<H", data, offset + 24)[0] == 0x020B,
                "PE must use the 64-bit PE32+ ABI")
        return "PE", machine
    if data.startswith(b"\xcf\xfa\xed\xfe"):
        require(len(data) >= 32, "truncated Mach-O header")
        cpu, _, filetype = struct.unpack_from("<III", data, 4)
        require(filetype == 2, "Mach-O is not an executable")
        return "Mach-O", cpu
    raise ReleaseError("unrecognized executable format; expected a native 64-bit binary")


def check_architecture(data: bytes, target: str) -> None:
    expected = target_info(target)[:2]
    actual = architecture(data)
    require(actual == expected, f"binary architecture {actual} does not match {target} ({expected})")


def install_text(version: str, target: str) -> bytes:
    _, _, _, binary = target_info(target)
    platform_note = {
        "x86_64-unknown-linux-gnu": (
            "Linux x86_64: built on Ubuntu 22.04. The native GUI needs a graphical\n"
            "desktop and GTK 3 runtime libraries. On Ubuntu 22.04: sudo apt install\n"
            "libgtk-3-0 libxcb-shape0 libxcb-xfixes0 ffmpeg\n"
            "On Ubuntu 24.04 and newer, the GTK package is libgtk-3-0t64 instead.\n"
            "Run from the extracted directory: ./video-toolkit --help\n"
        ),
        "aarch64-apple-darwin": (
            "macOS Apple silicon (arm64), built with a macOS 11.0 deployment target.\n"
            "The package smoke tests run on macOS 15; older versions are unverified.\n"
            "Run from Terminal in the extracted directory: ./video-toolkit --help\n"
        ),
        "x86_64-apple-darwin": (
            "macOS Intel (x86_64), built with a macOS 11.0 deployment target.\n"
            "The package smoke tests run on macOS 15; older versions are unverified.\n"
            "Run from Terminal in the extracted directory: ./video-toolkit --help\n"
        ),
        "x86_64-pc-windows-msvc": (
            "Windows x86_64: extract the ZIP, then open PowerShell in its directory.\n"
            "Run: .\\video-toolkit.exe --help\n"
        ),
    }[target]
    if target.endswith("apple-darwin"):
        platform_note += (
            "This binary is not Developer ID signed or notarized. macOS may block a\n"
            "downloaded executable. Only approve it after verifying its origin and\n"
            "SHA256SUMS; see Apple's guidance for opening apps from identified\n"
            "developers: https://support.apple.com/102445 .\n"
        )
    text = (
        f"Video Toolkit {version}\nTarget: {target}\n\n"
        "No Rust installation is needed to use this prebuilt package.\n"
        "FFmpeg and ffprobe are NOT bundled. Install both and ensure they are on\n"
        "PATH before using video conversion, cutting, slicing or audio merging.\n"
        "Official download guidance: https://ffmpeg.org/download.html\n"
        "Profile management, format listing and native GIF transparency do not\n"
        "need FFmpeg.\n\n"
        + platform_note + "\n"
        f"Launch {binary} with no arguments to open the desktop GUI, or use\n"
        "--help to see the CLI. A graphical desktop is required for the GUI.\n"
        "If you add this directory to PATH, video-toolkit is available elsewhere.\n\n"
        "Verify the archive against the release's SHA256SUMS before extracting.\n"
        "RELEASE.json records the source commit, target and packaged file hashes.\n"
        "Source and documentation: https://github.com/Alaneel/video-toolkit\n"
    )
    return text.encode("utf-8")


def read_regular(path: Path) -> bytes:
    require(path.is_file() and not path.is_symlink(), f"expected a regular file: {path}")
    require(path.stat().st_size <= MAX_MEMBER_SIZE, f"file is too large: {path}")
    return path.read_bytes()


def package(binary: Path, target: str, output_dir: Path, commit: str,
            tag: str | None = None, root: Path = ROOT) -> Path:
    version = root_version(root)
    validate_identity(version, commit)
    validate_tag(tag, version)
    _, _, suffix, binary_name = target_info(target)
    binary_data = read_regular(binary)
    check_architecture(binary_data, target)
    payloads = {
        binary_name: binary_data,
        "README.md": read_regular(root / "README.md"),
        "LICENSE": read_regular(root / "LICENSE"),
        "INSTALL.txt": install_text(version, target),
    }
    manifest = {
        "schema_version": 1,
        "version": version,
        "target": target,
        "commit": commit,
        "binary": binary_name,
        "binary_sha256": digest(binary_data),
        "files": {name: {"sha256": digest(data), "size": len(data)}
                  for name, data in sorted(payloads.items())},
    }
    payloads["RELEASE.json"] = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode()
    folder = f"video-toolkit-{version}-{target}"
    output_dir.mkdir(parents=True, exist_ok=True)
    output = output_dir / archive_name(version, target)
    require(not output.exists(), f"refusing to overwrite existing archive: {output}")
    # Build as a sibling temporary file; only publish the complete archive.
    with tempfile.NamedTemporaryFile(dir=output_dir, suffix=suffix, delete=False) as temp:
        temporary = Path(temp.name)
    try:
        if suffix == ".zip":
            with zipfile.ZipFile(temporary, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                for name, data in sorted(payloads.items()):
                    member = zipfile.ZipInfo(f"{folder}/{name}", (2020, 1, 1, 0, 0, 0))
                    member.create_system = 3
                    member.external_attr = (stat.S_IFREG | (0o755 if name == binary_name else 0o644)) << 16
                    member.compress_type = zipfile.ZIP_DEFLATED
                    archive.writestr(member, data)
        else:
            # The gzip header must not contain the random staging filename or
            # current time; identical inputs should create identical archives.
            with temporary.open("wb") as destination, gzip.GzipFile(
                filename="", mode="wb", fileobj=destination, mtime=0
            ) as compressed, tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                for name, data in sorted(payloads.items()):
                    member = tarfile.TarInfo(f"{folder}/{name}")
                    member.size = len(data)
                    member.mode = 0o755 if name == binary_name else 0o644
                    member.mtime = 0
                    archive.addfile(member, io.BytesIO(data))
        # Validate the archive we just made, rather than only validating inputs.
        verify(temporary, target, version, commit, check_filename=False)
        temporary.replace(output)
    finally:
        temporary.unlink(missing_ok=True)
    return output


def safe_name(name: str) -> None:
    path = PurePosixPath(name)
    require(name and "\\" not in name and not path.is_absolute()
            and all(part not in (".", "..") for part in name.split("/"))
            and "\x00" not in name and ":" not in name,
            f"unsafe archive member path: {name!r}")


def archive_members(path: Path, target: str) -> tuple[dict[str, bytes], dict[str, int]]:
    """Read bounded regular members without extracting or following links."""
    data: dict[str, bytes] = {}
    modes: dict[str, int] = {}
    total = 0
    if target_info(target)[2] == ".zip":
        with zipfile.ZipFile(path) as archive:
            require(len(archive.infolist()) <= 6, "too many archive members")
            for member in archive.infolist():
                safe_name(member.filename)
                require(not member.is_dir(), "unexpected directory member")
                mode = member.external_attr >> 16
                require(stat.S_IFMT(mode) in (0, stat.S_IFREG), "archive contains a link or special file")
                require(not member.flag_bits & 1, "encrypted members are not supported")
                require(member.filename not in data, "duplicate archive member")
                require(0 <= member.file_size <= MAX_MEMBER_SIZE, "archive member is too large")
                total += member.file_size
                require(total <= MAX_MEMBER_SIZE, "archive contents are too large")
                data[member.filename] = archive.read(member)
                modes[member.filename] = mode & 0o7777
    else:
        with tarfile.open(path, "r:gz") as archive:
            members = []
            for member in archive:
                members.append(member)
                require(len(members) <= 6, "too many archive members")
                safe_name(member.name)
                require(member.isreg() and not member.issparse(), "archive contains a link or special file")
                require(member.name not in data, "duplicate archive member")
                require(0 <= member.size <= MAX_MEMBER_SIZE, "archive member is too large")
                total += member.size
                require(total <= MAX_MEMBER_SIZE, "archive contents are too large")
                source = archive.extractfile(member)
                require(source is not None, "could not read archive member")
                with source:
                    data[member.name] = source.read(MAX_MEMBER_SIZE + 1)
                require(len(data[member.name]) == member.size, "archive member size mismatch")
                modes[member.name] = member.mode & 0o7777
    return data, modes


def json_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    output: dict[str, object] = {}
    for key, value in pairs:
        require(key not in output, f"duplicate JSON key: {key}")
        output[key] = value
    return output


def verify(path: Path, expected_target: str, expected_version: str,
           expected_commit: str, run_smoke: bool = False, media_smoke: bool = False,
           check_filename: bool = True) -> dict[str, object]:
    validate_identity(expected_version, expected_commit)
    _, _, suffix, binary = target_info(expected_target)
    if check_filename:
        require(path.name == archive_name(expected_version, expected_target),
                "archive filename does not match expected version and target")
    require(path.is_file() and not path.is_symlink(), "archive must be a regular file")
    require(path.stat().st_size <= MAX_MEMBER_SIZE, "archive is too large")
    contents, modes = archive_members(path, expected_target)
    folder = f"video-toolkit-{expected_version}-{expected_target}"
    names = {binary, "README.md", "LICENSE", "INSTALL.txt", "RELEASE.json"}
    require(set(contents) == {f"{folder}/{name}" for name in names},
            "archive must contain exactly the expected files in one versioned folder")
    files = {name: contents[f"{folder}/{name}"] for name in names}
    if suffix != ".zip":
        for name in names:
            require(modes[f"{folder}/{name}"] == (0o755 if name == binary else 0o644),
                    f"incorrect UNIX permissions for {name}")
    manifest = json.loads(files["RELEASE.json"], object_pairs_hook=json_object)
    require(isinstance(manifest, dict), "release manifest must be a JSON object")
    require(set(manifest) == {"schema_version", "version", "target", "commit", "binary", "binary_sha256", "files"},
            "release manifest has unexpected or missing fields")
    require(type(manifest["schema_version"]) is int and manifest["schema_version"] == 1,
            "unsupported release manifest schema")
    for key, expected in [("version", expected_version), ("target", expected_target),
                          ("commit", expected_commit), ("binary", binary)]:
        require(manifest[key] == expected, f"manifest {key} does not match expected {expected}")
    hashes = manifest["files"]
    require(isinstance(hashes, dict) and set(hashes) == names - {"RELEASE.json"},
            "manifest must hash every payload file")
    for name, record in hashes.items():
        require(isinstance(record, dict) and set(record) == {"sha256", "size"},
                f"invalid file record: {name}")
        require(type(record["size"]) is int and record["size"] == len(files[name]),
                f"payload size mismatch: {name}")
        require(record["sha256"] == digest(files[name]), f"payload SHA256 mismatch: {name}")
    require(manifest["binary_sha256"] == digest(files[binary]), "binary SHA256 mismatch")
    check_architecture(files[binary], expected_target)
    if run_smoke or media_smoke:
        with tempfile.TemporaryDirectory(prefix="video-toolkit-release-") as temporary:
            directory = Path(temporary)
            for name, data in files.items():
                destination = directory / name  # Names already checked against the exact allowlist.
                destination.write_bytes(data)
                destination.chmod(0o755 if name == binary else 0o644)
            smoke(directory / binary, expected_target, expected_version, directory, media_smoke)
    return manifest


def run(args: list[str | Path], directory: Path, env: dict[str, str] | None = None) -> str:
    command = [str(arg) for arg in args]
    result = subprocess.run(command, cwd=directory, env=env, stdin=subprocess.DEVNULL,
                            capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=120)
    require(result.returncode == 0,
            f"command failed ({result.returncode}): {command}\n{result.stdout}\n{result.stderr}")
    return result.stdout


def host_target() -> str | None:
    system, machine = platform.system(), platform.machine().lower()
    if system == "Linux" and machine in ("x86_64", "amd64"):
        return "x86_64-unknown-linux-gnu"
    if system == "Darwin" and machine in ("arm64", "aarch64"):
        return "aarch64-apple-darwin"
    if system == "Darwin" and machine in ("x86_64", "amd64"):
        return "x86_64-apple-darwin"
    if system == "Windows" and machine in ("x86_64", "amd64"):
        return "x86_64-pc-windows-msvc"
    return None


def smoke(binary: Path, target: str, version: str, directory: Path, media: bool) -> None:
    require(host_target() == target, f"cannot execute {target} binary on this host")
    no_media = directory / "empty PATH"
    no_media.mkdir()
    clean_env = os.environ.copy()
    clean_env["PATH"] = str(no_media)
    require(run([binary, "--version"], directory, clean_env).strip() == f"video-toolkit {version}",
            "unpacked executable version does not match manifest")
    require("gif-converter" in run([binary, "--help"], directory, clean_env),
            "unpacked executable help is incomplete")
    formats = run([binary, "formats"], directory, clean_env).lower()
    require(all(name in formats for name in ("mp4", "webm", "mkv", "gif")),
            "format listing is incomplete")
    profile_dir = directory / "isolated profiles"
    cli = [binary, "--profiles-dir", profile_dir]
    profile_name = "release-smoke"
    run(cli + ["profile", "create", profile_name, "--profile-type", "clipper",
               "--params", "ranges=0-1", "format=webm"], directory, clean_env)
    require(profile_name in run(cli + ["profile", "list"], directory, clean_env),
            "created profile is not listed")
    exported = directory / "profile with spaces.json"
    run(cli + ["profile", "export", profile_name, "--profile-type", "clipper", "--output", exported],
        directory, clean_env)
    saved = json.loads(exported.read_bytes())
    imported_dir = directory / "imported profiles"
    imported_cli = [binary, "--profiles-dir", imported_dir]
    run(imported_cli + ["profile", "import", exported], directory, clean_env)
    require(profile_name in run(imported_cli + ["profile", "list"], directory, clean_env),
            "imported profile is not listed")
    require(json.loads((imported_dir / "clipper" / f"{profile_name}.json").read_bytes()) == saved,
            "profile import changed the exported settings")
    # A one-pixel GIF exercises a genuine local operation even with an empty PATH.
    gif = directory / "native transparency.gif"
    original = bytes.fromhex("47494638396101000100800000000000ffffff2c00000000010001000002024401003b")
    gif.write_bytes(original)
    run([binary, "gif-transparency", gif, "--backup"], directory, clean_env)
    require(gif.with_suffix(".gif.bak").read_bytes() == original,
            "native GIF operation did not preserve the backup")
    require(b"\x21\xf9\x04\x01" in gif.read_bytes(), "native GIF operation did not add transparency")
    if media:
        media_smoke(binary, directory)


def probe(path: Path, directory: Path) -> dict[str, object]:
    return json.loads(run(["ffprobe", "-v", "error", "-show_streams", "-show_format",
                           "-of", "json", path], directory))


def media_smoke(binary: Path, directory: Path) -> None:
    require(shutil.which("ffmpeg") is not None and shutil.which("ffprobe") is not None,
            "--media-smoke requires FFmpeg and ffprobe on PATH")
    source = directory / "source with spaces.mp4"
    run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i",
         "color=c=navy:s=160x96:r=10:d=1.2", "-c:v", "libx264", "-threads", "1",
         "-pix_fmt", "yuv420p", source], directory)
    clips = directory / "clips with spaces"
    run([binary, "clipper", source, "--ranges", "0-0.5", "--output-dir", clips, "--format", "mp4"], directory)
    outputs = list(clips.glob("*.mp4"))
    require(len(outputs) == 1, "release clipper did not produce exactly one MP4")
    clip = probe(outputs[0], directory)
    require(float(clip["format"]["duration"]) > 0 and any(
        stream.get("codec_type") == "video" and stream.get("width") == 160
        for stream in clip["streams"]), "release clipper produced invalid media")
    gif = directory / "optimized preview.gif"
    run([binary, "gif-converter", source, "--output", gif, "--width", "80", "--fps", "5",
         "--max-size", "1", "--optimize"], directory)
    preview = probe(gif, directory)
    require(0 < gif.stat().st_size <= 1024 * 1024 and any(
        stream.get("codec_name") == "gif" and 0 < stream.get("width", 0) <= 80
        for stream in preview["streams"]), "release GIF conversion did not honor the target")
    audio = directory / "replacement audio.wav"
    run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i",
         "sine=frequency=880:sample_rate=48000:duration=1", "-c:a", "pcm_s16le", audio], directory)
    merged = directory / "merged with spaces.mp4"
    run([binary, "merger", source, audio, "--output", merged, "--shortest"], directory)
    streams = probe(merged, directory)["streams"]
    require({stream.get("codec_type") for stream in streams} == {"video", "audio"},
            "release merger did not preserve both video and replacement audio")


def checksums(directory: Path, version: str | None = None, expected_commit: str | None = None) -> Path:
    version = version or root_version()
    validate_identity(version, "0" * 40)
    if expected_commit is not None:
        validate_identity(version, expected_commit)
    expected = {archive_name(version, target) for target in TARGETS}
    entries = {path.name for path in directory.iterdir() if path.name != "SHA256SUMS"}
    require(entries == expected, "release directory must contain exactly the four expected platform archives")
    commits = set()
    lines = []
    for target in sorted(TARGETS):
        path = directory / archive_name(version, target)
        # Derive the commit, then independently verify the full manifest and payload.
        contents, _ = archive_members(path, target)
        manifest_path = f"video-toolkit-{version}-{target}/RELEASE.json"
        require(manifest_path in contents, "missing release manifest")
        manifest = json.loads(contents[manifest_path], object_pairs_hook=json_object)
        require(isinstance(manifest, dict) and isinstance(manifest.get("commit"), str), "missing manifest commit")
        commit = manifest["commit"]
        verify(path, target, version, expected_commit or commit)
        commits.add(commit)
        lines.append(f"{digest(read_regular(path))}  {path.name}\n")
    require(len(commits) == 1, "platform archives must come from the same source commit")
    output = directory / "SHA256SUMS"
    require(not output.is_symlink(), "checksum output must not be a symlink")
    output.write_text("".join(sorted(lines)), encoding="utf-8", newline="\n")
    return output


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    version_command = commands.add_parser("version", help="print Cargo version and optionally validate its tag")
    version_command.add_argument("--tag")
    pack = commands.add_parser("package", help="create a verified native release archive")
    pack.add_argument("--binary", type=Path, required=True)
    pack.add_argument("--target", choices=TARGETS, required=True)
    pack.add_argument("--output-dir", type=Path, required=True)
    pack.add_argument("--commit", required=True)
    pack.add_argument("--tag")
    inspect = commands.add_parser("verify", help="verify a release archive and optionally run its binary")
    inspect.add_argument("archive", type=Path)
    inspect.add_argument("--expected-target", choices=TARGETS, required=True)
    inspect.add_argument("--expected-version", required=True)
    inspect.add_argument("--expected-commit", required=True)
    inspect.add_argument("--run-smoke", action="store_true")
    inspect.add_argument("--media-smoke", action="store_true")
    sums = commands.add_parser("checksums", help="verify all four native archives and write SHA256SUMS")
    sums.add_argument("directory", type=Path)
    sums.add_argument("--expected-commit")
    args = parser.parse_args(argv)
    try:
        if args.command == "version":
            version = root_version()
            validate_tag(args.tag, version)
            print(version)
        elif args.command == "package":
            print(package(args.binary, args.target, args.output_dir, args.commit, args.tag))
        elif args.command == "verify":
            manifest = verify(args.archive, args.expected_target, args.expected_version,
                              args.expected_commit, args.run_smoke, args.media_smoke)
            print(f"Verified {args.archive.name}: {manifest['version']} / {manifest['target']} / {manifest['commit']}")
        else:
            print(checksums(args.directory, expected_commit=args.expected_commit))
        return 0
    except (ReleaseError, OSError, json.JSONDecodeError, UnicodeDecodeError, tarfile.TarError,
            zipfile.BadZipFile, subprocess.TimeoutExpired) as error:
        print(f"Release verification failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
