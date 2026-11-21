#!/usr/bin/env python3
"""Package only the published human-edited final cut for pending human review.

Standard library only: no API, subprocess, media decoding, private task reads,
score import, or results/status writes. This is NOT evidence.py's initial A/B pair.
Secure directory-relative creation currently requires a POSIX host (Mac/Linux).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import sys

ROOT = Path(__file__).absolute().parents[2]
KIND = "human_edited_final_output_review"
TASK_ID = "dev-rabbit-woodland"
VIDEO = "assets/video-agent/luna-human-edited-20s.mp4"
TASKS = "evals/video-agent/tasks.json"
DATASET = "evals/video-agent/dataset.json"
RUN = "evals/video-agent/results/dev-rabbit-woodland-20261018-luna.json"
ATTRIBUTION = "assets/video-agent/ATTRIBUTION.md"
# Pin the actual public bytes, not an editable source record's self-reported hash.
# Updating this one historical review target requires an explicit source review.
SOURCES = {
    VIDEO: (1462780, "4543e1b941fa964fe8487cad30e1ace70ae45e847c88d7537ed32bebb5201417"),
    TASKS: (4064, "e9a4794b492004a9bdb4bf3a2657058733f0f3f9724950e6a319020e05f69540"),
    DATASET: (1977, "f423b553ff6546ffa0a0fcd9cfb74ff4f0ce4d41210bdd9f82d1b89837ba03e6"),
    RUN: (22909, "12555966bffcb4600547ae2ae9a9f783bbdbf316aace2180c126b5cabf057415"),
    ATTRIBUTION: (1702, "93140b1b2824b5e264af2060c4393e4475fc55c9b288c2e5bbafddeed551f5b2"),
}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def absolute(path: Path) -> Path:
    require(".." not in path.parts, "Parent traversal is not permitted")
    return path.absolute()


def directory_fd(path: Path) -> int:
    """Walk existing directories without following any symlink, including ancestors."""
    require(os.name == "posix" and os.open in os.supports_dir_fd
            and hasattr(os, "O_NOFOLLOW") and hasattr(os, "O_DIRECTORY"),
            "Secure bundle creation requires POSIX directory-relative file access")
    path = absolute(path)
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    current = os.open(path.anchor, flags)
    try:
        for part in path.parts[1:]:
            following = os.open(part, flags, dir_fd=current)
            os.close(current)
            current = following
        return current
    except BaseException:
        os.close(current)
        raise


def source_bytes(root: Path, name: str) -> bytes:
    size, expected = SOURCES[name]
    path = root / name
    parent = directory_fd(path.parent)
    try:
        descriptor = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                             dir_fd=parent)
    finally:
        os.close(parent)
    with os.fdopen(descriptor, "rb") as stream:
        before = os.fstat(stream.fileno())
        require(stat.S_ISREG(before.st_mode) and before.st_size == size,
                "Public source must be a regular file of its pinned size")
        data = stream.read(size + 1)
        after = os.fstat(stream.fileno())
    identity = lambda info: (info.st_dev, info.st_ino, info.st_size,
                             info.st_mtime_ns, info.st_ctime_ns)
    require(identity(before) == identity(after), "Public source changed while read")
    require(len(data) == size and hashlib.sha256(data).hexdigest() == expected,
            "Public source SHA256 mismatch")
    return data


def no_duplicates(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "Duplicate JSON key")
        result[key] = value
    return result


def load(data: bytes) -> dict:
    value = json.loads(data, object_pairs_hook=no_duplicates)
    require(isinstance(value, dict), "Expected a public JSON object")
    return value


def encoded(value: dict) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False, allow_nan=False) + "\n").encode()


def review_form(sources: dict[str, bytes]) -> dict:
    suite, dataset, run = (load(sources[name]) for name in (TASKS, DATASET, RUN))
    task, = [item for item in suite["tasks"] if item["id"] == TASK_ID]
    clip, = [item for item in dataset["clips"] if item["id"] == task["clip_id"]]
    export, edit = run["verified_export"], run["author_edit"]
    require(task["split"] == clip["split"] == run["split"] == "dev"
            and run["task_id"] == TASK_ID and run["backend"] == "openai_responses",
            "Public development record identity mismatch")
    require(export["kind"] == "human_edited_or_other_plan" and export["asset"] == VIDEO
            and export["sha256"] == SOURCES[VIDEO][1]
            and edit["authorization"] == "direct_user_confirmation"
            and export["persisted_revision"] == edit["saved_revision"] == edit["approved_revision"]
            and export["persisted_plan_digest"] == edit["approved_plan_digest"],
            "Public edited-export binding mismatch")
    proposed = run["first_validated_proposal"]["decision"]["clips"]
    require(edit["final_clips"] == proposed[1:]
            and edit["removed_interval_ms"] == [proposed[0]["start_ms"], proposed[0]["end_ms"]],
            "Public edit recipe mismatch")
    ranges = [[item["start_ms"], item["end_ms"]] for item in edit["final_clips"]]
    require(sum(end - start for start, end in ranges)
            == export["persisted_duration_ms"] == task["target_duration_ms"],
            "Public target and edit duration mismatch")
    return {
        "schema_version": 1, "kind": KIND, "status": "pending_human_review",
        "task_id": TASK_ID, "suite_id": suite["suite_id"], "intent": task["intent"],
        "target_duration_ms": task["target_duration_ms"], "candidate_file": "final-output.mp4",
        "candidate_sha256": SOURCES[VIDEO][1],
        "scope": {"initial_model_proposal": False, "ab_comparison": False,
                  "blind": False, "heldout": False, "benchmark_acceptance": False,
                  "publication_ready": False, "suite_results_changed": False},
        "evidence_limit": "runtime_record_unattested; byte hashes only, no fresh probe, decode, playback or private task verification",
        "public_source_sha256": {name: digest for name, (_, digest) in SOURCES.items()},
        "recorded_recipe_not_reexecuted": {
            "runtime_code_commit": export["runtime_code_commit"],
            "prepared_input_sha256": run["input"]["prepared_sha256"],
            "original_source_sha256": run["input"]["original_source_sha256"],
            "prepared_source_ranges_ms": ranges,
            "original_source_ranges_ms": [[start + clip["start_ms"], end + clip["start_ms"]]
                                          for start, end in ranges],
            "removed_first_interval_ms": edit["removed_interval_ms"],
            "approved_revision": edit["approved_revision"],
            "approved_plan_digest": edit["approved_plan_digest"],
            "persisted_duration_ms": export["persisted_duration_ms"],
            "historical_probe": export["probe"],
        },
        "human_review": {
            "reviewer": None, "reviewed_at": None, "prior_task_or_footage_exposure": None,
            "watched_entire_final_output": None, "relevance_0_to_4": None,
            "irrelevant_seconds": None, "irrelevant_output_intervals_seconds": None,
            "notes": None, "source_omissions": None, "reason_faithfulness_0_to_4": None,
            "baseline_preference": None,
        },
        "unsupported_assessments": {
            "source_omissions": "Original input is not included; leave null.",
            "reason_faithfulness_0_to_4": "Complete original observations and reasons are not included; leave null.",
            "baseline_preference": "No baseline candidate is included; leave null.",
        },
        "attribution": dataset["source"]["attribution"],
        "license_url": dataset["source"]["license_url"],
        "changes": "User-directed first-clip removal from the 25-second draft; remaining four clips re-encoded. Review copy is byte-identical to the public 20-second final cut.",
    }


INSTRUCTIONS = """# 最终成片真人评审 / Final-output human review

