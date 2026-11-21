#!/usr/bin/env python3
"""Collect local dependency notices, not a legal/compliance audit.

Normal generation uses the locked, offline, platform-filtered Cargo graph and
never traverses the Cargo cache. License texts are read only from each resolved
package's own directory, a few named font directories, and explicitly hash-bound
workspace font assets. The explicit
--fetch-upstream option reads only pinned official license texts, not code.
Identical texts appear once by SHA256 to keep release archives small.
The single fixed official-release/source-equivalence proof additionally checks
the exact local archive and installed author/Cargo files; it requires POSIX
no-follow file access and does not authorize binary publication.
"""
from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import io
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import tarfile
import tomllib
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
MAX_NOTICE_BYTES = 8 * 1024 * 1024
MAX_INPUT_BYTES = 8 * 1024 * 1024
PREFIXES = ("license", "licence", "copying", "notice", "copyright")
FONT_CRATES = {"epaint", "epaint_default_fonts", "egui", "eframe"}
FONT_DIRS = ("fonts", "src/fonts", "assets/fonts")
SUPPLEMENT_ROOT = ROOT / "third_party/licenses"
RELEASE_MATCH_KEY = ("system-configuration-sys", "0.5.0")
RELEASE_MATCH_PROOF = {
    "path": "provenance/system-configuration-sys-0.5.0-d464f4f.json",
    "sha256": "dd864e9fb9a3732e0bda906387c7119bd25ebad20373639ccb6a3616ef1471cf",
}
WORKSPACE_FONTS = ({
    "name": "DM Sans",
    "asset": "assets/fonts/DM-Sans.ttf",
    "asset_sha256": "8cd08d97e89c24d0aa92edd2f0f4c8ee6195eee9b7c9f154865a58b02f0c1c0d",
    "license": "assets/fonts/OFL.txt",
    "license_sha256": "9af36190332437f5ecd09974de43c1f7c77a310a996cdd8ceb25628b458840e1",
    "source_record": "assets/fonts/README.md",
    "source_record_sha256": "0284fbc839c5b44100bf04a085adebd8a4cc4fc9e1aa374f58e8cc941bfe15f6",
}, {
    "name": "DM Sans SemiBold (derived static instance)",
    "asset": "assets/fonts/DM-Sans-SemiBold.ttf",
    "asset_sha256": "10b569ed19b058b0d7fd5cbeaec1224a745cba85d2e3cf9cb2b9c44430c420ec",
    "license": "assets/fonts/OFL.txt",
    "license_sha256": "9af36190332437f5ecd09974de43c1f7c77a310a996cdd8ceb25628b458840e1",
    "source_record": "assets/fonts/README.md",
    "source_record_sha256": "0284fbc839c5b44100bf04a085adebd8a4cc4fc9e1aa374f58e8cc941bfe15f6",
}, {
    "name": "DM Sans Bold (derived static instance)",
    "asset": "assets/fonts/DM-Sans-Bold.ttf",
    "asset_sha256": "9b73b14c419afcf87569069635a797558a78a425bdf8c235458720a4c4fe73cb",
    "license": "assets/fonts/OFL.txt",
    "license_sha256": "9af36190332437f5ecd09974de43c1f7c77a310a996cdd8ceb25628b458840e1",
    "source_record": "assets/fonts/README.md",
    "source_record_sha256": "0284fbc839c5b44100bf04a085adebd8a4cc4fc9e1aa374f58e8cc941bfe15f6",
})


class NoticeError(Exception):
    pass


def command(args: list[str]) -> bytes:
    """Never echo stderr, which can contain private installation paths."""
    try:
        result = subprocess.run(args, cwd=ROOT, stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                check=False, timeout=120)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise NoticeError(f"Could not complete local {args[0]} probe") from error
    if result.returncode:
        raise NoticeError(f"Local {args[0]} probe failed; no private paths were logged")
    return result.stdout


def relative_file(path: Path, base: Path) -> str:
    try:
        return path.resolve().relative_to(base.resolve()).as_posix()
    except ValueError as error:
        raise NoticeError("A notice file escapes its package directory") from error


def read_notice(path: Path, base: Path) -> bytes:
    relative_file(path, base)
    if path.is_symlink() or not path.is_file():
        raise NoticeError("A notice is not a regular, non-symlink file")
    if path.stat().st_size > MAX_INPUT_BYTES:
        raise NoticeError("A notice exceeds the per-file size limit")
    content = path.read_bytes()
    try:
        content.decode("utf-8")
    except UnicodeDecodeError as error:
        raise NoticeError("A notice is not UTF-8 text; manual review is needed") from error
    if b"\x00" in content:
        raise NoticeError("A notice contains binary data; manual review is needed")
    return content


def matching_files(directory: Path) -> list[Path]:
    if not directory.is_dir() or directory.is_symlink():
        return []
    return sorted((path for path in directory.iterdir()
                   if path.name.lower().startswith(PREFIXES) and path.is_file()),
                  key=lambda path: path.name)


def workspace_font_records(root: Path) -> list[dict]:
    """Bind the embedded asset, original license bytes and recorded source.

    These hashes identify the checked-in bytes, not a signed upstream attestation.
    No font binary is added to the notice text or copied into an archive here.
    """
    root = root.resolve(strict=True)
    records = []
    for descriptor in WORKSPACE_FONTS:
        record = dict(descriptor)
        for field in ("asset", "license", "source_record"):
            path = root
            for part in Path(descriptor[field]).parts:
                path /= part
                if path.is_symlink():
                    raise NoticeError("Workspace font files must not use symlinks")
            relative_file(path, root)
            if not path.is_file() or path.stat().st_size > MAX_INPUT_BYTES:
                raise NoticeError("Workspace font file is missing, nonregular or oversized")
            content = path.read_bytes() if field == "asset" else read_notice(path, root)
            if len(content) > MAX_INPUT_BYTES or hashlib.sha256(content).hexdigest() != descriptor[f"{field}_sha256"]:
                raise NoticeError("Workspace font asset, license or source-record SHA256 changed")
            record[f"{field}_bytes"] = content
        records.append(record)
    return records


