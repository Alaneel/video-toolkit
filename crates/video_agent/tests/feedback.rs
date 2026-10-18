//! Objective feedback tests use synthetic media only; no model or real user task is involved.
//! Run real media cases explicitly: cargo test -p video_agent --test feedback -- --ignored --test-threads=1
use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::Command,
    time::Duration,
};
use video_agent::{feedback, *};

fn synthetic_source(root: &Path, name: &str, color: &str, audio: bool) -> PathBuf {
    let path = root.join(name);
    let mut command = Command::new("ffmpeg");
    command.args(["-v", "error", "-nostdin", "-n", "-f", "lavfi", "-i"]);
    command.arg(format!("color=c={color}:s=160x90:r=30:d=3"));
    if audio {
        command.args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=3",
        ]);
    }
    command.args([
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-threads",
        "1",
        "-c:a",
        "aac",
    ]);
    assert!(command.arg(&path).status().unwrap().success());
    path
}

fn workspace(root: &Path, sources: &[PathBuf], target_ms: u64) -> Workspace {
    Workspace::create(
        &root.join("tasks"),
        sources,
        "SYNTHETIC media feedback fixture; no visual or semantic quality claim".into(),
        target_ms,
        Limits {
            initial_frames: 2,
            max_frames: 4,
            max_clip_ms: 3000,
            ..Limits::default()
        },
    )
    .unwrap()
}

fn manual_plan(task: &Task, intervals: &[(usize, u64, u64)]) -> EditPlan {
    EditPlan {
        revision: task.plan.as_ref().unwrap().revision,
        clips: intervals
            .iter()
            .enumerate()
            .map(|(index, (asset_index, start_ms, end_ms))| Clip {
                id: format!("manual-feedback-{}", index + 1),
                source_id: task.assets[*asset_index].id.clone(),
                start_ms: *start_ms,
                end_ms: *end_ms,
                reason: "MANUAL synthetic fixture, not a model selection".into(),
                uncertainty: "Synthetic colors establish order only, not semantic relevance".into(),
                evidence_ids: vec![],
                human_modified: true,
            })
            .collect(),
        summary: "MANUAL synthetic objective-feedback test".into(),
        audio_policy: AudioPolicy::PreserveWithSilence,
    }
}

fn draft(workspace: &Workspace, intervals: &[(usize, u64, u64)]) -> Task {
    let initial = workspace.manual_draft().unwrap();
    workspace
        .save_plan(manual_plan(&initial, intervals))
        .unwrap()
}

fn assert_review_state_unchanged(before: &Task, after: &Task) {
    assert_eq!(
        before.plan, after.plan,
        "Preview must not edit the task plan"
    );
    assert_eq!(
        serde_json::to_value(&before.approval).unwrap(),
        serde_json::to_value(&after.approval).unwrap(),
        "Preview must neither approve nor invalidate the reviewed task plan"
    );
    assert_eq!(
        serde_json::to_value(&before.export).unwrap(),
        serde_json::to_value(&after.export).unwrap(),
        "Preview is not a final export"
    );
    assert_eq!(before.model_calls, after.model_calls);
    assert_eq!(before.cloud_consent, after.cloud_consent);
}

fn managed_file(root: &Path, relative: impl AsRef<Path>) -> PathBuf {
    let relative = relative.as_ref();
    assert!(!relative.is_absolute());
    assert!(relative
        .components()
        .all(|component| matches!(component, Component::Normal(_))));
    let resolved = fs::canonicalize(root.join(relative)).unwrap();
    assert!(resolved.starts_with(fs::canonicalize(root).unwrap()));
    assert!(resolved.is_file());
    resolved
}

fn rgb_at(path: &Path, seconds: &str) -> [u8; 3] {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-ss", seconds, "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=1:1",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 3);
    output.stdout.try_into().unwrap()
}

