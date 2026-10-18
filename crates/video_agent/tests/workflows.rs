//! Real media tests are explicitly separate from the simulated vision provider.
//! Run: cargo test -p video_agent --test workflows -- --ignored --test-threads=1
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};
use video_agent::*;

fn source(root: &Path, name: &str, audio: bool) -> PathBuf {
    let path = root.join(name);
    let mut command = Command::new("ffmpeg");
    command.args([
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x180:rate=30:duration=4",
    ]);
    if audio {
        command.args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=4",
        ]);
    }
    command
        .args([
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
        ])
        .arg(&path);
    assert!(command.status().unwrap().success());
    path
}

fn limits() -> Limits {
    Limits {
        initial_frames: 4,
        max_frames: 12,
        max_clip_ms: 3000,
        ..Limits::default()
    }
}

fn task(root: &Path, sources: &[PathBuf]) -> Workspace {
    Workspace::create(
        &root.join("tasks"),
        sources,
        "Select visually useful intervals, without inferring audio".into(),
        2000,
        limits(),
    )
    .unwrap()
}

fn approve(workspace: &Workspace) -> Task {
    let task = workspace.load().unwrap();
    let digest = video_agent::workflow::plan_digest(&task, task.plan.as_ref().unwrap()).unwrap();
    workspace.approve(&digest).unwrap()
}

fn audio_rms(path: &Path, start_secs: &str) -> f64 {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args([
            "-ss", start_secs, "-t", "0.3", "-map", "0:a:0", "-ac", "1", "-ar", "8000", "-f",
            "s16le", "-",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let samples: Vec<f64> = output
        .stdout
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f64)
        .collect();
    assert!(!samples.is_empty());
    (samples.iter().map(|x| x * x).sum::<f64>() / samples.len() as f64).sqrt()
}

struct Simulated {
    calls: AtomicUsize,
    decisions: Mutex<Vec<VisionDecision>>,
}
impl Simulated {
    fn new(decisions: Vec<VisionDecision>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            decisions: Mutex::new(decisions),
        }
    }
}
impl VisionProvider for Simulated {
    fn identity(&self) -> (String, String, String) {
        (
            "SIMULATED-TEST-ONLY".into(),
            "not-a-real-model".into(),
            "test-v1".into(),
        )
    }
    fn ready(&self) -> bool {
        true
    }
    fn estimate(&self, request: &VisionRequest) -> Result<(u64, f64)> {
        Ok((u64::from(request.limits.max_output_tokens) + 100, 0.01))
    }
    fn decide(&self, request: &VisionRequest, cancel: &CancelToken) -> Result<VisionOutcome> {
        cancel.check()?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        let decision = self.decisions.lock().unwrap().remove(0);
        Ok(VisionOutcome {
            decision: decision.clone(),
            record: ModelCall {
                call_id: request.call_id.clone(),
                backend: "SIMULATED-TEST-ONLY".into(),
                model: "not-a-real-model".into(),
                requested_model: "not-a-real-model".into(),
                prompt_version: "test-v1".into(),
                sample_ids: vec![],
                provider_response_id: None,
                request_id: None,
                input_tokens: 80,
                output_tokens: 20,
                estimated_cost_usd: 0.001,
                elapsed_ms: 0,
                status: "received".into(),
                decision: Some(decision),
                error: None,
            },
        })
    }
}

fn proposal(sample: &Sample) -> VisionDecision {
    VisionDecision::ProposePlan {
        clips: vec![ClipProposal {
            source_id: sample.source_id.clone(),
            start_ms: sample.time_ms.saturating_sub(100),
            end_ms: sample.time_ms + 500,
            reason: "Synthetic test pattern visible in the submitted sample".into(),
            uncertainty: "Only one sampled frame, not a complete event".into(),
            evidence_ids: vec![sample.id.clone()],
        }],
        summary: "SIMULATED workflow test, no semantic-quality evidence".into(),
    }
}

#[test]
fn limits_reject_nan_and_unbounded_resources() {
    for invalid in [
        Limits {
            max_estimated_cost_usd: f64::NAN,
            ..Limits::default()
        },
        Limits {
            max_frames: 10000,
            ..Limits::default()
        },
        Limits {
            max_concurrency: 0,
            ..Limits::default()
        },
        Limits {
            max_concurrency: 5,
            ..Limits::default()
        },
    ] {
        assert!(video_agent::store::validate_limits(&invalid).is_err());
    }
}