def dependency_closure(metadata: dict) -> list[dict]:
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    root_manifest = (ROOT / "Cargo.toml").resolve()
    root_id = next((key for key, package in packages.items()
                    if Path(package["manifest_path"]).resolve() == root_manifest), None)
    if root_id is None:
        raise NoticeError("Could not identify this release's root Cargo package")
    pending = [root_id]
    seen: set[str] = set()
    while pending:
        package_id = pending.pop()
        if package_id in seen:
            continue
        seen.add(package_id)
        for dependency in nodes[package_id].get("deps", []):
            # Cargo --filter-platform has resolved platform cfg conditions.
            # Build dependencies are retained conservatively; dev-only edges
            # are not part of this release inventory.
            if any(kind.get("kind") != "dev" for kind in dependency["dep_kinds"]):
                pending.append(dependency["pkg"])
    return sorted((packages[key] for key in seen),
                  key=lambda package: (package["name"], package["version"]))


def missing_upstream_sources(target: str) -> list[dict]:
    metadata = json.loads(command(["cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--filter-platform", target]))
    workspace = set(metadata["workspace_members"])
    sources = []
    for package in dependency_closure(metadata):
        if package["id"] in workspace:
            continue
        base = Path(package["manifest_path"]).parent
        if matching_files(base) or package.get("license_file"):
            continue
        repository = urlparse(package.get("repository") or "")
        parts = repository.path.strip("/").split("/")
        if repository.hostname != "github.com" or repository.username or len(parts) < 2:
            continue
        owner, repo = parts[:2]
        repo = repo.removesuffix(".git")
        if not all(re.fullmatch(r"[A-Za-z0-9_.-]+", part) for part in (owner, repo)):
            continue
        try:
            vcs = json.loads((base / ".cargo_vcs_info.json").read_text("utf-8"))
        except (OSError, ValueError):
            continue
        commit = vcs.get("git", {}).get("sha1", "")
        if not re.fullmatch(r"[a-f0-9]{40}", commit) or vcs.get("git", {}).get("dirty"):
            continue
        sources.append({"crate": package["name"], "version": package["version"], "spdx": package.get("license"),
                        "repository": f"https://github.com/{owner}/{repo}", "commit": commit,
                        "contents_url": f"https://api.github.com/repos/{owner}/{repo}/contents/?ref={commit}"})
    return sources


def original_comment_header(source: bytes) -> bytes:
    """Only the leading contiguous copyright comment, never Rust code/docs."""
    lines = source.splitlines(keepends=True)
    selected = []
    for line in lines:
        if not line.startswith(b"//") or line.startswith((b"//!", b"///")):
            break
        selected.append(line)
    notice = b"".join(selected)
    if b"copyright" not in notice.lower() or b"licens" not in notice.lower():
        return b""
    return notice


def git_blob_sha(content: bytes) -> str:
    return hashlib.sha1(b"blob " + str(len(content)).encode("ascii") + b"\0" + content).hexdigest()


