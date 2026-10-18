//! Simulated reviewer + real synthetic media. These tests make no API/model-quality claims.
//! Run media cases: cargo test -p video_agent --test smart_feedback -- --ignored --test-threads=1
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    time::Duration,
};
use video_agent::{
    review::{
        self, PreviewReviewProvider, ReviewDecision, ReviewIssue, ReviewOutcome, ReviewRequest,
        ReviewVerdict,
    },
    smart_feedback, *,
};

#[derive(Clone, Copy)]
enum Behavior {
    ReviseBlueThenAccept,
    Accept,
    FailUnknown,
    RejectSchema,
    CitePreviewAsSource,
    CancelAfterResponse,
    ChangeSyntheticSource,
    OversizedRevisionMetadata,
}

struct SimulatedReviewer {
    behavior: Behavior,
    calls: AtomicUsize,
    estimate_tokens: Option<u64>,
    estimate_cost: Option<f64>,
    cancel_during_estimate: Option<CancelToken>,
    entered: Mutex<Option<mpsc::Sender<()>>>,
    gate: Option<Arc<(Mutex<bool>, Condvar)>>,
}

impl SimulatedReviewer {
    fn new(behavior: Behavior) -> Self {
        Self {
            behavior,
            calls: AtomicUsize::new(0),
            estimate_tokens: None,
            estimate_cost: None,
            cancel_during_estimate: None,
            entered: Mutex::new(None),
            gate: None,
        }
    }
    fn count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn record(&self, request: &ReviewRequest) -> ModelCall {
        ModelCall {
            call_id: request.call_id.clone(),
            backend: "simulation_preview_review".into(),
            requested_model: "simulation-review-model".into(),
            model: "simulation-review-model".into(),
            prompt_version: review::PROMPT_VERSION.into(),
            sample_ids: vec![],
            provider_response_id: None,
            request_id: None,
            input_tokens: 80,
            output_tokens: 20,
            estimated_cost_usd: 0.001,
            elapsed_ms: 0,
            status: "feedback_completed".into(),
            decision: None,
            error: None,
        }
    }
}

fn jpeg_center(path: &Path, expected_hash: &str) -> [u8; 3] {
    assert_eq!(media::hash_file(path).unwrap(), expected_hash);
    let image = image::open(path).unwrap().to_rgb8();
    assert!(image.width() > 0 && image.height() > 0);
    image.get_pixel(image.width() / 2, image.height() / 2).0
}

fn assert_supplied_images_are_real(request: &ReviewRequest) {
    assert!(!request.preview_frames.is_empty());
    assert!(request.preview_frames.len() <= 6);
    assert!(!request.source_samples.is_empty());
    assert!(request.source_samples.len() <= 8);
    assert_eq!(request.quality.plan_digest, request.plan_digest);
    assert_eq!(request.quality.semantic_evaluation, "not_evaluated");
    for frame in &request.preview_frames {
        jpeg_center(&frame.path, &frame.sha256);
    }
    for sample in &request.source_samples {
        jpeg_center(&sample.image_path, &sample.sha256);
        assert_eq!(
            sample.id,
            format!("{}-{}", sample.source_id, sample.time_ms)
        );
    }
}

fn blue_revision(request: &ReviewRequest) -> ReviewDecision {
    let blue_id = &request.assets[1].id;
    let evidence = request
        .source_samples
        .iter()
        .find(|sample| sample.source_id == *blue_id && sample.time_ms < 2000)
        .expect("The reviewer must receive a real source sample inside the proposed blue interval");
    let rgb = jpeg_center(&evidence.image_path, &evidence.sha256);
    assert!(
        rgb[2] > 180 && rgb[0] < 40,
        "Blue source evidence is not actually blue: {rgb:?}"
    );
    ReviewDecision {
        verdict: ReviewVerdict::Revise,
        summary: "SIMULATION: replace red with source-evidenced blue; no real model evaluated semantics".into(),
        issues: vec![ReviewIssue {
            clip_id: request.plan.clips[0].id.clone(), kind: "relevance".into(),
            detail: "SIMULATION checks a deliberately wrong synthetic color, not natural-language understanding".into(),
            evidence_ids: vec![request.preview_frames[0].id.clone()],
        }],
        revised_clips: vec![ClipProposal {
            source_id: blue_id.clone(), start_ms: 0, end_ms: 2000,
            reason: "SIMULATED source-evidenced blue interval".into(),
            uncertainty: "Sparse source JPEGs do not establish motion or sound".into(),
            evidence_ids: vec![evidence.id.clone()],
        }],
    }
}