#[test]
fn hashing_honors_cancellation_and_deadline_without_stale_claims() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("bytes.bin");
    fs::write(&path, vec![42u8; 1024]).unwrap();
    let cancel = CancelToken::default();
    cancel.cancel();
    assert!(matches!(
        video_agent::media::hash_file_bounded(&path, &cancel, Duration::from_secs(5)),
        Err(AgentError::Cancelled)
    ));
    assert!(matches!(
        video_agent::media::hash_file_bounded(&path, &CancelToken::default(), Duration::ZERO),
        Err(AgentError::TimedOut)
    ));
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; no real model is invoked"]
fn manual_human_edit_approval_export_and_idempotent_reopen() {
    let root = tempfile::tempdir().unwrap();
    let input = source(root.path(), "input.mp4", true);
    let original_hash = video_agent::media::hash_file(&input).unwrap();
    let workspace = task(root.path(), std::slice::from_ref(&input));
    workspace.sample(&CancelToken::default()).unwrap();
    let loaded = workspace.load().unwrap();
    assert_eq!(loaded.samples.len(), 4);
    assert_eq!(
        loaded.samples.iter().map(|s| s.time_ms).collect::<Vec<_>>(),
        vec![500, 1500, 2500, 3500]
    );
    let drafted = workspace.manual_draft().unwrap();
    assert!(drafted.model_calls.is_empty());
    assert!(workspace.export(&CancelToken::default()).is_err());
    approve(&workspace);
    let mut plan = drafted.plan.unwrap();
    plan.clips[0].start_ms = 300;
    plan.clips[0].end_ms = 1700;
    let edited = workspace.save_plan(plan).unwrap();
    assert!(edited.approval.is_none());
    assert_eq!(edited.plan.as_ref().unwrap().revision, 2);
    let old_digest = video_agent::workflow::plan_digest(
        &loaded,
        &EditPlan {
            revision: 1,
            clips: vec![],
            summary: "different".into(),
            audio_policy: AudioPolicy::Mute,
        },
    )
    .unwrap();
    assert!(workspace.approve(&old_digest).is_err());
    approve(&workspace);
    let completed = workspace.export(&CancelToken::default()).unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);
    let exported = completed.export.unwrap();
    assert!(exported.verified && exported.has_audio && exported.completed_at.is_some());
    video_agent::media::verify_output(
        &exported.path,
        1400,
        true,
        &CancelToken::default(),
        Duration::from_secs(30),
    )
    .unwrap();
    assert!(
        audio_rms(&exported.path, "0.3") > 1000.0,
        "Original sine audio must remain audible, not merely have a stream"
    );
    let reopened = Workspace::open(workspace.task_dir()).unwrap();
    let again = reopened.export(&CancelToken::default()).unwrap();
    assert_eq!(again.export.unwrap().sha256, exported.sha256);
    assert_eq!(
        fs::read_dir(workspace.task_dir().join("exports"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        video_agent::media::hash_file(&input).unwrap(),
        original_hash
    );
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; no real model is invoked"]
fn mixed_audio_sources_reorder_silence_and_mute() {
    let root = tempfile::tempdir().unwrap();
    let audible = source(root.path(), "audio.mp4", true);
    let silent = source(root.path(), "silent.mp4", false);
    let workspace = task(root.path(), &[audible, silent]);
    let mut plan = workspace.manual_draft().unwrap().plan.unwrap();
    // Manual initial draft exhausts the target in source1; explicitly ensure a
    // two-source draft by creating with a target requiring the second source.
    assert_eq!(plan.clips.len(), 1);
    let another = Workspace::create(
        &root.path().join("tasks"),
        &workspace
            .load()
            .unwrap()
            .assets
            .iter()
            .map(|a| a.path.clone())
            .collect::<Vec<_>>(),
        "Manual mixed audio test".into(),
        4500,
        limits(),
    )
    .unwrap();
    plan = another.manual_draft().unwrap().plan.unwrap();
    assert_eq!(plan.clips.len(), 2);
    plan.clips.reverse();
    plan.clips[0].end_ms = 900;
    plan.clips[1].end_ms = 1200;
    another.save_plan(plan).unwrap();
    approve(&another);
    let complete = another.export(&CancelToken::default()).unwrap();
    let record = complete.export.unwrap();
    video_agent::media::verify_output(
        &record.path,
        2100,
        true,
        &CancelToken::default(),
        Duration::from_secs(30),
    )
    .unwrap();
    assert!(
        audio_rms(&record.path, "0.2") < 10.0,
        "First reordered source had no audio; its normalized track must be silent"
    );
    assert!(
        audio_rms(&record.path, "1.2") > 1000.0,
        "Second reordered source's sine track must remain audible"
    );
    let mut plan = another.load().unwrap().plan.unwrap();
    plan.audio_policy = AudioPolicy::Mute;
    another.save_plan(plan).unwrap();
    approve(&another);
    let muted = another
        .export(&CancelToken::default())
        .unwrap()
        .export
        .unwrap();
    assert!(!muted.has_audio);
    video_agent::media::verify_output(
        &muted.path,
        2100,
        false,
        &CancelToken::default(),
        Duration::from_secs(30),
    )
    .unwrap();
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; simulated provider is not semantic validation"]
fn simulated_observe_propose_consent_provenance_and_budget() {
    let root = tempfile::tempdir().unwrap();
    let input = source(root.path(), "input.mp4", false);
    let workspace = task(root.path(), &[input]);
    let sampled = workspace.sample(&CancelToken::default()).unwrap();
    let mock = Simulated::new(vec![
        VisionDecision::ObserveInterval {
            source_id: "source-1".into(),
            start_ms: 0,
            end_ms: 1000,
            count: 2,
            reason: "Need denser samples within this interval".into(),
        },
        proposal(&sampled.samples[0]),
    ]);
    assert!(workspace.analyze(&mock, &CancelToken::default()).is_err());
    assert_eq!(mock.calls.load(Ordering::SeqCst), 0);
    workspace.set_consent(true).unwrap();
    let analyzed = workspace.analyze(&mock, &CancelToken::default()).unwrap();
    assert_eq!(analyzed.samples.len(), 6);
    assert_eq!(analyzed.usage.model_calls, 2);
    assert_eq!(analyzed.usage.input_tokens, 160);
    assert!(analyzed.approval.is_none());
    assert_eq!(analyzed.model_calls[1].sample_ids.len(), 6);
    let mut bad = analyzed.plan.unwrap();
    bad.clips[0].evidence_ids = vec!["invented-frame".into()];
    assert!(workspace.save_plan(bad).is_err());
    let invalid_provider = Simulated::new(vec![VisionDecision::ProposePlan {
        clips: vec![ClipProposal {
            source_id: "source-1".into(),
            start_ms: 0,
            end_ms: 500,
            reason: "Unsupported source interval".into(),
            uncertainty: "Unknown".into(),
            evidence_ids: vec!["source-1-3500".into()],
        }],
        summary: "invalid".into(),
    }]);
    assert!(workspace
        .analyze(&invalid_provider, &CancelToken::default())
        .is_err());
    assert!(workspace.load().unwrap().approval.is_none());
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; simulated provider is not semantic validation"]
fn rejected_proposal_stops_until_explicit_retry_and_retains_original_cost_and_decision() {
    struct FeedbackRecorder {
        inner: Simulated,
        feedback: Mutex<Vec<Option<String>>>,
    }
    impl VisionProvider for FeedbackRecorder {
        fn identity(&self) -> (String, String, String) {
            self.inner.identity()
        }
        fn ready(&self) -> bool {
            self.inner.ready()
        }
        fn estimate(&self, request: &VisionRequest) -> Result<(u64, f64)> {
            self.inner.estimate(request)
        }
        fn decide(&self, request: &VisionRequest, cancel: &CancelToken) -> Result<VisionOutcome> {
            self.feedback.lock().unwrap().push(request.feedback.clone());
            self.inner.decide(request, cancel)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let input = source(root.path(), "retry-fixture.mp4", false);
    let workspace = task(root.path(), &[input]);
    let sampled = workspace.sample(&CancelToken::default()).unwrap();
    workspace.set_consent(true).unwrap();
    let mut invalid = proposal(&sampled.samples[0]);
    if let VisionDecision::ProposePlan { clips, .. } = &mut invalid {
        clips[0].start_ms = 0;
        clips[0].end_ms = 3500; // Source is 4s, but max_clip_ms is 3000.
    }
    let provider = FeedbackRecorder {
        inner: Simulated::new(vec![invalid.clone(), proposal(&sampled.samples[0])]),
        feedback: Mutex::new(vec![]),
    };
    assert!(workspace
        .analyze(&provider, &CancelToken::default())
        .is_err());
    assert_eq!(
        provider.inner.calls.load(Ordering::SeqCst),
        1,
        "no automatic retry"
    );
    let rejected = workspace.load().unwrap();
    assert!(rejected.plan.is_none() && rejected.approval.is_none() && rejected.export.is_none());
    assert_eq!(rejected.usage.model_calls, 1);
    assert_eq!(rejected.model_calls[0].status, "rejected");
    assert_eq!(rejected.model_calls[0].decision.as_ref(), Some(&invalid));
    assert!((rejected.usage.estimated_cost_usd - 0.001).abs() < 1e-9);
    let prior_call = rejected.model_calls[0].clone();
    let original_limits = rejected.limits.clone();
    let retried = workspace
        .analyze(&provider, &CancelToken::default())
        .unwrap();
    assert_eq!(provider.inner.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        retried.model_calls[0], prior_call,
        "first failure remains intact"
    );
    assert_eq!(
        retried.limits, original_limits,
        "retry does not expand budgets"
    );
    assert_eq!(retried.usage.model_calls, 2);
    assert!((retried.usage.estimated_cost_usd - 0.002).abs() < 1e-9);
    assert_eq!(retried.model_calls[1].status, "validated");
    assert!(retried.approval.is_none() && retried.export.is_none());
    let feedback = provider.feedback.lock().unwrap();
    assert!(feedback[0].is_none());
    let correction = feedback[1].as_deref().unwrap();
    assert!(correction.contains("deterministic local validation"));
    assert!(correction.contains("maximum_clip_ms"));
    assert!(correction.contains("maximum_output_ms"));
    assert!(!correction.contains("retry-fixture.mp4"));
    assert!(!correction.contains(root.path().to_str().unwrap()));
    assert!(!correction.contains(prior_call.error.as_deref().unwrap()));
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; bounded-context simulated provider is not semantic validation"]
fn model_working_set_records_only_actual_submitted_ids_and_retains_all_saved_frames() {
    struct Captured {
        inner: Simulated,
        submitted: Mutex<Vec<Vec<String>>>,
    }
    impl VisionProvider for Captured {
        fn identity(&self) -> (String, String, String) {
            self.inner.identity()
        }
        fn ready(&self) -> bool {
            true
        }
        fn estimate(&self, request: &VisionRequest) -> Result<(u64, f64)> {
            self.inner.estimate(request)
        }
        fn decide(&self, request: &VisionRequest, cancel: &CancelToken) -> Result<VisionOutcome> {
            self.submitted
                .lock()
                .unwrap()
                .push(request.samples.iter().map(|s| s.id.clone()).collect());
            self.inner.decide(request, cancel)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let inputs = [
        source(root.path(), "first.mp4", false),
        source(root.path(), "second.mp4", false),
    ];
    let workspace = Workspace::create(
        &root.path().join("tasks"),
        &inputs,
        "Bounded context test only".into(),
        2000,
        Limits {
            initial_frames: 35,
            max_frames: 36,
            ..limits()
        },
    )
    .unwrap();
    let sampled = workspace.sample(&CancelToken::default()).unwrap();
    assert_eq!(sampled.samples.len(), 35);
    workspace.set_consent(true).unwrap();
    let first = sampled
        .samples
        .iter()
        .filter(|s| s.source_id == "source-1")
        .min_by_key(|s| s.time_ms)
        .unwrap();
    let provider = Captured {
        inner: Simulated::new(vec![proposal(first)]),
        submitted: Mutex::new(vec![]),
    };
    let analyzed = workspace
        .analyze(&provider, &CancelToken::default())
        .unwrap();
    assert_eq!(analyzed.samples, sampled.samples);
    let recorded = &analyzed.model_calls[0].sample_ids;
    assert_eq!(recorded.len(), 16);
    assert_eq!(&provider.submitted.lock().unwrap()[0], recorded);
    for source in ["source-1", "source-2"] {
        assert_eq!(
            recorded.iter().filter(|id| id.starts_with(source)).count(),
            8
        );
    }
    for sample in &sampled.samples {
        assert!(sample.image_path.is_file());
        assert_eq!(
            video_agent::media::hash_file(&sample.image_path).unwrap(),
            sample.sha256
        );
    }
    let omitted = sampled
        .samples
        .iter()
        .find(|s| !recorded.contains(&s.id))
        .unwrap();
    let invalid = Simulated::new(vec![proposal(omitted)]);
    assert!(workspace
        .analyze(&invalid, &CancelToken::default())
        .is_err());
    let rejected = workspace.load().unwrap();
    assert_eq!(rejected.model_calls[1].status, "rejected");
    assert!(!rejected.model_calls[1].sample_ids.contains(&omitted.id));
    assert!(rejected.model_calls[1]
        .error
        .as_deref()
        .unwrap()
        .contains("not submitted in this specific request"));
    assert_eq!(rejected.samples, sampled.samples);
    assert_eq!(rejected.limits, sampled.limits);
    assert!(rejected.approval.is_none() && rejected.export.is_none());
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; no real model is invoked"]
fn cancellation_source_change_and_output_protection() {
    let root = tempfile::tempdir().unwrap();
    let input = source(root.path(), "input.mp4", true);
    let workspace = task(root.path(), std::slice::from_ref(&input));
    let cancel = CancelToken::default();
    cancel.cancel();
    assert!(matches!(
        workspace.sample(&cancel),
        Err(AgentError::Cancelled)
    ));
    // Cancellation before an operation starts is a no-op; the separate live
    // cancellation test asserts persisted Cancelled after actual work begins.
    assert_eq!(workspace.load().unwrap().status, TaskStatus::Created);
    workspace.sample(&CancelToken::default()).unwrap();
    workspace.manual_draft().unwrap();
    let approved = approve(&workspace);
    let digest = approved.approval.unwrap().plan_digest;
    let output = workspace
        .task_dir()
        .join("exports")
        .join(format!("{digest}.mp4"));
    fs::write(&output, b"existing user file").unwrap();
    assert!(workspace.export(&CancelToken::default()).is_err());
    assert_eq!(fs::read(&output).unwrap(), b"existing user file");
    workspace.set_consent(true).unwrap();
    fs::write(&input, b"replacement source").unwrap();
    let stale = workspace.load().unwrap();
    assert_eq!(stale.status, TaskStatus::Stale);
    assert!(!stale.cloud_consent && stale.approval.is_none());
    assert!(workspace.approve("stale-digest").is_err());
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; no real model is invoked"]
fn crash_after_publish_reconciles_without_duplicate_and_missing_shard_recovers() {
    let root = tempfile::tempdir().unwrap();
    let input = source(root.path(), "input.mp4", true);
    let workspace = task(root.path(), &[input]);
    workspace.manual_draft().unwrap();
    approve(&workspace);
    let completed = workspace.export(&CancelToken::default()).unwrap();
    let output = completed.export.as_ref().unwrap().path.clone();
    // Explicit crash fixture: durable publication journal exists but final
    // completion write was lost. This is not a real model or live crash claim.
    let mut persisted = completed;
    persisted.status = TaskStatus::Exporting;
    persisted.export.as_mut().unwrap().completed_at = None;
    fs::write(
        workspace.task_dir().join("task.json"),
        serde_json::to_vec_pretty(&persisted).unwrap(),
    )
    .unwrap();
    let reopened = Workspace::open(workspace.task_dir()).unwrap();
    assert_eq!(
        reopened.export(&CancelToken::default()).unwrap().status,
        TaskStatus::Completed
    );
    assert_eq!(
        fs::read_dir(workspace.task_dir().join("exports"))
            .unwrap()
            .count(),
        1
    );
    // Remove only the test fixture's own generated output and cached shard to
    // emulate interrupted publication before their hard links were created.
    fs::remove_file(output).unwrap();
    fs::remove_file(&persisted.shards[0].path).unwrap();
    assert_eq!(
        reopened.export(&CancelToken::default()).unwrap().status,
        TaskStatus::Completed
    );
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; simulated provider is not semantic validation"]
fn hard_call_budget_retains_observations_and_explicit_retry_does_not_overrun() {
    let root = tempfile::tempdir().unwrap();
    let input = source(root.path(), "input.mp4", false);
    let mut capped = limits();
    capped.max_model_calls = 1;
    let workspace = Workspace::create(
        &root.path().join("tasks"),
        &[input],
        "Bounded observation fixture".into(),
        1000,
        capped,
    )
    .unwrap();
    workspace.sample(&CancelToken::default()).unwrap();
    workspace.set_consent(true).unwrap();
    let provider = Simulated::new(vec![VisionDecision::ObserveInterval {
        source_id: "source-1".into(),
        start_ms: 0,
        end_ms: 1000,
        count: 1,
        reason: "Resolve sample ambiguity".into(),
    }]);
    assert!(workspace
        .analyze(&provider, &CancelToken::default())
        .is_err());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(workspace
        .analyze(&provider, &CancelToken::default())
        .is_err());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(workspace.load().unwrap().usage.model_calls, 1);
    struct Overrun(Simulated);
    impl VisionProvider for Overrun {
        fn identity(&self) -> (String, String, String) {
            self.0.identity()
        }
        fn ready(&self) -> bool {
            true
        }
        fn estimate(&self, request: &VisionRequest) -> Result<(u64, f64)> {
            self.0.estimate(request)
        }
        fn decide(&self, request: &VisionRequest, cancel: &CancelToken) -> Result<VisionOutcome> {
            let mut outcome = self.0.decide(request, cancel)?;
            outcome.record.input_tokens = 100_000;
            outcome.record.estimated_cost_usd = 0.03;
            Ok(outcome)
        }
    }
    let input = workspace.load().unwrap().assets[0].path.clone();
    let overrun_workspace = task(root.path(), &[input]);
    overrun_workspace.sample(&CancelToken::default()).unwrap();
    overrun_workspace.set_consent(true).unwrap();
    let overrun = Overrun(Simulated::new(vec![VisionDecision::ProposePlan {
        clips: vec![],
        summary: "Test reservation overrun, no semantic claim".into(),
    }]));
    assert!(overrun_workspace
        .analyze(&overrun, &CancelToken::default())
        .is_err());
    let retained = overrun_workspace.load().unwrap();
    assert!(retained.plan.is_none());
    assert_eq!(retained.usage.input_tokens, 100_000);
    assert_eq!(retained.model_calls[0].status, "usage_exceeded_reservation");
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; simulated pre-send cancellation is not semantic validation"]
fn cancellation_before_provider_entry_is_not_sent_and_refunds_token_cost_reservation() {
    struct CancelDuringEstimate {
        cancel: CancelToken,
        entries: AtomicUsize,
    }
    impl VisionProvider for CancelDuringEstimate {
        fn identity(&self) -> (String, String, String) {
            (
                "SIMULATED-TEST-ONLY".into(),
                "not-a-real-model".into(),
                "test-v1".into(),
            )
        }
        fn ready(&self) -> bool {
            true
        }
        fn estimate(&self, request: &VisionRequest) -> Result<(u64, f64)> {
            self.cancel.cancel();
            Ok((u64::from(request.limits.max_output_tokens) + 100, 0.01))
        }
        fn decide(&self, _request: &VisionRequest, _cancel: &CancelToken) -> Result<VisionOutcome> {
            self.entries.fetch_add(1, Ordering::SeqCst);
            panic!("Provider must not be entered after local cancellation");
        }
    }
    let root = tempfile::tempdir().unwrap();
    let input = source(root.path(), "input.mp4", false);
    let workspace = task(root.path(), &[input]);
    workspace.sample(&CancelToken::default()).unwrap();
    workspace.set_consent(true).unwrap();
    let cancel = CancelToken::default();
    let provider = CancelDuringEstimate {
        cancel: cancel.clone(),
        entries: AtomicUsize::new(0),
    };
    assert!(matches!(
        workspace.analyze(&provider, &cancel),
        Err(AgentError::Cancelled)
    ));
    assert_eq!(provider.entries.load(Ordering::SeqCst), 0);
    let retained = Workspace::snapshot(workspace.task_dir()).unwrap();
    assert_eq!(retained.status, TaskStatus::Cancelled);
    assert_eq!(retained.usage.model_calls, 1);
    assert_eq!(retained.usage.input_tokens, 0);
    assert_eq!(retained.usage.output_tokens, 0);
    assert_eq!(retained.usage.estimated_cost_usd, 0.0);
    assert_eq!(retained.model_calls[0].status, "not_sent");
    assert!(retained.model_calls[0].provider_response_id.is_none());
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; interrupts a live encoder workflow"]
fn live_cancel_after_completed_shard_then_restart_reuses_it() {
    let root = tempfile::tempdir().unwrap();
    let first = source(root.path(), "first.mp4", true);
    let second = source(root.path(), "second.mp4", false);
    let workspace = Workspace::create(
        &root.path().join("tasks"),
        &[first, second],
        "Manual live cancellation fixture".into(),
        6000,
        limits(),
    )
    .unwrap();
    workspace.manual_draft().unwrap();
    approve(&workspace);
    let cancel = CancelToken::default();
    let worker_workspace = workspace.clone();
    let worker_cancel = cancel.clone();
    let handle = std::thread::spawn(move || worker_workspace.export(&worker_cancel));
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut saw_shard = false;
    while std::time::Instant::now() < deadline {
        if let Ok(bytes) = fs::read(workspace.task_dir().join("task.json")) {
            if let Ok(task) = serde_json::from_slice::<Task>(&bytes) {
                if !task.shards.is_empty() && task.status == TaskStatus::Exporting {
                    saw_shard = true;
                    cancel.cancel();
                    break;
                }
            }
        }
        if handle.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        saw_shard,
        "fixture did not observe the live intermediate checkpoint"
    );
    assert!(matches!(handle.join().unwrap(), Err(AgentError::Cancelled)));
    let reopened = Workspace::open(workspace.task_dir()).unwrap();
    assert_eq!(reopened.load().unwrap().status, TaskStatus::Cancelled);
    let completed = reopened.export(&CancelToken::default()).unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);
    assert!(completed.history.iter().any(|e| e.kind == "shard_reused"));
    video_agent::media::verify_output(
        &completed.export.unwrap().path,
        6000,
        true,
        &CancelToken::default(),
        Duration::from_secs(30),
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
#[ignore = "requires real FFmpeg/ffprobe; SIGKILLs only the production export owner, then test-cleans orphan media"]
fn hard_kill_export_owner_then_independent_resume_reuses_verified_shard() {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    const MODE: &str = "VIDEO_AGENT_EXPORT_KILL_TEST_MODE";
    const ROOT: &str = "VIDEO_AGENT_EXPORT_KILL_TEST_ROOT";
    if let Ok(mode) = std::env::var(MODE) {
        assert!(mode == "first_export" || mode == "resume_export");
        let dir = PathBuf::from(std::env::var_os(ROOT).unwrap());
        let workspace = Workspace::open(&dir).unwrap();
        let completed = workspace.export(&CancelToken::default()).unwrap();
        if mode == "first_export" {
            panic!("Fixture missed the live shard checkpoint before SIGKILL");
        }
        assert_eq!(completed.status, TaskStatus::Completed);
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let first = source(root.path(), "first.mp4", true);
    let second = source(root.path(), "second.mp4", false);
    let original_hashes = [
        media::hash_file(&first).unwrap(),
        media::hash_file(&second).unwrap(),
    ];
    let mut bounds = limits();
    bounds.process_timeout_secs = 60;
    let workspace = Workspace::create(
        &root.path().join("tasks"),
        &[first.clone(), second.clone()],
        "Manual production export hard-kill recovery fixture".into(),
        6000,
        bounds,
    )
    .unwrap();
    workspace.manual_draft().unwrap();
    approve(&workspace);
    let before = Workspace::snapshot(workspace.task_dir()).unwrap();
    let executable = std::env::current_exe().unwrap();
    let test_name = "hard_kill_export_owner_then_independent_resume_reuses_verified_shard";
    let mut worker = Command::new(&executable)
        .args(["--exact", test_name, "--ignored", "--nocapture"])
        .env(MODE, "first_export")
        .env(ROOT, workspace.task_dir())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let checkpoint = loop {
        if let Ok(saved) = Workspace::snapshot(workspace.task_dir()) {
            if saved.status == TaskStatus::Exporting
                && saved.shards.first().is_some_and(|s| s.path.is_file())
            {
                break saved;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "No live verified shard checkpoint"
        );
        assert!(
            worker.try_wait().unwrap().is_none(),
            "Export ended before the hard-kill checkpoint"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(store_lock_busy(workspace.task_dir()));
    // SIGKILL only the owner; media may briefly be orphaned. Keep its PID unreaped
    // while checking and cleaning the owned process group.
    worker.kill().unwrap();
    let worker_pid = worker.id();
    assert!(worker_pid > 1);
    assert_ne!(worker_pid, std::process::id());
    let process_group_of = |pid: u32| {
        let output = Command::new("ps")
            .args(["-o", "pgid=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Could not verify owned process group"
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap()
    };
    assert_eq!(process_group_of(worker_pid), worker_pid);
    assert_ne!(process_group_of(std::process::id()), worker_pid);
    // The product's watchdog dies with its owner; this cleanup is test-only,
    // not evidence of an OS-enforced orphan-process runtime quota.
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{worker_pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    assert!(!worker.wait().unwrap().success());
    let stopped = Workspace::snapshot(workspace.task_dir()).unwrap();
    assert_ne!(stopped.status, TaskStatus::Completed);
    assert!(!stopped.export.as_ref().is_some_and(|e| e.verified));
    let completed_shard = &checkpoint.shards[0];
    assert_eq!(
        media::hash_file(&completed_shard.path).unwrap(),
        completed_shard.sha256
    );
    assert!(Command::new(&executable)
        .args(["--exact", test_name, "--ignored", "--nocapture"])
        .env(MODE, "resume_export")
        .env(ROOT, workspace.task_dir())
        .status()
        .unwrap()
        .success());
    let resumed = workspace.load().unwrap();
    assert_eq!(resumed.status, TaskStatus::Completed);
    assert_eq!(resumed.usage.conservative_elapsed_ms, 60_000);
    assert!(resumed.usage.elapsed_ms >= before.usage.elapsed_ms + 60_000);
    assert_eq!(resumed.usage.execution_reserved_ms, 0);
    assert!(resumed.history.iter().any(|e| e.kind == "shard_reused"));
    let export = resumed.export.unwrap();
    media::verify_output(
        &export.path,
        6000,
        true,
        &CancelToken::default(),
        Duration::from_secs(30),
    )
    .unwrap();
    assert_eq!(
        fs::read_dir(workspace.task_dir().join("exports"))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "mp4"))
            .count(),
        1
    );
    assert_eq!(media::hash_file(&first).unwrap(), original_hashes[0]);
    assert_eq!(media::hash_file(&second).unwrap(), original_hashes[1]);
}

#[cfg(unix)]
fn store_lock_busy(root: &Path) -> bool {
    use fs2::FileExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("task.lock"))
        .unwrap();
    file.try_lock_exclusive().is_err()
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; simulated provider is not semantic validation"]
fn received_model_result_is_reused_after_cancel_without_second_request() {
    struct CancelAfterResponse(Simulated);
    impl VisionProvider for CancelAfterResponse {
        fn identity(&self) -> (String, String, String) {
            self.0.identity()
        }
        fn ready(&self) -> bool {
            // Simulate credentials disappearing after the first completed
            // response. Replaying a saved result must not require a new key.
            self.0.calls.load(Ordering::SeqCst) == 0
        }
        fn estimate(&self, request: &VisionRequest) -> Result<(u64, f64)> {
            self.0.estimate(request)
        }
        fn decide(&self, request: &VisionRequest, cancel: &CancelToken) -> Result<VisionOutcome> {
            let outcome = self.0.decide(request, cancel)?;
            cancel.cancel();
            Ok(outcome)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let input = source(root.path(), "input.mp4", false);
    let workspace = task(root.path(), &[input]);
    let sampled = workspace.sample(&CancelToken::default()).unwrap();
    workspace.set_consent(true).unwrap();
    let provider = CancelAfterResponse(Simulated::new(vec![proposal(&sampled.samples[0])]));
    assert!(matches!(
        workspace.analyze(&provider, &CancelToken::default()),
        Err(AgentError::Cancelled)
    ));
    assert_eq!(
        Workspace::snapshot(workspace.task_dir())
            .unwrap()
            .model_calls[0]
            .status,
        "received"
    );
    let completed = Workspace::open(workspace.task_dir())
        .unwrap()
        .analyze(&provider, &CancelToken::default())
        .unwrap();
    assert_eq!(provider.0.calls.load(Ordering::SeqCst), 1);
    assert_eq!(completed.usage.model_calls, 1);
    assert_eq!(completed.status, TaskStatus::Review);
    assert!(completed
        .history
        .iter()
        .any(|e| e.kind == "model_result_reused"));
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; no real model is invoked"]
fn exhausted_budget_is_checked_before_processing_preflight_full_hash() {
    let root = tempfile::tempdir().unwrap();
    let input = source(root.path(), "input.mp4", false);
    let workspace = task(root.path(), &[input]);
    let mut persisted = Workspace::snapshot(workspace.task_dir()).unwrap();
    persisted.usage.elapsed_ms = persisted.limits.max_execution_secs * 1000;
    // A bad fingerprint would yield Stale if source checking ran first.
    persisted.assets[0].sha256 = "0".repeat(64);
    fs::write(
        workspace.task_dir().join("task.json"),
        serde_json::to_vec(&persisted).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        workspace.sample(&CancelToken::default()),
        Err(AgentError::TimedOut)
    ));
    assert_eq!(
        Workspace::snapshot(workspace.task_dir()).unwrap().status,
        TaskStatus::Created
    );
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; verifies decoded timecodes, not model semantics"]
fn low_fps_and_nonzero_start_samples_use_actual_decoded_pts_and_deduplicate() {
    let root = tempfile::tempdir().unwrap();
    for offset in ["0", "5"] {
        let input = root.path().join(format!("two-fps-offset-{offset}.mp4"));
        assert!(Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=2:duration=4",
                "-c:v",
                "libx264",
                "-threads",
                "1",
                "-output_ts_offset",
                offset
            ])
            .arg(&input)
            .status()
            .unwrap()
            .success());
        let custom = Limits {
            initial_frames: 3,
            ..limits()
        };
        let workspace = Workspace::create(
            &root.path().join("tasks"),
            &[input],
            "Use actual visible sample timestamps".into(),
            1000,
            custom,
        )
        .unwrap();
        let sampled = workspace.sample(&CancelToken::default()).unwrap();
        assert_eq!(
            sampled
                .samples
                .iter()
                .map(|sample| sample.time_ms)
                .collect::<Vec<_>>(),
            vec![1000, 2000, 3500]
        );
        assert!(sampled
            .history
            .iter()
            .any(|event| event.kind == "sample_observed"
                && event.detail.contains("\"requested_time_ms\":666")
                && event.detail.contains("\"decoded_time_ms\":1000")));
        let repeated = Workspace::open(workspace.task_dir())
            .unwrap()
            .sample(&CancelToken::default())
            .unwrap();
        assert_eq!(repeated.usage.frames_created, 3);
        workspace.set_consent(true).unwrap();
        let provider = Simulated::new(vec![
            VisionDecision::ObserveInterval {
                source_id: "source-1".into(),
                start_ms: 0,
                end_ms: 1000,
                count: 3,
                reason: "Resolve sparse-frame uncertainty".into(),
            },
            proposal(&sampled.samples[0]),
        ]);
        let analyzed = workspace
            .analyze(&provider, &CancelToken::default())
            .unwrap();
        assert_eq!(analyzed.samples.len(), 4);
        assert_eq!(analyzed.usage.frames_created, 4);
        assert_eq!(
            analyzed.samples.iter().filter(|s| s.time_ms == 500).count(),
            1
        );
        assert_eq!(analyzed.model_calls[1].sample_ids.len(), 4);
    }
    let input = root.path().join("variable-frame-rate.mp4");
    assert!(Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=10:duration=4",
            "-vf",
            "select='not(mod(n,3))+not(mod(n,7))'",
            "-fps_mode",
            "vfr",
            "-c:v",
            "libx264",
            "-threads",
            "1"
        ])
        .arg(&input)
        .status()
        .unwrap()
        .success());
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "frame=best_effort_timestamp_time",
            "-of",
            "json",
        ])
        .arg(&input)
        .output()
        .unwrap();
    assert!(probe.status.success());
    let frames: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    let timestamps: Vec<u64> = frames["frames"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|frame| frame["best_effort_timestamp_time"].as_str())
        .map(|value| (value.parse::<f64>().unwrap() * 1000.0).round() as u64)
        .collect();
    let deltas: std::collections::HashSet<_> = timestamps
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect();
    assert!(
        deltas.len() > 1,
        "Fixture must actually contain variable frame intervals"
    );
    let workspace = Workspace::create(
        &root.path().join("tasks"),
        &[input],
        "Variable frame-rate timecodes".into(),
        1000,
        Limits {
            initial_frames: 3,
            ..limits()
        },
    )
    .unwrap();
    let sampled = workspace.sample(&CancelToken::default()).unwrap();
    let duration = sampled.assets[0].duration_ms;
    let expected: Vec<_> = (0..3)
        .map(|n| duration * (2 * n + 1) / 6)
        .map(|requested| {
            *timestamps
                .iter()
                .find(|actual| **actual >= requested)
                .unwrap()
        })
        .collect();
    assert_eq!(
        sampled
            .samples
            .iter()
            .map(|sample| sample.time_ms)
            .collect::<Vec<_>>(),
        expected
    );
}
