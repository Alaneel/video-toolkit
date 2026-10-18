//! Local FFmpeg parallelism, not real-model semantic validation.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
use video_agent::*;

fn source(root: &Path, low_fps: bool) -> PathBuf {
    let path = root.join(if low_fps {
        "sparse.mp4"
    } else {
        "parallel.mp4"
    });
    let input = if low_fps {
        "testsrc2=size=160x90:rate=2:duration=4"
    } else {
        "testsrc2=size=1280x720:rate=24:duration=8"
    };
    assert!(Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            input,
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-threads",
            "1"
        ])
        .arg(&path)
        .status()
        .unwrap()
        .success());
    path
}

fn create(root: &Path, input: &Path, workers: usize, initial: u32, maximum: u32) -> Workspace {
    Workspace::create(
        &root.join("tasks"),
        &[input.to_path_buf()],
        "Parallel local observation fixture; no semantic-quality claim".into(),
        1000,
        Limits {
            max_concurrency: workers,
            initial_frames: initial,
            max_frames: maximum,
            ..Limits::default()
        },
    )
    .unwrap()
}

fn child_peak(task: &Task, required_children: usize) -> usize {
    task.history
        .iter()
        .filter(|e| e.kind == "frame_batch_workers")
        .filter_map(|event| {
            let value: serde_json::Value = serde_json::from_str(&event.detail).unwrap();
            let children = value["children"].as_array().unwrap();
            if children.len() != required_children {
                return None;
            }
            let mut boundaries = Vec::new();
            let mut pids = std::collections::HashSet::new();
            for child in children {
                assert!(pids.insert(child["pid"].as_u64().unwrap()));
                let start = child["spawned_us"].as_u64().unwrap();
                let end = child["reaped_us"].as_u64().unwrap();
                assert!(end > start);
                boundaries.push((start, 1i32));
                boundaries.push((end, -1i32));
            }
            boundaries.sort_unstable(); // End before start when boundaries touch.
            let mut active = 0i32;
            let mut peak = 0;
            for (_, change) in boundaries {
                active += change;
                peak = peak.max(active);
            }
            Some(peak as usize)
        })
        .max()
        .unwrap_or(0)
}

#[test]
fn local_frame_worker_limits_and_usage_migration_are_bounded() {
    for workers in 1..=4 {
        video_agent::store::validate_limits(&Limits {
            max_concurrency: workers,
            ..Limits::default()
        })
        .unwrap();
    }
    for workers in [0, 5, usize::MAX] {
        assert!(video_agent::store::validate_limits(&Limits {
            max_concurrency: workers,
            ..Limits::default()
        })
        .is_err());
    }
    let mut old = serde_json::to_value(Usage::default()).unwrap();
    old.as_object_mut().unwrap().remove("peak_media_workers");
    old.as_object_mut().unwrap().remove("media_worker_ms");
    let restored: Usage = serde_json::from_value(old).unwrap();
    assert_eq!(restored.peak_media_workers, 0);
    assert_eq!(restored.media_worker_ms, 0);
}