fn accept() -> ReviewDecision {
    ReviewDecision {
        verdict: ReviewVerdict::Accept,
        summary:
            "SIMULATED acceptance of a synthetic fixture, not a human or real-model quality score"
                .into(),
        issues: vec![],
        revised_clips: vec![],
    }
}

fn oversized_revision_metadata(request: &ReviewRequest) -> ReviewDecision {
    assert_eq!(request.source_samples.len(), 4);
    let decision = ReviewDecision {
        verdict: ReviewVerdict::Revise,
        summary: "s".repeat(4000),
        issues: vec![ReviewIssue {
            clip_id: request.plan.clips[0].id.clone(),
            kind: "relevance".into(),
            detail: "SIMULATION: otherwise legal revision exceeds local preview metadata capacity"
                .into(),
            evidence_ids: vec![request.preview_frames[0].id.clone()],
        }],
        revised_clips: request
            .source_samples
            .iter()
            .map(|sample| {
                let asset = request
                    .assets
                    .iter()
                    .find(|asset| asset.id == sample.source_id)
                    .unwrap();
                let start_ms = sample
                    .time_ms
                    .saturating_sub(50)
                    .min(asset.duration_ms - 100);
                ClipProposal {
                    source_id: sample.source_id.clone(),
                    start_ms,
                    end_ms: start_ms + 100,
                    reason: "r".repeat(8000),
                    uncertainty: "u".repeat(8000),
                    evidence_ids: vec![sample.id.clone()],
                }
            })
            .collect(),
    };
    // This exercises the distinction between valid model evidence/schema and
    // the stricter serialized preview-plan limit, not an invalid model reply.
    review::validate_decision(request, &decision).unwrap();
    let candidate = EditPlan {
        revision: request.plan.revision,
        clips: decision
            .revised_clips
            .iter()
            .enumerate()
            .map(|(index, clip)| Clip {
                id: format!("feedback-1-{}", index + 1),
                source_id: clip.source_id.clone(),
                start_ms: clip.start_ms,
                end_ms: clip.end_ms,
                reason: clip.reason.clone(),
                uncertainty: clip.uncertainty.clone(),
                evidence_ids: clip.evidence_ids.clone(),
                human_modified: false,
            })
            .collect(),
        summary: decision.summary.clone(),
        audio_policy: request.plan.audio_policy,
    };
    assert!(serde_json::to_vec_pretty(&candidate).unwrap().len() + 1 > 65_536);
    decision
}

impl PreviewReviewProvider for SimulatedReviewer {
    fn identity(&self) -> (String, String, String) {
        (
            "simulation_preview_review".into(),
            "simulation-review-model".into(),
            review::PROMPT_VERSION.into(),
        )
    }
    fn ready(&self) -> bool {
        true
    }
    fn estimate(&self, request: &ReviewRequest) -> Result<(u64, f64)> {
        if let Some(cancel) = &self.cancel_during_estimate {
            cancel.cancel();
        }
        Ok((
            self.estimate_tokens
                .unwrap_or(u64::from(request.limits.max_output_tokens) + 1000),
            self.estimate_cost.unwrap_or(0.01),
        ))
    }
    fn review(&self, request: &ReviewRequest, cancel: &CancelToken) -> Result<ReviewOutcome> {
        cancel.check()?;
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        assert_supplied_images_are_real(request);
        if let Some(sender) = self.entered.lock().unwrap().take() {
            sender.send(()).unwrap();
        }
        if let Some(gate) = &self.gate {
            let (lock, available) = &**gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = available.wait(released).unwrap();
            }
        }
        let decision = match self.behavior {
            Behavior::ReviseBlueThenAccept => {
                let rgb = jpeg_center(
                    &request.preview_frames[0].path,
                    &request.preview_frames[0].sha256,
                );
                if call == 0 {
                    assert!(
                        rgb[0] > 180 && rgb[2] < 40,
                        "First real preview must be red: {rgb:?}"
                    );
                    blue_revision(request)
                } else {
                    assert_eq!(call, 1, "No unbounded third reviewer request");
                    assert!(
                        rgb[2] > 180 && rgb[0] < 40,
                        "Second real preview must be blue: {rgb:?}"
                    );
                    assert_eq!(request.plan.clips[0].source_id, request.assets[1].id);
                    accept()
                }
            }
            Behavior::Accept => accept(),
            Behavior::FailUnknown => {
                return Err(AgentError::Invalid(
                    "SIMULATED unknown transport failure".into(),
                ))
            }
            Behavior::RejectSchema => {
                let mut record = self.record(request);
                record.status = "feedback_response_rejected".into();
                record.error = Some("SIMULATED malformed tool response".into());
                return Err(AgentError::ModelResponseRejected {
                    message: "SIMULATED schema rejection with known usage".into(),
                    record: Box::new(record),
                });
            }
            Behavior::CitePreviewAsSource => {
                let mut decision = blue_revision(request);
                decision.revised_clips[0].evidence_ids = vec![request.preview_frames[0].id.clone()];
                decision
            }
            Behavior::CancelAfterResponse => {
                cancel.cancel();
                accept()
            }
            Behavior::ChangeSyntheticSource => {
                fs::OpenOptions::new()
                    .append(true)
                    .open(&request.assets[0].path)
                    .unwrap()
                    .write_all(b"external writer changes only this synthetic test fixture")
                    .unwrap();
                accept()
            }
            Behavior::OversizedRevisionMetadata => oversized_revision_metadata(request),
        };
        Ok(ReviewOutcome {
            decision,
            record: self.record(request),
        })
    }
}