fn audio_rms_at(path: &Path, seconds: &str) -> f64 {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-ss", seconds, "-i"])
        .arg(path)
        .args([
            "-t", "0.25", "-map", "0:a:0", "-ac", "1", "-ar", "8000", "-f", "s16le", "-",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let samples: Vec<f64> = output
        .stdout
        .chunks_exact(2)
        .map(|bytes| f64::from(i16::from_le_bytes([bytes[0], bytes[1]])))
        .collect();
    assert!(!samples.is_empty());
    (samples.iter().map(|sample| sample * sample).sum::<f64>() / samples.len() as f64).sqrt()
}

fn assert_quality_rejected(result: Result<feedback::QualityReport>) {
    if let Ok(report) = result {
        assert!(!report.objective_pass);
        assert_eq!(report.semantic_evaluation, "not_evaluated");
    }
}

#[test]
fn iteration_bounds_reject_before_creating_a_task_or_preview() {
    let root = tempfile::tempdir().unwrap();
    let absent = root.path().join("never-created-task");
    for maximum in [0, 4, u32::MAX] {
        let error = feedback::iterate(&absent, vec![], maximum, &CancelToken::default())
            .expect_err("Invalid iteration maximum must fail");
        let message = error.to_string().to_ascii_lowercase();
        assert!(
            message.contains("attempt") || message.contains("iteration"),
            "Bounds must be rejected before attempting task I/O: {message}"
        );
        assert!(!absent.exists());
    }
}

#[test]
fn iteration_candidate_queue_cannot_exceed_the_remaining_attempts() {
    let root = tempfile::tempdir().unwrap();
    let absent = root.path().join("never-created-task");
    let candidate = EditPlan {
        revision: 1,
        clips: vec![],
        summary: "Not rendered; checks the candidate queue contract only".into(),
        audio_policy: AudioPolicy::Mute,
    };
    for (maximum, candidates) in [
        (1, vec![candidate.clone()]),
        (2, vec![candidate.clone(), candidate.clone()]),
        (3, vec![candidate.clone(), candidate.clone(), candidate]),
    ] {
        let error = feedback::iterate(&absent, candidates, maximum, &CancelToken::default())
            .expect_err("Current plan already consumes the first attempt");
        assert!(error.to_string().to_ascii_lowercase().contains("candidate"));
        assert!(!absent.exists());
    }
}

#[test]
fn pre_cancelled_preview_does_not_create_a_task_or_preview() {
    let root = tempfile::tempdir().unwrap();
    let absent = root.path().join("never-created-task");
    let cancel = CancelToken::default();
    cancel.cancel();
    assert!(matches!(
        feedback::capture(&absent, &cancel),
        Err(AgentError::Cancelled)
    ));
    assert!(!absent.exists());
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; synthetic colors/audio only, no model"]
fn unapproved_whole_plan_preview_preserves_cut_order_audio_and_source_hashes() {
    let root = tempfile::tempdir().unwrap();
    let red = synthetic_source(root.path(), "red-audio.mp4", "red", true);
    let blue = synthetic_source(root.path(), "blue-silent.mp4", "blue", false);
    let source_hashes = [
        media::hash_file(&red).unwrap(),
        media::hash_file(&blue).unwrap(),
    ];
    let workspace = workspace(root.path(), &[red.clone(), blue.clone()], 2000);
    let before = draft(&workspace, &[(1, 0, 1000), (0, 0, 1000)]);
    assert!(before.approval.is_none());
    let validation =
        feedback::validate(workspace.task_dir(), None, &CancelToken::default()).unwrap();
    assert!(validation.sources_verified);
    assert_eq!(validation.clip_count, 2);
    assert_eq!(validation.plan_duration_ms, 2000);
    assert_eq!(validation.semantic_evaluation, "not_evaluated");
    let bundle = feedback::capture(workspace.task_dir(), &CancelToken::default()).unwrap();
    let media_path = managed_file(workspace.task_dir(), &bundle.media_path);
    managed_file(workspace.task_dir(), &bundle.plan_path);
    managed_file(workspace.task_dir(), &bundle.quality_path);
    managed_file(workspace.task_dir(), &bundle.manifest_path);
    assert_eq!(media::hash_file(&media_path).unwrap(), bundle.sha256);
    assert_eq!(
        bundle.plan_digest,
        workflow::plan_digest(&before, before.plan.as_ref().unwrap()).unwrap()
    );
    assert!(bundle.quality.objective_pass);
    assert_eq!(bundle.quality.semantic_evaluation, "not_evaluated");
    assert_eq!(bundle.quality.plan_duration_ms, 2000);
    assert_eq!(bundle.quality.target_duration_ms, 2000);
    assert!(bundle.quality.observed_duration_ms.abs_diff(2000) <= 150);
    assert_eq!((bundle.quality.width, bundle.quality.height), (640, 360));
    assert!(bundle.quality.has_audio);
    assert!(!bundle.quality.checks.is_empty());
    assert!(bundle.quality.checks.iter().all(|check| check.passed));
    let first = rgb_at(&media_path, "0.4");
    let second = rgb_at(&media_path, "1.4");
    assert!(
        first[2] > 180 && first[0] < 40,
        "First cut must be blue: {first:?}"
    );
    assert!(
        second[0] > 180 && second[2] < 40,
        "Second cut must be red: {second:?}"
    );
    assert!(audio_rms_at(&media_path, "0.4") < 20.0);
    assert!(audio_rms_at(&media_path, "1.4") > 100.0);
    let quality = feedback::quality(workspace.task_dir(), &CancelToken::default()).unwrap();
    assert!(quality.objective_pass);
    assert_eq!(quality.semantic_evaluation, "not_evaluated");
    let latest = feedback::latest(workspace.task_dir()).unwrap().unwrap();
    assert_eq!(latest.bundle_id, bundle.bundle_id);
    let trajectory = feedback::trajectory(workspace.task_dir()).unwrap();
    assert_eq!(trajectory.attempts_started, 1);
    assert!(!trajectory.events.is_empty());
    assert!(workspace.export(&CancelToken::default()).is_err());
    assert_review_state_unchanged(&before, &Workspace::snapshot(workspace.task_dir()).unwrap());
    let digest = workflow::plan_digest(&before, before.plan.as_ref().unwrap()).unwrap();
    let approved = workspace.approve(&digest).unwrap();
    assert!(approved.approval.is_some());
    let approved_preview =
        feedback::capture(workspace.task_dir(), &CancelToken::default()).unwrap();
    assert!(approved_preview.quality.matches_current_plan);
    assert!(approved_preview.quality.needs_human_review);
    assert_review_state_unchanged(
        &approved,
        &Workspace::snapshot(workspace.task_dir()).unwrap(),
    );
    assert_eq!(media::hash_file(&red).unwrap(), source_hashes[0]);
    assert_eq!(media::hash_file(&blue).unwrap(), source_hashes[1]);
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; unsafe preview sizes must not fall back to the source"]
fn empty_overlong_and_excessive_clip_previews_are_rejected_without_fallback() {
    let root = tempfile::tempdir().unwrap();
    let input = synthetic_source(root.path(), "source.mp4", "blue", false);
    let workspace = workspace(root.path(), std::slice::from_ref(&input), 1000);
    workspace.manual_draft().unwrap();
    let cases = [vec![], vec![(0, 0, 100); 33], vec![(0, 0, 3000); 11]];
    for intervals in cases {
        let current = Workspace::snapshot(workspace.task_dir()).unwrap();
        let saved = workspace
            .save_plan(manual_plan(&current, &intervals))
            .unwrap();
        assert!(feedback::capture(workspace.task_dir(), &CancelToken::default()).is_err());
        assert!(feedback::latest(workspace.task_dir()).unwrap().is_none());
        let previews = workspace.task_dir().join("previews");
        if previews.exists() {
            assert_eq!(fs::read_dir(previews).unwrap().count(), 0);
        }
        assert_review_state_unchanged(&saved, &Workspace::snapshot(workspace.task_dir()).unwrap());
    }
    // Every individual field is valid under the general editor contract, but
    // the complete preview plan must also fit its capture/recheck round trip.
    let current = Workspace::snapshot(workspace.task_dir()).unwrap();
    let mut oversized = manual_plan(&current, &[(0, 0, 100); 4]);
    for (index, clip) in oversized.clips.iter_mut().enumerate() {
        clip.id = format!("manual-metadata-{}", index + 1);
        clip.reason = format!("MANUAL{}", "r".repeat(7994));
        clip.uncertainty = "u".repeat(8000);
    }
    oversized.summary = "s".repeat(16000);
    assert!(serde_json::to_vec_pretty(&oversized).unwrap().len() > 65_536);
    let saved = workspace.save_plan(oversized).unwrap();
    let error = feedback::capture(workspace.task_dir(), &CancelToken::default())
        .expect_err("Preview metadata must remain readable by fresh quality checks");
    assert!(error.to_string().contains("64 KiB"));
    assert!(feedback::latest(workspace.task_dir()).unwrap().is_none());
    assert_eq!(
        fs::read_dir(workspace.task_dir().join("previews"))
            .unwrap()
            .count(),
        0
    );
    assert_review_state_unchanged(&saved, &Workspace::snapshot(workspace.task_dir()).unwrap());
    assert_eq!(
        feedback::trajectory(workspace.task_dir())
            .unwrap()
            .attempts_started,
        0,
        "Preflight-rejected sizes must never start a render attempt"
    );
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; bounded what-if candidates are never human edits"]
fn bounded_iteration_selects_a_candidate_without_rewriting_or_approving_the_task() {
    let root = tempfile::tempdir().unwrap();
    let input = synthetic_source(root.path(), "red.mp4", "red", true);
    let workspace = workspace(root.path(), std::slice::from_ref(&input), 2000);
    let before = draft(&workspace, &[(0, 0, 1000)]);
    let mut candidate = before.plan.clone().unwrap();
    candidate.clips[0].end_ms = 2000;
    let mut invalid_candidate = candidate.clone();
    invalid_candidate.clips[0].end_ms = 4000;
    assert!(feedback::iterate(
        workspace.task_dir(),
        vec![candidate.clone(), invalid_candidate],
        3,
        &CancelToken::default()
    )
    .is_err());
    assert_eq!(
        feedback::trajectory(workspace.task_dir())
            .unwrap()
            .attempts_started,
        0,
        "Preflight the entire queue before rendering the current plan"
    );
    let original_digest = workflow::plan_digest(&before, before.plan.as_ref().unwrap()).unwrap();
    let candidate_digest = workflow::plan_digest(&before, &candidate).unwrap();
    let report = feedback::iterate(
        workspace.task_dir(),
        vec![candidate],
        2,
        &CancelToken::default(),
    )
    .unwrap();
    assert_eq!(report.attempts.len(), 2);
    assert_eq!(report.attempts[0].plan_digest, original_digest);
    assert!(!report.attempts[0].quality.objective_pass);
    assert_eq!(report.attempts[1].plan_digest, candidate_digest);
    assert!(report.attempts[1].quality.objective_pass);
    let selected_path = managed_file(workspace.task_dir(), report.selected_plan.as_ref().unwrap());
    let selected: EditPlan = serde_json::from_slice(&fs::read(selected_path).unwrap()).unwrap();
    assert_eq!(
        workflow::plan_digest(&before, &selected).unwrap(),
        candidate_digest
    );
    assert!(report.task_plan_unchanged);
    assert!(report.needs_human_review);
    assert!(!report.stopped_reason.is_empty());
    assert_eq!(
        feedback::trajectory(workspace.task_dir())
            .unwrap()
            .attempts_started,
        2
    );
    let after = Workspace::snapshot(workspace.task_dir()).unwrap();
    assert_review_state_unchanged(&before, &after);
    assert!(after.approval.is_none());
    assert!(after.export.is_none());
    assert!(workspace.export(&CancelToken::default()).is_err());
    let fresh = feedback::quality(workspace.task_dir(), &CancelToken::default()).unwrap();
    assert_eq!(fresh.semantic_evaluation, "not_evaluated");
    assert!(!fresh.matches_current_plan);
    assert!(fresh.needs_human_review);
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; tests deliberate mutation of synthetic files"]
fn fresh_quality_rejects_preview_tampering_and_changed_sources() {
    let root = tempfile::tempdir().unwrap();
    let input = synthetic_source(root.path(), "source.mp4", "blue", false);
    let workspace = workspace(root.path(), std::slice::from_ref(&input), 1000);
    draft(&workspace, &[(0, 0, 1000)]);
    let bundle = feedback::capture(workspace.task_dir(), &CancelToken::default()).unwrap();
    let preview = managed_file(workspace.task_dir(), &bundle.media_path);
    let original_preview = fs::read(&preview).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&preview)
        .unwrap()
        .write_all(b"synthetic preview tampering")
        .unwrap();
    // latest is explicitly historical metadata, not a fresh verification claim.
    assert_eq!(
        feedback::latest(workspace.task_dir())
            .unwrap()
            .unwrap()
            .sha256,
        bundle.sha256
    );
    assert_quality_rejected(feedback::quality(
        workspace.task_dir(),
        &CancelToken::default(),
    ));
    let failed = feedback::trajectory(workspace.task_dir()).unwrap();
    let failed_event = failed.events.last().unwrap();
    assert_eq!(failed_event.kind, "failed");
    assert_eq!(failed_event.objective_pass, Some(false));
    fs::write(&preview, original_preview).unwrap();
    assert!(
        feedback::quality(workspace.task_dir(), &CancelToken::default())
            .unwrap()
            .objective_pass
    );
    fs::OpenOptions::new()
        .append(true)
        .open(&input)
        .unwrap()
        .write_all(b"external writer mutates only this synthetic source")
        .unwrap();
    assert_quality_rejected(feedback::quality(
        workspace.task_dir(),
        &CancelToken::default(),
    ));
    assert!(feedback::capture(workspace.task_dir(), &CancelToken::default()).is_err());
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; verifies persistent eight-attempt budget"]
fn preview_attempt_budget_survives_reopen_and_cancellation_does_not_charge_it() {
    let root = tempfile::tempdir().unwrap();
    let input = synthetic_source(root.path(), "source.mp4", "red", false);
    let workspace = workspace(root.path(), std::slice::from_ref(&input), 300);
    let before = draft(&workspace, &[(0, 0, 300)]);
    let cancel = CancelToken::default();
    cancel.cancel();
    assert!(matches!(
        feedback::capture(workspace.task_dir(), &cancel),
        Err(AgentError::Cancelled)
    ));
    assert_eq!(
        feedback::trajectory(workspace.task_dir())
            .unwrap()
            .attempts_started,
        0
    );
    for attempt in 1..=8 {
        let reopened = Workspace::open(workspace.task_dir()).unwrap();
        let bundle = feedback::capture(reopened.task_dir(), &CancelToken::default()).unwrap();
        assert_eq!(bundle.attempt, attempt);
        assert!(bundle.quality.objective_pass);
        assert_eq!(
            feedback::trajectory(reopened.task_dir())
                .unwrap()
                .attempts_started,
            attempt
        );
        if attempt == 1 {
            let state_path = reopened.task_dir().join("preview-trajectory.json");
            let state_bytes = fs::read(&state_path).unwrap();
            let media_path = managed_file(reopened.task_dir(), &bundle.media_path);
            let media_hash = media::hash_file(&media_path).unwrap();
            fs::remove_file(&state_path).unwrap();
            let missing = feedback::capture(reopened.task_dir(), &CancelToken::default())
                .expect_err("Deleting the trajectory must not restore preview credit");
            assert!(missing.to_string().to_ascii_lowercase().contains("missing"));
            fs::write(&state_path, &state_bytes).unwrap();
            let mut rollback: serde_json::Value = serde_json::from_slice(&state_bytes).unwrap();
            rollback["attempts_started"] = serde_json::json!(0);
            rollback["events"] = serde_json::json!([]);
            rollback["latest"] = serde_json::Value::Null;
            fs::write(&state_path, serde_json::to_vec(&rollback).unwrap()).unwrap();
            let rolled_back = feedback::capture(reopened.task_dir(), &CancelToken::default())
                .expect_err("A valid-looking older trajectory must not reset the task counter");
            assert!(rolled_back
                .to_string()
                .to_ascii_lowercase()
                .contains("trajectory"));
            assert_eq!(
                fs::read_dir(reopened.task_dir().join("previews"))
                    .unwrap()
                    .count(),
                1
            );
            assert_eq!(media::hash_file(&media_path).unwrap(), media_hash);
            fs::write(&state_path, state_bytes).unwrap();
            assert_eq!(
                feedback::trajectory(reopened.task_dir())
                    .unwrap()
                    .attempts_started,
                1
            );
        }
    }
    assert!(feedback::capture(workspace.task_dir(), &CancelToken::default()).is_err());
    assert_eq!(
        feedback::trajectory(workspace.task_dir())
            .unwrap()
            .attempts_started,
        8
    );
    assert_review_state_unchanged(&before, &Workspace::snapshot(workspace.task_dir()).unwrap());
    let probe = media::probe(&input, &CancelToken::default(), Duration::from_secs(10)).unwrap();
    assert_eq!(probe.duration_ms, 3000);
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; deterministic persisted cross-file crash window, no model"]
fn trajectory_started_before_task_history_recovers_upward_and_new_attempts_continue() {
    let root = tempfile::tempdir().unwrap();
    let input = synthetic_source(root.path(), "source.mp4", "blue", false);
    let source_before = fs::read(&input).unwrap();
    let workspace = workspace(root.path(), std::slice::from_ref(&input), 1000);
    let before = draft(&workspace, &[(0, 0, 1000)]);
    let digest = workflow::plan_digest(&before, before.plan.as_ref().unwrap()).unwrap();
    let task_path = workspace.task_dir().join("task.json");
    let task_before = fs::read(&task_path).unwrap();
    let interrupted = feedback::Trajectory {
        schema: "video-agent-trajectory/v1".into(),
        task_id: before.id.clone(),
        attempts_started: 1,
        max_attempts: feedback::MAX_ATTEMPTS,
        events: vec![feedback::TrajectoryEvent {
            sequence: 1,
            at: before.created_at.clone(),
            kind: "started".into(),
            attempt: 1,
            plan_digest: digest,
            bundle_id: None,
            objective_pass: None,
            error_code: None,
        }],
        latest: None,
        verification_scope: "historical_metadata_only_not_fresh_verification".into(),
    };
    // Reproduce exactly the durable prefix of attempt(): the trajectory write
    // completed, then its owner stopped before the task.history write. This is
    // deterministic disk-state injection, not a power-loss or SIGKILL claim.
    fs::write(
        workspace.task_dir().join("preview-trajectory.json"),
        serde_json::to_vec_pretty(&interrupted).unwrap(),
    )
    .unwrap();
    let historical = feedback::trajectory(workspace.task_dir()).unwrap();
    assert_eq!(historical.attempts_started, 1);
    assert_eq!(historical.events[0].kind, "started");
    assert!(historical.latest.is_none());
    assert!(feedback::latest(workspace.task_dir()).unwrap().is_none());
    assert_eq!(fs::read(&task_path).unwrap(), task_before);
    assert_eq!(
        fs::read_dir(workspace.task_dir().join("previews"))
            .unwrap()
            .count(),
        0
    );

    let resumed = Workspace::open(workspace.task_dir()).unwrap();
    let first_real = feedback::capture(resumed.task_dir(), &CancelToken::default()).unwrap();
    assert_eq!(
        first_real.attempt, 2,
        "Interrupted credit must never be refunded"
    );
    assert!(first_real.quality.objective_pass);
    let recovered = Workspace::snapshot(resumed.task_dir()).unwrap();
    assert_review_state_unchanged(&before, &recovered);
    assert!(recovered.usage.elapsed_ms >= before.usage.elapsed_ms);
    assert_eq!(
        recovered
            .history
            .iter()
            .filter(|event| event.kind == "preview_attempt_started")
            .count(),
        2
    );
    assert_eq!(
        recovered
            .history
            .iter()
            .filter(|event| event.kind == "preview_history_recovered")
            .count(),
        1
    );
    let trajectory = feedback::trajectory(resumed.task_dir()).unwrap();
    assert_eq!(trajectory.attempts_started, 2);
    assert!(!trajectory
        .events
        .iter()
        .any(|event| event.attempt == 1 && event.kind == "published"));
    assert_eq!(trajectory.latest.as_ref().unwrap().attempt, 2);

    let next = feedback::capture(resumed.task_dir(), &CancelToken::default()).unwrap();
    assert_eq!(next.attempt, 3);
    let after = Workspace::snapshot(resumed.task_dir()).unwrap();
    assert_eq!(
        after
            .history
            .iter()
            .filter(|event| event.kind == "preview_history_recovered")
            .count(),
        1
    );
    assert_eq!(
        after
            .history
            .iter()
            .filter(|event| event.kind == "preview_attempt_started")
            .count(),
        3
    );
    assert_review_state_unchanged(&before, &after);
    assert_eq!(fs::read(input).unwrap(), source_before);
}

#[cfg(unix)]
#[test]
#[ignore = "requires real FFmpeg/ffprobe; synthetic task directory symlink attack"]
fn preview_rejects_replaced_managed_directory_and_media_symlinks() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let input = synthetic_source(root.path(), "source.mp4", "red", false);
    let workspace = workspace(root.path(), std::slice::from_ref(&input), 1000);
    draft(&workspace, &[(0, 0, 1000)]);
    let bundle = feedback::capture(workspace.task_dir(), &CancelToken::default()).unwrap();
    let preview = managed_file(workspace.task_dir(), &bundle.media_path);
    let moved_media = root.path().join("moved-preview.mp4");
    fs::rename(&preview, &moved_media).unwrap();
    symlink(&moved_media, &preview).unwrap();
    assert_quality_rejected(feedback::quality(
        workspace.task_dir(),
        &CancelToken::default(),
    ));
    fs::remove_file(&preview).unwrap();
    fs::rename(&moved_media, &preview).unwrap();
    let relative_dir = Path::new(&bundle.relative_dir);
    let first_component = relative_dir.components().next().unwrap();
    assert!(matches!(first_component, Component::Normal(_)));
    let managed_root = workspace.task_dir().join(first_component.as_os_str());
    let moved_directory = root.path().join("moved-preview-directory");
    fs::rename(&managed_root, &moved_directory).unwrap();
    symlink(&moved_directory, &managed_root).unwrap();
    assert!(feedback::capture(workspace.task_dir(), &CancelToken::default()).is_err());
    assert_quality_rejected(feedback::quality(
        workspace.task_dir(),
        &CancelToken::default(),
    ));
}