#[test]
#[ignore = "requires real FFmpeg; simulates external source mutation during active workers"]
fn source_change_during_parallel_batch_invalidates_task_before_any_sample_publication() {
    use std::io::Write;
    let root = tempfile::tempdir().unwrap();
    let input = source(root.path(), false);
    let workspace = create(root.path(), &input, 4, 4, 8);
    workspace.set_consent(true).unwrap();
    let task_dir = workspace.task_dir().to_path_buf();
    let worker_dir = task_dir.clone();
    let worker = std::thread::spawn(move || {
        Workspace::open(&worker_dir)
            .unwrap()
            .sample(&CancelToken::default())
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    while temporary_workers(&task_dir) < 2 {
        assert!(Instant::now() < deadline, "No live extraction workers");
        assert!(
            !worker.is_finished(),
            "Sampler ended before source mutation checkpoint"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    // This is an external writer in the test, never a product write to input.
    fs::OpenOptions::new()
        .append(true)
        .open(&input)
        .unwrap()
        .write_all(b"external mutation fixture")
        .unwrap();
    assert!(worker.join().unwrap().is_err());
    let invalidated = workspace.load().unwrap();
    assert_eq!(invalidated.status, TaskStatus::Stale);
    assert!(!invalidated.cloud_consent);
    assert!(invalidated.approval.is_none());
    assert!(invalidated.samples.is_empty());
    assert_eq!(invalidated.usage.frames_created, 0);
    assert_eq!(temporary_workers(&task_dir), 0);
}

#[test]
#[ignore = "requires real FFmpeg; measures actual child spawn-to-reap overlap, not CPU utilization"]
fn actual_ffmpeg_children_overlap_at_two_and_four_worker_limits() {
    let root = tempfile::tempdir().unwrap();
    let input = source(root.path(), false);
    let original = media::hash_file(&input).unwrap();
    for workers in [1, 2, 4] {
        let workspace = create(root.path(), &input, workers, 8, 12);
        let sampled = workspace.sample(&CancelToken::default()).unwrap();
        assert_eq!(sampled.samples.len(), 8);
        assert_eq!(sampled.usage.peak_media_workers, workers as u32);
        assert!(sampled.usage.media_worker_ms > 0);
        assert_eq!(child_peak(&sampled, workers), workers);
        let before = sampled.usage.media_worker_ms;
        let again = Workspace::open(workspace.task_dir())
            .unwrap()
            .sample(&CancelToken::default())
            .unwrap();
        assert_eq!(again.usage.frames_created, 8);
        assert_eq!(
            again.usage.media_worker_ms, before,
            "Verified existing seeks must not spawn workers again"
        );
    }
    assert_eq!(media::hash_file(&input).unwrap(), original);
}

struct ObserveThenPropose {
    calls: AtomicUsize,
    count: u32,
}
impl VisionProvider for ObserveThenPropose {
    fn identity(&self) -> (String, String, String) {
        (
            "SIMULATED-TEST-ONLY".into(),
            "not-a-real-model".into(),
            "parallel-test-v1".into(),
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
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        let decision = if index == 0 {
            VisionDecision::ObserveInterval {
                source_id: "source-1".into(),
                start_ms: 0,
                end_ms: 1000,
                count: self.count,
                reason: "Resolve a sparse synthetic observation interval".into(),
            }
        } else {
            let sample = &request.samples[0];
            VisionDecision::ProposePlan {
                clips: vec![ClipProposal {
                    source_id: sample.source_id.clone(),
                    start_ms: sample.time_ms.saturating_sub(100),
                    end_ms: sample.time_ms + 400,
                    reason: "Synthetic pattern in the submitted sample".into(),
                    uncertainty: "SIMULATED provider, no real semantic validation".into(),
                    evidence_ids: vec![sample.id.clone()],
                }],
                summary: "SIMULATED parallel observation fixture".into(),
            }
        };
        Ok(VisionOutcome {
            decision: decision.clone(),
            record: ModelCall {
                call_id: request.call_id.clone(),
                backend: "SIMULATED-TEST-ONLY".into(),
                requested_model: "not-a-real-model".into(),
                model: "not-a-real-model".into(),
                prompt_version: "parallel-test-v1".into(),
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

#[test]
#[ignore = "requires real FFmpeg; simulated provider only requests local observations"]
fn parallel_followup_observations_deduplicate_actual_low_fps_pts_and_enforce_frame_credits() {
    let root = tempfile::tempdir().unwrap();
    let input = source(root.path(), true);
    let workspace = create(root.path(), &input, 4, 1, 5);
    workspace.sample(&CancelToken::default()).unwrap();
    workspace.set_consent(true).unwrap();
    let provider = ObserveThenPropose {
        calls: AtomicUsize::new(0),
        count: 4,
    };
    let analyzed = workspace
        .analyze(&provider, &CancelToken::default())
        .unwrap();
    assert_eq!(analyzed.status, TaskStatus::Review);
    assert_eq!(analyzed.usage.peak_media_workers, 4);
    assert_eq!(analyzed.usage.frames_created, 3);
    assert_eq!(
        analyzed
            .samples
            .iter()
            .map(|s| s.time_ms)
            .collect::<Vec<_>>(),
        vec![2000, 500, 1000]
    );
    assert_eq!(analyzed.model_calls[1].sample_ids.len(), 3);
    assert_eq!(
        analyzed
            .history
            .iter()
            .filter(|e| e.kind == "sample_observed")
            .count(),
        5
    );
    let reopened = Workspace::open(workspace.task_dir())
        .unwrap()
        .load()
        .unwrap();
    assert_eq!(reopened.usage.frames_created, 3);

    let limited = create(root.path(), &input, 4, 1, 4);
    limited.sample(&CancelToken::default()).unwrap();
    limited.set_consent(true).unwrap();
    let provider = ObserveThenPropose {
        calls: AtomicUsize::new(0),
        count: 4,
    };
    assert!(limited.analyze(&provider, &CancelToken::default()).is_err());
    let saved = limited.load().unwrap();
    assert_eq!(saved.usage.frames_created, 1);
    assert_eq!(saved.usage.peak_media_workers, 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

fn temporary_workers(dir: &Path) -> usize {
    fs::read_dir(dir.join("frames"))
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("frame-"))
        .count()
}

#[test]
#[ignore = "requires real FFmpeg; cancels an active parallel batch and resumes prior committed samples"]
fn cancelling_parallel_batch_joins_workers_retains_prior_batch_and_resumes_without_duplicate_samples(
) {
    let root = tempfile::tempdir().unwrap();
    let input = source(root.path(), false);
    let original = media::hash_file(&input).unwrap();
    let workspace = create(root.path(), &input, 4, 16, 20);
    let task_dir = workspace.task_dir().to_path_buf();
    let cancel = CancelToken::default();
    let worker_cancel = cancel.clone();
    let worker_dir = task_dir.clone();
    let worker =
        std::thread::spawn(move || Workspace::open(&worker_dir).unwrap().sample(&worker_cancel));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let task = Workspace::snapshot(&task_dir).unwrap();
        if task.samples.len() >= 4 && temporary_workers(&task_dir) >= 2 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Did not observe active second extraction batch"
        );
        assert!(
            !worker.is_finished(),
            "Sampling finished before cancellation checkpoint"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    cancel.cancel();
    assert!(matches!(worker.join().unwrap(), Err(AgentError::Cancelled)));
    assert_eq!(
        temporary_workers(&task_dir),
        0,
        "All worker TempDirs must be gone when cancellation returns"
    );
    let stopped = workspace.load().unwrap();
    assert_eq!(stopped.status, TaskStatus::Cancelled);
    assert!(stopped.samples.len() >= 4 && stopped.samples.len() < 16);
    let before = fs::read(task_dir.join("task.json")).unwrap();
    std::thread::sleep(Duration::from_millis(75));
    assert_eq!(
        fs::read(task_dir.join("task.json")).unwrap(),
        before,
        "No background worker may publish after cancellation returns"
    );
    let resumed = Workspace::open(&task_dir)
        .unwrap()
        .sample(&CancelToken::default())
        .unwrap();
    assert_eq!(resumed.samples.len(), 16);
    assert_eq!(resumed.usage.frames_created, 16);
    assert_eq!(
        resumed
            .samples
            .iter()
            .map(|s| &s.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        16
    );
    assert_eq!(resumed.usage.execution_reserved_ms, 0);
    assert_eq!(media::hash_file(&input).unwrap(), original);
}

#[cfg(unix)]
#[test]
#[ignore = "requires real FFmpeg; SIGKILLs production parallel sampler owner, test-cleans orphan media, resumes in second process"]
fn hard_kill_parallel_sampler_charges_one_batch_bound_and_resumes_unique_samples() {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    const MODE: &str = "VIDEO_AGENT_PARALLEL_KILL_TEST_MODE";
    const ROOT: &str = "VIDEO_AGENT_PARALLEL_KILL_TEST_ROOT";
    if let Ok(mode) = std::env::var(MODE) {
        assert!(mode == "first_sample" || mode == "resume_sample");
        let path = PathBuf::from(std::env::var_os(ROOT).unwrap());
        let sampled = Workspace::open(&path)
            .unwrap()
            .sample(&CancelToken::default())
            .unwrap();
        assert_ne!(
            mode, "first_sample",
            "Missed active parallel batch checkpoint"
        );
        assert_eq!(sampled.samples.len(), 16);
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let input = source(root.path(), false);
    let original = media::hash_file(&input).unwrap();
    let workspace = Workspace::create(
        &root.path().join("tasks"),
        std::slice::from_ref(&input),
        "Parallel hard-kill accounting fixture; no model".into(),
        1000,
        Limits {
            initial_frames: 16,
            max_frames: 20,
            max_concurrency: 4,
            process_timeout_secs: 10,
            model_timeout_secs: 10,
            ..Limits::default()
        },
    )
    .unwrap();
    let executable = std::env::current_exe().unwrap();
    let name = "hard_kill_parallel_sampler_charges_one_batch_bound_and_resumes_unique_samples";
    let mut worker = Command::new(&executable)
        .args(["--exact", name, "--ignored", "--nocapture"])
        .env(MODE, "first_sample")
        .env(ROOT, workspace.task_dir())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let saved = Workspace::snapshot(workspace.task_dir()).unwrap();
        if saved.samples.len() >= 4 && temporary_workers(workspace.task_dir()) >= 2 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "No live second parallel batch checkpoint"
        );
        assert!(
            worker.try_wait().unwrap().is_none(),
            "Sampler ended before kill checkpoint"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    // Kill only the owner, keeping its PID unreaped during group verification.
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
    // Test-only orphan cleanup is not an application CPU/wall-time quota.
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{worker_pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    assert!(!worker.wait().unwrap().success());
    let stopped = Workspace::snapshot(workspace.task_dir()).unwrap();
    assert!(stopped.samples.len() >= 4 && stopped.samples.len() < 16);
    assert_eq!(stopped.usage.execution_reserved_ms, 10_000);
    let old_samples = stopped.samples.clone();
    assert!(Command::new(&executable)
        .args(["--exact", name, "--ignored", "--nocapture"])
        .env(MODE, "resume_sample")
        .env(ROOT, workspace.task_dir())
        .status()
        .unwrap()
        .success());
    let resumed = workspace.load().unwrap();
    assert_eq!(
        resumed.usage.conservative_elapsed_ms, 10_000,
        "One batch bound, not four per-worker charges"
    );
    assert_eq!(resumed.usage.execution_reserved_ms, 0);
    assert_eq!(resumed.usage.frames_created, 16);
    assert_eq!(resumed.samples.len(), 16);
    assert_eq!(
        resumed
            .samples
            .iter()
            .map(|s| &s.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        16
    );
    for sample in old_samples {
        assert!(resumed.samples.contains(&sample));
        assert_eq!(media::hash_file(&sample.image_path).unwrap(), sample.sha256);
    }
    let reopened = workspace.load().unwrap();
    assert_eq!(reopened.usage.conservative_elapsed_ms, 10_000);
    assert_eq!(reopened.usage.model_calls, 0);
    assert_eq!(media::hash_file(&input).unwrap(), original);
}