fn source(root: &Path, name: &str, color: &str) -> PathBuf {
    let path = root.join(name);
    assert!(Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-n", "-f", "lavfi", "-i"])
        .arg(format!("color=c={color}:s=160x90:r=30:d=3"))
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=3",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-threads",
            "1",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac"
        ])
        .arg(&path)
        .status()
        .unwrap()
        .success());
    path
}

fn task(root: &Path) -> (Workspace, [PathBuf; 2], Task) {
    let sources = [
        source(root, "red.mp4", "red"),
        source(root, "blue.mp4", "blue"),
    ];
    let workspace = Workspace::create(&root.join("tasks"), &sources,
        "SYNTHETIC fixture: choose blue instead of the deliberately wrong red draft; no semantic-quality claim".into(),
        2000, Limits { initial_frames: 4, max_frames: 36, max_clip_ms: 3000, ..Limits::default() }).unwrap();
    workspace.sample(&CancelToken::default()).unwrap();
    let before = workspace.manual_draft().unwrap();
    assert_eq!(
        before.plan.as_ref().unwrap().clips[0].source_id,
        before.assets[0].id
    );
    assert_eq!(before.plan.as_ref().unwrap().clips[0].end_ms, 2000);
    assert!(before.model_calls.is_empty());
    (workspace, sources, before)
}

fn assert_no_authority_change(before: &Task, after: &Task) {
    assert_eq!(before.plan, after.plan);
    assert_eq!(
        serde_json::to_value(&before.approval).unwrap(),
        serde_json::to_value(&after.approval).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&before.export).unwrap(),
        serde_json::to_value(&after.export).unwrap()
    );
}

#[test]
fn consent_and_pre_cancellation_are_checked_before_task_path_or_writes() {
    let root = tempfile::tempdir().unwrap();
    let absent = root.path().join("not-a-task");
    let provider = SimulatedReviewer::new(Behavior::Accept);
    let error =
        smart_feedback::run(&absent, &provider, 3, false, &CancelToken::default()).unwrap_err();
    assert!(error.to_string().to_ascii_lowercase().contains("consent"));
    assert!(!absent.exists());
    let cancel = CancelToken::default();
    cancel.cancel();
    assert!(matches!(
        smart_feedback::run(&absent, &provider, 3, true, &cancel),
        Err(AgentError::Cancelled)
    ));
    assert!(!absent.exists());
    assert_eq!(provider.count(), 0);
}

