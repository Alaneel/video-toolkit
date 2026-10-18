#!/usr/bin/env python3
"""Fetch licensed fixed footage and prepare evaluation inputs/baselines, never an AI result.

Python 3.11+, curl, FFmpeg and ffprobe. All destinations are explicit and existing files
are verified or refused, not overwritten. This is dataset preparation, not an
alternative media executor for the product.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import uuid
import zipfile

HERE = Path(__file__).resolve().parent
DATASET = json.loads((HERE / "dataset.json").read_text())
SUITE = json.loads((HERE / "tasks.json").read_text())


def digest(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def checked(path: Path, expected_hash: str, expected_bytes: int | None = None) -> Path:
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"Not a regular file: {path}")
    if expected_bytes is not None and path.stat().st_size != expected_bytes:
        raise ValueError(f"Unexpected file size: {path}")
    if digest(path) != expected_hash:
        raise ValueError(f"SHA256 mismatch; will not replace existing file: {path}")
    return path


def new_directory(path: Path) -> Path:
    path = path.expanduser().resolve()
    path.mkdir(parents=True, exist_ok=True)
    return path


def publish_new(partial: Path, destination: Path) -> None:
    """Hard linking on the same filesystem publishes without an overwrite race."""
    os.link(partial, destination)
    partial.unlink()


def write_new_json(path: Path, value: dict) -> None:
    # Exclusive creation makes a stale or competing run visible instead of
    # silently rewriting evidence. Failures leave the existing record intact.
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, ensure_ascii=False)
        stream.write("\n")


def tool(name: str) -> str:
    executable = shutil.which(name)
    if executable is None:
        raise ValueError(f"Install {name} and make it available on PATH")
    return executable


def execute(args: list[str], timeout: int = 300) -> str:
    result = subprocess.run(args, capture_output=True, text=True, timeout=timeout, check=False)
    if result.returncode:
        raise ValueError(f"Command failed ({Path(args[0]).name}): {result.stderr[-3000:]}")
    return result.stdout


def probe(path: Path) -> dict:
    return json.loads(execute([tool("ffprobe"), "-v", "error", "-show_format", "-show_streams", "-of", "json", str(path)]))


def verify_media(path: Path, expected_ms: int) -> dict:
    details = probe(path)
    video = [s for s in details["streams"] if s["codec_type"] == "video"]
    audio = [s for s in details["streams"] if s["codec_type"] == "audio"]
    duration = round(float(details["format"]["duration"]) * 1000)
    if len(video) != 1 or len(audio) != 1 or abs(duration - expected_ms) > 150:
        raise ValueError(f"Media duration/stream mismatch: {path}")
    if audio[0].get("sample_rate") != "48000" or audio[0].get("channels") != 2:
        raise ValueError(f"Expected stereo 48000 Hz source audio: {path}")
    execute([tool("ffmpeg"), "-v", "error", "-xerror", "-i", str(path), "-map", "0:v:0", "-map", "0:a:0", "-f", "null", "-"])
    return {"duration_ms": duration, "video_codec": video[0]["codec_name"],
            "width": video[0]["width"], "height": video[0]["height"],
            "audio_codec": audio[0]["codec_name"], "audio_sample_rate": 48000,
            "audio_channels": 2, "full_decode": "passed"}


def fetch(cache_dir: Path) -> Path:
    cache = new_directory(cache_dir)
    source = DATASET["source"]
    archive = cache / source["archive_name"]
    if not archive.exists():
        partial = cache / f"download-{uuid.uuid4().hex}.partial"
        try:
            # Use the OS curl trust store; standalone macOS Python installs
            # often lack a configured CA bundle. Never bypass TLS verification.
            # Only this exclusive temporary file may be filled by the fixed
            # download command, never an existing source or finished artifact.
            partial.open("xb").close()
            execute([tool("curl"), "--fail", "--silent", "--show-error", "--location",
                     "--proto", "=https", "--proto-redir", "=https", "--max-redirs", "5",
                     "--connect-timeout", "20", "--max-time", "300", "--retry", "2",
                     "--max-filesize", str(source["archive_bytes"]), "--output", str(partial),
                     source["url"]], timeout=930)
            checked(partial, source["archive_sha256"], source["archive_bytes"])
            publish_new(partial, archive)
        finally:
            partial.unlink(missing_ok=True)
    checked(archive, source["archive_sha256"], source["archive_bytes"])
    destination = cache / source["member"]
    if destination.exists():
        return checked(destination, source["member_sha256"], source["member_bytes"])
    partial = cache / f"extract-{uuid.uuid4().hex}.partial"
    try:
        with zipfile.ZipFile(archive) as bundle:
            members = bundle.infolist()
            if len(members) != 1 or members[0].filename != source["member"]:
                raise ValueError("Archive member list differs from the frozen source")
            if members[0].file_size != source["member_bytes"]:
                raise ValueError("Archive member exceeds or differs from expected size")
            # Do not extract archive paths. Stream the one validated member to
            # an explicitly selected, exclusive file.
            with bundle.open(members[0]) as entry, partial.open("xb") as output:
                shutil.copyfileobj(entry, output)
        checked(partial, source["member_sha256"], source["member_bytes"])
        publish_new(partial, destination)
    finally:
        partial.unlink(missing_ok=True)
    return destination


def encoding_args() -> list[str]:
    return ["-c:v", "libx264", "-preset", "fast", "-crf", "20", "-pix_fmt", "yuv420p",
            "-r", "24", "-c:a", "aac", "-ar", "48000", "-ac", "2", "-b:a", "128k",
            "-movflags", "+faststart"]


def prepare(source_path: Path, output_dir: Path, include_heldout: bool = False) -> list[dict]:
    source_path = checked(source_path.resolve(), DATASET["source"]["member_sha256"], DATASET["source"]["member_bytes"])
    output = new_directory(output_dir)
    ffmpeg_version = execute([tool("ffmpeg"), "-version"]).splitlines()[0]
    records = []
    for clip in DATASET["clips"]:
        if clip["split"] == "heldout" and not include_heldout:
            continue
        destination = output / f"{clip['id']}.mp4"
        record_path = output / f"{clip['id']}.json"
        expected_ms = clip["end_ms"] - clip["start_ms"]
        if record_path.exists():
            record = json.loads(record_path.read_text())
            if record.get("clip") != clip or record.get("source_sha256") != DATASET["source"]["member_sha256"]:
                raise ValueError(f"Existing clip record differs from frozen input: {record_path}")
            checked(destination, record["sha256"], record["bytes"])
            verify_media(destination, expected_ms)
            records.append(record)
            continue
        if destination.exists():
            raise FileExistsError(f"Unrecorded output exists, refusing overwrite: {destination}")
        partial = output / f"{clip['id']}-{uuid.uuid4().hex}.partial.mp4"
        args = [tool("ffmpeg"), "-hide_banner", "-loglevel", "error", "-nostdin", "-n",
                "-ss", str(clip["start_ms"] / 1000), "-i", str(source_path),
                "-t", str(expected_ms / 1000), "-map", "0:v:0", "-map", "0:a:0",
                "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2", *encoding_args(), str(partial)]
        try:
            execute(args)
            media = verify_media(partial, expected_ms)
            record = {"schema_version": 1, "dataset_id": DATASET["dataset_id"],
                      "recipe_version": DATASET["preparation"]["recipe_version"],
                      "clip": clip, "source_sha256": DATASET["source"]["member_sha256"],
                      "file": destination.name, "sha256": digest(partial), "bytes": partial.stat().st_size,
                      "ffmpeg_version": ffmpeg_version, "media": media,
                      "source_time_mapping": "original_ms = prepared_ms + clip.start_ms",
                      "attribution": DATASET["source"]["attribution"],
                      "license_url": DATASET["source"]["license_url"],
                      "changes": DATASET["source"]["changes"]}
            publish_new(partial, destination)
            write_new_json(record_path, record)
            records.append(record)
        finally:
            partial.unlink(missing_ok=True)
    return records


def uniform_windows(duration_ms: int, target_ms: int, count: int = 4) -> list[dict]:
    if duration_ms <= 0 or target_ms <= 0 or target_ms > duration_ms or count <= 0 or target_ms < count:
        raise ValueError("Invalid uniform baseline duration/target/window count")
    # Integer-only split guarantees the windows sum to the exact target, with
    # the remainder assigned deterministically to the earliest windows.
    capacities = [duration_ms * (i + 1) // count - duration_ms * i // count for i in range(count)]
    widths = [target_ms // count] * count
    remainder = target_ms % count
    for index, capacity in enumerate(capacities):
        if remainder and widths[index] < capacity:
            widths[index] += 1
            remainder -= 1
    if remainder:
        raise ValueError("Baseline windows cannot fit source strata")
    result = []
    for index, width in enumerate(widths):
        stratum_start = duration_ms * index // count
        stratum_end = duration_ms * (index + 1) // count
        if width > stratum_end - stratum_start:
            raise ValueError("Baseline window cannot fit its source stratum")
        start = stratum_start + (stratum_end - stratum_start - width) // 2
        result.append({"start_ms": start, "end_ms": start + width})
    return result


def baseline(task_id: str, prepared_dir: Path, output_dir: Path, allow_heldout: bool = False) -> dict:
    tasks = [task for task in SUITE["tasks"] if task["id"] == task_id]
    if len(tasks) != 1:
        raise ValueError(f"Unknown frozen task: {task_id}")
    task = tasks[0]
    if task["split"] == "heldout" and not allow_heldout:
        raise ValueError("Heldout export requires --allow-heldout; do not inspect it during debugging")
    clip = next(c for c in DATASET["clips"] if c["id"] == task["clip_id"])
    prepared = prepared_dir.resolve()
    source_path = prepared / f"{clip['id']}.mp4"
    input_record = json.loads((prepared / f"{clip['id']}.json").read_text())
    if input_record.get("clip") != clip or input_record.get("source_sha256") != DATASET["source"]["member_sha256"]:
        raise ValueError("Prepared input record differs from frozen dataset")
    checked(source_path, input_record["sha256"], input_record["bytes"])
    verify_media(source_path, clip["end_ms"] - clip["start_ms"])
    output = new_directory(output_dir)
    video_path = output / f"{task_id}-uniform.mp4"
    record_path = output / f"{task_id}-uniform.json"
    if video_path.exists() or record_path.exists():
        raise FileExistsError("Baseline output already exists; use a different explicit output directory")
    windows = uniform_windows(input_record["media"]["duration_ms"], task["target_duration_ms"])
    # One FFmpeg invocation is enough for dataset preparation. The actual
    # product export and its approval/resume guarantees live in Rust.
    filters = []
    pairs = []
    for i, window in enumerate(windows):
        start, end = window["start_ms"] / 1000, window["end_ms"] / 1000
        filters.extend([f"[0:v:0]trim=start={start}:end={end},setpts=PTS-STARTPTS[v{i}]",
                        f"[0:a:0]atrim=start={start}:end={end},asetpts=PTS-STARTPTS[a{i}]"])
        pairs.extend([f"[v{i}]", f"[a{i}]"])
    filters.append("".join(pairs) + f"concat=n={len(windows)}:v=1:a=1[v][a]")
    partial = output / f"baseline-{uuid.uuid4().hex}.partial.mp4"
    try:
        execute([tool("ffmpeg"), "-hide_banner", "-loglevel", "error", "-nostdin", "-n", "-i", str(source_path),
                 "-filter_complex", ";".join(filters), "-map", "[v]", "-map", "[a]",
                 *encoding_args(), str(partial)])
        media = verify_media(partial, task["target_duration_ms"])
        for window in windows:
            window["original_start_ms"] = clip["start_ms"] + window["start_ms"]
            window["original_end_ms"] = clip["start_ms"] + window["end_ms"]
        record = {"schema_version": 1, "kind": "nonsemantic_baseline", "baseline_id": SUITE["baseline"]["id"],
                  "task_id": task_id, "input_sha256": input_record["sha256"], "clips": windows,
                  "source": clip["id"], "file": video_path.name, "sha256": digest(partial),
                  "bytes": partial.stat().st_size, "media": media,
                  "ffmpeg_version": execute([tool("ffmpeg"), "-version"]).splitlines()[0],
                  "quality_status": "pending_human_review", "model_used": False,
                  "attribution": DATASET["source"]["attribution"], "license_url": DATASET["source"]["license_url"],
                  "changes": "Trimmed, concatenated, re-encoded; source audio retained. No semantic selection."}
        publish_new(partial, video_path)
        write_new_json(record_path, record)
        return record
    finally:
        partial.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    fetch_parser = commands.add_parser("fetch", help="Download and verify the fixed official source")
    fetch_parser.add_argument("--cache-dir", required=True, type=Path)
    prepare_parser = commands.add_parser("prepare", help="Create development excerpt; heldout is opt-in")
    prepare_parser.add_argument("--source", required=True, type=Path)
    prepare_parser.add_argument("--output-dir", required=True, type=Path)
    prepare_parser.add_argument("--include-heldout", action="store_true")
    baseline_parser = commands.add_parser("baseline", help="Export a real uniform, nonsemantic comparison video")
    baseline_parser.add_argument("--task", required=True)
    baseline_parser.add_argument("--prepared-dir", required=True, type=Path)
    baseline_parser.add_argument("--output-dir", required=True, type=Path)
    baseline_parser.add_argument("--allow-heldout", action="store_true")
    args = parser.parse_args()
    try:
        if args.command == "fetch":
            print(fetch(args.cache_dir))
        elif args.command == "prepare":
            print(json.dumps(prepare(args.source, args.output_dir, args.include_heldout), indent=2))
        else:
            print(json.dumps(baseline(args.task, args.prepared_dir, args.output_dir, args.allow_heldout), indent=2))
        return 0
    except (ValueError, OSError, subprocess.TimeoutExpired, zipfile.BadZipFile) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