NOT 初稿 / NOT A/B / NOT blind / NOT heldout / NOT 模型或产品验收通过。
目的仅为 human_edited_final_output_review。这是用户指令删除首段后的20秒
成片，不是原始模型初稿，也不是新的真实模型闭环或连续操作录屏。

1. 用本机播放器打开 final-output.mp4，完整看完。无需网络或上传。
2. 按 review.json 内原始 intent 评价这个最终成片。相关性 0–4：
   0=没有所需内容；1=少量匹配；2=约半匹配；3=大部分匹配；4=几乎全部清楚匹配。
3. 真人自行填写 human_review 的评审人/化名、日期及先前接触情况、是否完整观看、
   relevance_0_to_4、irrelevant_seconds、成片内不相关区间和 notes。
   区间使用成片播放秒数，不是原片时间。无法判断时保留 null 并说明原因。
4. source_omissions、reason_faithfulness_0_to_4、baseline_preference 必须保留 null：
   包内没有完整原片、原始观察/理由或基线，不能推算遗漏、忠实度或比较优势。
5. 保留原始空白表，另存填写副本交回。不要在成片或 provenance/recipe 字段上改写。

所有答案初始均为 null；工具没有生成、导入或验证真人分数。metadata 哈希只绑定
公开文件字节，不是供应商签名、重新解码或语义评价。本包不更新 results/status.json，
不读取旧私有Task，也不授权新的模型调用、发布或解除6项许可正文分发门槛。
历史探测/配方仅从绑定的公开记录摘取，未重新执行。不要把该最终成片的评分算给初稿。

素材署名、CC BY 3.0 链接和修改说明见原字节保留的 ATTRIBUTION.md。
"""


def write_new(directory: int, name: str, data: bytes) -> None:
    descriptor = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                         0o600, dir_fd=directory)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def create(output: Path, root: Path = ROOT) -> dict:
    output, root = absolute(output), absolute(root)
    # Validate every source before creating anything. Copy these exact checked
    # bytes, not a file reopened after validation. No broad directory collection.
    sources = {name: source_bytes(root, name) for name in SOURCES}
    review = review_form(sources)
    parent = directory_fd(output.parent)
    directory = None
    try:
        os.mkdir(output.name, 0o700, dir_fd=parent)  # Existing file/link/directory refuses.
        directory = os.open(output.name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                            dir_fd=parent)
        for name, data in (("final-output.mp4", sources[VIDEO]),
                           ("ATTRIBUTION.md", sources[ATTRIBUTION]),
                           ("README.md", INSTRUCTIONS.encode()), ("review.json", encoded(review))):
            write_new(directory, name, data)
        os.fsync(directory)
    finally:
        if directory is not None:
            os.close(directory)
        os.close(parent)
    return review


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        result = create(args.output_dir)
        print(json.dumps({"kind": result["kind"], "status": result["status"],
                          "semantic_scores_generated": False, "candidate_sha256": result["candidate_sha256"]}))
        return 0
    except (ValueError, OSError, KeyError, TypeError) as error:
        # Native errors may contain private paths. Never echo their raw text.
        print(f"ERROR: {str(error) if isinstance(error, ValueError) else type(error).__name__}",
              file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
