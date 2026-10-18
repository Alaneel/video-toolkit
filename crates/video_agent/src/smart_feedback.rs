//! Bounded render -> sparse-image model review -> evidenced revision loop.
//! All candidates remain what-if plans until a separate explicit human adoption.
use crate::domain::*;
use crate::execution::Clock;
use crate::feedback::{self, PreviewBundle};
use crate::review::{
    self, PreviewImage, PreviewReviewProvider, ReviewDecision, ReviewRequest, ReviewVerdict,
};
use crate::store::{self, invalid, now};
use crate::workflow::{check_sources_during, plan_digest, validate_plan};
use crate::{media, Workspace};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, process::Child, time::Instant};

mod observations;
mod state;

const MAX_ROUNDS: u32 = 3;
const MAX_EVENTS: usize = 96;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmartRound {
    pub round: u32,
    pub preview: PreviewBundle,
    pub review: Option<ReviewDecision>,
    pub model_call_id: Option<String>,
    pub proposal_call_id: Option<String>,
    pub rendered_observations: Vec<PreviewImage>,
    pub source_observations: Vec<Sample>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmartEvent {
    pub sequence: usize,
    pub at: String,
    pub kind: String,
    pub round: u32,
    pub plan_digest: String,
    pub call_id: Option<String>,
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmartReport {
    pub schema: String,
    pub run_id: String,
    pub base_plan_digest: String,
    pub max_rounds: u32,
    pub rounds: Vec<SmartRound>,
    pub phase: String,
    pub stopped_reason: Option<String>,
    pub selected_plan: Option<String>,
    pub needs_human_review: bool,
    pub task_plan_unchanged: bool,
    pub matches_current_plan: bool,
    pub semantic_evaluation: String,
    pub verification_scope: String,
    pub events: Vec<SmartEvent>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    NeedPreview,
    NeedReview,
    Calling,
    Received,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingResult {
    record: ModelCall,
    decision: Option<ReviewDecision>,
    error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    schema: String,
    task_id: String,
    run_id: String,
    base_plan_digest: String,
    base_revision: u64,
    max_rounds: u32,
    preview_floor: u32,
    created_at: String,
    phase: Phase,
    candidate: EditPlan,
    candidate_from_call: Option<String>,
    rounds: Vec<SmartRound>,
    pending_request: Option<ReviewRequest>,
    reservation: Option<ModelCall>,
    pending_result: Option<PendingResult>,
    stopped_reason: Option<String>,
    selected_plan: Option<String>,
    events: Vec<SmartEvent>,
}

impl Job {
    fn record(&mut self, kind: &str, code: Option<&str>) -> Result<()> {
        if self.events.len() >= MAX_EVENTS {
            return Err(invalid("Smart feedback event budget exhausted"));
        }
        self.events.push(SmartEvent {
            sequence: self.events.len() + 1,
            at: now(),
            kind: kind.into(),
            round: self.rounds.len() as u32,
            plan_digest: self.rounds.last().map_or_else(
                || self.base_plan_digest.clone(),
                |r| r.preview.plan_digest.clone(),
            ),
            call_id: self.reservation.as_ref().map(|r| r.call_id.clone()),
            code: code.map(str::to_owned),
        });
        Ok(())
    }
    fn stop(&mut self, reason: &str) -> Result<()> {
        self.phase = Phase::Completed;
        self.stopped_reason = Some(reason.into());
        self.record("stopped", Some(reason))
    }
    fn report(&self, task: &Task) -> Result<SmartReport> {
        Ok(SmartReport {
            schema: "video-agent-smart-feedback/v1".into(),
            run_id: self.run_id.clone(),
            base_plan_digest: self.base_plan_digest.clone(),
            max_rounds: self.max_rounds,
            rounds: self.rounds.clone(),
            phase: serde_json::to_value(self.phase)?
                .as_str()
                .unwrap_or("invalid")
                .into(),
            stopped_reason: self.stopped_reason.clone(),
            selected_plan: self.selected_plan.clone(),
            needs_human_review: true,
            task_plan_unchanged: true,
            matches_current_plan: task
                .plan
                .as_ref()
                .map(|p| plan_digest(task, p))
                .transpose()?
                .as_ref()
                == Some(&self.base_plan_digest),
            semantic_evaluation:
                "model_self_assessment_of_sparse_frames_not_human_or_full_video_evaluation".into(),
            verification_scope: "historical_metadata_only_not_fresh_verification".into(),
            events: self.events.clone(),
        })
    }
}

fn check_base(task: &Task, job: &Job) -> Result<()> {
    let plan = task
        .plan
        .as_ref()
        .ok_or_else(|| invalid("No saved draft for smart feedback"))?;
    if task.id != job.task_id
        || plan.revision != job.base_revision
        || plan_digest(task, plan)? != job.base_plan_digest
    {
        return Err(invalid(
            "The saved plan changed during smart feedback; reload before continuing",
        ));
    }
    Ok(())
}

fn reserve(
    task: &mut Task,
    request: &ReviewRequest,
    provider: &dyn PreviewReviewProvider,
) -> Result<ModelCall> {
    if !provider.ready() {
        return Err(invalid("Preview review provider is not configured"));
    }
    if task.usage.model_calls >= task.limits.max_model_calls {
        return Err(invalid("Shared model-call budget exhausted"));
    }
    let (tokens, cost) = provider.estimate(request)?;
    let output = u64::from(task.limits.max_output_tokens);
    if tokens < output
        || !cost.is_finite()
        || cost < 0.0
        || task
            .usage
            .input_tokens
            .saturating_add(task.usage.output_tokens)
            .saturating_add(tokens)
            > task.limits.max_total_tokens
        || task.usage.estimated_cost_usd + cost > task.limits.max_estimated_cost_usd + 1e-9
    {
        return Err(invalid(
            "Preview review would exceed the shared token or estimated-cost budget",
        ));
    }
    let (backend, model, prompt) = provider.identity();
    let record = ModelCall {
        call_id: request.call_id.clone(),
        backend,
        requested_model: model.clone(),
        model,
        prompt_version: prompt,
        sample_ids: submitted_ids(request),
        provider_response_id: None,
        request_id: None,
        input_tokens: tokens - output,
        output_tokens: output,
        estimated_cost_usd: cost,
        elapsed_ms: 0,
        status: "feedback_reserved".into(),
        decision: None,
        error: None,
    };
    task.usage.model_calls += 1;
    task.usage.input_tokens += record.input_tokens;
    task.usage.output_tokens += record.output_tokens;
    task.usage.estimated_cost_usd += cost;
    task.model_calls.push(record.clone());
    store::event(
        task,
        "feedback_call_reserved",
        format!(
            "{} reserved before provider admission; unknown usage remains charged",
            record.call_id
        ),
    );
    Ok(record)
}

fn submitted_ids(request: &ReviewRequest) -> Vec<String> {
    request
        .source_samples
        .iter()
        .map(|s| format!("source:{}", s.id))
        .chain(
            request
                .preview_frames
                .iter()
                .map(|s| format!("preview:{}", s.id)),
        )
        .collect()
}

fn record_index(task: &Task, job: &Job) -> Result<usize> {
    let reservation = job
        .reservation
        .as_ref()
        .ok_or_else(|| invalid("Review reservation is missing"))?;
    task.model_calls
        .iter()
        .position(|r| r.call_id == reservation.call_id)
        .ok_or_else(|| invalid("Required review reservation disappeared; refusing a budget reset"))
}

fn valid_actual(reserved: &ModelCall, actual: &ModelCall) -> bool {
    let bounded = |value: &str, limit: usize| {
        !value.is_empty()
            && value.len() <= limit
            && value.is_ascii()
            && !value.chars().any(char::is_control)
    };
    actual.call_id == reserved.call_id
        && actual.backend == reserved.backend
        && actual.requested_model == reserved.requested_model
        && actual.prompt_version == reserved.prompt_version
        && bounded(&actual.model, 128)
        && actual.estimated_cost_usd.is_finite()
        && actual.estimated_cost_usd >= 0.0
        && actual
            .provider_response_id
            .as_deref()
            .is_none_or(|id| bounded(id, 256))
        && actual
            .request_id
            .as_deref()
            .is_none_or(|id| bounded(id, 256))
        && (actual.backend != "openai_responses" || actual.provider_response_id.is_some())
}

fn reconcile(task: &mut Task, job: &Job) -> Result<bool> {
    let index = record_index(task, job)?;
    let reserved = job.reservation.as_ref().unwrap();
    let pending = job
        .pending_result
        .as_ref()
        .ok_or_else(|| invalid("Received review record is missing"))?;
    let actual = &pending.record;
    if !valid_actual(reserved, actual) {
        return Err(invalid(
            "Review usage identity does not match its reservation",
        ));
    }
    let exceeded = actual.input_tokens.saturating_add(actual.output_tokens)
        > reserved.input_tokens.saturating_add(reserved.output_tokens)
        || actual.estimated_cost_usd > reserved.estimated_cost_usd + 1e-9;
    if task.model_calls[index].status == "feedback_reserved" {
        if task.usage.input_tokens < reserved.input_tokens
            || task.usage.output_tokens < reserved.output_tokens
            || task.usage.estimated_cost_usd + 1e-9 < reserved.estimated_cost_usd
        {
            return Err(invalid("Review accounting counters are inconsistent"));
        }
        task.usage.input_tokens =
            (task.usage.input_tokens - reserved.input_tokens).saturating_add(actual.input_tokens);
        task.usage.output_tokens = (task.usage.output_tokens - reserved.output_tokens)
            .saturating_add(actual.output_tokens);
        task.usage.estimated_cost_usd = (task.usage.estimated_cost_usd
            - reserved.estimated_cost_usd
            + actual.estimated_cost_usd)
            .max(0.0);
        let mut record = actual.clone();
        record.sample_ids = submitted_ids(
            job.pending_request
                .as_ref()
                .ok_or_else(|| invalid("Review request identity missing"))?,
        );
        record.decision = None;
        record.status = if exceeded {
            "feedback_usage_exceeded"
        } else if pending.decision.is_none() {
            "feedback_response_rejected"
        } else {
            "feedback_received"
        }
        .into();
        task.model_calls[index] = record;
    } else if task.model_calls[index].provider_response_id != actual.provider_response_id
        || task.model_calls[index].input_tokens != actual.input_tokens
        || task.model_calls[index].output_tokens != actual.output_tokens
        || (task.model_calls[index].estimated_cost_usd - actual.estimated_cost_usd).abs() > 1e-9
    {
        return Err(invalid("Received review usage changed after settlement"));
    }
    Ok(exceeded)
}

/// Only the live owner can know that provider.review was never entered. A
/// reopened Calling record is unknown and is deliberately not refunded.
fn not_sent(root: &Path, job: &mut Job) -> Result<()> {
    let _lock = store::lock(root)?;
    let mut task = store::read(root)?;
    let index = record_index(&task, job)?;
    let record = &mut task.model_calls[index];
    if record.status != "feedback_reserved"
        || task.usage.input_tokens < record.input_tokens
        || task.usage.output_tokens < record.output_tokens
        || task.usage.estimated_cost_usd + 1e-9 < record.estimated_cost_usd
    {
        return Err(invalid(
            "Cannot reconcile a known-unsent review reservation",
        ));
    }
    task.usage.input_tokens -= record.input_tokens;
    task.usage.output_tokens -= record.output_tokens;
    task.usage.estimated_cost_usd =
        (task.usage.estimated_cost_usd - record.estimated_cost_usd).max(0.0);
    record.input_tokens = 0;
    record.output_tokens = 0;
    record.estimated_cost_usd = 0.0;
    record.status = "feedback_not_sent".into();
    record.error=Some("Live local admission stopped before the provider was entered; tokens/cost refunded, attempt retained".into());
    job.stop("local_admission_stopped_before_model")?;
    store::save(root, &task)?;
    state::save(root, job)
}

/// Render and review at most three rounds. Re-entry resumes durable local stages,
/// never silently repeats an interrupted/unknown external model call.
pub fn run(
    workspace: &Path,
    provider: &dyn PreviewReviewProvider,
    max_rounds: u32,
    consent_preview_upload: bool,
    cancel: &CancelToken,
) -> Result<SmartReport> {
    if !(1..=MAX_ROUNDS).contains(&max_rounds) {
        return Err(invalid("Smart feedback requires 1–3 rounds"));
    }
    if !consent_preview_upload {
        return Err(invalid("Explicit consent is required for source and rendered JPEGs, intent, plan and objective facts"));
    }
    cancel.check()?;
    let workspace = Workspace::open_with_cancel(workspace, cancel)?;
    let root = workspace.task_dir();
    let _run_lock = state::run_lock(root)?;
    let mut job = feedback::with_task(root, cancel, |root, task, _| {
        if task.samples.is_empty() {
            return Err(invalid("Create source observations before smart feedback"));
        }
        state::load_or_create(root, task, max_rounds)
    })?;
    let mut admitted_here = false;
    loop {
        if job.phase == Phase::Completed {
            let task = Workspace::snapshot(root)?;
            return job.report(&task);
        }
        if cancel.is_cancelled() {
            if job.phase == Phase::Calling && admitted_here {
                not_sent(root, &mut job)?;
            }
            return Err(AgentError::Cancelled);
        }
        match job.phase {
            Phase::NeedPreview => {
                let reusable = feedback::with_task(root, cancel, |root, task, clock| {
                    check_base(task, &job)?;
                    if !provider.ready() {
                        return Err(invalid("Preview review provider is not configured"));
                    }
                    if task.usage.model_calls >= task.limits.max_model_calls {
                        return Err(invalid(
                            "Shared model-call budget exhausted before rendering",
                        ));
                    }
                    state::reusable_preview(root, task, &job, cancel, clock)
                })?;
                let bundle = match reusable {
                    Some(bundle) => bundle,
                    None => feedback::capture_plan(root, job.candidate.clone(), cancel)?,
                };
                feedback::with_task(root, cancel, |root, task, _| {
                    check_base(task, &job)?;
                    if bundle.plan_digest != plan_digest(task, &job.candidate)? {
                        return Err(invalid("Preview does not match the requested candidate"));
                    }
                    job.rounds.push(SmartRound {
                        round: job.rounds.len() as u32 + 1,
                        preview: bundle,
                        review: None,
                        model_call_id: None,
                        proposal_call_id: job.candidate_from_call.clone(),
                        rendered_observations: vec![],
                        source_observations: vec![],
                    });
                    job.phase = Phase::NeedReview;
                    job.record("preview_ready", None)?;
                    state::save(root, &job)
                })?;
            }
            Phase::NeedReview => {
                feedback::with_task(root, cancel, |root, task, clock| {
                    check_base(task, &job)?;
                    if !provider.ready() {
                        return Err(invalid("Preview review provider is not configured"));
                    }
                    if task.usage.model_calls >= task.limits.max_model_calls {
                        return Err(invalid("Shared model-call budget exhausted"));
                    }
                    let bundle = &job
                        .rounds
                        .last()
                        .ok_or_else(|| invalid("Missing round preview"))?
                        .preview;
                    let quality = feedback::verify_bundle(root, task, bundle, cancel, clock)?;
                    let preview_frames =
                        observations::rendered(root, task, bundle, &job.candidate, cancel, clock)?;
                    let source_samples =
                        observations::sources(root, task, &job.candidate, cancel, clock)?;
                    let request = ReviewRequest {
                        call_id: format!("feedback-call-{}", task.usage.model_calls + 1),
                        intent: task.intent.clone(),
                        target_duration_ms: task.target_duration_ms,
                        assets: task.assets.clone(),
                        plan: job.candidate.clone(),
                        plan_digest: bundle.plan_digest.clone(),
                        preview_sha256: bundle.sha256.clone(),
                        quality,
                        preview_frames,
                        source_samples,
                        limits: task.limits.clone(),
                    };
                    let reservation = reserve(task, &request, provider)?;
                    store::save(root, task)?;
                    job.pending_request = Some(request);
                    job.reservation = Some(reservation);
                    job.pending_result = None;
                    job.phase = Phase::Calling;
                    job.record("model_reserved", None)?;
                    state::save(root, &job)
                })?;
                admitted_here = true;
            }
            Phase::Calling => {
                if !admitted_here {
                    feedback::with_task(root, cancel, |root, task, _| {
                        check_base(task, &job)?;
                        let index = record_index(task, &job)?;
                        if task.model_calls[index].status == "feedback_reserved" {
                            task.model_calls[index].status =
                                "feedback_interrupted_unknown_usage".into();
                            task.model_calls[index].error=Some("Previous owner stopped before durably recording a review; reservation retained and request not repeated".into());
                        }
                        let reason = if task.model_calls[index].status == "feedback_not_sent" {
                            "local_admission_stopped_before_model"
                        } else {
                            "interrupted_model_call_usage_unknown"
                        };
                        job.stop(reason)?;
                        store::save(root, task)?;
                        state::save(root, &job)
                    })?;
                    continue;
                }
                let sent = feedback::with_task(root, cancel, |root, task, clock| {
                    check_base(task, &job)?;
                    let index = record_index(task, &job)?;
                    if task.model_calls[index].status != "feedback_reserved" {
                        return Err(invalid("Review call is not in its reserved state"));
                    }
                    let identity = provider.identity();
                    let reserved = job.reservation.as_ref().unwrap();
                    if (
                        reserved.backend.clone(),
                        reserved.requested_model.clone(),
                        reserved.prompt_version.clone(),
                    ) != identity
                    {
                        return Err(invalid("Review provider changed after reservation"));
                    }
                    let mut request = job
                        .pending_request
                        .clone()
                        .ok_or_else(|| invalid("Reserved review request missing"))?;
                    observations::verify_request_images(root, task, &request, cancel, clock)?;
                    let timeout = clock.timeout(task.limits.model_timeout_secs)?;
                    if timeout.as_secs() == 0 {
                        return Err(AgentError::TimedOut);
                    }
                    request.limits.model_timeout_secs = timeout.as_secs();
                    job.pending_request = Some(request.clone());
                    job.record("model_started", None)?;
                    state::save(root, &job)?;
                    let started = Instant::now();
                    let result = provider.review(&request, cancel);
                    let pending = match result {
                        Ok(outcome) => PendingResult {
                            record: outcome.record,
                            decision: Some(outcome.decision),
                            error_code: None,
                        },
                        Err(AgentError::ModelResponseRejected { record, .. }) => PendingResult {
                            record: *record,
                            decision: None,
                            error_code: Some("response_rejected".into()),
                        },
                        Err(error) => {
                            task.model_calls[index].status = "feedback_failed_unknown_usage".into();
                            task.model_calls[index].elapsed_ms =
                                started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                            task.model_calls[index].error=Some("Preview review failed; response/transport details withheld and reservation retained".into());
                            job.stop("model_request_failed_unknown_usage")?;
                            store::save(root, task)?;
                            state::save(root, &job)?;
                            return Err(error);
                        }
                    };
                    let mut pending = pending;
                    if !valid_actual(job.reservation.as_ref().unwrap(), &pending.record) {
                        task.model_calls[index].status = "feedback_invalid_usage_record".into();
                        task.model_calls[index].error=Some("Provider returned an invalid or foreign usage identity; reservation retained".into());
                        job.stop("invalid_provider_usage_record")?;
                        store::save(root, task)?;
                        state::save(root, &job)?;
                        return Err(invalid(
                            "Provider usage identity does not match its reservation",
                        ));
                    }
                    pending.record.sample_ids = submitted_ids(&request);
                    pending.record.decision = None;
                    pending.record.error = pending
                        .error_code
                        .as_ref()
                        .map(|_| "Response rejected; raw provider content withheld".into());
                    pending.record.elapsed_ms =
                        started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                    job.pending_result = Some(pending);
                    job.phase = Phase::Received;
                    job.record("model_received", None)?;
                    // Journal the decision BEFORE reconciling Task. Recovery can
                    // settle known usage and consume it without another request.
                    state::save(root, &job)?;
                    reconcile(task, &job)?;
                    store::save(root, task)
                });
                if let Err(error) = sent {
                    if job.phase == Phase::Calling {
                        not_sent(root, &mut job)?;
                    }
                    return Err(error);
                }
                admitted_here = false;
            }
            Phase::Received => {
                feedback::with_task(root, cancel, |root, task, clock| {
                    check_base(task, &job)?;
                    let exceeded = reconcile(task, &job)?;
                    store::save(root, task)?;
                    if exceeded {
                        job.stop("reported_usage_exceeded_reservation")?;
                        state::save(root, &job)?;
                        return Err(invalid(
                            "Reported review usage exceeded its reservation; actual usage retained",
                        ));
                    }
                    let request = job
                        .pending_request
                        .as_ref()
                        .ok_or_else(|| invalid("Received review request missing"))?;
                    let pending = job
                        .pending_result
                        .as_ref()
                        .ok_or_else(|| invalid("Received review missing"))?;
                    let Some(decision) = pending.decision.clone() else {
                        job.stop("model_response_rejected")?;
                        state::save(root, &job)?;
                        return Err(invalid(
                            "Review response was rejected; reported usage retained",
                        ));
                    };
                    if review::validate_decision(request, &decision).is_err() {
                        let index = record_index(task, &job)?;
                        task.model_calls[index].status = "feedback_response_rejected".into();
                        task.model_calls[index].error =
                            Some("Review failed local evidence/schema validation".into());
                        job.stop("model_response_rejected")?;
                        store::save(root, task)?;
                        state::save(root, &job)?;
                        return Err(invalid("Review failed local evidence/schema validation"));
                    }
                    check_sources_during(task, cancel, clock)?;
                    let call_id = job.reservation.as_ref().unwrap().call_id.clone();
                    let index = record_index(task, &job)?;
                    let round = job
                        .rounds
                        .last_mut()
                        .ok_or_else(|| invalid("Missing received round"))?;
                    round.review = Some(decision.clone());
                    round.model_call_id = Some(call_id.clone());
                    round.rendered_observations = request.preview_frames.clone();
                    round.source_observations = request.source_samples.clone();
                    match decision.verdict {
                        ReviewVerdict::Accept => {
                            let objective = &round.preview.quality;
                            if objective
                                .checks
                                .iter()
                                .any(|c| !c.passed && c.name != "target_duration_alignment")
                            {
                                job.stop("objective_checks_rejected")?;
                            } else {
                                job.selected_plan = Some(round.preview.plan_path.clone());
                                let reason = if objective.objective_pass {
                                    "model_accept_requires_human_review"
                                } else {
                                    "model_accept_with_objective_warnings_requires_human_review"
                                };
                                job.stop(reason)?;
                            }
                            task.model_calls[index].status = "feedback_accept".into();
                        }
                        ReviewVerdict::Uncertain => {
                            task.model_calls[index].status = "feedback_uncertain".into();
                            job.stop("model_uncertain_requires_human_review")?;
                        }
                        ReviewVerdict::Revise => {
                            let candidate = EditPlan {
                                revision: job.base_revision,
                                clips: decision
                                    .revised_clips
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, c)| Clip {
                                        id: format!("feedback-{}-{}", job.rounds.len(), i + 1),
                                        source_id: c.source_id,
                                        start_ms: c.start_ms,
                                        end_ms: c.end_ms,
                                        reason: c.reason,
                                        uncertainty: c.uncertainty,
                                        evidence_ids: c.evidence_ids,
                                        human_modified: false,
                                    })
                                    .collect(),
                                summary: decision.summary,
                                audio_policy: job.candidate.audio_policy,
                            };
                            if let Err(error) = validate_plan(task, &candidate, true)
                                .and_then(|_| feedback::validate_candidate(task, &candidate))
                            {
                                task.model_calls[index].status =
                                    "feedback_response_rejected".into();
                                task.model_calls[index].error=Some("Model revision exceeded local candidate/evidence/preview limits; actual reported usage retained".into());
                                job.stop("model_revision_rejected")?;
                                store::save(root, task)?;
                                state::save(root, &job)?;
                                return Err(error);
                            }
                            let no_edit = candidate
                                .clips
                                .iter()
                                .map(|c| (&c.source_id, c.start_ms, c.end_ms))
                                .eq(job
                                    .candidate
                                    .clips
                                    .iter()
                                    .map(|c| (&c.source_id, c.start_ms, c.end_ms)));
                            task.model_calls[index].status = "feedback_revision_validated".into();
                            if no_edit {
                                job.stop("revision_has_no_edit_changes")?;
                            } else if job.rounds.len() >= job.max_rounds as usize {
                                job.stop("round_limit_reached")?;
                            } else {
                                job.candidate = candidate;
                                job.candidate_from_call = Some(call_id);
                                job.phase = Phase::NeedPreview;
                                job.record("revision_validated", None)?;
                            }
                        }
                    }
                    job.pending_request = None;
                    job.reservation = None;
                    job.pending_result = None;
                    store::save(root, task)?;
                    state::save(root, &job)
                })?;
            }
            Phase::Completed => unreachable!(),
        }
    }
}

/// Saved metadata only. No hash verification, rendering, provider configuration or calls.
pub fn status(workspace: &Path) -> Result<Option<SmartReport>> {
    let workspace = Workspace::open(workspace)?;
    let task = store::read(workspace.task_dir())?;
    state::latest(workspace.task_dir(), &task)?
        .map(|j| j.report(&task))
        .transpose()
}

/// Explicit human adoption retains actual model provenance and revokes approval.
pub fn adopt(workspace: &Path, expected_run_id: &str, cancel: &CancelToken) -> Result<Task> {
    let workspace = Workspace::open_with_cancel(workspace, cancel)?;
    let root = workspace.task_dir();
    let _guard = state::run_lock(root)?;
    feedback::with_task(root, cancel, |root, task, clock| {
        let job =
            state::latest(root, task)?.ok_or_else(|| invalid("No smart feedback candidate"))?;
        check_base(task, &job)?;
        if job.run_id != expected_run_id
            || job.phase != Phase::Completed
            || job.selected_plan.is_none()
        {
            return Err(invalid(
                "Explicit adoption must name the completed, selected run",
            ));
        }
        let round = job
            .rounds
            .last()
            .ok_or_else(|| invalid("Selected round missing"))?;
        if job.selected_plan.as_ref() != Some(&round.preview.plan_path)
            || round.review.as_ref().map(|r| r.verdict) != Some(ReviewVerdict::Accept)
        {
            return Err(invalid("Candidate has not received a valid model review"));
        }
        feedback::verify_bundle(root, task, &round.preview, cancel, clock)?;
        let mut plan: EditPlan = state::read_plan(root, &round.preview)?;
        feedback::validate_candidate(task, &plan)?;
        if task.plan.as_ref() == Some(&plan) {
            return Ok(task.clone());
        }
        let proposal_call_id = round
            .proposal_call_id
            .as_ref()
            .ok_or_else(|| invalid("Model revision provenance missing"))?;
        let call = task
            .model_calls
            .iter()
            .find(|c| {
                &c.call_id == proposal_call_id
                    && c.prompt_version == review::PROMPT_VERSION
                    && c.status == "feedback_revision_validated"
            })
            .ok_or_else(|| invalid("Validated revision call missing"))?;
        if plan
            .clips
            .iter()
            .flat_map(|c| &c.evidence_ids)
            .any(|id| !call.sample_ids.contains(&format!("source:{id}")))
        {
            return Err(invalid(
                "Adopted candidate exceeds the proposing call's source evidence",
            ));
        }
        plan.revision = job
            .base_revision
            .checked_add(1)
            .ok_or_else(|| invalid("Revision overflow"))?;
        task.plan = Some(plan);
        task.approval = None;
        task.status = TaskStatus::Review;
        task.last_error = None;
        if let Some(export) = &mut task.export {
            export.verified = false;
        }
        let digest = plan_digest(task, task.plan.as_ref().unwrap())?;
        store::event(task,"smart_feedback_adopted",serde_json::json!({"run_id":job.run_id,"base_plan_digest":job.base_plan_digest,"selected_plan_digest":digest,"revision":task.plan.as_ref().unwrap().revision,"proposal_call_id":proposal_call_id}).to_string());
        store::save(root, task)?;
        Ok(task.clone())
    })
}

/// Freshly verify the bound smart bundle before launching the fixed local player.
pub fn spawn_preview(workspace: &Path, cancel: &CancelToken) -> Result<Child> {
    feedback::with_task(workspace, cancel, |root, task, clock| {
        let job = state::latest(root, task)?.ok_or_else(|| invalid("No smart preview"))?;
        check_base(task, &job)?;
        let round = job
            .rounds
            .last()
            .ok_or_else(|| invalid("No smart preview round"))?;
        let info = feedback::verify_bundle(root, task, &round.preview, cancel, clock)?;
        let asset = Asset {
            id: "verified-smart-preview".into(),
            path: root.join(&round.preview.media_path),
            sha256: round.preview.sha256.clone(),
            size: 0,
            modified_ns: 0,
            duration_ms: info.plan_duration_ms,
            width: info.width,
            height: info.height,
            has_audio: info.has_audio,
        };
        let clip = Clip {
            id: "preview-playback".into(),
            source_id: asset.id.clone(),
            start_ms: 0,
            end_ms: info.plan_duration_ms,
            reason: "Local reviewed preview".into(),
            uncertainty: "Not a final deliverable".into(),
            evidence_ids: vec![],
            human_modified: true,
        };
        media::preview(&asset, &clip)
    })
}

fn adopted_call(task: &Task) -> Option<&ModelCall> {
    let plan = task.plan.as_ref()?;
    let digest = plan_digest(task, plan).ok()?;
    for event in task
        .history
        .iter()
        .rev()
        .filter(|e| e.kind == "smart_feedback_adopted")
    {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&event.detail) else {
            continue;
        };
        if value["revision"].as_u64() != Some(plan.revision)
            || value["selected_plan_digest"].as_str() != Some(digest.as_str())
        {
            continue;
        }
        if let Some(call) = value["proposal_call_id"].as_str().and_then(|id| {
            task.model_calls.iter().find(|call| {
                call.call_id == id
                    && call.prompt_version == review::PROMPT_VERSION
                    && call.status == "feedback_revision_validated"
                    && plan.clips.iter().all(|clip| {
                        !clip.human_modified
                            && !clip.evidence_ids.is_empty()
                            && clip
                                .evidence_ids
                                .iter()
                                .all(|id| call.sample_ids.contains(&format!("source:{id}")))
                    })
            })
        }) {
            return Some(call);
        }
    }
    None
}

/// Validated proposal provenance, including explicitly labeled simulation providers.
pub fn is_adopted_model_plan(task: &Task) -> bool {
    adopted_call(task).is_some()
}

/// Production-provider badge; never label a scripted test provider as a real model.
pub fn is_real_adopted_model_plan(task: &Task) -> bool {
    adopted_call(task).is_some_and(|call| {
        call.backend == "openai_responses"
            && call
                .provider_response_id
                .as_deref()
                .is_some_and(|id| !id.is_empty())
    })
}