def bounded_regular_bytes(path: Path, base: Path) -> bytes:
    """Read a named local proof/source/archive, never links or special files."""
    if os.name != "posix" or not hasattr(os, "O_NOFOLLOW") or not hasattr(os, "O_NONBLOCK"):
        raise NoticeError("Official release-source verification requires POSIX no-follow file access")
    relative_file(path, base)
    current = Path(path.absolute().anchor)
    for part in path.absolute().parts[1:]:
        current /= part
        if current.is_symlink():
            raise NoticeError("Release-source proof files must not use symlinks")
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        before = os.fstat(stream.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_size > MAX_INPUT_BYTES:
            raise NoticeError("Release-source proof input is nonregular or oversized")
        content = stream.read(MAX_INPUT_BYTES + 1)
        after = os.fstat(stream.fileno())
    identity = lambda info: (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns)
    if identity(before) != identity(after) or len(content) != before.st_size:
        raise NoticeError("Release-source proof input changed while read")
    return content


def release_source_proof(entry: dict) -> dict:
    """One reviewed exception, not a general path/commit/license substitution API."""
    if (entry.get("crate"), entry.get("version")) != RELEASE_MATCH_KEY or entry.get("provenance") != "official_release_source_match":
        raise NoticeError("Unsupported official release-source match")
    if entry.get("source_match_proof") != RELEASE_MATCH_PROOF:
        raise NoticeError("Official release-source proof path or SHA256 changed")
    content = bounded_regular_bytes(SUPPLEMENT_ROOT / RELEASE_MATCH_PROOF["path"], SUPPLEMENT_ROOT)
    if hashlib.sha256(content).hexdigest() != RELEASE_MATCH_PROOF["sha256"]:
        raise NoticeError("Pinned official release-source proof SHA256 changed")
    proof = json.loads(content)
    snapshot = proof["official_tree_snapshot"]
    tree = {item["path"]: item for item in snapshot["tree"]}
    if (snapshot["sha"] != proof["official_tree_sha"] or snapshot["truncated"] is not False
            or len(tree) != len(snapshot["tree"])):
        raise NoticeError("Official release tree snapshot is incomplete or ambiguous")
    author_paths = {item["upstream_path"] for item in proof["author_files"]}
    tree_author_paths = {path for path, item in tree.items()
                         if path.startswith(proof["upstream_directory"] + "/") and item["type"] == "blob"}
    if author_paths != tree_author_paths:
        raise NoticeError("Official release tree author-file set does not match the proof")
    for item in proof["author_files"] + proof["license_files"]:
        original = tree.get(item.get("upstream_path", item.get("source_path")))
        if (not original or original["type"] != "blob" or original["mode"] != "100644"
                or original["size"] != item["size"] or original["sha"] != item["git_blob_sha"]):
            raise NoticeError("Official release file does not match its original Git tree entry")
    if any(entry.get(field) != proof.get(field) for field in ("crate", "version", "repository", "spdx", "packaged_commit_status")):
        raise NoticeError("Official release-source entry metadata changed")
    if entry.get("commit") != proof["packaged_vcs_commit"] or entry.get("files") != proof["license_files"] or entry.get("unresolved"):
        raise NoticeError("Official release-source entry changed its packaged commit or license binding")
    return proof


def release_archive_members(content: bytes, proof: dict) -> dict[str, bytes]:
    """The pinned crate has exactly nine author files and two Cargo-generated files."""
    expected = {item["archive_path"]: item for item in proof["author_files"]}
    generated = set(proof["excluded_packaging_generated_files"])
    prefix = f"{proof['crate']}-{proof['version']}/"
    contents = {}
    try:
        with tarfile.open(fileobj=io.BytesIO(content), mode="r:gz") as bundle:
            members = bundle.getmembers()
            if len(members) != len(expected) + len(generated):
                raise NoticeError("Official release archive member set changed")
            for member in members:
                if not member.name.startswith(prefix):
                    raise NoticeError("Official release archive member escapes its package")
                relative = member.name[len(prefix):]
                if (relative not in expected.keys() | generated or relative in contents
                        or not member.isfile() or member.size > MAX_INPUT_BYTES):
                    raise NoticeError("Official release archive has extra, duplicate or unsafe members")
                stream = bundle.extractfile(member)
                if stream is None:
                    raise NoticeError("Official release archive member is unreadable")
                data = stream.read(MAX_INPUT_BYTES + 1)
                if len(data) != member.size:
                    raise NoticeError("Official release archive member size changed")
                contents[relative] = data
    except tarfile.TarError as error:
        raise NoticeError("Official release archive is invalid") from error
    if contents.keys() != expected.keys() | generated:
        raise NoticeError("Official release archive author-file set changed")
    for relative, item in expected.items():
        data = contents[relative]
        if (len(data) != item["size"] or hashlib.sha256(data).hexdigest() != item["sha256"]
                or git_blob_sha(data) != item["git_blob_sha"]):
            raise NoticeError("Official release archive author-file fingerprint changed")
    for name in ("Cargo.toml.orig", "Cargo.toml"):
        package = tomllib.loads(contents[name].decode("utf-8"))["package"]
        if any(package.get(field) != proof[expected_field] for field, expected_field in
               (("name", "crate"), ("version", "version"), ("repository", "repository"),
                ("license", "spdx"), ("authors", "authors"))):
            raise NoticeError("Official release original or generated Cargo metadata changed")
    vcs = json.loads(contents[".cargo_vcs_info.json"])
    if vcs != {"git": {"sha1": proof["packaged_vcs_commit"]}, "path_in_vcs": proof["packaged_vcs_path"]}:
        raise NoticeError("Official release packaged VCS metadata changed")
    return contents


def verify_release_source_match(package: dict, locked_checksum: str, entry: dict) -> tuple[dict, list[tuple[dict, bytes]]]:
    proof = release_source_proof(entry)
    if any(package.get(field) != proof[expected] for field, expected in
           (("name", "crate"), ("version", "version"), ("repository", "repository"),
            ("license", "spdx"), ("authors", "authors"))):
        raise NoticeError("Resolved release-source package identity changed")
    base = Path(package["manifest_path"]).parent
    if base.name != "system-configuration-sys-0.5.0" or base.parent.parent.name != "src":
        raise NoticeError("Official release match requires the resolved registry package directory")
    archive = base.parent.parent.parent / "cache" / base.parent.name / "system-configuration-sys-0.5.0.crate"
    content = bounded_regular_bytes(archive, archive.parent)
    if locked_checksum != proof["package_sha256"] or hashlib.sha256(content).hexdigest() != locked_checksum:
        raise NoticeError("Official release cached archive does not match the pinned Cargo.lock checksum")
    archived_files = release_archive_members(content, proof)
    root_names = {name.split("/", 1)[0] for name in archived_files}
    marker_names = set(proof["excluded_installed_cargo_markers"])
    children = {child.name: child for child in base.iterdir()}
    if not root_names <= children.keys() or children.keys() - root_names - marker_names:
        raise NoticeError("Installed release package file set changed")
    source_dir = base / "src"
    if source_dir.is_symlink() or not source_dir.is_dir():
        raise NoticeError("Installed release source directory is unsafe")
    source_names = {name[len("src/"):] for name in archived_files if name.startswith("src/")}
    if {child.name for child in source_dir.iterdir()} != source_names:
        raise NoticeError("Installed release author-file set changed")
    for name in children.keys() & marker_names:
        bounded_regular_bytes(base / name, base)
    for relative, data in archived_files.items():
        if bounded_regular_bytes(base / relative, base) != data:
            raise NoticeError("Installed release author or Cargo packaging file differs from the locked archive")
    installed_vcs = json.loads(bounded_regular_bytes(base / ".cargo_vcs_info.json", base))
    if installed_vcs != {"git": {"sha1": proof["packaged_vcs_commit"]}, "path_in_vcs": proof["packaged_vcs_path"]}:
        raise NoticeError("Installed release packaged VCS metadata changed")
    licenses = []
    for item in proof["license_files"]:
        expected_url = f"https://raw.githubusercontent.com/mullvad/system-configuration-rs/{proof['pinned_official_commit']}/{item['source_path']}"
        if item["source_url"] != expected_url or item["path"] != f"texts/{item['sha256']}.txt":
            raise NoticeError("Official release license URL or text path changed")
        data = bounded_regular_bytes(SUPPLEMENT_ROOT / item["path"], SUPPLEMENT_ROOT)
        if (len(data) != item["size"] or hashlib.sha256(data).hexdigest() != item["sha256"]
                or git_blob_sha(data) != item["git_blob_sha"]):
            raise NoticeError("Official release full license text fingerprint changed")
        licenses.append((item, data))
    return proof, licenses


def verify_current_gap_views(manifest: dict, completed: set[tuple[str, str]],
                             shipped: set[tuple[str, str]] | None = None) -> int:
    """Check current gaps against preserved history.

    A historical gap is accounted for when its text was recovered (`completed`)
    or when that exact crate version is no longer in the shipped dependency
    closure (`shipped`). Retired gaps keep their history but need no re-proof.
    """
    historical = manifest.get("historical_unresolved_review")
    if historical is None:
        return len(manifest.get("unresolved_review", {}).get("items", []))
    original = {(item["crate"], item["version"]) for item in historical["items"]}
    retired = original - shipped if shipped is not None else set()
    if completed & retired:
        raise NoticeError("A gap cannot be both completed and retired")
    completed = completed | retired
    remaining = original - completed
    current = {(item["crate"], item["version"]) for item in manifest["unresolved_review"]["items"]}
    cached = manifest["cached_source_review"]
    original_cached = manifest["historical_cached_source_review"]
    reviewed = {(item["crate"], item["version"]) for item in cached["items"]}
    if (len(historical["items"]) != 6 or len(cached["items"]) != 6 or original != reviewed
            or len(manifest["unresolved_review"]["items"]) != len(remaining)
            or original_cached.get("full_license_gaps_remaining") != 6
            or cached["items"] != original_cached["items"]
            or cached["source_header_notices"] != original_cached["source_header_notices"]
            or cached.get("historical_full_license_gaps_at_review") != 6 or current != remaining
            or cached.get("full_license_gaps_remaining") != len(remaining)):
        raise NoticeError("Current original-license gaps do not match preserved history and verified completions")
    historical_entries = manifest.get("historical_exact_commit_entries")
    if not isinstance(historical_entries, list) or len(historical_entries) != 1:
        raise NoticeError("Original unreachable packaged-commit entry was not preserved")
    old_entry = historical_entries[0]
    if (old_entry.get("crate"), old_entry.get("version")) != RELEASE_MATCH_KEY or old_entry.get("files") != [] or old_entry.get("commit") != "592a485494be9d63a60e54944dd2fcc4f077ec58" or not old_entry.get("unresolved"):
        raise NoticeError("Original unreachable packaged-commit fact changed")
    expected_current = [item for item in historical["items"] if (item["crate"], item["version"]) not in completed]
    if manifest["unresolved_review"]["items"] != expected_current:
        raise NoticeError("Current gap details differ from preserved historical facts")
    return len(remaining)


def verify_source_notice(base: Path, package_checksum: str, descriptor: dict) -> bytes:
    """A source header has a different role from a full LICENSE text."""
    if descriptor.get("role") != "copyright_and_license_reference_only" or descriptor.get("resolves_full_license_gap") is not False:
        raise NoticeError("Source copyright notices must not masquerade as full license text")
    source_path = descriptor.get("source_path", "")
    if source_path != "src/lib.rs" or descriptor.get("line_start") != 1:
        raise NoticeError("Unsupported source-notice extraction location")
    if descriptor.get("package_sha256") != package_checksum:
        raise NoticeError("Source notice does not match the locked crate archive checksum")
    source = read_notice(base / source_path, base)
    if hashlib.sha256(source).hexdigest() != descriptor.get("source_sha256"):
        raise NoticeError("Source notice provenance SHA256 changed")
    header = original_comment_header(source)
    digest = hashlib.sha256(header).hexdigest()
    if not header or descriptor.get("line_end") != len(header.splitlines()) or descriptor.get("sha256") != digest:
        raise NoticeError("Source notice is not the exact original leading comment")
    if descriptor.get("path") != f"headers/{digest}.txt":
        raise NoticeError("Source notice text path/hash is invalid")
    stored = read_notice(SUPPLEMENT_ROOT / descriptor["path"], SUPPLEMENT_ROOT)
    if stored != header:
        raise NoticeError("Source notice text differs from the source comment")
    return stored


def inspect_cached_source_headers(target: str) -> dict:
    """Bounded offline follow-up for the six already documented unresolved crates."""
    manifest_path = SUPPLEMENT_ROOT / "manifest.json"
    manifest = json.loads(read_notice(manifest_path, SUPPLEMENT_ROOT))
    items = manifest.get("historical_unresolved_review", manifest.get("unresolved_review", {})).get("items", [])
    if len(items) != 6:
        raise NoticeError("Cached-source review is limited to the six documented gaps")
    metadata = json.loads(command(["cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--filter-platform", target]))
    packages = {(package["name"], package["version"]): package for package in dependency_closure(metadata)}
    locked = {(package["name"], package["version"]): package.get("checksum") for package in tomllib.loads((ROOT / "Cargo.lock").read_text("utf-8"))["package"]}
    findings = []
    notices = []
    pattern = re.compile(rb"\b(copyright|permission\s+is\s+hereby|license|licence|warranty|MIT)\b", re.I)
    for item in items:
        key = (item["crate"], item["version"])
        package = packages[key]
        base = Path(package["manifest_path"]).parent
        archive = base.parent.parent.parent / "cache" / base.parent.name / f"{key[0]}-{key[1]}.crate"
        if archive.is_symlink() or not archive.is_file() or archive.stat().st_size > MAX_INPUT_BYTES:
            raise NoticeError("Cached crate archive is unavailable or unsafe")
        content = archive.read_bytes()
        checksum = hashlib.sha256(content).hexdigest()
        if len(content) > MAX_INPUT_BYTES or checksum != locked[key]:
            raise NoticeError("A cached unresolved crate archive does not match Cargo.lock")
        files = []
        hits = []
        packaged_metadata = None
        with tarfile.open(archive, "r:gz") as bundle:
            members = bundle.getmembers()
            if len(members) > 512:
                raise NoticeError("Cached crate review exceeds member limit")
            prefix = f"{key[0]}-{key[1]}/"
            original_manifest = next((entry for entry in members if entry.name == prefix + "Cargo.toml.orig"), None)
            if original_manifest is None:
                original_manifest = next(entry for entry in members if entry.name == prefix + "Cargo.toml")
            if not original_manifest.isfile() or original_manifest.size > MAX_INPUT_BYTES:
                raise NoticeError("Cached package metadata is not a bounded regular file")
            packaged = tomllib.loads(bundle.extractfile(original_manifest).read().decode("utf-8"))["package"]
            packaged_metadata = {field: packaged.get(field) for field in ("name", "version", "license", "authors", "repository", "license-file")}
            for member in sorted(members, key=lambda value: value.name):
                relative = member.name.removeprefix(prefix)
                if not member.name.startswith(prefix) or not relative.startswith("src/") or not relative.endswith(".rs"):
                    continue
                if not member.isfile() or member.size > MAX_INPUT_BYTES or ".." in Path(relative).parts:
                    raise NoticeError("Cached source member is unsafe")
                source = bundle.extractfile(member).read()
                source_hash = hashlib.sha256(source).hexdigest()
                if hashlib.sha256(read_notice(base / relative, base)).hexdigest() != source_hash:
                    raise NoticeError("Installed source differs from the locked cached crate")
                files.append({"path": relative, "sha256": source_hash, "line_count": len(source.splitlines())})
                for number, line in enumerate(source.splitlines(), 1):
                    if pattern.search(line):
                        hits.append({"path": relative, "line": number, "comment_line": line.lstrip().startswith(b"//")})
                if key == ("system-configuration-sys", "0.5.0") and relative == "src/lib.rs":
                    header = original_comment_header(source)
                    if len(header.splitlines()) != 7:
                        raise NoticeError("Expected original seven-line copyright header changed")
                    digest = hashlib.sha256(header).hexdigest()
                    descriptor = {"crate": key[0], "version": key[1], "role": "copyright_and_license_reference_only", "resolves_full_license_gap": False,
                                  "source_path": relative, "source_sha256": source_hash, "package_sha256": checksum, "line_start": 1, "line_end": 7,
                                  "sha256": digest, "path": f"headers/{digest}.txt"}
                    path = SUPPLEMENT_ROOT / descriptor["path"]
                    path.parent.mkdir(exist_ok=True)
                    if path.is_symlink():
                        raise NoticeError("Refusing symlink source notice")
                    path.write_bytes(header)
                    verify_source_notice(base, checksum, descriptor)
                    notices.append(descriptor)
        findings.append({"crate": key[0], "version": key[1], "package_sha256": checksum, "cargo_lock_checksum_matches": True,
                         "installed_sources_match_cached_archive": True, "packaged_metadata": packaged_metadata, "source_files": files, "legal_term_hit_locations": hits,
                         "finding": "Copyright and SPDX-license-reference comment preserved; full MIT grant/warranty text not found" if hits else "No copyright/license/MIT grant/warranty text found in any packaged src/*.rs module"})
    completed = set()
    for entry in manifest["entries"]:
        if entry.get("provenance") == "official_release_source_match":
            key = (entry["crate"], entry["version"])
            verify_release_source_match(packages[key], locked[key], entry)
            completed.add(key)
    remaining = len(items) - len(completed)
    manifest["cached_source_review"] = {"scope": "Historical six-crate offline source/header inspection; author-source facts are preserved separately from current full-text completions. No code was executed or copied into the project",
                                       "historical_full_license_gaps_at_review": 6,
                                       "full_license_gaps_remaining": remaining, "items": findings, "source_header_notices": notices}
    verify_current_gap_views(manifest, completed)
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return {"reviewed_crates": len(findings), "source_files": sum(len(item["source_files"]) for item in findings), "source_header_notices": len(notices), "full_license_gaps_remaining": remaining}


def fetch_upstream(target: str) -> dict:
    """Explicit opt-in acquisition of license TEXT only, never source code."""
    sources = missing_upstream_sources(target)
    groups: dict[tuple[str, str], list[dict]] = {}
    for source in sources:
        groups.setdefault((source["repository"], source["commit"]), []).append(source)

    def fetch_group(item: tuple[tuple[str, str], list[dict]]) -> tuple[list[dict], list[tuple[dict, bytes]]]:
        (repository, commit), crates = item
        slug = urlparse(repository).path.strip("/")
        try:
            listing = json.loads(command(["gh", "api", f"repos/{slug}/contents/?ref={commit}"]))
            if not isinstance(listing, list):
                raise NoticeError("Upstream license directory listing is invalid")
            files = []
            for entry in listing:
                name = entry.get("name", "")
                if entry.get("type") != "file" or not name.lower().startswith(PREFIXES):
                    continue
                if not re.fullmatch(r"[A-Za-z0-9_.-]+", name) or entry.get("size", MAX_INPUT_BYTES + 1) > MAX_INPUT_BYTES:
                    continue
                content = command(["gh", "api", "-H", "Accept: application/vnd.github.raw+json", f"repos/{slug}/contents/{name}?ref={commit}"])
                if len(content) > MAX_INPUT_BYTES or b"\x00" in content:
                    raise NoticeError("Upstream license text exceeds bounds or is binary")
                content.decode("utf-8")
                digest = hashlib.sha256(content).hexdigest()
                descriptor = {"path": f"texts/{digest}.txt", "sha256": digest,
                              "source_path": name, "source_url": f"https://raw.githubusercontent.com/{slug}/{commit}/{name}"}
                files.append((descriptor, content))
            return crates, files
        except (NoticeError, ValueError, KeyError):
            return crates, []

    SUPPLEMENT_ROOT.mkdir(parents=True, exist_ok=True)
    (SUPPLEMENT_ROOT / "texts").mkdir(exist_ok=True)
    manifest = {"schema_version": 1, "target": target, "provenance": "Exact commit IDs from each installed crate's .cargo_vcs_info.json; no moving branches", "entries": []}
    unique_texts = set()
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
        for crates, files in executor.map(fetch_group, sorted(groups.items())):
            for descriptor, content in files:
                path = SUPPLEMENT_ROOT / descriptor["path"]
                if path.is_symlink():
                    raise NoticeError("Refusing a symlink supplemental license file")
                path.write_bytes(content)
                unique_texts.add(descriptor["sha256"])
            for source in crates:
                entry = {key: value for key, value in source.items() if key != "contents_url"}
                entry["files"] = [descriptor for descriptor, _ in files]
                if not files:
                    entry["unresolved"] = "No readable code license found at exact upstream commit; manual review required"
                manifest["entries"].append(entry)
    manifest["entries"].sort(key=lambda entry: (entry["crate"], entry["version"]))
    manifest_path = SUPPLEMENT_ROOT / "manifest.json"
    if manifest_path.is_symlink():
        raise NoticeError("Refusing a symlink supplemental manifest")
    if manifest_path.is_file():
        previous = json.loads(read_notice(manifest_path, SUPPLEMENT_ROOT))
        if previous.get("target") == target:
            for key in ("unresolved_review", "cached_source_review", "historical_unresolved_review", "historical_cached_source_review", "historical_exact_commit_entries"):
                if key in previous:
                    manifest[key] = previous[key]
            prior = {(entry["crate"], entry["version"]): entry for entry in previous["entries"]}
            for index, entry in enumerate(manifest["entries"]):
                preserved = prior.get((entry["crate"], entry["version"]))
                if preserved and preserved.get("provenance") == "official_release_source_match":
                    release_source_proof(preserved)  # Reject altered proof, do not silently demote it.
                    manifest["entries"][index] = preserved
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return {"exact_commit_groups": len(groups), "crate_entries": len(manifest["entries"]), "texts": len(unique_texts),
            "unresolved_entries": sum(not entry["files"] for entry in manifest["entries"])}


def collect(target: str) -> tuple[bytes, dict]:
    metadata = json.loads(command(["cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--filter-platform", target]))
    packages = dependency_closure(metadata)
    resolved_nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    workspace_ids = set(metadata["workspace_members"])
    texts: dict[str, bytes] = {}
    inventory: list[str] = []
    unresolved: list[str] = []
    font_count = 0
    third_party_count = 0
    supplemental = {}
    source_notices = {}
    manifest = {}
    completed_release_matches = set()
    manifest_path = SUPPLEMENT_ROOT / "manifest.json"
    if manifest_path.is_file():
        manifest = json.loads(read_notice(manifest_path, SUPPLEMENT_ROOT))
        if manifest.get("schema_version") != 1 or manifest.get("target") != target:
            raise NoticeError("Supplemental license manifest schema or target does not match")
        supplemental = {(entry["crate"], entry["version"]): entry for entry in manifest["entries"]}
        source_notices = {(entry["crate"], entry["version"]): entry for entry in manifest.get("cached_source_review", {}).get("source_header_notices", [])}
    locked_checksums = {(package["name"], package["version"]): package.get("checksum") for package in tomllib.loads((ROOT / "Cargo.lock").read_text("utf-8"))["package"]}
    exact_sources = {(source["crate"], source["version"]): source for source in missing_upstream_sources(target)}

    def include(path: Path, base: Path, label: str) -> str | None:
        try:
            content = read_notice(path, base)
            digest = hashlib.sha256(content).hexdigest()
            texts.setdefault(digest, content)
            return f"  {label}: {relative_file(path, base)} -> TEXT SHA256 {digest}"
        except (NoticeError, OSError):
            unresolved.append(f"{label}: local notice unavailable or unsafe; manual review required")
            return None

    inventory.append("FIRST-PARTY WORKSPACE\n" + "\n".join(
        f"  {package['name']} {package['version']} (covered by separately shipped project LICENSE)"
        for package in packages if package["id"] in workspace_ids))

    for font in workspace_font_records(ROOT):
        texts.setdefault(font["license_sha256"], font["license_bytes"])
        inventory.append("\n".join([
            f"WORKSPACE EMBEDDED FONT {font['name']}",
            f"  Asset: {font['asset']} -> SHA256 {font['asset_sha256']} ({len(font['asset_bytes'])} bytes)",
            f"  Source record: {font['source_record']} -> SHA256 {font['source_record_sha256']}",
            "  Source URL/retrieval date are recorded locally; byte hashes are not an upstream signature or pinned-commit claim.",
            f"  Original font copyright/license: {font['license']} -> TEXT SHA256 {font['license_sha256']}",
        ]))
        font_count += 1

    for package in packages:
        if package["id"] in workspace_ids:
            continue
        third_party_count += 1
        name = f"{package['name']} {package['version']}"
        base = Path(package["manifest_path"]).parent
        license_expression = package.get("license") or "NOT DECLARED"
        repository = package.get("repository") or "NOT DECLARED"
        # Repository metadata is upstream public source attribution; remove
        # URL userinfo defensively if a private source ever includes it.
        if "://" in repository and "@" in repository.split("://", 1)[1].split("/", 1)[0]:
            repository = "REDACTED URL USERINFO; inspect manifest locally"
        lines = [f"CRATE {name}", f"  SPDX declared by upstream: {license_expression}", f"  Repository: {repository}"]
        if package.get("authors"):
            lines.append("  Declared authors: " + "; ".join(package["authors"]))
        candidates = matching_files(base)
        declared_file = package.get("license_file")
        if declared_file:
            candidate = Path(declared_file)
            if not candidate.is_absolute():
                candidate = base / candidate
            candidates.append(candidate)
        seen_files: set[Path] = set()
        code_notices = 0
        for path in candidates:
            if path in seen_files:
                continue
            seen_files.add(path)
            entry = include(path, base, f"{name} code notice")
            if entry:
                lines.append(entry)
                code_notices += 1
        supplement = supplemental.get((package["name"], package["version"]))
        if supplement and (supplement.get("provenance") == "official_release_source_match" or "source_match_proof" in supplement):
            proof, original_licenses = verify_release_source_match(
                package, locked_checksums.get((package["name"], package["version"])), supplement)
            completed_release_matches.add((package["name"], package["version"]))
            lines.extend([
                "  Full original texts recovered by official_release_source_match (not a legal/compliance approval).",
                f"  Packaged .cargo_vcs_info.json commit: {proof['packaged_vcs_commit']} (unreachable at declared repository; historical failure preserved).",
                f"  Separate fixed official version-bump commit: {proof['pinned_official_commit']} (NOT the packaged VCS commit; unsigned).",
                f"  Official author date: {proof['official_author_date']}; committer date: {proof['official_committer_date']}.",
                f"  Exact locked .crate SHA256: {proof['package_sha256']}; 9/9 author files match archive and installed source by bytes, SHA256 and Git blob SHA.",
                f"  Restricted source-equivalence proof: {RELEASE_MATCH_PROOF['path']} -> SHA256 {RELEASE_MATCH_PROOF['sha256']}",
            ])
            for descriptor, content in original_licenses:
                texts.setdefault(descriptor["sha256"], content)
                lines.append(f"  Official release-source code notice: {descriptor['source_url']} -> TEXT SHA256 {descriptor['sha256']}")
                code_notices += 1
        elif not code_notices and supplement:
            expected_source = exact_sources.get((package["name"], package["version"]))
            if expected_source and all(supplement.get(key) == expected_source.get(key) for key in ("repository", "commit")):
                for descriptor in supplement.get("files", []):
                    digest = descriptor.get("sha256", "")
                    if not re.fullmatch(r"[a-f0-9]{64}", digest) or descriptor.get("path") != f"texts/{digest}.txt":
                        raise NoticeError("Supplemental license path/hash is invalid")
                    source_path = descriptor.get("source_path", "")
                    if not re.fullmatch(r"[A-Za-z0-9_.-]+", source_path) or not source_path.lower().startswith(PREFIXES):
                        raise NoticeError("Supplemental file is not an approved code-license filename")
                    slug = urlparse(supplement["repository"]).path.strip("/")
                    expected_url = f"https://raw.githubusercontent.com/{slug}/{supplement['commit']}/{source_path}"
                    if descriptor.get("source_url") != expected_url:
                        raise NoticeError("Supplemental license URL is not pinned to its exact official commit")
                    content = read_notice(SUPPLEMENT_ROOT / descriptor["path"], SUPPLEMENT_ROOT)
                    if hashlib.sha256(content).hexdigest() != digest:
                        raise NoticeError("Supplemental license content SHA256 changed")
                    texts.setdefault(digest, content)
                    lines.append(f"  Exact upstream commit: {supplement['commit']} (from packaged .cargo_vcs_info.json)")
                    lines.append(f"  Supplement code notice: {expected_url} -> TEXT SHA256 {digest}")
                    code_notices += 1
            else:
                unresolved.append(f"{name}: supplemental license does not match the packaged upstream commit")
        source_notice = source_notices.get((package["name"], package["version"]))
        if source_notice:
            original = verify_source_notice(base, locked_checksums.get((package["name"], package["version"])), source_notice)
            texts.setdefault(source_notice["sha256"], original)
            lines.append(f"  Original source copyright/reference notice (NOT complete license terms): {source_notice['source_path']} lines {source_notice['line_start']}-{source_notice['line_end']} -> TEXT SHA256 {source_notice['sha256']}")
            lines.append(f"  Source file SHA256: {source_notice['source_sha256']}; locked .crate SHA256: {source_notice['package_sha256']}")
            lines.append("  This snippet preserves attribution but does not resolve the missing full-license-text gap.")
        if not code_notices:
            unresolved.append(f"{name}: no readable crate-root LICENSE/LICENCE/COPYING/NOTICE/COPYRIGHT or declared license_file found")
            lines.append("  UNRESOLVED: upstream code license text is absent from this installed crate package")
        if not package.get("license") and not declared_file:
            unresolved.append(f"{name}: neither SPDX expression nor license_file was declared")
        if package["name"] in FONT_CRATES:
            features = resolved_nodes[package["id"]].get("features", [])
            lines.append("  Resolved features: " + ", ".join(sorted(features)))
            for relative in FONT_DIRS:
                font_dir = base / relative
                if not font_dir.is_dir() or font_dir.is_symlink():
                    continue
                for path in sorted(font_dir.iterdir(), key=lambda item: item.name):
                    if path.is_file() and path.suffix.lower() in {".ttf", ".otf"}:
                        # Inventory names only. Font binaries are not copied.
                        lines.append("  Default-font package asset: " + relative_file(path, base))
                        font_count += 1
                    if path.is_file() and (path.name.lower().startswith(PREFIXES)
                                           or path.suffix.lower() in {".txt", ".md"}):
                        entry = include(path, base, f"{name} font notice")
                        if entry:
                            lines.append(entry)
        inventory.append("\n".join(lines))

    if manifest:
        shipped = {(package["name"], package["version"]) for package in packages}
        verify_current_gap_views(manifest, completed_release_matches, shipped)

    version = command(["rustc", "--version"]).decode("utf-8").strip()
    sysroot = Path(command(["rustc", "--print", "sysroot"]).decode("utf-8").strip())
    rust_doc = sysroot / "share/doc/rust"
    standard_lines = ["RUST STANDARD LIBRARY / COMPILER RUNTIME NOTICES", f"  Toolchain: {version}",
                      "  Upstream-supplied standard-library copyright bundle is preserved verbatim (HTML source).",
                      "  Adjacent upstream license texts are a conservative superset, not an assertion that every covered component/platform is linked."]
    std_copyright = rust_doc / "COPYRIGHT-library.html"
    if std_copyright.is_file():
        entry = include(std_copyright, rust_doc, "Rust standard library copyright")
        if entry:
            standard_lines.append(entry)
    else:
        unresolved.append("Rust standard library: installed toolchain lacks COPYRIGHT-library.html; runtime notices require manual collection")
    license_dir = rust_doc / "licenses"
    if license_dir.is_dir() and not license_dir.is_symlink():
        for path in sorted(license_dir.iterdir(), key=lambda item: item.name):
            if path.is_file() and path.suffix.lower() == ".txt":
                entry = include(path, rust_doc, "Rust toolchain license text")
                if entry:
                    standard_lines.append(entry)
    else:
        unresolved.append("Rust standard library: installed toolchain license-text directory is absent")
    inventory.append("\n".join(standard_lines))

    header = ["VIDEO TOOLKIT / VIDEO AGENT — MECHANICALLY COLLECTED THIRD-PARTY NOTICES",
              f"Target inventory: {target}", f"Locked Cargo packages reachable through normal/build edges: {len(packages)}",
              f"Third-party packages: {third_party_count}; named font assets: {font_count}; distinct original texts: {len(texts)}",
              "", "SCOPE AND LIMITATIONS",
              "This is a notice collection, NOT a completed legal or license-compliance audit. Normal generation/checking is offline.",
              "Cargo's platform-filtered resolution may include build-only or workspace-feature-unified dependencies; this is not a binary-symbol SBOM.",
              "SPDX strings and authors/repository fields are upstream declarations, not independently adjudicated license conclusions.",
              "Missing local license text and unreadable files remain explicit unresolved items below. No replacement license is invented.",
              "Supplemental code notices use packaged commit IDs, or the one explicit official_release_source_match verified against the exact locked archive and installed source; fixed URLs/text hashes are checked offline.",
              "Source copyright/reference snippets, when present, retain their exact source/archive provenance and do not count as full license-text resolution.",
              "Default epaint and hash-bound workspace embedded-font notices are included separately; they do not substitute for code licenses.",
              "FFmpeg, ffprobe, ffplay, macOS frameworks, and OS fonts are external prerequisites, not bundled in this release.",
              "No Cargo cache, source tree, OS font binary, credential, or local absolute installation path is included.",
              "Identical original UTF-8 texts are stored once; the inventory refers to their exact byte SHA256.",
              "Text markers/newlines separate entries; original text bytes between markers are not rewritten.",
              "", f"UNRESOLVED ITEMS ({len(unresolved)})"]
    header.extend(f"- {item}" for item in sorted(set(unresolved)))
    if not unresolved:
        header.append("None detected by these mechanical checks; this is not proof of legal completeness.")
    output = ("\n".join(header) + "\n\n" + "\n\n".join(inventory) + "\n\nORIGINAL TEXTS\n").encode("utf-8")
    for digest, content in sorted(texts.items()):
        output += f"\n===== BEGIN TEXT SHA256 {digest} =====\n".encode("ascii")
        output += content
        output += f"\n===== END TEXT SHA256 {digest} =====\n".encode("ascii")
    if len(output) > MAX_NOTICE_BYTES:
        raise NoticeError("Collected notices exceed the 8 MiB notice limit; review before packaging")
    return output, {"packages": len(packages), "third_party": third_party_count, "font_assets": font_count,
                    "distinct_texts": len(texts), "unresolved": len(unresolved), "bytes": len(output)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", default="aarch64-apple-darwin")
    parser.add_argument("--output", type=Path, default=ROOT / "THIRD_PARTY_NOTICES.txt")
    parser.add_argument("--check", action="store_true", help="verify existing file without writing")
    parser.add_argument("--upstream-sources", action="store_true", help="list exact public commit URLs for missing local code licenses; no network access")
    parser.add_argument("--fetch-upstream", action="store_true", help="explicitly GET only license texts from exact official GitHub commits, using gh; never source code")
    parser.add_argument("--inspect-cached-source-headers", action="store_true", help="offline bounded review of the six documented gaps; preserve legal comment only, never dependency code")
    args = parser.parse_args()
    if not args.target or len(args.target) > 128 or any(not (character.isalnum() or character in "_-") for character in args.target):
        raise NoticeError("Invalid Rust target identifier")
    if args.upstream_sources:
        print(json.dumps(missing_upstream_sources(args.target), indent=2, sort_keys=True))
        return 0
    if args.fetch_upstream:
        print(json.dumps(fetch_upstream(args.target), sort_keys=True))
        return 0
    if args.inspect_cached_source_headers:
        print(json.dumps(inspect_cached_source_headers(args.target), sort_keys=True))
        return 0
    output, summary = collect(args.target)
    if args.check:
        if not args.output.is_file() or args.output.read_bytes() != output:
            raise NoticeError("Notice file is missing or differs from the current locked local inventory")
    else:
        if args.output.is_symlink():
            raise NoticeError("Refusing to replace a symlink notice file")
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=args.output.parent, delete=False) as temporary:
            temporary.write(output)
            temporary.flush()
            os.fsync(temporary.fileno())
            temporary_path = Path(temporary.name)
        os.replace(temporary_path, args.output)
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (NoticeError, OSError, ValueError, KeyError) as error:
        # Error details intentionally exclude filesystem locations and subprocess stderr.
        message = str(error) if isinstance(error, NoticeError) else "Invalid or unavailable local metadata; inspect locally"
        print(f"Notice collection failed: {message}", file=sys.stderr)
        raise SystemExit(1)