#[test]
fn smart_round_count_is_bounded_before_task_path_or_writes() {
    let root = tempfile::tempdir().unwrap();
    let absent = root.path().join("not-a-task");
    let provider = SimulatedReviewer::new(Behavior::Accept);
    for rounds in [0, 4, u32::MAX] {
        assert!(
            smart_feedback::run(&absent, &provider, rounds, true, &CancelToken::default()).is_err()
        );
    }
    assert!(!absent.exists());
    assert_eq!(provider.count(), 0);
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; explicit simulated review, no network AI"]
fn two_round_render_review_revision_adoption_and_explicitly_approved_export() {
    let root = tempfile::tempdir().unwrap();
    let (workspace, sources, before) = task(root.path());
    let source_hashes = [
        media::hash_file(&sources[0]).unwrap(),
        media::hash_file(&sources[1]).unwrap(),
    ];
    let provider = SimulatedReviewer::new(Behavior::ReviseBlueThenAccept);
    let report = smart_feedback::run(
        workspace.task_dir(),
        &provider,
        3,
        true,
        &CancelToken::default(),
    )
    .unwrap();
    assert_eq!(provider.count(), 2);
    assert_eq!(report.rounds.len(), 2);
    assert_eq!(
        report.rounds[0].review.as_ref().unwrap().verdict,
        ReviewVerdict::Revise
    );
    assert_eq!(
        report.rounds[1].review.as_ref().unwrap().verdict,
        ReviewVerdict::Accept
    );
    assert!(report.selected_plan.is_some());
    assert!(report.needs_human_review && report.task_plan_unchanged && report.matches_current_plan);
    assert!(report.semantic_evaluation.contains("model_self_assessment"));
    let after = Workspace::snapshot(workspace.task_dir()).unwrap();
    assert_no_authority_change(&before, &after);
    assert_eq!(after.usage.model_calls, 2);
    assert_eq!(
        (after.usage.input_tokens, after.usage.output_tokens),
        (160, 40)
    );
    assert!(after.usage.frames_created > before.usage.frames_created);
    assert!(after.usage.frames_created <= after.limits.max_frames);
    assert!(after.model_calls.iter().all(|call| call.decision.is_none()));
    assert!(after.model_calls.iter().all(|call| call
        .sample_ids
        .iter()
        .any(|id| id.starts_with("source:"))
        && call.sample_ids.iter().any(|id| id.starts_with("preview:"))));
    let again = smart_feedback::run(
        workspace.task_dir(),
        &provider,
        3,
        true,
        &CancelToken::default(),
    )
    .unwrap();
    assert_eq!(again.run_id, report.run_id);
    assert_eq!(provider.count(), 2);
    let repeated = Workspace::snapshot(workspace.task_dir()).unwrap();
    assert_eq!(repeated.usage.frames_created, after.usage.frames_created);
    assert_eq!(repeated.usage.model_calls, after.usage.model_calls);
    assert_eq!(repeated.usage.input_tokens, after.usage.input_tokens);
    assert_eq!(repeated.usage.output_tokens, after.usage.output_tokens);
    assert!((repeated.usage.estimated_cost_usd - after.usage.estimated_cost_usd).abs() < 1e-12);
    assert!(smart_feedback::adopt(
        workspace.task_dir(),
        "wrong-run-id",
        &CancelToken::default()
    )
    .is_err());
    assert!(!smart_feedback::is_adopted_model_plan(&repeated));
    let adopted = smart_feedback::adopt(
        workspace.task_dir(),
        &report.run_id,
        &CancelToken::default(),
    )
    .unwrap();
    let plan = adopted.plan.as_ref().unwrap();
    assert_eq!(plan.revision, before.plan.as_ref().unwrap().revision + 1);
    assert_eq!(plan.clips[0].source_id, adopted.assets[1].id);
    assert!(plan
        .clips
        .iter()
        .all(|clip| !clip.human_modified && !clip.evidence_ids.is_empty()));
    assert!(smart_feedback::is_adopted_model_plan(&adopted));
    assert!(adopted.approval.is_none() && adopted.export.is_none());
    assert!(workspace.export(&CancelToken::default()).is_err());
    let digest = workflow::plan_digest(&adopted, plan).unwrap();
    workspace.approve(&digest).unwrap();
    let exported = workspace.export(&CancelToken::default()).unwrap();
    let output = exported.export.as_ref().unwrap();
    assert!(output.verified);
    assert_eq!(output.plan_digest, digest);
    let path = workspace.task_dir().join(&output.path);
    media::verify_output(
        &path,
        2000,
        true,
        &CancelToken::default(),
        Duration::from_secs(30),
    )
    .unwrap();
    assert_eq!(media::hash_file(&sources[0]).unwrap(), source_hashes[0]);
    assert_eq!(media::hash_file(&sources[1]).unwrap(), source_hashes[1]);
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; simulated unknown provider failure must not retry"]
fn unknown_provider_failure_retains_the_reservation_and_is_never_resent() {
    let root = tempfile::tempdir().unwrap();
    let (workspace, _, before) = task(root.path());
    let provider = SimulatedReviewer::new(Behavior::FailUnknown);
    assert!(smart_feedback::run(
        workspace.task_dir(),
        &provider,
        3,
        true,
        &CancelToken::default()
    )
    .is_err());
    assert_eq!(provider.count(), 1);
    let failed = Workspace::snapshot(workspace.task_dir()).unwrap();
    assert_no_authority_change(&before, &failed);
    assert_eq!(failed.usage.model_calls, 1);
    assert_eq!(failed.usage.input_tokens, 1000);
    assert_eq!(
        failed.usage.output_tokens,
        u64::from(failed.limits.max_output_tokens)
    );
    assert!((failed.usage.estimated_cost_usd - 0.01).abs() < 1e-12);
    assert_eq!(
        failed.model_calls[0].status,
        "feedback_failed_unknown_usage"
    );
    let resumed = smart_feedback::run(
        workspace.task_dir(),
        &provider,
        3,
        true,
        &CancelToken::default(),
    )
    .unwrap();
    assert!(resumed.selected_plan.is_none());
    assert!(resumed
        .stopped_reason
        .as_deref()
        .unwrap()
        .contains("unknown"));
    assert_eq!(provider.count(), 1);
    let again = Workspace::snapshot(workspace.task_dir()).unwrap();
    assert_eq!(again.usage.frames_created, failed.usage.frames_created);
    assert_eq!(again.usage.input_tokens, failed.usage.input_tokens);
    assert_eq!(again.usage.output_tokens, failed.usage.output_tokens);
    let run_directory = workspace.task_dir().join("feedback-runs");
    let state_path = run_directory.join(format!("{}.json", resumed.run_id));
    let state_bytes = fs::read(&state_path).unwrap();
    fs::write(&state_path, b"{broken synthetic journal").unwrap();
    assert!(smart_feedback::run(
        workspace.task_dir(),
        &provider,
        3,
        true,
        &CancelToken::default()
    )
    .is_err());
    assert_eq!(
        provider.count(),
        1,
        "Broken journals must never reset a failed call"
    );
    fs::write(&state_path, &state_bytes).unwrap();
    #[cfg(unix)]
    {
        let moved_directory = root.path().join("moved-feedback-runs");
        fs::rename(&run_directory, &moved_directory).unwrap();
        std::os::unix::fs::symlink(&moved_directory, &run_directory).unwrap();
        assert!(smart_feedback::run(
            workspace.task_dir(),
            &provider,
            3,
            true,
            &CancelToken::default()
        )
        .is_err());
        assert_eq!(provider.count(), 1);
        assert_eq!(
            fs::read(moved_directory.join(format!("{}.json", resumed.run_id))).unwrap(),
            state_bytes
        );
    }
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; known response survives cancellation without another call"]
fn received_decision_is_durable_before_cancel_and_resumes_without_resending() {
    let root = tempfile::tempdir().unwrap();
    let (workspace, _, before) = task(root.path());
    let provider = SimulatedReviewer::new(Behavior::CancelAfterResponse);
    let cancel = CancelToken::default();
    assert!(matches!(
        smart_feedback::run(workspace.task_dir(), &provider, 3, true, &cancel),
        Err(AgentError::Cancelled)
    ));
    assert_eq!(provider.count(), 1);
    let received = Workspace::snapshot(workspace.task_dir()).unwrap();
    assert_no_authority_change(&before, &received);
    assert_eq!(
        (received.usage.input_tokens, received.usage.output_tokens),
        (80, 20)
    );
    let resumed = smart_feedback::run(
        workspace.task_dir(),
        &provider,
        3,
        true,
        &CancelToken::default(),
    )
    .unwrap();
    assert_eq!(provider.count(), 1);
    assert_eq!(resumed.rounds.len(), 1);
    assert!(resumed.selected_plan.is_some());
    assert_eq!(
        resumed.rounds[0].review.as_ref().unwrap().verdict,
        ReviewVerdict::Accept
    );
    let after = Workspace::snapshot(workspace.task_dir()).unwrap();
    assert_no_authority_change(&before, &after);
    assert_eq!(after.usage.frames_created, received.usage.frames_created);
    assert_eq!(after.usage.model_calls, 1);
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; deterministic known-unsent crash-prefix injection"]
fn calling_journal_with_already_refunded_not_sent_task_resumes_without_double_refund() {
    let root = tempfile::tempdir().unwrap();
    let (workspace, _, before) = task(root.path());
    let cancel = CancelToken::default();
    let mut provider = SimulatedReviewer::new(Behavior::Accept);
    provider.cancel_during_estimate = Some(cancel.clone());
    assert!(matches!(
        smart_feedback::run(workspace.task_dir(), &provider, 3, true, &cancel),
        Err(AgentError::Cancelled)
    ));
    assert_eq!(provider.count(), 0);
    let not_sent = Workspace::snapshot(workspace.task_dir()).unwrap();
    assert_no_authority_change(&before, &not_sent);
    assert_eq!(not_sent.usage.model_calls, 1);
    assert_eq!(
        (not_sent.usage.input_tokens, not_sent.usage.output_tokens),
        (0, 0)
    );
    assert_eq!(not_sent.usage.estimated_cost_usd, 0.0);
    assert_eq!(not_sent.model_calls.len(), 1);
    assert_eq!(not_sent.model_calls[0].status, "feedback_not_sent");
    assert_eq!(not_sent.model_calls[0].input_tokens, 0);
    assert_eq!(not_sent.model_calls[0].output_tokens, 0);
    assert_eq!(not_sent.model_calls[0].estimated_cost_usd, 0.0);
    let stopped = smart_feedback::status(workspace.task_dir())
        .unwrap()
        .unwrap();
    assert_eq!(stopped.phase, "completed");
    assert_eq!(
        stopped.stopped_reason.as_deref(),
        Some("local_admission_stopped_before_model")
    );
    let state_path = workspace
        .task_dir()
        .join("feedback-runs")
        .join(format!("{}.json", stopped.run_id));
    let mut journal: serde_json::Value =
        serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    assert!(journal["pending_request"].is_object());
    assert!(journal["reservation"].is_object());
    assert!(journal["pending_result"].is_null());
    // Deterministically inject the on-disk prefix after Task's known-unsent
    // refund was saved but before Job's completion write. This is not SIGKILL.
    journal["phase"] = serde_json::json!("calling");
    journal["stopped_reason"] = serde_json::Value::Null;
    let events = journal["events"].as_array_mut().unwrap();
    assert_eq!(events.last().unwrap()["kind"], "stopped");
    events.pop();
    fs::write(&state_path, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();
    for _ in 0..2 {
        let resumed = smart_feedback::run(
            workspace.task_dir(),
            &provider,
            3,
            true,
            &CancelToken::default(),
        )
        .unwrap();
        assert_eq!(resumed.run_id, stopped.run_id);
        assert_eq!(resumed.phase, "completed");
        assert_eq!(
            resumed.stopped_reason.as_deref(),
            Some("local_admission_stopped_before_model")
        );
        assert!(resumed.selected_plan.is_none());
        assert_eq!(provider.count(), 0);
        let after = Workspace::snapshot(workspace.task_dir()).unwrap();
        assert_no_authority_change(&before, &after);
        assert_eq!(after.usage.model_calls, 1);
        assert_eq!(after.usage.input_tokens, 0);
        assert_eq!(after.usage.output_tokens, 0);
        assert_eq!(after.usage.estimated_cost_usd, 0.0);
        assert_eq!(after.usage.frames_created, not_sent.usage.frames_created);
        assert_eq!(after.model_calls[0].status, "feedback_not_sent");
    }
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; valid model schema exceeds local preview metadata limit"]
fn oversized_evidenced_revision_is_durably_rejected_without_replaying_received_response() {
    let root = tempfile::tempdir().unwrap();
    let (workspace, _, before) = task(root.path());
    let provider = SimulatedReviewer::new(Behavior::OversizedRevisionMetadata);
    assert!(smart_feedback::run(
        workspace.task_dir(),
        &provider,
        3,
        true,
        &CancelToken::default()
    )
    .is_err());
    assert_eq!(provider.count(), 1);
    let rejected = Workspace::snapshot(workspace.task_dir()).unwrap();
    assert_no_authority_change(&before, &rejected);
    assert_eq!(rejected.usage.model_calls, 1);
    assert_eq!(
        (rejected.usage.input_tokens, rejected.usage.output_tokens),
        (80, 20)
    );
    assert!((rejected.usage.estimated_cost_usd - 0.001).abs() < 1e-12);
    assert_eq!(rejected.model_calls[0].status, "feedback_response_rejected");
    assert!(rejected.model_calls[0].decision.is_none());
    let stopped = smart_feedback::status(workspace.task_dir())
        .unwrap()
        .unwrap();
    assert_eq!(stopped.phase, "completed");
    assert_eq!(
        stopped.stopped_reason.as_deref(),
        Some("model_revision_rejected")
    );
    assert!(stopped.selected_plan.is_none());
    for _ in 0..2 {
        let resumed = smart_feedback::run(
            workspace.task_dir(),
            &provider,
            3,
            true,
            &CancelToken::default(),
        )
        .unwrap();
        assert_eq!(resumed.run_id, stopped.run_id);
        assert_eq!(resumed.phase, "completed");
        assert_eq!(
            resumed.stopped_reason.as_deref(),
            Some("model_revision_rejected")
        );
        assert!(resumed.selected_plan.is_none());
        assert_eq!(provider.count(), 1);
        let after = Workspace::snapshot(workspace.task_dir()).unwrap();
        assert_no_authority_change(&before, &after);
        assert_eq!(after.usage.model_calls, rejected.usage.model_calls);
        assert_eq!(after.usage.input_tokens, rejected.usage.input_tokens);
        assert_eq!(after.usage.output_tokens, rejected.usage.output_tokens);
        assert_eq!(
            after.usage.estimated_cost_usd,
            rejected.usage.estimated_cost_usd
        );
        assert_eq!(after.usage.frames_created, rejected.usage.frames_created);
        assert_eq!(after.model_calls[0].status, "feedback_response_rejected");
    }
    assert!(smart_feedback::adopt(
        workspace.task_dir(),
        &stopped.run_id,
        &CancelToken::default()
    )
    .is_err());
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; malformed or malicious simulated response retains actual usage"]
fn rejected_review_schema_or_preview_as_source_evidence_cannot_change_the_plan() {
    for behavior in [Behavior::RejectSchema, Behavior::CitePreviewAsSource] {
        let root = tempfile::tempdir().unwrap();
        let (workspace, _, before) = task(root.path());
        let provider = SimulatedReviewer::new(behavior);
        assert!(smart_feedback::run(
            workspace.task_dir(),
            &provider,
            3,
            true,
            &CancelToken::default()
        )
        .is_err());
        let rejected = Workspace::snapshot(workspace.task_dir()).unwrap();
        assert_no_authority_change(&before, &rejected);
        assert_eq!(provider.count(), 1);
        assert_eq!(rejected.usage.model_calls, 1);
        assert_eq!(
            (rejected.usage.input_tokens, rejected.usage.output_tokens),
            (80, 20)
        );
        assert!((rejected.usage.estimated_cost_usd - 0.001).abs() < 1e-12);
        assert_eq!(rejected.model_calls[0].status, "feedback_response_rejected");
        assert!(rejected.model_calls[0].decision.is_none());
        let status = smart_feedback::run(
            workspace.task_dir(),
            &provider,
            3,
            true,
            &CancelToken::default(),
        )
        .unwrap();
        assert!(status.selected_plan.is_none());
        assert_eq!(provider.count(), 1);
    }
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; shared model-call, token and estimated-cost limits"]
fn insufficient_shared_call_token_or_cost_budget_admits_no_provider_call() {
    for budget in ["calls", "tokens", "cost"] {
        let root = tempfile::tempdir().unwrap();
        let (workspace, _, mut before) = task(root.path());
        let mut provider = SimulatedReviewer::new(Behavior::Accept);
        match budget {
            "calls" => {
                before.limits.max_model_calls = 1;
                before.usage.model_calls = 1;
                let requestless_record = ModelCall {
                    call_id: "already-used-fixture-call".into(),
                    backend: "SIMULATION".into(),
                    requested_model: "fixture".into(),
                    model: "fixture".into(),
                    prompt_version: "fixture".into(),
                    sample_ids: vec![],
                    provider_response_id: None,
                    request_id: None,
                    input_tokens: 0,
                    output_tokens: 0,
                    estimated_cost_usd: 0.0,
                    elapsed_ms: 0,
                    status: "completed".into(),
                    decision: None,
                    error: None,
                };
                before.model_calls.push(requestless_record);
                // Only this synthetic test's persisted task is deliberately precharged.
                fs::write(
                    workspace.task_dir().join("task.json"),
                    serde_json::to_vec_pretty(&before).unwrap(),
                )
                .unwrap();
            }
            "tokens" => provider.estimate_tokens = Some(before.limits.max_total_tokens + 1),
            _ => provider.estimate_cost = Some(before.limits.max_estimated_cost_usd + 0.01),
        }
        assert!(smart_feedback::run(
            workspace.task_dir(),
            &provider,
            3,
            true,
            &CancelToken::default()
        )
        .is_err());
        assert_eq!(provider.count(), 0, "{budget}");
        let after = Workspace::snapshot(workspace.task_dir()).unwrap();
        assert_no_authority_change(&before, &after);
        assert_eq!(after.usage.model_calls, before.usage.model_calls);
    }
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; reaching round cap is not model acceptance"]
fn a_revision_at_the_last_round_is_not_selected_or_adoptable() {
    let root = tempfile::tempdir().unwrap();
    let (workspace, _, before) = task(root.path());
    let provider = SimulatedReviewer::new(Behavior::ReviseBlueThenAccept);
    let report = smart_feedback::run(
        workspace.task_dir(),
        &provider,
        1,
        true,
        &CancelToken::default(),
    )
    .unwrap();
    assert_eq!(provider.count(), 1);
    assert_eq!(report.rounds.len(), 1);
    assert_eq!(
        report.rounds[0].review.as_ref().unwrap().verdict,
        ReviewVerdict::Revise
    );
    assert!(report.selected_plan.is_none());
    assert_eq!(
        report.stopped_reason.as_deref(),
        Some("round_limit_reached")
    );
    assert!(smart_feedback::adopt(
        workspace.task_dir(),
        &report.run_id,
        &CancelToken::default()
    )
    .is_err());
    assert_no_authority_change(&before, &Workspace::snapshot(workspace.task_dir()).unwrap());
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; real overlapping run ownership, no model network"]
fn concurrent_smart_runs_have_one_owner_and_no_duplicate_external_admission() {
    let root = tempfile::tempdir().unwrap();
    let (workspace, _, _) = task(root.path());
    let (sender, entered) = mpsc::channel();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let mut first = SimulatedReviewer::new(Behavior::Accept);
    first.entered = Mutex::new(Some(sender));
    first.gate = Some(gate.clone());
    let first = Arc::new(first);
    let task_path = workspace.task_dir().to_path_buf();
    let worker_path = task_path.clone();
    let worker_provider = first.clone();
    let worker = std::thread::spawn(move || {
        smart_feedback::run(
            &worker_path,
            &*worker_provider,
            3,
            true,
            &CancelToken::default(),
        )
    });
    let reached = entered.recv_timeout(Duration::from_secs(30));
    if reached.is_err() {
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let result = worker.join().unwrap();
        panic!("First owner did not reach provider: {result:?}");
    }
    let second = Arc::new(SimulatedReviewer::new(Behavior::Accept));
    let second_provider = second.clone();
    let (completed, competing_result) = mpsc::channel();
    let competitor = std::thread::spawn(move || {
        completed
            .send(smart_feedback::run(
                &task_path,
                &*second_provider,
                3,
                true,
                &CancelToken::default(),
            ))
            .unwrap();
    });
    let competing = competing_result.recv_timeout(Duration::from_secs(5));
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    competitor.join().unwrap();
    let first_result = worker.join().unwrap();
    assert!(competing
        .expect("A competing owner must fail promptly, not block behind a live provider")
        .is_err());
    assert_eq!(second.count(), 0);
    assert!(first_result.unwrap().selected_plan.is_some());
    assert_eq!(first.count(), 1);
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; deliberately mutate only the synthetic source"]
fn source_changed_while_reviewing_cannot_become_an_accepted_or_adopted_candidate() {
    let root = tempfile::tempdir().unwrap();
    let (workspace, _, before) = task(root.path());
    let provider = SimulatedReviewer::new(Behavior::ChangeSyntheticSource);
    assert!(smart_feedback::run(
        workspace.task_dir(),
        &provider,
        3,
        true,
        &CancelToken::default()
    )
    .is_err());
    assert_eq!(provider.count(), 1);
    let after = Workspace::snapshot(workspace.task_dir()).unwrap();
    assert_no_authority_change(&before, &after);
    assert_eq!(after.usage.model_calls, 1);
    assert_eq!(
        (after.usage.input_tokens, after.usage.output_tokens),
        (80, 20)
    );
    assert!(!smart_feedback::is_adopted_model_plan(&after));
    let status = smart_feedback::status(workspace.task_dir())
        .unwrap()
        .unwrap();
    assert!(status.selected_plan.is_none());
}
