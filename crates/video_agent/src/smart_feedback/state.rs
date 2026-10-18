//! Private, bounded smart-loop journals. Metadata is local, not signed evidence.
use super::{Job, Phase, SmartRound, MAX_EVENTS, MAX_ROUNDS};
use crate::domain::*;
use crate::execution::Clock;
use crate::feedback::{self, PreviewBundle};
use crate::review::{self, ReviewDecision, ReviewVerdict};
use crate::store::{self, invalid, now};
use crate::workflow::{plan_digest, validate_plan};
use fs2::FileExt;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

const MAX_STATE_BYTES: u64 = 512 * 1024;
const HEAD: &str = "smart-feedback-head.json";
const DIRECTORY: &str = "feedback-runs";
const SCHEMA: &str = "video-agent-smart-state/v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Head {
    schema: String,
    task_id: String,
    run_id: String,
}

pub(super) struct Guard(File);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub(super) fn run_lock(root: &Path) -> Result<Guard> {
    let root = fs::canonicalize(root)?;
    let path = root.join("smart-feedback.lock");
    match fs::symlink_metadata(&path) {
        Ok(_) => store::regular(&path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock_exclusive()
        .map_err(|_| invalid("Smart feedback is already active in another operation"))?;
    Ok(Guard(file))
}

fn hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn run_id(value: &str) -> bool {
    value.strip_prefix("smart-").is_some_and(hex)
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
}

fn code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
}

fn read<T: DeserializeOwned>(path: &Path, limit: u64) -> Result<T> {
    store::regular(path)?;
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(invalid("Smart feedback metadata exceeds its byte limit"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid(
            "Smart feedback metadata grew beyond its byte limit",
        ));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn directory(root: &Path, create: bool) -> Result<Option<PathBuf>> {
    let path = root.join(DIRECTORY);
    if !exists(&path)? {
        return if create {
            store::owned_dir(root, DIRECTORY).map(Some)
        } else {
            Ok(None)
        };
    }
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || fs::canonicalize(&path)?.parent() != Some(root)
    {
        return Err(invalid("Smart feedback directory escaped its task"));
    }
    Ok(Some(path))
}

fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if exists(path)? {
        store::regular(path)?;
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Missing journal parent"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn encoded(value: &impl Serialize, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > limit {
        return Err(invalid("Smart feedback metadata exceeds its byte limit"));
    }
    Ok(bytes)
}

fn plan_metadata(task: &Task, plan: &EditPlan) -> Result<()> {
    validate_plan(task, plan, false)?;
    if plan.clips.is_empty()
        || plan.clips.len() > feedback::MAX_CLIPS
        || plan
            .clips
            .iter()
            .map(|clip| clip.end_ms - clip.start_ms)
            .sum::<u64>()
            > feedback::MAX_PREVIEW_MS
        || serde_json::to_vec_pretty(plan)?.len() as u64 + 1 > feedback::MAX_PLAN_BYTES
    {
        return Err(invalid("Smart feedback candidate exceeds preview bounds"));
    }
    Ok(())
}

fn bundle_identity(bundle: &PreviewBundle) -> Result<()> {
    let expected_id = format!("{:02}-{}", bundle.attempt, bundle.plan_digest);
    let relative = format!("previews/{expected_id}");
    if bundle.schema != "video-agent-preview/v1"
        || !(1..=feedback::MAX_ATTEMPTS).contains(&bundle.attempt)
        || !hex(&bundle.plan_digest)
        || !hex(&bundle.sha256)
        || bundle.bundle_id != expected_id
        || bundle.relative_dir != relative
        || bundle.media_path != format!("{relative}/preview.mp4")
        || bundle.plan_path != format!("{relative}/plan.json")
        || bundle.quality_path != format!("{relative}/quality.json")
        || bundle.manifest_path != format!("{relative}/manifest.json")
        || bundle.quality.plan_digest != bundle.plan_digest
        || bundle.quality.schema != "video-agent-objective-quality/v1"
        || bundle.quality.semantic_evaluation != "not_evaluated"
        || bundle.created_at.is_empty()
        || bundle.created_at.len() > 100
        || bundle.quality.checks.len() > 32
    {
        return Err(invalid("Invalid fixed smart-preview bundle identity"));
    }
    Ok(())
}

fn contained(root: &Path, path: &Path) -> bool {
    path.is_absolute()
        && path.starts_with(root)
        && path.strip_prefix(root).is_ok_and(|relative| {
            relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        })
}

fn rendered_path(root: &Path, bundle: &PreviewBundle, image: &review::PreviewImage) -> bool {
    let Some(name) = image.path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(index) = name
        .strip_suffix(".jpg")
        .and_then(|name| name.parse::<usize>().ok())
    else {
        return false;
    };
    index < 6
        && name == format!("{index:02}.jpg")
        && image.path
            == root
                .join("feedback-frames")
                .join(&bundle.bundle_id)
                .join(name)
        && image.id == format!("rendered-{}-{index}", bundle.bundle_id)
}

fn round_metadata(root: &Path, task: &Task, round: &SmartRound) -> Result<()> {
    bundle_identity(&round.preview)?;
    if round.round == 0
        || round.round > MAX_ROUNDS
        || round.rendered_observations.len() > 6
        || round.source_observations.len() > 8
        || round
            .model_call_id
            .as_ref()
            .is_some_and(|id| !identifier(id))
        || round
            .proposal_call_id
            .as_ref()
            .is_some_and(|id| !identifier(id))
        || round.review.is_some() != round.model_call_id.is_some()
    {
        return Err(invalid("Invalid smart round counters or review identity"));
    }
    for image in &round.rendered_observations {
        if !identifier(&image.id)
            || !hex(&image.sha256)
            || !contained(root, &image.path)
            || !rendered_path(root, &round.preview, image)
            || image.output_time_ms >= round.preview.quality.plan_duration_ms
            || !task.assets.iter().any(|asset| {
                asset.id == image.source_id && image.source_time_ms < asset.duration_ms
            })
        {
            return Err(invalid("Invalid smart rendered observation"));
        }
    }
    for sample in &round.source_observations {
        if !task.samples.contains(sample)
            || sample.image_path != root.join("frames").join(format!("{}.jpg", sample.id))
            || !contained(root, &sample.image_path)
        {
            return Err(invalid(
                "Smart source observation is not in the task inventory",
            ));
        }
    }
    if let Some(id) = &round.model_call_id {
        if !task.model_calls.iter().any(|record| &record.call_id == id) {
            return Err(invalid("Smart round model record disappeared"));
        }
    }
    Ok(())
}

fn validate(root: &Path, task: &Task, job: &Job) -> Result<()> {
    if job.schema != SCHEMA
        || job.task_id != task.id
        || !run_id(&job.run_id)
        || !hex(&job.base_plan_digest)
        || job.run_id != format!("smart-{}", job.base_plan_digest)
        || job.base_revision == 0
        || job.candidate.revision != job.base_revision
        || !(1..=MAX_ROUNDS).contains(&job.max_rounds)
        || job.preview_floor > feedback::MAX_ATTEMPTS
        || job.rounds.len() > job.max_rounds as usize
        || job.events.len() > MAX_EVENTS
        || job.created_at.is_empty()
        || job.created_at.len() > 100
        || job
            .candidate_from_call
            .as_ref()
            .is_some_and(|id| !identifier(id))
        || job
            .stopped_reason
            .as_ref()
            .is_some_and(|reason| !code(reason))
    {
        return Err(invalid("Invalid smart feedback state identity or bounds"));
    }
    plan_metadata(task, &job.candidate)?;
    let candidate_digest = plan_digest(task, &job.candidate)?;
    let mut previous_attempt = job.preview_floor;
    for (index, round) in job.rounds.iter().enumerate() {
        round_metadata(root, task, round)?;
        if round.round != index as u32 + 1 || round.preview.attempt <= previous_attempt {
            return Err(invalid("Smart preview rounds are out of order"));
        }
        if index == 0 {
            if round.preview.plan_digest != job.base_plan_digest || round.proposal_call_id.is_some()
            {
                return Err(invalid("First smart round is not bound to the base plan"));
            }
        } else if round.proposal_call_id != job.rounds[index - 1].model_call_id {
            return Err(invalid("Smart revision lost its proposing model identity"));
        }
        previous_attempt = round.preview.attempt;
    }
    if job.rounds.is_empty()
        && (candidate_digest != job.base_plan_digest || job.candidate_from_call.is_some())
    {
        return Err(invalid(
            "Initial smart candidate differs from the saved base plan",
        ));
    }
    for (index, event) in job.events.iter().enumerate() {
        if event.sequence != index + 1
            || event.at.is_empty()
            || event.at.len() > 100
            || event.round > job.rounds.len() as u32
            || !hex(&event.plan_digest)
            || !matches!(
                event.kind.as_str(),
                "preview_ready"
                    | "model_reserved"
                    | "model_started"
                    | "model_received"
                    | "revision_validated"
                    | "stopped"
            )
            || event.call_id.as_ref().is_some_and(|id| !identifier(id))
            || event.code.as_ref().is_some_and(|value| !code(value))
        {
            return Err(invalid("Invalid smart feedback event"));
        }
    }
    match job.phase {
        Phase::NeedPreview => {
            if job.rounds.len() >= job.max_rounds as usize
                || job.pending_request.is_some()
                || job.reservation.is_some()
                || job.pending_result.is_some()
            {
                return Err(invalid("Invalid pending smart-preview phase"));
            }
            if let Some(last) = job.rounds.last() {
                if last.review.as_ref().map(|review| review.verdict) != Some(ReviewVerdict::Revise)
                    || job.candidate_from_call != last.model_call_id
                {
                    return Err(invalid("Pending revision has no prior model proposal"));
                }
            }
        }
        Phase::NeedReview | Phase::Calling | Phase::Received => {
            let last = job
                .rounds
                .last()
                .ok_or_else(|| invalid("Smart review phase has no preview"))?;
            if last.preview.plan_digest != candidate_digest
                || last.review.is_some()
                || last.proposal_call_id != job.candidate_from_call
            {
                return Err(invalid(
                    "Smart review phase is not bound to its current candidate",
                ));
            }
            match job.phase {
                Phase::NeedReview
                    if job.pending_request.is_some()
                        || job.reservation.is_some()
                        || job.pending_result.is_some() =>
                {
                    return Err(invalid("Unreserved review contains pending call data"))
                }
                Phase::Calling
                    if job.pending_request.is_none()
                        || job.reservation.is_none()
                        || job.pending_result.is_some() =>
                {
                    return Err(invalid("Calling review lost its durable reservation"))
                }
                Phase::Received
                    if job.pending_request.is_none()
                        || job.reservation.is_none()
                        || job.pending_result.is_none() =>
                {
                    return Err(invalid("Received review lost its durable result"))
                }
                _ => {}
            }
        }
        Phase::Completed => {
            if job.stopped_reason.is_none() {
                return Err(invalid("Completed smart run has no stopped reason"));
            }
        }
    }
    if job.phase != Phase::Completed
        && (job.stopped_reason.is_some() || job.selected_plan.is_some())
    {
        return Err(invalid("Active smart run advertises completed selection"));
    }
    if let Some(selected) = &job.selected_plan {
        let last = job
            .rounds
            .last()
            .ok_or_else(|| invalid("Selected smart plan has no round"))?;
        if selected != &last.preview.plan_path
            || last.review.as_ref().map(|review| review.verdict) != Some(ReviewVerdict::Accept)
        {
            return Err(invalid(
                "Selected smart plan is not an accepted fixed round artifact",
            ));
        }
    }
    match (&job.pending_request, &job.reservation) {
        (None, None) => {
            if job.pending_result.is_some() {
                return Err(invalid("Orphan smart response record"));
            }
        }
        (Some(request), Some(reservation)) => {
            let last = job
                .rounds
                .last()
                .ok_or_else(|| invalid("Pending smart call has no preview"))?;
            let mut limits = request.limits.clone();
            limits.model_timeout_secs = task.limits.model_timeout_secs;
            if request.plan != job.candidate
                || request.assets != task.assets
                || request.intent != task.intent
                || request.target_duration_ms != task.target_duration_ms
                || request.limits.model_timeout_secs > task.limits.model_timeout_secs
                || limits != task.limits
                || request.plan_digest != candidate_digest
                || request.preview_sha256 != last.preview.sha256
                || request.call_id != reservation.call_id
                || reservation.sample_ids != super::submitted_ids(request)
                || reservation.status != "feedback_reserved"
                || reservation.decision.is_some()
                || !reservation.estimated_cost_usd.is_finite()
                || reservation.estimated_cost_usd < 0.0
                || !task
                    .model_calls
                    .iter()
                    .any(|record| record.call_id == reservation.call_id)
            {
                return Err(invalid("Pending smart call identity or accounting changed"));
            }
            // Pure in-memory request checks. This sentinel is never persisted,
            // sent, scored or claimed as a review; it reuses the typed validator.
            review::validate_decision(
                request,
                &ReviewDecision {
                    verdict: ReviewVerdict::Uncertain,
                    summary: "Metadata validation only".into(),
                    issues: vec![],
                    revised_clips: vec![],
                },
            )?;
            for frame in &request.preview_frames {
                if !contained(root, &frame.path) || !rendered_path(root, &last.preview, frame) {
                    return Err(invalid("Pending preview image escaped its task"));
                }
            }
            for sample in &request.source_samples {
                if !task.samples.contains(sample)
                    || sample.image_path != root.join("frames").join(format!("{}.jpg", sample.id))
                    || !contained(root, &sample.image_path)
                {
                    return Err(invalid(
                        "Pending source sample is not in the task inventory",
                    ));
                }
            }
            if let Some(pending) = &job.pending_result {
                if pending.record.call_id != reservation.call_id
                    || !pending.record.estimated_cost_usd.is_finite()
                    || pending.record.estimated_cost_usd < 0.0
                    || pending
                        .error_code
                        .as_deref()
                        .is_some_and(|value| value != "response_rejected")
                {
                    return Err(invalid("Invalid pending smart response identity"));
                }
                // A received, locally invalid decision must remain recoverable
                // for the owner to reconcile known usage and then reject it.
            }
        }
        _ => return Err(invalid("Incomplete smart request reservation pair")),
    }
    Ok(())
}

pub(super) fn save(root: &Path, job: &Job) -> Result<()> {
    let root = fs::canonicalize(root)?;
    let task = store::read(&root)?;
    validate(&root, &task, job)?;
    let bytes = encoded(job, MAX_STATE_BYTES)?;
    let directory = directory(&root, true)?.unwrap();
    let path = directory.join(format!("{}.json", job.run_id));
    if exists(&path)? {
        let previous: Job = read(&path, MAX_STATE_BYTES)?;
        validate(&root, &task, &previous)?;
        if previous.run_id != job.run_id
            || previous.base_revision != job.base_revision
            || previous.max_rounds != job.max_rounds
            || previous.preview_floor != job.preview_floor
            || previous.created_at != job.created_at
            || previous.rounds.len() > job.rounds.len()
            || previous.events.len() > job.events.len()
            || (previous.phase == Phase::Completed
                && serde_json::to_value(&previous)? != serde_json::to_value(job)?)
            || previous.rounds.iter().zip(&job.rounds).any(|(old, new)| {
                old.round != new.round
                    || old.preview.bundle_id != new.preview.bundle_id
                    || old.preview.plan_digest != new.preview.plan_digest
                    || old.preview.sha256 != new.preview.sha256
                    || old.proposal_call_id != new.proposal_call_id
                    || old
                        .review
                        .as_ref()
                        .is_some_and(|review| new.review.as_ref() != Some(review))
            })
        {
            return Err(invalid(
                "Smart feedback journal cannot roll back or change its fixed limits",
            ));
        }
    }
    atomic(&path, &bytes)?;
    let head = Head {
        schema: "video-agent-smart-head/v1".into(),
        task_id: job.task_id.clone(),
        run_id: job.run_id.clone(),
    };
    atomic(&root.join(HEAD), &encoded(&head, 1024)?)
}

fn activation(task: &Task, id: &str) -> Result<bool> {
    let count = task
        .history
        .iter()
        .filter(|event| event.kind == "smart_feedback_started" && event.detail == id)
        .count();
    if count > 1 {
        return Err(invalid("Duplicate smart feedback activation records"));
    }
    Ok(count == 1)
}

pub(super) fn load_or_create(root: &Path, task: &mut Task, max_rounds: u32) -> Result<Job> {
    if !(1..=MAX_ROUNDS).contains(&max_rounds) {
        return Err(invalid("Smart feedback requires 1–3 fixed rounds"));
    }
    let root = fs::canonicalize(root)?;
    let plan = task
        .plan
        .as_ref()
        .ok_or_else(|| invalid("No draft for smart feedback"))?;
    feedback::validate_candidate(task, plan)?;
    let digest = plan_digest(task, plan)?;
    let id = format!("smart-{digest}");
    let active = activation(task, &id)?;
    let existing = directory(&root, false)?.map(|directory| directory.join(format!("{id}.json")));
    let existing = match existing {
        Some(path) if exists(&path)? => Some(path),
        _ => None,
    };
    let mut recovered = false;
    let job = if let Some(path) = existing {
        let job: Job = read(&path, MAX_STATE_BYTES)?;
        validate(&root, task, &job)?;
        if job.run_id != id || job.base_revision != plan.revision || job.max_rounds != max_rounds {
            return Err(invalid(
                "Existing smart run has a different base revision or fixed round limit",
            ));
        }
        recovered = !active;
        job
    } else {
        if active {
            return Err(invalid(
                "Required smart feedback state is missing; refusing a budget reset",
            ));
        }
        Job {
            schema: SCHEMA.into(),
            task_id: task.id.clone(),
            run_id: id,
            base_plan_digest: digest,
            base_revision: plan.revision,
            max_rounds,
            preview_floor: feedback::trajectory(&root)?.attempts_started,
            created_at: now(),
            phase: Phase::NeedPreview,
            candidate: plan.clone(),
            candidate_from_call: None,
            rounds: vec![],
            pending_request: None,
            reservation: None,
            pending_result: None,
            stopped_reason: None,
            selected_plan: None,
            events: vec![],
        }
    };
    // State precedes activation; loading its stable file repairs a missing head.
    save(&root, &job)?;
    let mut task_changed = false;
    if !active {
        store::event(task, "smart_feedback_started", job.run_id.clone());
        if recovered {
            store::event(task, "smart_feedback_history_recovered", job.run_id.clone());
        }
        task_changed = true;
    }
    let owned_call = if matches!(job.phase, Phase::Calling | Phase::Received) {
        job.reservation
            .as_ref()
            .map(|record| record.call_id.as_str())
    } else {
        None
    };
    let mut orphaned = Vec::new();
    for record in &mut task.model_calls {
        if record.status == "feedback_reserved" && owned_call != Some(record.call_id.as_str()) {
            record.status = "feedback_interrupted_unknown_usage".into();
            record.error = Some("A durable task reservation had no active smart-call journal; usage remains charged and this request is not replayed".into());
            orphaned.push(record.call_id.clone());
        }
    }
    for call in orphaned {
        store::event(task, "smart_feedback_orphan_reservation_recovered", call);
        task_changed = true;
    }
    if task_changed {
        store::save(&root, task)?;
    }
    Ok(job)
}

fn read_run(root: &Path, task: &Task, id: &str) -> Result<Job> {
    if !run_id(id) {
        return Err(invalid("Invalid smart run identifier"));
    }
    let directory = directory(root, false)?
        .ok_or_else(|| invalid("Required smart run directory is missing"))?;
    let job: Job = read(&directory.join(format!("{id}.json")), MAX_STATE_BYTES)?;
    validate(root, task, &job)?;
    if job.run_id != id {
        return Err(invalid("Smart run filename does not match its identity"));
    }
    Ok(job)
}

pub(super) fn latest(root: &Path, task: &Task) -> Result<Option<Job>> {
    let root = fs::canonicalize(root)?;
    if let Some(plan) = &task.plan {
        let current = format!("smart-{}", plan_digest(task, plan)?);
        if let Some(directory) = directory(&root, false)? {
            if exists(&directory.join(format!("{current}.json")))? {
                return read_run(&root, task, &current).map(Some);
            }
        }
        if activation(task, &current)? {
            return Err(invalid(
                "Required smart feedback state is missing; refusing a budget reset",
            ));
        }
    }
    let path = root.join(HEAD);
    if !exists(&path)? {
        return Ok(None);
    }
    let head: Head = read(&path, 1024)?;
    if head.schema != "video-agent-smart-head/v1"
        || head.task_id != task.id
        || !run_id(&head.run_id)
    {
        return Err(invalid("Invalid or escaped smart feedback head"));
    }
    read_run(&root, task, &head.run_id).map(Some)
}

fn preview_directory(root: &Path, id: &str) -> Result<PathBuf> {
    let (number, digest) = id
        .split_once('-')
        .ok_or_else(|| invalid("Invalid preview directory identity"))?;
    if number.len() != 2
        || !number.bytes().all(|byte| byte.is_ascii_digit())
        || !hex(digest)
        || !number
            .parse::<u32>()
            .is_ok_and(|number| (1..=feedback::MAX_ATTEMPTS).contains(&number))
    {
        return Err(invalid("Invalid preview directory identity"));
    }
    let previews = root.join("previews");
    let directory = previews.join(id);
    for (path, parent) in [(&previews, root), (&directory, previews.as_path())] {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || fs::canonicalize(path)?.parent() != Some(parent)
        {
            return Err(invalid("Smart preview directory escaped its task"));
        }
    }
    Ok(directory)
}

pub(super) fn read_plan(root: &Path, bundle: &PreviewBundle) -> Result<EditPlan> {
    let root = fs::canonicalize(root)?;
    bundle_identity(bundle)?;
    let directory = preview_directory(&root, &bundle.bundle_id)?;
    read(&directory.join("plan.json"), feedback::MAX_PLAN_BYTES)
}

pub(super) fn reusable_preview(
    root: &Path,
    task: &Task,
    job: &Job,
    cancel: &CancelToken,
    clock: &Clock,
) -> Result<Option<PreviewBundle>> {
    cancel.check()?;
    if job.phase != Phase::NeedPreview {
        return Ok(None);
    }
    validate(root, task, job)?;
    let floor = job
        .rounds
        .iter()
        .map(|round| round.preview.attempt)
        .max()
        .unwrap_or(job.preview_floor)
        .max(job.preview_floor);
    let digest = plan_digest(task, &job.candidate)?;
    let trajectory = feedback::trajectory(root)?;
    let event = trajectory
        .events
        .iter()
        .filter(|event| {
            event.kind == "published" && event.plan_digest == digest && event.attempt > floor
        })
        .min_by_key(|event| event.attempt);
    let Some(event) = event else {
        return Ok(None);
    };
    let id = format!("{:02}-{digest}", event.attempt);
    if event.bundle_id.as_deref() != Some(id.as_str()) {
        return Err(invalid(
            "Published preview event has a different fixed identity",
        ));
    }
    let directory = preview_directory(root, &id)?;
    let mut bundle: PreviewBundle = read(&directory.join("bundle.json"), feedback::MAX_PLAN_BYTES)?;
    bundle_identity(&bundle)?;
    if bundle.bundle_id != id || bundle.plan_digest != digest || bundle.attempt != event.attempt {
        return Err(invalid(
            "Reusable preview does not match its published candidate",
        ));
    }
    bundle.quality = feedback::verify_bundle(root, task, &bundle, cancel, clock)?;
    bundle.verification_scope = "verified_on_reuse_only_not_a_final_deliverable".into();
    Ok(Some(bundle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feedback::QualityReport;
    use crate::review::{PreviewImage, ReviewRequest};
    use crate::smart_feedback::SmartRound;
    use crate::{media, Workspace};

    fn fixture(root: &Path) -> Task {
        // Structural state tests use inert local bytes, not playable media or
        // genuine model evidence. Only the explicitly ignored test renders.
        let bytes = b"inert smart-state source fixture";
        let path = root.join("inert-source.bin");
        fs::write(&path, bytes).unwrap();
        let metadata = path.metadata().unwrap();
        let task = Task {
            schema_version: 1,
            id: root.file_name().unwrap().to_string_lossy().into(),
            created_at: now(),
            intent: "Metadata-only smart-state test, no media/model claim".into(),
            target_duration_ms: 1000,
            assets: vec![Asset {
                id: "source-1".into(),
                path,
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
                    uncertainty: "Not a playable or model-reviewed sample".into(),
                    evidence_ids: vec![],
                    human_modified: true,
                }],
                summary: "MANUAL state fixture".into(),
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
        };
        store::save(root, &task).unwrap();
        task
    }

    fn rooted() -> (tempfile::TempDir, PathBuf, Task) {
        let temporary = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temporary.path()).unwrap();
        let task = fixture(&root);
        (temporary, root, task)
    }

    fn fake_bundle(task: &Task) -> PreviewBundle {
        let digest = plan_digest(task, task.plan.as_ref().unwrap()).unwrap();
        let id = format!("01-{digest}");
        let directory = format!("previews/{id}");
        PreviewBundle {
            schema: "video-agent-preview/v1".into(),
            bundle_id: id,
            plan_digest: digest.clone(),
            attempt: 1,
            relative_dir: directory.clone(),
            media_path: format!("{directory}/preview.mp4"),
            plan_path: format!("{directory}/plan.json"),
            quality_path: format!("{directory}/quality.json"),
            manifest_path: format!("{directory}/manifest.json"),
            sha256: "a".repeat(64),
            created_at: now(),
            verification_scope: "historical_fixture_not_verified".into(),
            quality: QualityReport {
                schema: "video-agent-objective-quality/v1".into(),
                plan_digest: digest,
                objective_pass: false,
                semantic_evaluation: "not_evaluated".into(),
                checks: vec![],
                plan_duration_ms: 1000,
                target_duration_ms: 1000,
                observed_duration_ms: 1000,
                width: 640,
                height: 360,
                has_audio: false,
                matches_current_plan: true,
                needs_human_review: true,
            },
        }
    }

    fn calling(root: &Path, task: &mut Task, job: &mut Job) {
        let frames = root.join("frames");
        fs::create_dir(&frames).unwrap();
        let source = Sample {
            id: "source-1-500".into(),
            source_id: "source-1".into(),
            time_ms: 500,
            image_path: frames.join("source-1-500.jpg"),
            sha256: "b".repeat(64),
        };
        task.samples.push(source.clone());
        task.usage.frames_created = 1;
        store::event(task, "sample_observed", serde_json::json!({"sample_id":source.id,"source_id":source.source_id,"decoded_time_ms":source.time_ms}).to_string());
        let bundle = fake_bundle(task);
        let image = PreviewImage {
            id: format!("rendered-{}-0", bundle.bundle_id),
            path: root
                .join("feedback-frames")
                .join(&bundle.bundle_id)
                .join("00.jpg"),
            sha256: "c".repeat(64),
            output_time_ms: 500,
            clip_id: "manual-fixture".into(),
            source_id: "source-1".into(),
            source_time_ms: 500,
        };
        let request = ReviewRequest {
            call_id: "feedback-call-1".into(),
            intent: task.intent.clone(),
            target_duration_ms: task.target_duration_ms,
            assets: task.assets.clone(),
            plan: task.plan.clone().unwrap(),
            plan_digest: bundle.plan_digest.clone(),
            preview_sha256: bundle.sha256.clone(),
            quality: bundle.quality.clone(),
            preview_frames: vec![image],
            source_samples: vec![source],
            limits: task.limits.clone(),
        };
        let reservation = ModelCall {
            call_id: request.call_id.clone(),
            backend: "SIMULATED-STATE-TEST".into(),
            requested_model: "not-a-model".into(),
            model: "not-a-model".into(),
            prompt_version: review::PROMPT_VERSION.into(),
            sample_ids: super::super::submitted_ids(&request),
            provider_response_id: None,
            request_id: None,
            input_tokens: 100,
            output_tokens: 2500,
            estimated_cost_usd: 0.01,
            elapsed_ms: 0,
            status: "feedback_reserved".into(),
            decision: None,
            error: None,
        };
        task.model_calls.push(reservation.clone());
        task.usage.model_calls = 1;
        task.usage.input_tokens = 100;
        task.usage.output_tokens = 2500;
        task.usage.estimated_cost_usd = 0.01;
        store::save(root, task).unwrap();
        job.rounds.push(SmartRound {
            round: 1,
            preview: bundle,
            review: None,
            model_call_id: None,
            proposal_call_id: None,
            rendered_observations: vec![],
            source_observations: vec![],
        });
        job.pending_request = Some(request);
        job.reservation = Some(reservation);
        job.phase = Phase::Calling;
        job.record("model_reserved", None).unwrap();
        save(root, job).unwrap();
    }

    #[test]
    fn run_lock_is_exclusive_and_released_on_drop() {
        let (_temporary, root, _task) = rooted();
        let guard = run_lock(&root).unwrap();
        assert!(run_lock(&root).is_err());
        drop(guard);
        assert!(run_lock(&root).is_ok());
    }

    #[test]
    fn initial_state_is_stable_private_and_round_limits_cannot_be_changed() {
        let (_temporary, root, mut task) = rooted();
        let before = task.plan.clone();
        let first = load_or_create(&root, &mut task, 2).unwrap();
        assert_eq!(
            first.run_id,
            format!(
                "smart-{}",
                plan_digest(&task, before.as_ref().unwrap()).unwrap()
            )
        );
        assert_eq!(first.phase, Phase::NeedPreview);
        assert!(first.rounds.is_empty());
        assert!(first.pending_request.is_none());
        let bytes = fs::read(root.join(DIRECTORY).join(format!("{}.json", first.run_id))).unwrap();
        let reloaded = load_or_create(&root, &mut task, 2).unwrap();
        assert_eq!(reloaded.run_id, first.run_id);
        assert!(load_or_create(&root, &mut task, 3).is_err());
        assert_eq!(
            fs::read(root.join(DIRECTORY).join(format!("{}.json", first.run_id))).unwrap(),
            bytes
        );
        assert_eq!(
            task.history
                .iter()
                .filter(|event| event.kind == "smart_feedback_started")
                .count(),
            1
        );
        assert_eq!(task.plan, before);
        assert!(task.approval.is_none());
        assert!(task.export.is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(root.join(DIRECTORY).join(format!("{}.json", first.run_id)))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn state_before_activation_and_missing_head_recover_without_reinitialization() {
        let (_temporary, root, mut task) = rooted();
        let original = task.clone();
        let job = load_or_create(&root, &mut task, 2).unwrap();
        // Deterministic durable prefix: state exists, activation write did not.
        store::save(&root, &original).unwrap();
        task = original;
        fs::remove_file(root.join(HEAD)).unwrap();
        assert_eq!(latest(&root, &task).unwrap().unwrap().run_id, job.run_id);
        let recovered = load_or_create(&root, &mut task, 2).unwrap();
        assert_eq!(recovered.run_id, job.run_id);
        assert_eq!(recovered.created_at, job.created_at);
        assert_eq!(task.history.iter().filter(|event| event.kind == "smart_feedback_started" && event.detail == job.run_id).count(), 1);
        assert_eq!(
            task.history
                .iter()
                .filter(|event| event.kind == "smart_feedback_history_recovered")
                .count(),
            1
        );
        assert!(root.join(HEAD).is_file());
        load_or_create(&root, &mut task, 2).unwrap();
        assert_eq!(
            task.history
                .iter()
                .filter(|event| event.kind == "smart_feedback_history_recovered")
                .count(),
            1
        );
    }

    #[test]
    fn activated_state_deletion_does_not_recreate_or_refund_the_run() {
        let (_temporary, root, mut task) = rooted();
        let job = load_or_create(&root, &mut task, 1).unwrap();
        let path = root.join(DIRECTORY).join(format!("{}.json", job.run_id));
        fs::remove_file(&path).unwrap();
        assert!(load_or_create(&root, &mut task, 1).is_err());
        assert!(latest(&root, &task).is_err());
        assert!(!path.exists());
        assert_eq!(task.usage.model_calls, 0);
    }

    #[test]
    fn current_plan_state_wins_over_head_and_archived_runs_are_not_overwritten() {
        let (_temporary, root, mut task) = rooted();
        let first = load_or_create(&root, &mut task, 1).unwrap();
        let old_path = root.join(DIRECTORY).join(format!("{}.json", first.run_id));
        let old_bytes = fs::read(&old_path).unwrap();
        task.plan.as_mut().unwrap().revision += 1;
        task.plan.as_mut().unwrap().summary.push_str(" changed");
        store::save(&root, &task).unwrap();
        let second = load_or_create(&root, &mut task, 2).unwrap();
        atomic(
            &root.join(HEAD),
            &encoded(
                &Head {
                    schema: "video-agent-smart-head/v1".into(),
                    task_id: task.id.clone(),
                    run_id: first.run_id.clone(),
                },
                1024,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(latest(&root, &task).unwrap().unwrap().run_id, second.run_id);
        task.plan.as_mut().unwrap().revision += 1;
        store::save(&root, &task).unwrap();
        assert_eq!(latest(&root, &task).unwrap().unwrap().run_id, first.run_id);
        assert_eq!(fs::read(old_path).unwrap(), old_bytes);
    }

    #[test]
    fn malformed_head_foreign_task_and_oversized_state_fail_closed() {
        let (_temporary, root, mut task) = rooted();
        let job = load_or_create(&root, &mut task, 1).unwrap();
        let path = root.join(DIRECTORY).join(format!("{}.json", job.run_id));
        let original = fs::read(&path).unwrap();
        fs::write(&path, vec![b' '; MAX_STATE_BYTES as usize + 1]).unwrap();
        assert!(latest(&root, &task).is_err());
        fs::write(&path, &original).unwrap();
        let mut foreign: Job = serde_json::from_slice(&original).unwrap();
        foreign.task_id = "foreign-task".into();
        fs::write(&path, encoded(&foreign, MAX_STATE_BYTES).unwrap()).unwrap();
        assert!(latest(&root, &task).is_err());
        fs::write(&path, original).unwrap();
        task.plan.as_mut().unwrap().revision += 1;
        store::save(&root, &task).unwrap();
        fs::write(root.join(HEAD), b"{\"schema\":\"video-agent-smart-head/v1\",\"task_id\":\"fixture\",\"run_id\":\"../outside\"}").unwrap();
        assert!(latest(&root, &task).is_err());
    }

    #[test]
    fn calling_is_not_replayed_and_incomplete_or_switched_requests_are_rejected() {
        let (_temporary, root, mut task) = rooted();
        let mut job = load_or_create(&root, &mut task, 2).unwrap();
        calling(&root, &mut task, &mut job);
        let reloaded = load_or_create(&root, &mut task, 2).unwrap();
        assert_eq!(reloaded.phase, Phase::Calling);
        assert_eq!(task.model_calls[0].status, "feedback_reserved");
        assert_eq!(task.usage.model_calls, 1);
        let mut switched = reloaded.clone();
        switched.pending_request.as_mut().unwrap().preview_sha256 = "d".repeat(64);
        assert!(save(&root, &switched).is_err());
        let mut missing = reloaded.clone();
        missing.phase = Phase::Received;
        assert!(save(&root, &missing).is_err());
        let mut escaped = reloaded.clone();
        escaped.rounds[0].preview.plan_path = "../outside.json".into();
        assert!(save(&root, &escaped).is_err());
        let mut expanded = reloaded;
        expanded
            .pending_request
            .as_mut()
            .unwrap()
            .limits
            .max_total_tokens += 1;
        assert!(save(&root, &expanded).is_err());
    }

    #[test]
    fn orphan_task_reservation_remains_charged_and_is_marked_unknown_once() {
        let (_temporary, root, mut task) = rooted();
        let mut job = load_or_create(&root, &mut task, 2).unwrap();
        let initial = fs::read(root.join(DIRECTORY).join(format!("{}.json", job.run_id))).unwrap();
        calling(&root, &mut task, &mut job);
        // Task reserve was saved, but Calling was not: restore the exact prior
        // smart journal while retaining its durable task token/cost charge.
        fs::write(
            root.join(DIRECTORY).join(format!("{}.json", job.run_id)),
            initial,
        )
        .unwrap();
        let before = serde_json::to_value(&task.usage).unwrap();
        load_or_create(&root, &mut task, 2).unwrap();
        assert_eq!(
            task.model_calls[0].status,
            "feedback_interrupted_unknown_usage"
        );
        assert_eq!(serde_json::to_value(&task.usage).unwrap(), before);
        assert_eq!(
            task.history
                .iter()
                .filter(|event| event.kind == "smart_feedback_orphan_reservation_recovered")
                .count(),
            1
        );
        load_or_create(&root, &mut task, 2).unwrap();
        assert_eq!(
            task.history
                .iter()
                .filter(|event| event.kind == "smart_feedback_orphan_reservation_recovered")
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn lock_and_run_symlinks_never_follow_an_external_target() {
        use std::os::unix::fs::symlink;
        let (_temporary, root, mut task) = rooted();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("untouched");
        fs::write(&target, b"must remain unchanged").unwrap();
        symlink(&target, root.join("smart-feedback.lock")).unwrap();
        assert!(run_lock(&root).is_err());
        let job = load_or_create(&root, &mut task, 1).unwrap();
        let path = root.join(DIRECTORY).join(format!("{}.json", job.run_id));
        fs::remove_file(&path).unwrap();
        symlink(&target, &path).unwrap();
        assert!(latest(&root, &task).is_err());
        assert_eq!(fs::read(target).unwrap(), b"must remain unchanged");
    }

    #[test]
    #[ignore = "requires real FFmpeg/ffprobe; published preview recovery only, no model"]
    fn published_preview_is_reused_after_stage_interruption_but_partial_is_not() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("synthetic.mp4");
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
        let workspace = Workspace::create(
            &temporary.path().join("tasks"),
            std::slice::from_ref(&source),
            "SYNTHETIC smart preview recovery test".into(),
            1000,
            Limits::default(),
        )
        .unwrap();
        workspace.manual_draft().unwrap();
        let root = workspace.task_dir();
        let cancel = CancelToken::default();
        let job = feedback::with_task(root, &cancel, |root, task, _| load_or_create(root, task, 2))
            .unwrap();
        let bundle = feedback::capture_plan(root, job.candidate.clone(), &cancel).unwrap();
        // The published preview is durable, but the owner's Job still says NeedPreview.
        let reused = feedback::with_task(root, &cancel, |root, task, clock| {
            reusable_preview(root, task, &job, &cancel, clock)
        })
        .unwrap()
        .unwrap();
        assert_eq!(reused.bundle_id, bundle.bundle_id);
        assert_eq!(feedback::trajectory(root).unwrap().attempts_started, 1);
        let mut floor = job.clone();
        floor.preview_floor = bundle.attempt;
        let next_id = format!("02-{}", bundle.plan_digest);
        fs::create_dir(root.join("previews").join(&next_id)).unwrap();
        fs::write(
            root.join("previews").join(next_id).join("bundle.json"),
            b"partial and not published",
        )
        .unwrap();
        let absent = feedback::with_task(root, &cancel, |root, task, clock| {
            reusable_preview(root, task, &floor, &cancel, clock)
        })
        .unwrap();
        assert!(absent.is_none());
        // No second publication was fabricated; the arbitrary directory does
        // not become a finished round or consume a fresh preview attempt.
        assert_eq!(feedback::trajectory(root).unwrap().attempts_started, 1);
        assert_eq!(workspace.load().unwrap().usage.model_calls, 0);
    }
}
