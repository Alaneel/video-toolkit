#!/usr/bin/env python3
"""Collect local dependency notices, not a legal/compliance audit.

Normal generation uses the locked, offline, platform-filtered Cargo graph and
never traverses the Cargo cache. License texts are read only from each resolved
package's own directory and a few named font directories. The explicit
--fetch-upstream option reads only pinned official license texts, not code.
Identical texts appear once by SHA256 to keep release archives small.
"""
from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tarfile
import tomllib
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
MAX_NOTICE_BYTES = 2 * 1024 * 1024
MAX_INPUT_BYTES = 8 * 1024 * 1024
PREFIXES = ("license", "licence", "copying", "notice", "copyright")
FONT_CRATES = {"epaint", "epaint_default_fonts", "egui", "eframe"}
FONT_DIRS = ("fonts", "src/fonts", "assets/fonts")
SUPPLEMENT_ROOT = ROOT / "third_party/licenses"


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
    items = manifest.get("unresolved_review", {}).get("items", [])
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
    manifest["cached_source_review"] = {"scope": "Offline inspection of exact cached crate archives and installed src/*.rs modules only; no code was executed or copied into the project",
                                       "full_license_gaps_remaining": 6, "items": findings, "source_header_notices": notices}
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return {"reviewed_crates": len(findings), "source_files": sum(len(item["source_files"]) for item in findings), "source_header_notices": len(notices), "full_license_gaps_remaining": 6}


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
            for key in ("unresolved_review", "cached_source_review"):
                if key in previous:
                    manifest[key] = previous[key]
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
        if not code_notices and supplement:
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
              "Supplemental code notices, when present, are pinned to packaged upstream commit IDs and checked against a committed URL/SHA256 manifest.",
              "Source copyright/reference snippets, when present, retain their exact source/archive provenance and do not count as full license-text resolution.",
              "The default epaint font notices are included separately; they do not substitute for the crate's code license.",
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
        raise NoticeError("Collected notices exceed the 2 MiB release limit; review before packaging")
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
