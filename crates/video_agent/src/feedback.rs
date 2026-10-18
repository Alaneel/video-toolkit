//! Bounded, approval-free what-if previews, not final exports or semantic scores.
//! Reuses the task lock, source checks, execution ledger and fixed media backend.
use crate::domain::*;
use crate::execution::Clock;
use crate::store::{self, invalid, now};
use crate::workflow::{check_sources_during, plan_digest, validate_plan};
use crate::{media, Workspace};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Lifetime preview admission limit per task, not per iterate invocation.
pub const MAX_ATTEMPTS: u32 = 8;
const MAX_EVENTS: usize = 128;
pub const MAX_PREVIEW_MS: u64 = 30_000;
pub const MAX_CLIPS: usize = 32;
/// Post-generation acceptance limit per shard/final MP4; not an OS quota.
pub const MAX_MEDIA_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_PLAN_BYTES: u64 = 65_536;
const STATE: &str = "preview-trajectory.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationReport {
    pub schema: String,
    pub plan_digest: String,
    pub revision: u64,
    pub clip_count: usize,
    pub plan_duration_ms: u64,
    pub target_duration_ms: u64,
    pub sources_verified: bool,
    pub semantic_evaluation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityReport {
    pub schema: String,
    pub plan_digest: String,
    pub objective_pass: bool,
    pub semantic_evaluation: String,
    pub checks: Vec<QualityCheck>,
    pub plan_duration_ms: u64,
    pub target_duration_ms: u64,
    pub observed_duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
    pub matches_current_plan: bool,
    pub needs_human_review: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewBundle {
    pub schema: String,
    pub bundle_id: String,
    pub plan_digest: String,
    pub attempt: u32,
    pub relative_dir: String,
    pub media_path: String,
    pub plan_path: String,
    pub quality_path: String,
    pub manifest_path: String,
    pub sha256: String,
    pub created_at: String,
    pub verification_scope: String,
    pub quality: QualityReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrajectoryEvent {
    pub sequence: usize,
    pub at: String,
    pub kind: String,
    pub attempt: u32,
    pub plan_digest: String,
    pub bundle_id: Option<String>,
    pub objective_pass: Option<bool>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trajectory {
    pub schema: String,
    pub task_id: String,
    pub attempts_started: u32,
    pub max_attempts: u32,
    pub events: Vec<TrajectoryEvent>,
    pub latest: Option<PreviewBundle>,
    pub verification_scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IterationReport {
    pub schema: String,
    pub attempts: Vec<PreviewBundle>,
    pub stopped_reason: String,
    pub selected_plan: Option<String>,
    pub needs_human_review: bool,
    pub task_plan_unchanged: bool,
}

fn read_json<T: DeserializeOwned>(path: &Path, limit: u64) -> Result<T> {
    store::regular(path)?;
    if path.metadata()?.len() > limit {
        return Err(invalid("Preview metadata exceeds size limit"));
    }
    Ok(serde_json::from_reader(File::open(path)?)?)
}

fn write_new(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn save_state(root: &Path, state: &Trajectory) -> Result<()> {
    let path = root.join(STATE);
    if fs::symlink_metadata(&path).is_ok() {
        store::regular(&path)?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer_pretty(&mut temporary, state)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    File::open(root)?.sync_all()?;
    Ok(())
}

fn hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn validate_bundle(bundle: &PreviewBundle) -> Result<()> {
    let id = format!("{:02}-{}", bundle.attempt, bundle.plan_digest);
    let directory = format!("previews/{id}");
    if bundle.schema != "video-agent-preview/v1"
        || !(1..=MAX_ATTEMPTS).contains(&bundle.attempt)
        || !hex_digest(&bundle.plan_digest)
        || !hex_digest(&bundle.sha256)
        || bundle.bundle_id != id
        || bundle.relative_dir != directory
        || bundle.media_path != format!("{directory}/preview.mp4")
        || bundle.plan_path != format!("{directory}/plan.json")
        || bundle.quality_path != format!("{directory}/quality.json")
        || bundle.manifest_path != format!("{directory}/manifest.json")
        || bundle.quality.plan_digest != bundle.plan_digest
    {
        return Err(invalid("Invalid or escaped preview bundle identity"));
    }
    Ok(())
}

fn read_state(root: &Path, task: &Task) -> Result<Trajectory> {
    let path = root.join(STATE);
    if let Err(error) = fs::symlink_metadata(&path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error.into());
        }
        if task
            .history
            .iter()
            .any(|e| e.kind == "preview_attempt_started")
        {
            return Err(invalid(
                "Required preview trajectory is missing; refusing to reset its budget",
            ));
        }
        return Ok(Trajectory {
            schema: "video-agent-trajectory/v1".into(),
            task_id: task.id.clone(),
            attempts_started: 0,
            max_attempts: MAX_ATTEMPTS,
            events: vec![],
            latest: None,
            verification_scope: "historical_metadata_only_not_fresh_verification".into(),
        });
    }
    let state: Trajectory = read_json(&path, 512 * 1024)?;
    let recorded_attempts = task
        .history
        .iter()
        .filter(|event| event.kind == "preview_attempt_started")
        .count();
    if state.schema != "video-agent-trajectory/v1"
        || state.task_id != task.id
        || state.max_attempts != MAX_ATTEMPTS
        || state.attempts_started > MAX_ATTEMPTS
        // The trajectory is written first. A valid leading started record is
        // conservative charged credit, not a successful render. Cheap readers
        // may inspect it; only an exclusive owner reconciles task.history.
        // A trajectory behind task.history is still a rollback, never credit.
        || (state.attempts_started as usize) < recorded_attempts
        || state.events.len() > MAX_EVENTS
        || state.events.iter().enumerate().any(|(i, event)| {
            event.sequence != i + 1
                || event.attempt == 0
                || event.attempt > state.attempts_started
                || !hex_digest(&event.plan_digest)
                || event.at.len() > 100
                || !matches!(
                    event.kind.as_str(),
                    "started" | "published" | "failed" | "rechecked"
                )
        })
        || state.events.iter().filter(|e| e.kind == "started").count()
            != state.attempts_started as usize
    {
        return Err(invalid("Invalid preview trajectory or counters"));
    }
    validate_events(&state)?;
    if let Some(bundle) = &state.latest {
        validate_bundle(bundle)?;
        if bundle.attempt > state.attempts_started
            || !state.events.iter().any(|event| {
                event.kind == "published"
                    && event.attempt == bundle.attempt
                    && event.plan_digest == bundle.plan_digest
                    && event.bundle_id.as_ref() == Some(&bundle.bundle_id)
            })
        {
            return Err(invalid("Preview head exceeds recorded attempt budget"));
        }
    }
    Ok(state)
}

fn validate_events(state: &Trajectory) -> Result<()> {
    let mut started = 0;
    for event in &state.events {
        if event.kind == "started" {
            started += 1;
            if event.attempt != started
                || event.bundle_id.is_some()
                || event.objective_pass.is_some()
                || event.error_code.is_some()
            {
                return Err(invalid("Invalid preview attempt ordering"));
            }
        } else if event.attempt > started {
            return Err(invalid("Preview event has no started attempt"));
        }
        if event
            .bundle_id
            .as_ref()
            .is_some_and(|id| *id != format!("{:02}-{}", event.attempt, event.plan_digest))
            || event.error_code.as_deref().is_some_and(|code| {
                !matches!(
                    code,
                    "cancelled"
                        | "deadline_exceeded"
                        | "preview_rejected_or_media_failed"
                        | "recheck_failed"
                )
            })
            || (event.kind == "failed" && event.objective_pass == Some(true))
        {
            return Err(invalid("Invalid preview event identity or failure facts"));
        }
    }
    Ok(())
}

fn record(
    state: &mut Trajectory,
    kind: &str,
    attempt: u32,
    digest: &str,
    bundle: Option<&PreviewBundle>,
    error_code: Option<&str>,
) -> Result<()> {
    if state.events.len() >= MAX_EVENTS {
        return Err(invalid("Preview trajectory event budget exhausted"));
    }
    state.events.push(TrajectoryEvent {
        sequence: state.events.len() + 1,
        at: now(),
        kind: kind.into(),
        attempt,
        plan_digest: digest.into(),
        bundle_id: bundle.map(|b| b.bundle_id.clone()),
        objective_pass: bundle.map(|b| b.quality.objective_pass),
        error_code: error_code.map(str::to_owned),
    });
    Ok(())
}

fn recover_started_history(root: &Path, task: &mut Task) -> Result<()> {
    let state = read_state(root, task)?;
    let recorded_attempts = task
        .history
        .iter()
        .filter(|event| event.kind == "preview_attempt_started")
        .count();
    let missing: Vec<_> = state
        .events
        .iter()
        .filter(|event| event.kind == "started")
        .skip(recorded_attempts)
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    for event in &missing {
        store::event(
            task,
            "preview_attempt_started",
            format!(
                "Attempt {}; plan digest {}; recovered conservatively from its durable started record; no render completion inferred",
                event.attempt, event.plan_digest
            ),
        );
    }
    store::event(
        task,
        "preview_history_recovered",
        format!(
            "Recovered {} missing task-history start record(s), preserving all {} charged preview attempts. No attempt was refunded, restarted in place, approved, or declared rendered.",
            missing.len(), state.attempts_started
        ),
    );
    // A retry after another interruption only appends the still-missing suffix.
    // This atomic task write does not change the authoritative trajectory.
    store::save(root, task)
}

pub(crate) fn with_task<T>(
    workspace: &Path,
    cancel: &CancelToken,
    operation: impl FnOnce(&Path, &mut Task, &Clock) -> Result<T>,
) -> Result<T> {
    cancel.check()?;
    let workspace = Workspace::open_with_cancel(workspace, cancel)?;
    let root = workspace.task_dir();
    let _lock = store::lock(root)?;
    let mut task = workspace.usable_with(cancel)?;
    let clock = Clock::new(root, &mut task)?;
    let result =
        recover_started_history(root, &mut task).and_then(|()| operation(root, &mut task, &clock));
    clock.finish(&mut task)?;
    store::save(root, &task)?;
    result
}

fn validation(task: &Task, plan: &EditPlan) -> Result<ValidationReport> {
    validate_plan(task, plan, false)?;
    Ok(ValidationReport {
        schema: "video-agent-plan-validation/v1".into(),
        plan_digest: plan_digest(task, plan)?,
        revision: plan.revision,
        clip_count: plan.clips.len(),
        plan_duration_ms: plan.clips.iter().map(|c| c.end_ms - c.start_ms).sum(),
        target_duration_ms: task.target_duration_ms,
        sources_verified: true,
        semantic_evaluation: "not_evaluated".into(),
    })
}

pub fn validate(
    workspace: &Path,
    plan: Option<EditPlan>,
    cancel: &CancelToken,
) -> Result<ValidationReport> {
    with_task(workspace, cancel, |_, task, _| {
        let plan = plan
            .as_ref()
            .or(task.plan.as_ref())
            .ok_or_else(|| invalid("No draft to validate"))?;
        validation(task, plan)
    })
}

fn objective_report(
    task: &Task,
    plan: &EditPlan,
    info: &media::MediaInfo,
) -> Result<QualityReport> {
    let validation = validation(task, plan)?;
    let duration = validation.plan_duration_ms;
    let target_tolerance = 250.max(task.target_duration_ms / 20);
    let mut checks = Vec::new();
    let mut add = |name: &str, passed: bool, detail: String| {
        checks.push(QualityCheck {
            name: name.into(),
            passed,
            detail,
        });
    };
    add(
        "plan_structure",
        true,
        "Existing source/range/audio-policy validators accepted the plan".into(),
    );
    add(
        "full_decode",
        true,
        "All video/audio streams decoded with FFmpeg -xerror".into(),
    );
    add(
        "duration_matches_plan",
        info.duration_ms.abs_diff(duration) <= 150.max(duration / 100),
        format!("observed={}ms planned={}ms", info.duration_ms, duration),
    );
    add(
        "preview_dimensions",
        info.width == 640 && info.height == 360,
        format!("{}x{}", info.width, info.height),
    );
    add(
        "audio_policy",
        info.has_audio == (plan.audio_policy == AudioPolicy::PreserveWithSilence),
        format!("audio_present={}", info.has_audio),
    );
    add(
        "target_duration_alignment",
        duration.abs_diff(task.target_duration_ms) <= target_tolerance,
        format!(
            "planned={}ms target={}ms tolerance={}ms; this arithmetic goal is not semantic quality",
            duration, task.target_duration_ms, target_tolerance
        ),
    );
    let matches_current_plan = task
        .plan
        .as_ref()
        .map(|p| plan_digest(task, p))
        .transpose()?
        .as_ref()
        == Some(&validation.plan_digest);
    Ok(QualityReport {
        schema: "video-agent-objective-quality/v1".into(),
        plan_digest: validation.plan_digest,
        objective_pass: checks.iter().all(|check| check.passed),
        semantic_evaluation: "not_evaluated".into(),
        checks,
        plan_duration_ms: duration,
        target_duration_ms: task.target_duration_ms,
        observed_duration_ms: info.duration_ms,
        width: info.width,
        height: info.height,
        has_audio: info.has_audio,
        matches_current_plan,
        needs_human_review: true,
    })
}

fn preview_validation(task: &Task, plan: &EditPlan) -> Result<ValidationReport> {
    let validated = validation(task, plan)?;
    if plan.clips.is_empty()
        || plan.clips.len() > MAX_CLIPS
        || validated.plan_duration_ms > MAX_PREVIEW_MS
    {
        return Err(invalid(
            "Preview requires 1–32 clips and at most 30 seconds; no full-source fallback",
        ));
    }
    if serde_json::to_vec_pretty(plan)?.len() as u64 + 1 > MAX_PLAN_BYTES {
        return Err(invalid("Preview plan exceeds the 64 KiB round-trip limit"));
    }
    Ok(validated)
}

/// A what-if must stay bound to the saved revision, without editing that plan.
pub(crate) fn validate_candidate(task: &Task, plan: &EditPlan) -> Result<()> {
    preview_validation(task, plan)?;
    let current = task
        .plan
        .as_ref()
        .ok_or_else(|| invalid("No saved draft for a preview candidate"))?;
    if plan.revision != current.revision {
        return Err(invalid(
            "Candidate revision must match the current saved revision",
        ));
    }
    Ok(())
}

fn render(
    root: &Path,
    task: &Task,
    plan: &EditPlan,
    attempt: u32,
    cancel: &CancelToken,
    clock: &Clock,
) -> Result<PreviewBundle> {
    let validated = preview_validation(task, plan)?;
    let previews = store::owned_dir(root, "previews")?;
    let temporary = tempfile::Builder::new()
        .prefix("render-")
        .tempdir_in(&previews)?;
    let mut paths = Vec::new();
    let mut durations = Vec::new();
    for clip in &plan.clips {
        cancel.check()?;
        let asset = task
            .assets
            .iter()
            .find(|a| a.id == clip.source_id)
            .ok_or_else(|| invalid("Unknown preview source"))?;
        let key = media::digest(&serde_json::to_vec(&(
            &asset.sha256,
            clip.start_ms,
            clip.end_ms,
            plan.audio_policy,
        ))?);
        let path = temporary.path().join(format!("{key}.mp4"));
        if !path.exists() {
            media::shard(
                asset,
                clip,
                plan.audio_policy,
                &path,
                cancel,
                clock.timeout(task.limits.process_timeout_secs)?,
            )?;
            if path.metadata()?.len() > MAX_MEDIA_BYTES {
                return Err(invalid("Preview shard exceeds accepted size limit"));
            }
        }
        paths.push(path);
        durations.push(clip.end_ms - clip.start_ms);
    }
    let media_path = temporary.path().join("preview.mp4");
    media::concat(
        &paths,
        &durations,
        &media_path,
        cancel,
        clock.timeout(task.limits.process_timeout_secs)?,
    )?;
    if media_path.metadata()?.len() > MAX_MEDIA_BYTES {
        return Err(invalid("Preview exceeds accepted size limit"));
    }
    let info = media::verify_output(
        &media_path,
        validated.plan_duration_ms,
        plan.audio_policy == AudioPolicy::PreserveWithSilence,
        cancel,
        clock.timeout(task.limits.process_timeout_secs)?,
    )?;
    let sha256 = media::hash_file_bounded(
        &media_path,
        cancel,
        clock.timeout(task.limits.process_timeout_secs)?,
    )?;
    check_sources_during(task, cancel, clock)?;
    cancel.check()?;
    let quality = objective_report(task, plan, &info)?;
    let id = format!("{attempt:02}-{}", validated.plan_digest);
    let relative_dir = format!("previews/{id}");
    let bundle = PreviewBundle {
        schema: "video-agent-preview/v1".into(),
        bundle_id: id.clone(),
        plan_digest: validated.plan_digest,
        attempt,
        media_path: format!("{relative_dir}/preview.mp4"),
        plan_path: format!("{relative_dir}/plan.json"),
        quality_path: format!("{relative_dir}/quality.json"),
        manifest_path: format!("{relative_dir}/manifest.json"),
        relative_dir,
        sha256,
        created_at: now(),
        verification_scope: "verified_at_capture_only_not_a_final_deliverable".into(),
        quality,
    };
    let destination = previews.join(&id);
    fs::create_dir(&destination)?;
    fs::hard_link(&media_path, destination.join("preview.mp4"))?;
    write_new(&destination.join("plan.json"), plan)?;
    write_new(&destination.join("quality.json"), &bundle.quality)?;
    write_new(&destination.join("bundle.json"), &bundle)?;
    // Only fixed relative artifact references, no source paths, intent, keys or shell text.
    let manifest = serde_json::json!({
        "protocol_version": "preview-bundle/v1", "bundle_id": id,
        "bundle_kind": "capture", "software": "video-toolkit", "recipe": "bounded-cut-v1",
        "status": "ok", "created_at": bundle.created_at,
        "generator": {"entry_point": "video-toolkit agent-task preview", "backend": "ffmpeg", "harness_version": env!("CARGO_PKG_VERSION")},
        "source": {"plan_fingerprint": bundle.plan_digest, "sources": task.assets.iter().map(|a| serde_json::json!({"id":a.id,"sha256":a.sha256})).collect::<Vec<_>>()},
        "summary_path": "quality.json",
        "artifacts": [{"artifact_id":"cut", "role":"preview-clip", "kind":"clip", "media_type":"video/mp4", "path":"preview.mp4", "width":info.width,"height":info.height,"duration_s":info.duration_ms as f64 / 1000.0,"bytes":media_path.metadata()?.len(),"sha256":bundle.sha256}],
        "warnings": ["What-if preview, not final export or human approval", "Visual/semantic quality has not been evaluated"],
        "metrics": {"objective_pass":bundle.quality.objective_pass,"planned_duration_ms":bundle.quality.plan_duration_ms}
    });
    write_new(&destination.join("manifest.json"), &manifest)?;
    #[cfg(unix)]
    File::open(&destination)?.sync_all()?;
    Ok(bundle)
}

fn attempt(
    root: &Path,
    task: &mut Task,
    plan: &EditPlan,
    state: &mut Trajectory,
    cancel: &CancelToken,
    clock: &Clock,
) -> Result<PreviewBundle> {
    cancel.check()?;
    preview_validation(task, plan)?;
    let digest = plan_digest(task, plan)?;
    if state.attempts_started >= MAX_ATTEMPTS || state.events.len() + 2 > MAX_EVENTS {
        return Err(invalid("Persistent preview attempt/event budget exhausted"));
    }
    state.attempts_started += 1;
    let number = state.attempts_started;
    record(state, "started", number, &digest, None, None)?;
    save_state(root, state)?;
    store::event(
        task,
        "preview_attempt_started",
        format!("Attempt {number}; plan digest {digest}; no approval or final export authority"),
    );
    store::save(root, task)?;
    match render(root, task, plan, number, cancel, clock) {
        Ok(bundle) => {
            record(state, "published", number, &digest, Some(&bundle), None)?;
            state.latest = Some(bundle.clone());
            save_state(root, state)?;
            Ok(bundle)
        }
        Err(error) => {
            let code = match error {
                AgentError::Cancelled => "cancelled",
                AgentError::TimedOut => "deadline_exceeded",
                _ => "preview_rejected_or_media_failed",
            };
            record(state, "failed", number, &digest, None, Some(code))?;
            save_state(root, state)?;
            Err(error)
        }
    }
}

pub fn capture(workspace: &Path, cancel: &CancelToken) -> Result<PreviewBundle> {
    with_task(workspace, cancel, |root, task, clock| {
        let plan = task
            .plan
            .clone()
            .ok_or_else(|| invalid("No draft to preview"))?;
        let mut state = read_state(root, task)?;
        attempt(root, task, &plan, &mut state, cancel, clock)
    })
}

/// Render an explicit, same-revision what-if; never save or approve the plan.
/// This owns its task lock/clock and must not be nested inside with_task().
pub(crate) fn capture_plan(
    workspace: &Path,
    plan: EditPlan,
    cancel: &CancelToken,
) -> Result<PreviewBundle> {
    with_task(workspace, cancel, |root, task, clock| {
        validate_candidate(task, &plan)?;
        let mut state = read_state(root, task)?;
        attempt(root, task, &plan, &mut state, cancel, clock)
    })
}

/// Cheap historical metadata; deliberately performs no source/media verification.
pub fn trajectory(workspace: &Path) -> Result<Trajectory> {
    let workspace = Workspace::open(workspace)?;
    let task = store::read(workspace.task_dir())?;
    read_state(workspace.task_dir(), &task)
}

/// Like trajectory: a previous successful capture is not a fresh attestation.
pub fn latest(workspace: &Path) -> Result<Option<PreviewBundle>> {
    Ok(trajectory(workspace)?.latest)
}

fn bundle_root(root: &Path, bundle: &PreviewBundle) -> Result<PathBuf> {
    validate_bundle(bundle)?;
    let previews = root.join("previews");
    let path = previews.join(&bundle.bundle_id);
    for (directory, parent) in [(&previews, root), (&path, previews.as_path())] {
        let metadata = fs::symlink_metadata(directory)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || fs::canonicalize(directory)?.parent() != Some(parent)
        {
            return Err(invalid("Preview directory escaped its task"));
        }
    }
    Ok(path)
}

/// Freshly verify this exact bundle while the caller owns the task lock/clock.
/// It deliberately does not consult latest or acquire a second execution budget.
pub(crate) fn verify_bundle(
    root: &Path,
    task: &Task,
    bundle: &PreviewBundle,
    cancel: &CancelToken,
    clock: &Clock,
) -> Result<QualityReport> {
    let directory = bundle_root(root, bundle)?;
    let plan: EditPlan = read_json(&directory.join("plan.json"), MAX_PLAN_BYTES)?;
    if plan_digest(task, &plan)? != bundle.plan_digest {
        return Err(invalid("Captured preview plan changed"));
    }
    let path = directory.join("preview.mp4");
    store::regular(&path)?;
    if path.metadata()?.len() > MAX_MEDIA_BYTES
        || media::hash_file_bounded(
            &path,
            cancel,
            clock.timeout(task.limits.process_timeout_secs)?,
        )? != bundle.sha256
    {
        return Err(invalid("Captured preview media changed"));
    }
    validate_plan(task, &plan, false)?;
    let duration = plan.clips.iter().map(|c| c.end_ms - c.start_ms).sum();
    let info = media::verify_output(
        &path,
        duration,
        plan.audio_policy == AudioPolicy::PreserveWithSilence,
        cancel,
        clock.timeout(task.limits.process_timeout_secs)?,
    )?;
    check_sources_during(task, cancel, clock)?;
    objective_report(task, &plan, &info)
}

pub fn quality(workspace: &Path, cancel: &CancelToken) -> Result<QualityReport> {
    with_task(workspace, cancel, |root, task, clock| {
        let mut state = read_state(root, task)?;
        if state.events.len() >= MAX_EVENTS {
            return Err(invalid("Preview trajectory event budget exhausted"));
        }
        let bundle = state
            .latest
            .clone()
            .ok_or_else(|| invalid("No captured preview"))?;
        let checked = verify_bundle(root, task, &bundle, cancel, clock);
        let mut event_bundle = bundle.clone();
        match &checked {
            Ok(report) => event_bundle.quality = report.clone(),
            Err(_) => event_bundle.quality.objective_pass = false,
        }
        record(
            &mut state,
            if checked.is_ok() {
                "rechecked"
            } else {
                "failed"
            },
            bundle.attempt,
            &bundle.plan_digest,
            Some(&event_bundle),
            checked.as_ref().err().map(|_| "recheck_failed"),
        )?;
        save_state(root, &state)?;
        checked
    })
}

/// Evaluate current draft then caller-supplied revisions. Never edit/approve/export
/// a task on the caller's behalf, and never invoke a model or infer semantics.
pub fn iterate(
    workspace: &Path,
    candidates: Vec<EditPlan>,
    max_attempts: u32,
    cancel: &CancelToken,
) -> Result<IterationReport> {
    if !(1..=3).contains(&max_attempts) || candidates.len() >= max_attempts as usize {
        return Err(invalid(
            "Iteration requires 1–3 attempts and at most max_attempts-1 candidate plans",
        ));
    }
    with_task(workspace, cancel, |root, task, clock| {
        let current = task
            .plan
            .clone()
            .ok_or_else(|| invalid("No draft to iterate"))?;
        // Preflight the entire bounded queue before any expensive rendering.
        for plan in std::iter::once(&current).chain(candidates.iter()) {
            validate_candidate(task, plan)?;
        }
        let mut state = read_state(root, task)?;
        let mut attempts = Vec::new();
        let mut selected_plan = None;
        for plan in std::iter::once(&current)
            .chain(candidates.iter())
            .take(max_attempts as usize)
        {
            let bundle = attempt(root, task, plan, &mut state, cancel, clock)?;
            let passed = bundle.quality.objective_pass;
            if passed {
                selected_plan = Some(bundle.plan_path.clone());
            }
            attempts.push(bundle);
            if passed {
                break;
            }
        }
        Ok(IterationReport {
            schema: "video-agent-iteration/v1".into(),
            stopped_reason: if selected_plan.is_some() {
                "objective_checks_passed_requires_review"
            } else {
                "candidate_or_attempt_limit_reached"
            }
            .into(),
            selected_plan,
            attempts,
            needs_human_review: true,
            task_plan_unchanged: true,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata_fixture(root: &Path) -> Task {
        // Inert bytes are sufficient for structural/historical reads. This is
        // explicitly not a playable media or successful-source-probe fixture.
        let bytes = b"inert preview journal fixture; no media or model claim";
        let path = root.join("inert-source.bin");
        fs::write(&path, bytes).unwrap();
        let metadata = path.metadata().unwrap();
        Task {
            schema_version: 1,
            id: root.file_name().unwrap().to_string_lossy().into(),
            created_at: now(),
            intent: "Metadata-only preview recovery fixture".into(),
            target_duration_ms: 1000,
            assets: vec![Asset {
                id: "source-1".into(),
                path: fs::canonicalize(path).unwrap(),
                sha256: media::digest(bytes),
                size: metadata.len(),
                modified_ns: metadata
                    .modified()
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64,
                duration_ms: 3000,
                width: 160,
                height: 90,
                has_audio: false,
            }],
            samples: vec![],
            model_calls: vec![],
            plan: Some(EditPlan {
                revision: 1,
                clips: vec![Clip {
                    id: "manual-fixture".into(),
                    source_id: "source-1".into(),
                    start_ms: 0,
                    end_ms: 1000,
                    reason: "MANUAL inert metadata fixture".into(),
                    uncertainty: "No playable media or semantics were evaluated".into(),
                    evidence_ids: vec![],
                    human_modified: true,
                }],
                summary: "MANUAL metadata fixture, not rendered".into(),
                audio_policy: AudioPolicy::Mute,
            }),
            approval: None,
            status: TaskStatus::Review,
            last_error: None,
            limits: Limits::default(),
            usage: Usage::default(),
            cloud_consent: false,
            export: None,
            shards: vec![],
            history: vec![],
        }
    }

    fn state() -> Trajectory {
        Trajectory {
            schema: "video-agent-trajectory/v1".into(),
            task_id: "test-only".into(),
            attempts_started: 1,
            max_attempts: MAX_ATTEMPTS,
            events: vec![TrajectoryEvent {
                sequence: 1,
                at: now(),
                kind: "started".into(),
                attempt: 1,
                plan_digest: "a".repeat(64),
                bundle_id: None,
                objective_pass: None,
                error_code: None,
            }],
            latest: None,
            verification_scope: "historical_metadata_only_not_fresh_verification".into(),
        }
    }

    #[test]
    fn started_attempts_must_be_unique_and_ordered() {
        let mut state = state();
        assert!(validate_events(&state).is_ok());
        state.events.push(state.events[0].clone());
        assert!(validate_events(&state).is_err());
    }

    #[test]
    fn terminal_event_cannot_precede_its_attempt() {
        let mut state = state();
        state.events[0].kind = "published".into();
        assert!(validate_events(&state).is_err());
    }

    #[test]
    fn event_references_cannot_escape_or_advertise_failed_passes() {
        let mut state = state();
        let mut event = state.events[0].clone();
        event.sequence = 2;
        event.kind = "failed".into();
        event.error_code = Some("recheck_failed".into());
        event.bundle_id = Some(format!("01-{}", event.plan_digest));
        event.objective_pass = Some(false);
        state.events.push(event);
        assert!(validate_events(&state).is_ok());
        state.events[1].objective_pass = Some(true);
        assert!(validate_events(&state).is_err());
        state.events[1].objective_pass = Some(false);
        state.events[1].bundle_id = Some("../outside".into());
        assert!(validate_events(&state).is_err());
        state.events[1].bundle_id = None;
        state.events[1].error_code =
            Some("raw paths and provider errors are not event codes".into());
        assert!(validate_events(&state).is_err());
    }

    #[test]
    fn cross_file_started_window_is_readable_and_recovers_history_once_without_render_claims() {
        let root = tempfile::tempdir().unwrap();
        let task = metadata_fixture(root.path());
        store::save(root.path(), &task).unwrap();
        let mut state = state();
        state.task_id = task.id.clone();
        state.events[0].plan_digest = plan_digest(&task, task.plan.as_ref().unwrap()).unwrap();
        save_state(root.path(), &state).unwrap();
        let task_before = fs::read(root.path().join("task.json")).unwrap();
        let trajectory_before = fs::read(root.path().join(STATE)).unwrap();

        // Reproduce the precise persisted prefix: trajectory started was
        // synced, but the following task.history write never completed.
        let historical = trajectory(root.path()).unwrap();
        assert_eq!(historical.attempts_started, 1);
        assert_eq!(historical.events.len(), 1);
        assert_eq!(historical.events[0].kind, "started");
        assert!(historical.latest.is_none());
        assert!(latest(root.path()).unwrap().is_none());
        assert_eq!(
            fs::read(root.path().join("task.json")).unwrap(),
            task_before
        );

        let _lock = store::lock(root.path()).unwrap();
        let mut recovered = store::read(root.path()).unwrap();
        recover_started_history(root.path(), &mut recovered).unwrap();
        assert_eq!(recovered.plan, task.plan);
        assert!(recovered.approval.is_none());
        assert!(recovered.export.is_none());
        assert_eq!(
            recovered
                .history
                .iter()
                .filter(|e| e.kind == "preview_attempt_started")
                .count(),
            1
        );
        assert_eq!(
            recovered
                .history
                .iter()
                .filter(|e| e.kind == "preview_history_recovered")
                .count(),
            1
        );
        let recovered_bytes = fs::read(root.path().join("task.json")).unwrap();
        recover_started_history(root.path(), &mut recovered).unwrap();
        assert_eq!(
            fs::read(root.path().join("task.json")).unwrap(),
            recovered_bytes
        );
        assert_eq!(
            fs::read(root.path().join(STATE)).unwrap(),
            trajectory_before
        );
        assert_eq!(
            read_state(root.path(), &recovered)
                .unwrap()
                .attempts_started,
            1
        );
        assert!(read_state(root.path(), &recovered)
            .unwrap()
            .latest
            .is_none());
    }

    #[test]
    fn history_recovery_rejects_missing_or_rolled_back_trajectories_without_refunding_credit() {
        let root = tempfile::tempdir().unwrap();
        let mut task = metadata_fixture(root.path());
        store::event(
            &mut task,
            "preview_attempt_started",
            "Already durably charged attempt 1",
        );
        store::event(
            &mut task,
            "preview_attempt_started",
            "Already durably charged attempt 2",
        );
        store::save(root.path(), &task).unwrap();
        let before = fs::read(root.path().join("task.json")).unwrap();
        assert!(trajectory(root.path()).is_err());
        let _lock = store::lock(root.path()).unwrap();
        assert!(recover_started_history(root.path(), &mut task).is_err());
        assert!(!root.path().join(STATE).exists());
        let mut rolled_back = state();
        rolled_back.task_id = task.id.clone();
        save_state(root.path(), &rolled_back).unwrap();
        assert!(read_state(root.path(), &task).is_err());
        assert!(recover_started_history(root.path(), &mut task).is_err());
        assert_eq!(fs::read(root.path().join("task.json")).unwrap(), before);
        assert_eq!(task.history.len(), 2);
    }

    #[test]
    fn candidate_validation_is_same_revision_and_preserves_the_saved_plan() {
        let root = tempfile::tempdir().unwrap();
        let task = metadata_fixture(root.path());
        let original = task.plan.clone();
        let mut candidate = original.clone().unwrap();
        candidate.clips[0].end_ms = 2000;
        assert!(validate_candidate(&task, &candidate).is_ok());
        candidate.revision += 1;
        assert!(validate_candidate(&task, &candidate).is_err());
        candidate.revision -= 1;
        candidate.clips[0].end_ms = 4000;
        assert!(validate_candidate(&task, &candidate).is_err());
        assert_eq!(task.plan, original);
        assert!(task.approval.is_none());
        assert!(task.export.is_none());
    }

    #[test]
    #[ignore = "requires real FFmpeg/ffprobe; exact-bundle what-if verification, no model"]
    fn same_revision_capture_and_exact_bundle_verification_do_not_follow_a_newer_latest() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("synthetic-blue.mp4");
        assert!(std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=160x90:r=30:d=3",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
                "-threads",
                "1"
            ])
            .arg(&source)
            .status()
            .unwrap()
            .success());
        let source_before = fs::read(&source).unwrap();
        let workspace = Workspace::create(
            &root.path().join("tasks"),
            std::slice::from_ref(&source),
            "SYNTHETIC exact bundle test, no visual semantics".into(),
            1000,
            Limits::default(),
        )
        .unwrap();
        let task = workspace.manual_draft().unwrap();
        let original = task.plan.clone().unwrap();
        let mut candidate = original.clone();
        candidate.clips[0].end_ms = 2000;
        let cancel = CancelToken::default();
        let what_if = capture_plan(workspace.task_dir(), candidate.clone(), &cancel).unwrap();
        assert!(!what_if.quality.matches_current_plan);
        let newer = capture(workspace.task_dir(), &cancel).unwrap();
        assert_ne!(what_if.bundle_id, newer.bundle_id);
        assert_eq!(
            latest(workspace.task_dir()).unwrap().unwrap().bundle_id,
            newer.bundle_id
        );
        let report = with_task(workspace.task_dir(), &cancel, |root, task, clock| {
            verify_bundle(root, task, &what_if, &cancel, clock)
        })
        .unwrap();
        assert_eq!(report.plan_digest, what_if.plan_digest);
        assert!(!report.matches_current_plan);
        candidate.revision += 1;
        assert!(capture_plan(workspace.task_dir(), candidate, &cancel).is_err());
        let after = workspace.load().unwrap();
        assert_eq!(after.plan.as_ref(), Some(&original));
        assert!(after.approval.is_none());
        assert!(after.export.is_none());
        assert_eq!(
            trajectory(workspace.task_dir()).unwrap().attempts_started,
            2
        );
        assert_eq!(fs::read(source).unwrap(), source_before);
    }
}
