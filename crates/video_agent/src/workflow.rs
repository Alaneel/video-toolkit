use crate::domain::*;
use crate::execution::Clock;
use crate::store::{event, invalid, now};
use crate::vision::VisionProvider;
use crate::{batch, media, store};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

const MODEL_WORKING_SET_FRAMES: usize = 16;

/// A single local job. Each mutating operation holds an OS file lock; no global state.
#[derive(Debug, Clone)]
pub struct Workspace {
    dir: PathBuf,
}

struct ExtractedFrame {
    _temporary: tempfile::TempDir,
    partial: PathBuf,
    actual_time_ms: u64,
    sha256: String,
}

impl Workspace {
    pub fn create(
        parent: &Path,
        inputs: &[PathBuf],
        intent: String,
        target_duration_ms: u64,
        limits: Limits,
    ) -> Result<Self> {
        Self::create_with_cancel(
            parent,
            inputs,
            intent,
            target_duration_ms,
            limits,
            &CancelToken::default(),
        )
    }

    pub fn create_with_cancel(
        parent: &Path,
        inputs: &[PathBuf],
        intent: String,
        target_duration_ms: u64,
        limits: Limits,
        cancel: &CancelToken,
    ) -> Result<Self> {
        store::validate_limits(&limits)?;
        if intent.trim().is_empty() || intent.len() > 8000 {
            return Err(invalid("Describe your intent in 1–8000 bytes"));
        }
        if inputs.is_empty()
            || inputs.len() > limits.max_sources
            || target_duration_ms < 100
            || target_duration_ms > limits.max_output_ms
        {
            return Err(invalid(
                "Input count or target duration exceeds task limits",
            ));
        }
        if limits.initial_frames < inputs.len() as u32 {
            return Err(invalid(
                "Initial frame budget must cover every input at least once",
            ));
        }
        fs::create_dir_all(parent)?;
        let parent = fs::canonicalize(parent)?;
        let temp = tempfile::Builder::new()
            .prefix("video-agent-")
            .tempdir_in(&parent)?;
        let workspace = Self {
            dir: fs::canonicalize(temp.into_path())?,
        };
        let _lock = store::lock(&workspace.dir)?;
        for name in ["frames", "previews", "shards", "exports"] {
            store::owned_dir(&workspace.dir, name)?;
        }
        let task_id = workspace.dir.file_name().unwrap().to_string_lossy().into();
        let clock = Clock::initial(&workspace.dir, task_id, &limits)?;
        let mut assets = Vec::new();
        let mut seen = HashSet::new();
        for input in inputs {
            cancel.check()?;
            let path = fs::canonicalize(input)?;
            store::regular(&path)?;
            if !seen.insert(path.clone()) {
                return Err(invalid("Duplicate source path"));
            }
            let metadata = path.metadata()?;
            if metadata.len() > 100 * 1024 * 1024 * 1024 {
                return Err(invalid("Input exceeds 100 GiB safety limit"));
            }
            let sha256 = media::hash_file_bounded(
                &path,
                cancel,
                clock.timeout(limits.process_timeout_secs)?,
            )?;
            let info = media::probe(&path, cancel, clock.timeout(limits.process_timeout_secs)?)?;
            if media::hash_file_bounded(&path, cancel, clock.timeout(limits.process_timeout_secs)?)?
                != sha256
            {
                return Err(invalid("Input changed while being inspected"));
            }
            let id = format!("source-{}", assets.len() + 1);
            assets.push(Asset {
                id,
                path,
                sha256,
                size: metadata.len(),
                modified_ns: modified_ns(&metadata),
                duration_ms: info.duration_ms,
                width: info.width,
                height: info.height,
                has_audio: info.has_audio,
            });
        }
        cancel.check()?;
        let mut task = Task {
            schema_version: 1,
            id: workspace.dir.file_name().unwrap().to_string_lossy().into(),
            created_at: now(),
            intent,
            target_duration_ms,
            assets,
            samples: vec![],
            model_calls: vec![],
            plan: None,
            approval: None,
            status: TaskStatus::Created,
            last_error: None,
            limits,
            usage: Usage::default(),
            cloud_consent: false,
            export: None,
            shards: vec![],
            history: vec![],
        };
        event(
            &mut task,
            "created",
            "Sources fingerprinted locally. No video or sampled image has been sent to a model.",
        );
        clock.finish(&mut task)?;
        store::save(&workspace.dir, &task)?;
        Ok(workspace)
    }

    pub fn open(dir: &Path) -> Result<Self> {
        Self::open_with_cancel(dir, &CancelToken::default())
    }

    pub fn open_with_cancel(dir: &Path, cancel: &CancelToken) -> Result<Self> {
        cancel.check()?;
        let workspace = Self {
            dir: fs::canonicalize(dir)?,
        };
        // Opening a handle is structural only. Display callers use load(), and
        // every action verifies source fingerprints against its remaining task
        // execution budget. This avoids unbudgeted duplicate full-file reads.
        store::read(&workspace.dir)?;
        Ok(workspace)
    }

    pub fn task_dir(&self) -> &Path {
        &self.dir
    }

    /// Atomic journal snapshot for progress/error display only. It deliberately
    /// does not attest that sources are still valid, and authorizes no action.
    /// All processing and confirmation entry points revalidate fingerprints.
    pub fn snapshot(dir: &Path) -> Result<Task> {
        let canonical = fs::canonicalize(dir)?;
        let mut task = store::read(&canonical)?;
        // Persisted verification is not a fresh attestation of the current
        // file. A progress-only snapshot must never enable delivery/playback.
        if let Some(export) = &mut task.export {
            export.verified = false;
        }
        Ok(task)
    }

    pub fn load(&self) -> Result<Task> {
        self.load_with_cancel(&CancelToken::default())
    }

    pub fn load_with_cancel(&self, cancel: &CancelToken) -> Result<Task> {
        cancel.check()?;
        let _lock = store::lock(&self.dir)?;
        self.read_checked_with(cancel)
    }

    fn read_checked_with(&self, cancel: &CancelToken) -> Result<Task> {
        let view_only = |mut saved: Task| {
            if let Some(export) = &mut saved.export {
                export.verified = false;
            }
            if saved.status != TaskStatus::Stale {
                saved.status = TaskStatus::Failed;
            }
            saved.last_error = Some("Execution budget exhausted. Saved metadata is view-only; source and output fingerprints were not reverified. Processing remains disabled.".into());
            saved
        };
        let mut task = store::read(&self.dir)?;
        let clock = match Clock::new(&self.dir, &mut task) {
            Ok(clock) => clock,
            Err(AgentError::TimedOut) => {
                // Viewing saved records must remain possible after exhaustion,
                // but it must not silently perform unbudgeted full-file reads
                // or grant a fresh output/source verification attestation.
                return Ok(view_only(store::read(&self.dir)?));
            }
            Err(error) => return Err(error),
        };
        let result = (|| {
            if let Err(error) = check_sources_during(&task, cancel, &clock) {
                if matches!(error, AgentError::Cancelled | AgentError::TimedOut) {
                    return Err(error);
                }
                if task.status != TaskStatus::Stale {
                    task.status = TaskStatus::Stale;
                    task.approval = None;
                    task.cloud_consent = false;
                    task.last_error = Some(error.to_string());
                    event(&mut task, "source_invalidated", "An input no longer matches its recorded fingerprint. Create a new task to re-inspect it.");
                    store::save(&self.dir, &task)?;
                }
            }
            if let Err(error) = self.check_export(
                &task,
                cancel,
                clock.timeout(task.limits.process_timeout_secs)?,
            ) {
                if matches!(error, AgentError::Cancelled | AgentError::TimedOut) {
                    return Err(error);
                }
                self.invalidate_export(&mut task, error.to_string())?;
            }
            Ok(())
        })();
        clock.finish(&mut task)?;
        store::save(&self.dir, &task)?;
        match result {
            Ok(()) => Ok(task),
            Err(AgentError::TimedOut) => Ok(view_only(task)),
            Err(error) => Err(error),
        }
    }

    fn usable_with(&self, cancel: &CancelToken) -> Result<Task> {
        cancel.check()?;
        let mut task = store::read(&self.dir)?;
        if task.status == TaskStatus::Stale {
            return Err(invalid(
                "Sources changed or disappeared. This task is stale; create a new task.",
            ));
        }
        let clock = Clock::new(&self.dir, &mut task)?;
        let checked = check_sources_during(&task, cancel, &clock);
        if let Err(error) = checked {
            clock.finish(&mut task)?;
            match error {
                AgentError::Cancelled => task.status = TaskStatus::Cancelled,
                AgentError::TimedOut => task.status = TaskStatus::Failed,
                _ => {
                    task.status = TaskStatus::Stale;
                    task.approval = None;
                    task.cloud_consent = false;
                }
            }
            task.last_error = Some(error.to_string());
            event(&mut task, "preflight_stopped", error.to_string());
            store::save(&self.dir, &task)?;
            return Err(error);
        }
        let checked_export = match clock.timeout(task.limits.process_timeout_secs) {
            Ok(timeout) => self.check_export(&task, cancel, timeout),
            Err(error) => Err(error),
        };
        if let Err(error) = checked_export {
            if matches!(error, AgentError::Cancelled | AgentError::TimedOut) {
                clock.finish(&mut task)?;
                task.status = if matches!(error, AgentError::Cancelled) {
                    TaskStatus::Cancelled
                } else {
                    TaskStatus::Failed
                };
                task.last_error = Some(error.to_string());
                event(&mut task, "preflight_stopped", error.to_string());
                store::save(&self.dir, &task)?;
                return Err(error);
            }
            self.invalidate_export(&mut task, error.to_string())?;
        }
        clock.finish(&mut task)?;
        store::save(&self.dir, &task)?;
        Ok(task)
    }

    fn invalidate_export(&self, task: &mut Task, reason: String) -> Result<()> {
        if let Some(export) = &mut task.export {
            export.verified = false;
        }
        if task.status != TaskStatus::Stale {
            task.status = TaskStatus::Failed;
        }
        let message = format!("Saved output is no longer a verified deliverable: {reason}");
        if task.last_error.as_deref() != Some(&message) {
            task.last_error = Some(message.clone());
            event(task, "output_invalidated", message);
        }
        store::save(&self.dir, task)
    }

    fn check_export(&self, task: &Task, cancel: &CancelToken, timeout: Duration) -> Result<()> {
        let Some(export) = &task.export else {
            return if task.status == TaskStatus::Completed {
                Err(invalid("Completed task has no output record"))
            } else {
                Ok(())
            };
        };
        if !export.verified && task.status != TaskStatus::Completed {
            return Ok(());
        }
        if export.plan_digest.len() != 64
            || !export.plan_digest.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(invalid("Output plan digest is malformed"));
        }
        let exports = store::owned_dir(&self.dir, "exports")?;
        let expected_path = exports.join(format!("{}.mp4", export.plan_digest));
        if export.path != expected_path {
            return Err(invalid(
                "Output path is not the exact task-owned plan artifact",
            ));
        }
        if task.status == TaskStatus::Completed {
            let plan = task
                .plan
                .as_ref()
                .ok_or_else(|| invalid("Completed output has no current edit plan"))?;
            validate_plan(task, plan, false)?;
            if plan.clips.is_empty() || plan_digest(task, plan)? != export.plan_digest {
                return Err(invalid(
                    "Completed output does not belong to the current edit plan",
                ));
            }
            let approval = task
                .approval
                .as_ref()
                .ok_or_else(|| invalid("Completed output has no matching human approval"))?;
            if approval.plan_digest != export.plan_digest || approval.revision != plan.revision {
                return Err(invalid(
                    "Completed output is not bound to the confirmed plan revision",
                ));
            }
            if !export.verified || export.completed_at.is_none() {
                return Err(invalid("Output publication was not completed and verified"));
            }
            if export.has_audio != (plan.audio_policy == AudioPolicy::PreserveWithSilence) {
                return Err(invalid(
                    "Output audio policy metadata does not match the confirmed plan",
                ));
            }
        }
        // A valid pending publication journal may precede the no-clobber link.
        // It is recoverable by export(), but never a completed deliverable.
        if !export.path.exists()
            && export.completed_at.is_none()
            && task.status != TaskStatus::Completed
        {
            return Ok(());
        }
        store::regular(&export.path)?;
        if export.sha256.len() != 64
            || media::hash_file_bounded(&export.path, cancel, timeout)? != export.sha256
        {
            return Err(invalid(
                "Output content no longer matches its verified SHA256",
            ));
        }
        Ok(())
    }

    fn finish(&self, task: &mut Task, clock: &Clock, result: Result<()>) -> Result<Task> {
        match result {
            Ok(()) => {
                clock.finish(task)?;
                task.last_error = None;
                store::save(&self.dir, task)?;
                Ok(task.clone())
            }
            Err(error) => {
                task.status = if matches!(error, AgentError::Cancelled) {
                    TaskStatus::Cancelled
                } else {
                    TaskStatus::Failed
                };
                task.last_error = Some(error.to_string());
                if !matches!(error, AgentError::Cancelled | AgentError::TimedOut) {
                    if let Err(source_error) =
                        check_sources_during(task, &CancelToken::default(), clock)
                    {
                        if !matches!(source_error, AgentError::Cancelled | AgentError::TimedOut) {
                            task.status = TaskStatus::Stale;
                            task.approval = None;
                            task.cloud_consent = false;
                        }
                    }
                }
                event(task, "operation_stopped", error.to_string());
                clock.finish(task)?;
                store::save(&self.dir, task)?;
                Err(error)
            }
        }
    }

    pub fn sample(&self, cancel: &CancelToken) -> Result<Task> {
        let _lock = store::lock(&self.dir)?;
        let mut task = self.usable_with(cancel)?;
        let clock = Clock::new(&self.dir, &mut task)?;
        let result = (|| {
            cancel.check()?;
            task.status = TaskStatus::Sampling;
            store::save(&self.dir, &task)?;
            let asset_count = task.assets.len();
            let mut requests = Vec::new();
            for index in 0..asset_count {
                let asset = task.assets[index].clone();
                let count = task.limits.initial_frames as usize / asset_count
                    + usize::from(index < task.limits.initial_frames as usize % asset_count);
                // Midpoints, not claims about the unsampled intervals between them.
                for n in 0..count {
                    let time = (asset.duration_ms as u128 * (2 * n + 1) as u128
                        / (2 * count) as u128) as u64;
                    requests.push((asset.clone(), time.min(asset.duration_ms.saturating_sub(1))));
                }
            }
            self.add_samples(&mut task, requests, cancel, &clock)?;
            task.status = if task.cloud_consent {
                TaskStatus::Review
            } else {
                TaskStatus::AwaitingConsent
            };
            event(&mut task, "sampling_complete", "Uniform source-timecode JPEG samples are ready. Only these frames have been observed.");
            Ok(())
        })();
        self.finish(&mut task, &clock, result)
    }

    fn add_samples(
        &self,
        task: &mut Task,
        requests: Vec<(Asset, u64)>,
        cancel: &CancelToken,
        clock: &Clock,
    ) -> Result<()> {
        cancel.check()?;
        let mut seen = HashSet::new();
        let mut pending = Vec::new();
        let mut new_seeks = 0usize;
        for (asset, time_ms) in requests {
            cancel.check()?;
            if time_ms >= asset.duration_ms {
                return Err(invalid("Observation timestamp is outside source"));
            }
            if !seen.insert((asset.id.clone(), time_ms)) {
                continue;
            }
            if let Some(existing) = sample_for_seek(task, &asset.id, time_ms) {
                if existing.image_path.exists() {
                    self.check_sample(
                        existing,
                        cancel,
                        clock.timeout(task.limits.process_timeout_secs)?,
                    )?;
                    continue;
                }
            } else {
                new_seeks += 1;
            }
            pending.push((asset, time_ms));
        }
        // Unknown seeks are admitted at worst-case one new decoded frame each.
        // A crash may require re-decoding unpublished seeks. This limit bounds
        // authoritative unique observations, not physical decode attempts;
        // repeated unfinished work still consumes durable execution budget.
        if new_seeks > (task.limits.max_frames - task.usage.frames_created) as usize {
            return Err(invalid(
                "Observation batch would exceed the remaining frame budget",
            ));
        }
        let frames = store::owned_dir(&self.dir, "frames")?;
        for requests in pending.chunks(task.limits.max_concurrency) {
            cancel.check()?;
            check_sources_during(task, cancel, clock)?;
            let timeout = clock.timeout(task.limits.process_timeout_secs)?;
            let batch_started = Instant::now();
            let deadline = batch_started + timeout;
            let process_traces: Vec<_> = requests.iter().map(|_| Mutex::new(None)).collect();
            let outcome = batch::run(
                requests.len(),
                task.limits.max_concurrency,
                cancel,
                deadline,
                |index, worker_cancel, remaining| {
                    let worker_started = Instant::now();
                    let temporary = tempfile::Builder::new()
                        .prefix("frame-")
                        .tempdir_in(&frames)?;
                    let partial = temporary.path().join("frame.jpg");
                    let (asset, time_ms) = &requests[index];
                    let (actual_time_ms, _) = media::frame_traced(
                        &asset.path,
                        *time_ms,
                        &partial,
                        worker_cancel,
                        remaining.saturating_sub(worker_started.elapsed()),
                        Some(&process_traces[index]),
                    )?;
                    let sha256 = media::hash_file_bounded(
                        &partial,
                        worker_cancel,
                        remaining.saturating_sub(worker_started.elapsed()),
                    )?;
                    Ok(ExtractedFrame {
                        _temporary: temporary,
                        partial,
                        actual_time_ms,
                        sha256,
                    })
                },
            )?;
            // Every worker is joined before Clock is touched again, source
            // fingerprints are checked, or any sample becomes authoritative.
            task.usage.peak_media_workers = task.usage.peak_media_workers.max(outcome.peak_workers);
            task.usage.media_worker_ms =
                task.usage.media_worker_ms.saturating_add(outcome.worker_ms);
            let traces: Vec<_> = process_traces.iter().enumerate().filter_map(|(index, trace)| trace.lock().ok().and_then(|trace| trace.as_ref().map(|span| serde_json::json!({
                "job_index": index, "source_id": requests[index].0.id,
                "requested_time_ms": requests[index].1, "pid": span.pid,
                "spawned_us": span.started.duration_since(batch_started).as_micros().min(u128::from(u64::MAX)) as u64,
                "reaped_us": span.completed.duration_since(batch_started).as_micros().min(u128::from(u64::MAX)) as u64,
                "returned_ok": outcome.results[index].is_ok(),
            })))).collect();
            event(
                task,
                "frame_batch_workers",
                serde_json::to_string(&serde_json::json!({
                    "ceiling": task.limits.max_concurrency, "peak_workers": outcome.peak_workers,
                    "worker_ms": outcome.worker_ms, "children": traces,
                    "interval_semantics": "actual FFmpeg child spawn-to-reap wall intervals, not CPU utilization",
                }))?,
            );
            store::save(&self.dir, task)?;
            cancel.check()?;
            if Instant::now() >= deadline {
                return Err(AgentError::TimedOut);
            }
            let mut first_error = None;
            let mut completed = Vec::new();
            for result in outcome.results {
                match result {
                    Ok(frame) => completed.push(frame),
                    Err(error) => {
                        if first_error.is_none()
                            || matches!(first_error, Some(AgentError::Cancelled))
                        {
                            first_error = Some(error);
                        }
                    }
                }
            }
            if let Some(error) = first_error {
                return Err(error);
            }
            check_sources_during(task, cancel, clock)?;
            for ((asset, time_ms), frame) in requests.iter().zip(completed) {
                cancel.check()?;
                self.publish_sample(task, asset, *time_ms, frame, cancel, clock)?;
            }
        }
        Ok(())
    }

    fn publish_sample(
        &self,
        task: &mut Task,
        asset: &Asset,
        time_ms: u64,
        frame: ExtractedFrame,
        cancel: &CancelToken,
        clock: &Clock,
    ) -> Result<()> {
        let actual_time_ms = frame.actual_time_ms;
        if actual_time_ms >= asset.duration_ms {
            return Err(invalid(
                "Decoded frame timestamp falls outside source duration",
            ));
        }
        let sha256 = frame.sha256;
        let frames = store::owned_dir(&self.dir, "frames")?;
        let id = format!("{}-{actual_time_ms}", asset.id);
        let path = frames.join(format!("{id}.jpg"));
        let existing = sample_for_seek(task, &asset.id, time_ms)
            .cloned()
            .or_else(|| task.samples.iter().find(|s| s.id == id).cloned());
        if let Some(existing) = &existing {
            if existing.time_ms != actual_time_ms {
                return Err(invalid(
                    "Regenerated frame timestamp differs from recorded observation",
                ));
            }
            if sha256 != existing.sha256 {
                return Err(invalid(
                    "Regenerated pending frame differs from recorded content",
                ));
            }
            if existing.image_path != path {
                return Err(invalid("Stored sample path is not its canonical task path"));
            }
        } else {
            if path.exists() {
                return Err(invalid(
                    "Untracked frame already exists; refusing overwrite",
                ));
            }
            // Journal reservation and sample fingerprint BEFORE publication. A
            // restart can regenerate a missing image without double-charging it.
            if task.usage.frames_created >= task.limits.max_frames {
                return Err(invalid("Frame budget exhausted before sample publication"));
            }
            task.usage.frames_created += 1;
            task.samples.push(Sample {
                id: id.clone(),
                source_id: asset.id.clone(),
                time_ms: actual_time_ms,
                image_path: path.clone(),
                sha256,
            });
        }
        let observed = FrameObservation {
            source_id: asset.id.clone(),
            requested_time_ms: time_ms,
            decoded_time_ms: actual_time_ms,
            sample_id: id,
        };
        event(task, "sample_observed", serde_json::to_string(&observed)?);
        store::save(&self.dir, task)?;
        if !path.exists() {
            fs::hard_link(&frame.partial, &path)?;
        } else {
            self.check_sample(
                existing
                    .as_ref()
                    .ok_or_else(|| invalid("Untracked sample appeared during publication"))?,
                cancel,
                clock.timeout(task.limits.process_timeout_secs)?,
            )?;
        }
        store::save(&self.dir, task)?;
        Ok(())
    }

    fn check_sample(&self, sample: &Sample, cancel: &CancelToken, timeout: Duration) -> Result<()> {
        let frames = store::owned_dir(&self.dir, "frames")?;
        if sample.image_path.parent() != Some(frames.as_path()) {
            return Err(invalid("Sample path escaped task frames directory"));
        }
        store::regular(&sample.image_path)?;
        if sample.image_path.metadata()?.len() > 1_048_576 {
            return Err(invalid("Sample image exceeds size limit"));
        }
        if media::hash_file_bounded(&sample.image_path, cancel, timeout)? != sample.sha256 {
            return Err(invalid("Sample changed; refusing to send it to the model"));
        }
        Ok(())
    }

    pub fn set_consent(&self, consent: bool) -> Result<Task> {
        self.set_consent_with_cancel(consent, &CancelToken::default())
    }

    pub fn set_consent_with_cancel(&self, consent: bool, cancel: &CancelToken) -> Result<Task> {
        let _lock = store::lock(&self.dir)?;
        let mut task = self.usable_with(cancel)?;
        task.cloud_consent = consent;
        event(
            &mut task,
            "cloud_consent",
            if consent {
                "User allows JPEG samples (including bounded follow-up observations), source IDs, timestamps, intent, and media dimensions to be sent to configured OpenAI vision model. Original video and audio remain local."
            } else {
                "User revoked permission to send any samples to the model."
            },
        );
        store::save(&self.dir, &task)?;
        Ok(task)
    }

    pub fn analyze(&self, provider: &dyn VisionProvider, cancel: &CancelToken) -> Result<Task> {
        let _lock = store::lock(&self.dir)?;
        let mut task = self.usable_with(cancel)?;
        let clock = Clock::new(&self.dir, &mut task)?;
        let result = (|| {
            cancel.check()?;
            if !task.cloud_consent {
                return Err(invalid(
                    "Cloud consent is required before sending any sampled images",
                ));
            }
            if task.samples.is_empty() {
                return Err(invalid(
                    "Sample the source video before requesting visual selection",
                ));
            }
            for record in &mut task.model_calls {
                if record.status == "reserved" {
                    record.status = "interrupted_unknown_usage".into();
                    record.error = Some("Previous process stopped before a response was durably recorded. Reserved budget retained.".into());
                }
            }
            task.status = TaskStatus::Analyzing;
            task.approval = None;
            store::save(&self.dir, &task)?;
            // analyze() is an explicit user action. A rejected proposal stops
            // its original invocation; only a later invocation gets this fixed
            // feedback plus whitelisted timing facts, never arbitrary persisted
            // errors, paths, or model prose.
            let mut feedback = rejected_proposal_feedback(&task);
            loop {
                cancel.check()?;
                clock.timeout(task.limits.model_timeout_secs)?;
                check_sources_during(&task, cancel, &clock)?;
                for sample in &task.samples {
                    self.check_sample(
                        sample,
                        cancel,
                        clock.timeout(task.limits.process_timeout_secs)?,
                    )?;
                }
                if let Some(index) = task
                    .model_calls
                    .iter()
                    .rposition(|c| c.status == "received" && c.decision.is_some())
                {
                    let decision = task.model_calls[index].decision.clone().unwrap();
                    event(&mut task, "model_result_reused", "Applying a durably recorded model result after interruption; no repeated API request.");
                    if self.apply_decision(&mut task, index, decision, cancel, &clock)? {
                        break;
                    }
                    feedback = Some("Previously requested observations have completed after resuming. Use the additional samples to propose a plan or request a different bounded observation.".into());
                    continue;
                }
                if task.usage.model_calls >= task.limits.max_model_calls {
                    return Err(invalid("Model call budget exhausted. Existing observations and results are retained."));
                }
                if !provider.ready() {
                    return Err(invalid("Vision backend is not configured; persisted model results were retained, and no new request was made"));
                }
                let call_id = format!("call-{}", task.usage.model_calls + 1);
                let mut request = VisionRequest {
                    call_id: call_id.clone(),
                    intent: task.intent.clone(),
                    target_duration_ms: task.target_duration_ms,
                    assets: task.assets.clone(),
                    samples: model_working_set(&task.assets, &task.samples),
                    remaining_frames: task.limits.max_frames - task.usage.frames_created,
                    limits: task.limits.clone(),
                    feedback: feedback.take(),
                };
                // The HTTP backend must use the smaller remaining operation deadline.
                let (reserved_tokens, reserved_cost) = provider.estimate(&request)?;
                if !reserved_cost.is_finite()
                    || reserved_cost < 0.0
                    || reserved_tokens < u64::from(task.limits.max_output_tokens)
                    || task
                        .usage
                        .input_tokens
                        .saturating_add(task.usage.output_tokens)
                        .saturating_add(reserved_tokens)
                        > task.limits.max_total_tokens
                    || task.usage.estimated_cost_usd + reserved_cost
                        > task.limits.max_estimated_cost_usd + 1e-9
                {
                    return Err(invalid(
                        "Next model request would exceed the token or estimated-cost budget",
                    ));
                }
                let (backend, model, prompt_version) = provider.identity();
                if clock.timeout(task.limits.model_timeout_secs)?.as_secs() == 0 {
                    return Err(AgentError::TimedOut);
                }
                let index = task.model_calls.len();
                task.model_calls.push(ModelCall {
                    call_id,
                    backend,
                    requested_model: model.clone(),
                    model,
                    prompt_version,
                    sample_ids: request.samples.iter().map(|s| s.id.clone()).collect(),
                    provider_response_id: None,
                    request_id: None,
                    input_tokens: reserved_tokens - u64::from(task.limits.max_output_tokens),
                    output_tokens: u64::from(task.limits.max_output_tokens),
                    estimated_cost_usd: reserved_cost,
                    elapsed_ms: 0,
                    status: "reserved".into(),
                    decision: None,
                    error: None,
                });
                task.usage.model_calls += 1;
                task.usage.input_tokens +=
                    reserved_tokens - u64::from(task.limits.max_output_tokens);
                task.usage.output_tokens += u64::from(task.limits.max_output_tokens);
                task.usage.estimated_cost_usd += reserved_cost;
                event(&mut task, "model_call_reserved", format!("Call {} reserved before network request. Budget remains charged if usage is unknown.", index + 1));
                store::save(&self.dir, &task)?;
                // Refresh the durable execution reservation immediately before
                // the provider work, after local usage estimation/journaling.
                // Never round a sub-second remaining deadline up to one second.
                let send_timeout = if cancel.is_cancelled() {
                    Err(AgentError::Cancelled)
                } else {
                    clock.timeout(task.limits.model_timeout_secs)
                };
                let send_timeout = match send_timeout {
                    Ok(timeout) if timeout.as_secs() > 0 => timeout,
                    result => {
                        // This process knows decide() has not been entered.
                        // Do not mislabel a local admission timeout as unknown
                        // provider billing. The local attempt still counts.
                        let reserved = &mut task.model_calls[index];
                        task.usage.input_tokens -= reserved.input_tokens;
                        task.usage.output_tokens -= reserved.output_tokens;
                        task.usage.estimated_cost_usd =
                            (task.usage.estimated_cost_usd - reserved.estimated_cost_usd).max(0.0);
                        reserved.input_tokens = 0;
                        reserved.output_tokens = 0;
                        reserved.estimated_cost_usd = 0.0;
                        reserved.status = "not_sent".into();
                        reserved.error = Some("Local execution admission stopped before the provider was invoked; token/cost reservation refunded, attempt count retained.".into());
                        store::save(&self.dir, &task)?;
                        return Err(result.err().unwrap_or(AgentError::TimedOut));
                    }
                };
                request.limits.model_timeout_secs = send_timeout.as_secs();
                let called_at = Instant::now();
                let outcome = match provider.decide(&request, cancel) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        account_provider_failure(
                            &mut task,
                            index,
                            &request,
                            &error,
                            called_at.elapsed().as_millis() as u64,
                        );
                        store::save(&self.dir, &task)?;
                        return Err(error);
                    }
                };
                let record = &outcome.record;
                if record.call_id != request.call_id
                    || !record.estimated_cost_usd.is_finite()
                    || record.estimated_cost_usd < 0.0
                {
                    task.model_calls[index].status = "invalid_usage_record".into();
                    task.model_calls[index].error = Some(
                        "Provider record did not match its reserved request; reservation retained"
                            .into(),
                    );
                    store::save(&self.dir, &task)?;
                    return Err(invalid(
                        "Provider usage record did not match the reserved request",
                    ));
                }
                let exceeded_reservation = record.input_tokens.saturating_add(record.output_tokens)
                    > reserved_tokens
                    || record.estimated_cost_usd > reserved_cost + 1e-9;
                task.usage.input_tokens = (task.usage.input_tokens
                    - (reserved_tokens - u64::from(task.limits.max_output_tokens)))
                .saturating_add(record.input_tokens);
                task.usage.output_tokens = (task.usage.output_tokens
                    - u64::from(task.limits.max_output_tokens))
                .saturating_add(record.output_tokens);
                task.usage.estimated_cost_usd = (task.usage.estimated_cost_usd - reserved_cost
                    + record.estimated_cost_usd)
                    .max(0.0);
                let mut record = outcome.record;
                record.sample_ids = request.samples.iter().map(|s| s.id.clone()).collect();
                record.decision = Some(outcome.decision.clone());
                record.elapsed_ms = called_at.elapsed().as_millis() as u64;
                record.status = if exceeded_reservation {
                    "usage_exceeded_reservation"
                } else {
                    "received"
                }
                .into();
                if exceeded_reservation {
                    record.error = Some("Reported usage exceeded conservative local reservation; selection rejected and actual reported usage retained".into());
                }
                task.model_calls[index] = record;
                store::save(&self.dir, &task)?;
                if exceeded_reservation {
                    return Err(invalid("Provider usage exceeded the local reservation; no selection accepted. Reported usage was retained."));
                }
                cancel.check()?;
                check_sources_during(&task, cancel, &clock)?;
                clock.timeout(task.limits.model_timeout_secs)?;
                if self.apply_decision(&mut task, index, outcome.decision, cancel, &clock)? {
                    break;
                }
                feedback = Some("Requested observations are included. Propose a plan or request a different bounded observation. Do not infer unsampled events or audio.".into());
            }
            Ok(())
        })();
        self.finish(&mut task, &clock, result)
    }

    /// Returns true for a completed proposal, false when another bounded model
    /// decision is needed. Received decisions are replayable across restarts.
    fn apply_decision(
        &self,
        task: &mut Task,
        index: usize,
        decision: VisionDecision,
        cancel: &CancelToken,
        clock: &Clock,
    ) -> Result<bool> {
        let result = (|| match decision {
            VisionDecision::ObserveInterval {
                source_id,
                start_ms,
                end_ms,
                count,
                reason,
            } => {
                let asset = task
                    .assets
                    .iter()
                    .find(|a| a.id == source_id)
                    .cloned()
                    .ok_or_else(|| invalid("Model requested an unknown source"))?;
                if start_ms >= end_ms
                    || end_ms > asset.duration_ms
                    || count == 0
                    || count > task.limits.max_frames
                    || reason.trim().is_empty()
                    || reason.len() > 4000
                {
                    return Err(invalid(
                        "Model observation request violates interval/frame limits",
                    ));
                }
                let times: Vec<_> = (0..count)
                    .map(|n| {
                        start_ms
                            + ((end_ms - start_ms) as u128 * (2 * n + 1) as u128
                                / (2 * count) as u128) as u64
                    })
                    .collect();
                let new_times: HashSet<_> = times
                    .iter()
                    .filter(|time| sample_for_seek(task, &asset.id, **time).is_none())
                    .collect();
                if new_times.len() > (task.limits.max_frames - task.usage.frames_created) as usize {
                    return Err(invalid(
                        "Model observation request would exceed the remaining frame budget",
                    ));
                }
                let submitted: HashSet<_> =
                    task.model_calls[index].sample_ids.iter().cloned().collect();
                self.add_samples(
                    task,
                    times
                        .iter()
                        .map(|time| (asset.clone(), (*time).min(asset.duration_ms - 1)))
                        .collect(),
                    cancel,
                    clock,
                )?;
                if times.iter().all(|time| {
                    sample_for_seek(task, &asset.id, *time)
                        .is_some_and(|sample| submitted.contains(&sample.id))
                }) {
                    return Err(invalid("Requested seeks decoded only previously observed frames; refusing a no-progress loop"));
                }
                event(
                    task,
                    "model_observation",
                    format!("{source_id} {start_ms}–{end_ms} ms: {reason}"),
                );
                task.model_calls[index].status = "observations_applied".into();
                store::save(&self.dir, task)?;
                Ok(false)
            }
            VisionDecision::ProposePlan { clips, summary } => {
                let revision = task
                    .plan
                    .as_ref()
                    .map_or(1, |p| p.revision.saturating_add(1));
                let plan = EditPlan {
                    revision,
                    clips: clips
                        .into_iter()
                        .enumerate()
                        .map(|(i, c)| Clip {
                            id: format!("clip-{}", i + 1),
                            source_id: c.source_id,
                            start_ms: c.start_ms,
                            end_ms: c.end_ms,
                            reason: c.reason,
                            uncertainty: c.uncertainty,
                            evidence_ids: c.evidence_ids,
                            human_modified: false,
                        })
                        .collect(),
                    summary,
                    audio_policy: AudioPolicy::PreserveWithSilence,
                };
                validate_plan(task, &plan, true)?;
                if plan
                    .clips
                    .iter()
                    .flat_map(|c| &c.evidence_ids)
                    .any(|id| !task.model_calls[index].sample_ids.contains(id))
                {
                    return Err(invalid("Model proposal cited a frame that was not submitted in this specific request"));
                }
                task.plan = Some(plan);
                task.status = TaskStatus::Review;
                task.model_calls[index].status = "validated".into();
                event(
                    task,
                    "model_plan",
                    "Visual model draft saved for human review. Export remains unapproved.",
                );
                store::save(&self.dir, task)?;
                Ok(true)
            }
        })();
        if let Err(error) = &result {
            if !matches!(error, AgentError::Cancelled | AgentError::TimedOut) {
                task.model_calls[index].status = "rejected".into();
                task.model_calls[index].error = Some(error.to_string());
                store::save(&self.dir, task)?;
            }
        }
        result
    }

    pub fn manual_draft(&self) -> Result<Task> {
        self.manual_draft_with_cancel(&CancelToken::default())
    }

    pub fn manual_draft_with_cancel(&self, cancel: &CancelToken) -> Result<Task> {
        let _lock = store::lock(&self.dir)?;
        let mut task = self.usable_with(cancel)?;
        let mut clips = Vec::new();
        let mut remaining = task.target_duration_ms;
        for asset in &task.assets {
            let duration = remaining
                .min(asset.duration_ms)
                .min(task.limits.max_clip_ms);
            if duration < 100 {
                continue;
            }
            clips.push(Clip {
                id: format!("clip-{}", clips.len() + 1),
                source_id: asset.id.clone(),
                start_ms: 0,
                end_ms: duration,
                reason: "MANUAL draft: first bounded interval, not semantic AI selection".into(),
                uncertainty: "No model has judged this interval. Review the source yourself."
                    .into(),
                evidence_ids: vec![],
                human_modified: true,
            });
            remaining -= duration;
            if remaining < 100 {
                break;
            }
        }
        let revision = task
            .plan
            .as_ref()
            .map_or(1, |p| p.revision.saturating_add(1));
        let plan = EditPlan {
            revision,
            clips,
            summary: "MANUAL workflow draft — no AI selection or semantic-quality claim".into(),
            audio_policy: AudioPolicy::PreserveWithSilence,
        };
        validate_plan(&task, &plan, false)?;
        task.plan = Some(plan);
        task.approval = None;
        task.status = TaskStatus::Review;
        event(&mut task, "manual_plan", "User explicitly created a manual, non-AI draft to review and exercise the local export workflow.");
        store::save(&self.dir, &task)?;
        Ok(task)
    }

    pub fn save_plan(&self, plan: EditPlan) -> Result<Task> {
        self.save_plan_with_cancel(plan, &CancelToken::default())
    }

    pub fn save_plan_with_cancel(&self, mut plan: EditPlan, cancel: &CancelToken) -> Result<Task> {
        let _lock = store::lock(&self.dir)?;
        let mut task = self.usable_with(cancel)?;
        let previous = task
            .plan
            .as_ref()
            .ok_or_else(|| invalid("No draft to edit"))?;
        if plan.revision != previous.revision {
            return Err(invalid(
                "This draft is stale. Reload before saving changes.",
            ));
        }
        validate_plan(&task, &plan, false)?;
        for (index, clip) in plan.clips.iter_mut().enumerate() {
            let Some(old_index) = previous.clips.iter().position(|c| c.id == clip.id) else {
                if !clip.human_modified
                    || !clip.evidence_ids.is_empty()
                    || !clip.reason.starts_with("MANUAL")
                {
                    return Err(invalid("New human clips must be explicitly labeled MANUAL, human-modified, and have no fabricated model evidence"));
                }
                continue;
            };
            let old = &previous.clips[old_index];
            if clip.source_id != old.source_id
                || clip.evidence_ids != old.evidence_ids
                || clip.reason != old.reason
                || clip.uncertainty != old.uncertainty
            {
                return Err(invalid(
                    "Human edits must retain their original model/manual provenance",
                ));
            }
            clip.human_modified = old.human_modified
                || clip.start_ms != old.start_ms
                || clip.end_ms != old.end_ms
                || index != old_index;
        }
        if plan == *previous {
            return Ok(task);
        }
        plan.revision = previous
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("Plan revision overflow"))?;
        task.plan = Some(plan);
        task.approval = None;
        task.status = TaskStatus::Review;
        event(&mut task, "human_edit", "Human changed the draft; previous approval invalidated. Original selection evidence is retained, not extended to unobserved content.");
        store::save(&self.dir, &task)?;
        Ok(task)
    }

    pub fn approve(&self, expected_digest: &str) -> Result<Task> {
        self.approve_with_cancel(expected_digest, &CancelToken::default())
    }

    pub fn approve_with_cancel(&self, expected_digest: &str, cancel: &CancelToken) -> Result<Task> {
        let _lock = store::lock(&self.dir)?;
        let mut task = self.usable_with(cancel)?;
        let plan = task
            .plan
            .as_ref()
            .ok_or_else(|| invalid("No edit plan to confirm"))?;
        validate_plan(&task, plan, false)?;
        if plan.clips.is_empty() {
            return Err(invalid(
                "No clips selected. An empty/refused model draft cannot be exported.",
            ));
        }
        let digest = plan_digest(&task, plan)?;
        if digest != expected_digest {
            return Err(invalid("The reviewed plan changed before confirmation. Reload and review the exact current revision."));
        }
        task.approval = Some(Approval {
            plan_digest: digest,
            revision: plan.revision,
            confirmed_at: now(),
        });
        event(&mut task, "human_approval", "User confirmed this exact plan revision, ordered time ranges, source hashes, and audio policy.");
        store::save(&self.dir, &task)?;
        Ok(task)
    }

    pub fn export(&self, cancel: &CancelToken) -> Result<Task> {
        let _lock = store::lock(&self.dir)?;
        let mut task = self.usable_with(cancel)?;
        let clock = Clock::new(&self.dir, &mut task)?;
        let result = self.export_inner(&mut task, cancel, &clock);
        self.finish(&mut task, &clock, result)
    }

    fn export_inner(&self, task: &mut Task, cancel: &CancelToken, clock: &Clock) -> Result<()> {
        cancel.check()?;
        let plan = task.plan.clone().ok_or_else(|| invalid("No edit plan"))?;
        validate_plan(task, &plan, false)?;
        if plan.clips.is_empty() {
            return Err(invalid("Cannot export empty plan"));
        }
        let digest = plan_digest(task, &plan)?;
        let approval = task
            .approval
            .as_ref()
            .ok_or_else(|| invalid("Human confirmation is required before export"))?;
        if approval.revision != plan.revision || approval.plan_digest != digest {
            return Err(invalid(
                "Plan changed after approval; confirm the current revision again",
            ));
        }
        let exports = store::owned_dir(&self.dir, "exports")?;
        let output = exports.join(format!("{digest}.mp4"));
        let expected_ms: u64 = plan.clips.iter().map(|c| c.end_ms - c.start_ms).sum();
        let expect_audio = plan.audio_policy == AudioPolicy::PreserveWithSilence;
        if output.exists() {
            let record = task.export.as_ref().ok_or_else(|| {
                invalid("Output already exists without a tracked export; refusing overwrite")
            })?;
            if record.plan_digest != digest || record.path != output || !record.verified {
                return Err(invalid(
                    "Output already exists for a different or unverified export",
                ));
            }
            store::regular(&output)?;
            if media::hash_file_bounded(
                &output,
                cancel,
                clock.timeout(task.limits.process_timeout_secs)?,
            )? != record.sha256
            {
                return Err(invalid(
                    "Existing output was modified; refusing to replace it",
                ));
            }
            media::verify_output(
                &output,
                expected_ms,
                expect_audio,
                cancel,
                clock.timeout(task.limits.process_timeout_secs)?,
            )?;
            check_sources_during(task, cancel, clock)?;
            task.export.as_mut().unwrap().completed_at = Some(now());
            task.status = TaskStatus::Completed;
            event(
                task,
                "export_reused",
                "Verified existing output reused; no duplicate file created.",
            );
            return Ok(());
        }
        task.status = TaskStatus::Exporting;
        store::save(&self.dir, task)?;
        let shards = store::owned_dir(&self.dir, "shards")?;
        let mut paths = Vec::new();
        for clip in &plan.clips {
            cancel.check()?;
            check_sources_during(task, cancel, clock)?;
            let asset = task
                .assets
                .iter()
                .find(|a| a.id == clip.source_id)
                .unwrap()
                .clone();
            let key = media::digest(&serde_json::to_vec(&(
                "h264-640x360-fps30-aac48k-stereo-v1",
                &asset.sha256,
                clip.start_ms,
                clip.end_ms,
                plan.audio_policy,
            ))?);
            let path = shards.join(format!("{key}.mp4"));
            let expected = clip.end_ms - clip.start_ms;
            let existing = task.shards.iter().find(|s| s.clip_digest == key).cloned();
            if let Some(existing) = existing.filter(|s| s.path.exists()) {
                if existing.path != path {
                    return Err(invalid("Stored shard path escaped task"));
                }
                store::regular(&path)?;
                if media::hash_file_bounded(
                    &path,
                    cancel,
                    clock.timeout(task.limits.process_timeout_secs)?,
                )? != existing.sha256
                {
                    return Err(invalid("Cached shard changed; refusing to reuse it"));
                }
                media::verify_output(
                    &path,
                    expected,
                    expect_audio,
                    cancel,
                    clock.timeout(task.limits.process_timeout_secs)?,
                )?;
                event(
                    task,
                    "shard_reused",
                    format!(
                        "{} reused after full hash/probe/decode verification",
                        clip.id
                    ),
                );
            } else {
                if path.exists() {
                    return Err(invalid("Untracked shard exists; refusing overwrite"));
                }
                let temporary = tempfile::Builder::new()
                    .prefix("encoding-")
                    .tempdir_in(&shards)?;
                let partial = temporary.path().join("partial.mp4");
                media::shard(
                    &asset,
                    clip,
                    plan.audio_policy,
                    &partial,
                    cancel,
                    clock.timeout(task.limits.process_timeout_secs)?,
                )?;
                let info = media::verify_output(
                    &partial,
                    expected,
                    expect_audio,
                    cancel,
                    clock.timeout(task.limits.process_timeout_secs)?,
                )?;
                let sha256 = media::hash_file_bounded(
                    &partial,
                    cancel,
                    clock.timeout(task.limits.process_timeout_secs)?,
                )?;
                check_sources_during(task, cancel, clock)?;
                task.shards.retain(|s| s.clip_digest != key);
                task.shards.push(Shard {
                    clip_digest: key.clone(),
                    path: path.clone(),
                    sha256,
                    duration_ms: info.duration_ms,
                });
                event(task, "shard_verified", format!("{} encoded and verified; restart can reuse this completed clip, not an arbitrary encoding position", clip.id));
                store::save(&self.dir, task)?;
                fs::hard_link(&partial, &path)?;
            }
            paths.push(path);
        }
        let temporary = tempfile::Builder::new()
            .prefix("export-")
            .tempdir_in(&exports)?;
        let partial = temporary.path().join("partial.mp4");
        let durations: Vec<_> = plan.clips.iter().map(|c| c.end_ms - c.start_ms).collect();
        media::concat(
            &paths,
            &durations,
            &partial,
            cancel,
            clock.timeout(task.limits.process_timeout_secs)?,
        )?;
        let info = media::verify_output(
            &partial,
            expected_ms,
            expect_audio,
            cancel,
            clock.timeout(task.limits.process_timeout_secs)?,
        )?;
        let sha256 = media::hash_file_bounded(
            &partial,
            cancel,
            clock.timeout(task.limits.process_timeout_secs)?,
        )?;
        check_sources_during(task, cancel, clock)?;
        cancel.check()?;
        FileSync::sync(&partial)?;
        // Journal publication BEFORE atomic, no-clobber link. If we crash after the link,
        // the next export reconciles this exact hash instead of overwriting or duplicating.
        task.export = Some(ExportRecord {
            plan_digest: digest,
            path: output.clone(),
            sha256,
            duration_ms: info.duration_ms,
            has_audio: info.has_audio,
            verified: true,
            completed_at: None,
        });
        store::save(&self.dir, task)?;
        fs::hard_link(&partial, &output)?;
        #[cfg(unix)]
        std::fs::File::open(&exports)?.sync_all()?;
        task.export.as_mut().unwrap().completed_at = Some(now());
        task.status = TaskStatus::Completed;
        event(task, "export_complete", "Verified H.264 MP4 published atomically without overwriting. Audio policy and duration checked; all streams decoded successfully.");
        Ok(())
    }

    pub fn preview_frame(
        &self,
        source_id: &str,
        time_ms: u64,
        cancel: &CancelToken,
    ) -> Result<PathBuf> {
        let _lock = store::lock(&self.dir)?;
        let mut task = self.usable_with(cancel)?;
        let clock = Clock::new(&self.dir, &mut task)?;
        let asset = task
            .assets
            .iter()
            .find(|a| a.id == source_id)
            .cloned()
            .ok_or_else(|| invalid("Unknown source"))?;
        if time_ms >= asset.duration_ms {
            return Err(invalid("Preview timestamp is outside source"));
        }
        let previews = store::owned_dir(&self.dir, "previews")?;
        let path = previews.join(format!("{}-{time_ms}.jpg", asset.id));
        let result = (|| {
            if path.exists() {
                store::regular(&path)?;
                return Ok(());
            }
            if task.usage.frames_created >= task.limits.max_frames {
                return Err(invalid("Frame budget exhausted; use video playback instead of additional frame extraction"));
            }
            // Local human preview extraction also counts toward the frame cap;
            // it is not automatically added to the model's observed sample set.
            task.usage.frames_created += 1;
            store::save(&self.dir, &task)?;
            let temporary = tempfile::Builder::new()
                .prefix("preview-")
                .tempdir_in(&previews)?;
            let partial = temporary.path().join("preview.jpg");
            media::frame(
                &asset.path,
                time_ms,
                &partial,
                cancel,
                clock.timeout(task.limits.process_timeout_secs)?,
            )?;
            check_sources_during(&task, cancel, &clock)?;
            fs::hard_link(partial, &path)?;
            Ok(())
        })();
        self.finish(&mut task, &clock, result)?;
        Ok(path)
    }

    pub fn spawn_preview(&self, clip: &Clip) -> Result<std::process::Child> {
        self.spawn_preview_with_cancel(clip, &CancelToken::default())
    }

    pub fn spawn_preview_with_cancel(
        &self,
        clip: &Clip,
        cancel: &CancelToken,
    ) -> Result<std::process::Child> {
        let _lock = store::lock(&self.dir)?;
        let task = self.usable_with(cancel)?;
        validate_clip(&task, clip)?;
        let asset = task.assets.iter().find(|a| a.id == clip.source_id).unwrap();
        media::preview(asset, clip)
    }
}

struct FileSync;
impl FileSync {
    fn sync(path: &Path) -> Result<()> {
        fs::File::open(path)?.sync_all()?;
        Ok(())
    }
}

/// A failed response can still have trustworthy, billed provider usage. Keep
/// that trace without ever accepting its incomplete/invalid selection. Network
/// errors and records outside the reserved request retain conservative charges.
fn account_provider_failure(
    task: &mut Task,
    index: usize,
    request: &VisionRequest,
    error: &AgentError,
    elapsed_ms: u64,
) -> bool {
    let reserved = task.model_calls[index].clone();
    if let AgentError::ModelResponseRejected { message, record } = error {
        let actual_tokens = record.input_tokens.checked_add(record.output_tokens);
        let reserved_tokens = reserved.input_tokens.checked_add(reserved.output_tokens);
        let valid_usage = record.call_id == request.call_id
            && actual_tokens
                .zip(reserved_tokens)
                .is_some_and(|(actual, limit)| actual <= limit)
            && record.estimated_cost_usd.is_finite()
            && record.estimated_cost_usd >= 0.0
            && record.estimated_cost_usd <= reserved.estimated_cost_usd + 1e-9
            && task.usage.input_tokens >= reserved.input_tokens
            && task.usage.output_tokens >= reserved.output_tokens
            && task.usage.estimated_cost_usd + 1e-9 >= reserved.estimated_cost_usd;
        if valid_usage {
            task.usage.input_tokens = (task.usage.input_tokens - reserved.input_tokens)
                .saturating_add(record.input_tokens);
            task.usage.output_tokens = (task.usage.output_tokens - reserved.output_tokens)
                .saturating_add(record.output_tokens);
            task.usage.estimated_cost_usd = (task.usage.estimated_cost_usd
                - reserved.estimated_cost_usd
                + record.estimated_cost_usd)
                .max(0.0);
            let mut known = record.as_ref().clone();
            known.sample_ids = request
                .samples
                .iter()
                .map(|sample| sample.id.clone())
                .collect();
            known.decision = None;
            known.status = "response_rejected".into();
            known.error = Some(message.clone());
            known.elapsed_ms = elapsed_ms;
            task.model_calls[index] = known;
            event(task, "model_response_rejected", "Provider response was incomplete or invalid. Verified usage and response trace were retained; no selection was accepted.");
            return true;
        }
    }
    task.model_calls[index].status = "failed_unknown_usage".into();
    task.model_calls[index].error = Some(error.to_string());
    task.model_calls[index].elapsed_ms = elapsed_ms;
    task.model_calls[index].decision = None;
    false
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameObservation {
    source_id: String,
    requested_time_ms: u64,
    decoded_time_ms: u64,
    sample_id: String,
}

fn sample_for_seek<'a>(
    task: &'a Task,
    source_id: &str,
    requested_time_ms: u64,
) -> Option<&'a Sample> {
    for event in task
        .history
        .iter()
        .rev()
        .filter(|event| event.kind == "sample_observed")
    {
        let Ok(observation) = serde_json::from_str::<FrameObservation>(&event.detail) else {
            continue;
        };
        if observation.source_id == source_id && observation.requested_time_ms == requested_time_ms
        {
            return task.samples.iter().find(|sample| {
                sample.id == observation.sample_id
                    && sample.source_id == source_id
                    && sample.time_ms == observation.decoded_time_ms
            });
        }
    }
    // store::read has already required a decoded-PTS audit for every Sample.
    // An exact known decoded timestamp therefore needs no new seek alias.
    // Earlier pre-release samples without that audit are rejected, not migrated.
    task.samples
        .iter()
        .find(|sample| sample.source_id == source_id && sample.time_ms == requested_time_ms)
}

fn modified_ns(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos().min(u64::MAX as u128) as u64)
}

fn check_sources_during(task: &Task, cancel: &CancelToken, clock: &Clock) -> Result<()> {
    check_sources_bounded(
        task,
        cancel,
        clock.timeout(task.limits.process_timeout_secs)?,
    )
}

fn check_sources_bounded(task: &Task, cancel: &CancelToken, timeout: Duration) -> Result<()> {
    let mut ids = HashSet::new();
    let started = Instant::now();
    for asset in &task.assets {
        cancel.check()?;
        if !ids.insert(&asset.id)
            || !asset.path.is_absolute()
            || fs::canonicalize(&asset.path)? != asset.path
        {
            return Err(invalid("Source identity or canonical path changed"));
        }
        store::regular(&asset.path)?;
        let metadata = asset.path.metadata()?;
        if metadata.len() != asset.size
            || modified_ns(&metadata) != asset.modified_ns
            || media::hash_file_bounded(
                &asset.path,
                cancel,
                timeout.saturating_sub(started.elapsed()),
            )? != asset.sha256
        {
            return Err(invalid(format!(
                "Source {} no longer matches its original fingerprint",
                asset.id
            )));
        }
    }
    Ok(())
}

pub fn plan_digest(task: &Task, plan: &EditPlan) -> Result<String> {
    Ok(media::digest(&serde_json::to_vec(&(
        1u32,
        plan,
        task.assets
            .iter()
            .map(|a| (&a.id, &a.sha256))
            .collect::<Vec<_>>(),
    ))?))
}

fn validate_clip(task: &Task, clip: &Clip) -> Result<()> {
    let asset = task
        .assets
        .iter()
        .find(|a| a.id == clip.source_id)
        .ok_or_else(|| invalid("Clip references unknown source"))?;
    if clip.id.is_empty()
        || clip.id.len() > 128
        || clip.start_ms >= clip.end_ms
        || clip.end_ms > asset.duration_ms
        || clip.end_ms - clip.start_ms < 100
        || clip.end_ms - clip.start_ms > task.limits.max_clip_ms
    {
        return Err(invalid(
            "Clip range violates source bounds or duration limits",
        ));
    }
    if clip.reason.len() > 8000
        || clip.uncertainty.len() > 8000
        || clip.evidence_ids.len() > task.limits.max_frames as usize
    {
        return Err(invalid("Clip explanation exceeds limits"));
    }
    Ok(())
}

/// Deterministic temporal coverage only: no visual/intent scoring or deletion.
/// Valid tasks have at most eight sources, so every nonempty source gets quota.
fn model_working_set(assets: &[Asset], samples: &[Sample]) -> Vec<Sample> {
    if samples.len() <= MODEL_WORKING_SET_FRAMES {
        return samples.to_vec();
    }
    let source_count = assets.len().max(1);
    let base = MODEL_WORKING_SET_FRAMES / source_count;
    let remainder = MODEL_WORKING_SET_FRAMES % source_count;
    let mut selected = Vec::with_capacity(MODEL_WORKING_SET_FRAMES);
    for (source_index, asset) in assets.iter().enumerate() {
        let quota = base + usize::from(source_index < remainder);
        let mut source: Vec<_> = samples.iter().filter(|s| s.source_id == asset.id).collect();
        source.sort_by(|a, b| a.time_ms.cmp(&b.time_ms).then_with(|| a.id.cmp(&b.id)));
        let count = quota.min(source.len());
        for index in 0..count {
            let position = if count == 1 {
                0
            } else {
                index * (source.len() - 1) / (count - 1)
            };
            selected.push(source[position].clone());
        }
    }
    selected
}

fn rejected_proposal_feedback(task: &Task) -> Option<String> {
    let call = task
        .model_calls
        .last()
        .filter(|call| call.status == "rejected")?;
    let Some(VisionDecision::ProposePlan { clips, .. }) = &call.decision else {
        return None;
    };
    let previous: Vec<_> = clips.iter().take(120).map(|clip| {
        // Unknown model-provided IDs can contain arbitrary text: only echo IDs
        // already present in the validated local source/sample inventories.
        let source = task.assets.iter().find(|a| a.id == clip.source_id);
        let evidence: Vec<_> = clip.evidence_ids.iter()
            .filter_map(|id| task.samples.iter().find(|sample| &sample.id == id))
            .take(task.limits.max_frames as usize).collect();
        let outside: Vec<_> = evidence.iter().filter(|sample| {
            sample.source_id != clip.source_id || sample.time_ms < clip.start_ms
                || sample.time_ms >= clip.end_ms
        }).map(|sample| &sample.id).collect();
        let duration = clip.end_ms.checked_sub(clip.start_ms);
        serde_json::json!({
            "source_id": source.map(|a| &a.id), "source_known": source.is_some(),
            "start_ms": clip.start_ms, "end_ms": clip.end_ms, "duration_ms": duration,
            "excess_over_maximum_clip_ms": duration.map(|n| n.saturating_sub(task.limits.max_clip_ms)),
            "source_duration_ms": source.map(|a| a.duration_ms),
            "evidence_ids": evidence.iter().map(|sample| &sample.id).collect::<Vec<_>>(),
            "evidence_outside_clip_ids": outside,
            "unrecognized_evidence_count": clip.evidence_ids.iter()
                .filter(|id| !task.samples.iter().any(|sample| &sample.id == *id)).count()
        })
    }).collect();
    Some(serde_json::json!({
        "message": "The preceding proposal was rejected by deterministic local validation and was not accepted. The user explicitly requested this new attempt. Correct the typed timing facts below using all displayed constraints. This feedback does not approve or export anything.",
        "rules": "Each clip must last minimum_clip_ms through maximum_clip_ms; summed durations must be <= maximum_output_ms; source intervals must be in bounds and non-overlapping; every cited sample must belong to its clip's source and half-open interval [start_ms, end_ms). A sample exactly at end_ms is outside: omit that citation or choose another legal range. Prefer fewer/shorter clips or an empty plan when needed. Optional legal windows are arithmetic examples, not semantic labels. Do not infer unsampled events or audio.",
        "minimum_clip_ms": 100, "maximum_clip_ms": task.limits.max_clip_ms,
        "maximum_output_ms": task.limits.max_output_ms, "target_duration_ms": task.target_duration_ms,
        "proposed_total_duration_ms": clips.iter().try_fold(0u64, |sum, clip| {
            sum.checked_add(clip.end_ms.checked_sub(clip.start_ms)?)
        }),
        "previous_proposal_clips": previous,
        "omitted_clip_count": clips.len().saturating_sub(120)
    }).to_string())
}

fn validate_plan(task: &Task, plan: &EditPlan, model: bool) -> Result<()> {
    if plan.revision == 0 || plan.clips.len() > 120 || plan.summary.len() > 16000 {
        return Err(invalid("Edit plan exceeds limits"));
    }
    let mut ids = HashSet::new();
    let mut total = 0u64;
    let mut ranges: Vec<(&str, u64, u64)> = Vec::new();
    for clip in &plan.clips {
        validate_clip(task, clip)?;
        if !ids.insert(&clip.id) {
            return Err(invalid("Duplicate clip identity"));
        }
        total = total
            .checked_add(clip.end_ms - clip.start_ms)
            .ok_or_else(|| invalid("Plan duration overflow"))?;
        if total > task.limits.max_output_ms {
            return Err(invalid("Plan exceeds maximum output duration"));
        }
        if model
            && (clip.evidence_ids.is_empty()
                || clip.reason.trim().is_empty()
                || clip.uncertainty.trim().is_empty())
        {
            return Err(invalid(
                "Model clips require sampled visual evidence, a reason, and uncertainty",
            ));
        }
        if model
            && ranges.iter().any(|(source, start, end)| {
                *source == clip.source_id && clip.start_ms < *end && *start < clip.end_ms
            })
        {
            return Err(invalid(
                "Model proposed duplicate or overlapping source intervals",
            ));
        }
        ranges.push((&clip.source_id, clip.start_ms, clip.end_ms));
        let mut evidence = HashSet::new();
        for id in &clip.evidence_ids {
            if !evidence.insert(id) {
                return Err(invalid("Duplicate evidence ID"));
            }
            let sample = task
                .samples
                .iter()
                .find(|s| &s.id == id)
                .ok_or_else(|| invalid("Clip references an unobserved frame"))?;
            if sample.source_id != clip.source_id
                || (model && (sample.time_ms < clip.start_ms || sample.time_ms >= clip.end_ms))
            {
                return Err(invalid(
                    "Model evidence must belong to the candidate source interval",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod working_set_tests {
    use super::*;

    fn inventory(counts: &[usize]) -> (Vec<Asset>, Vec<Sample>) {
        let mut assets = Vec::new();
        let mut samples = Vec::new();
        for (index, count) in counts.iter().enumerate() {
            let id = format!("source-{}", index + 1);
            assets.push(Asset {
                id: id.clone(),
                path: PathBuf::from("not-read-by-selection-test"),
                sha256: "unused".into(),
                size: 0,
                modified_ns: 0,
                duration_ms: 100_000,
                width: 640,
                height: 360,
                has_audio: false,
            });
            samples.extend((0..*count).map(|n| {
                let time = n as u64 * 1000;
                Sample {
                    id: format!("{id}-{time}"),
                    source_id: id.clone(),
                    time_ms: time,
                    image_path: PathBuf::from("not-read-by-selection-test.jpg"),
                    sha256: "unused".into(),
                }
            }));
        }
        (assets, samples)
    }

    #[test]
    fn small_working_sets_keep_every_sample_and_original_order() {
        let (assets, mut samples) = inventory(&[5, 5, 6]);
        samples.reverse();
        assert_eq!(model_working_set(&assets, &samples), samples);
        assert!(model_working_set(&assets, &[]).is_empty());
    }

    #[test]
    fn thirty_five_frames_select_sixteen_uniform_endpoints_without_mutating_inventory() {
        let (assets, samples) = inventory(&[35]);
        let original = samples.clone();
        let selected = model_working_set(&assets, &samples);
        assert_eq!(selected.len(), MODEL_WORKING_SET_FRAMES);
        for (index, sample) in selected.iter().enumerate() {
            assert_eq!(sample, &samples[index * 34 / 15]);
        }
        assert_eq!(selected.first(), samples.first());
        assert_eq!(selected.last(), samples.last());
        let mut reversed = samples.clone();
        reversed.reverse();
        assert_eq!(model_working_set(&assets, &reversed), selected);
        assert_eq!(samples, original);
    }

    #[test]
    fn source_quotas_are_balanced_with_stable_remainders_and_no_sparse_redistribution() {
        for (counts, expected) in [
            (vec![20, 20], vec![8, 8]),
            (vec![20, 20, 20], vec![6, 5, 5]),
            (vec![35, 1], vec![8, 1]),
            (vec![35, 0, 2], vec![6, 0, 2]),
        ] {
            let (assets, samples) = inventory(&counts);
            let selected = model_working_set(&assets, &samples);
            assert!(selected.len() <= MODEL_WORKING_SET_FRAMES);
            for (asset, count) in assets.iter().zip(expected) {
                let source: Vec<_> = selected
                    .iter()
                    .filter(|s| s.source_id == asset.id)
                    .collect();
                assert_eq!(source.len(), count);
                assert!(source
                    .windows(2)
                    .all(|pair| (pair[0].time_ms, &pair[0].id) < (pair[1].time_ms, &pair[1].id)));
                for sample in &source {
                    assert!(sample.time_ms < asset.duration_ms);
                    assert!(samples.contains(sample));
                }
                if count > 0 {
                    let all: Vec<_> = samples.iter().filter(|s| s.source_id == asset.id).collect();
                    assert_eq!(source.first(), all.first());
                    assert_eq!(source.last(), all.last());
                }
            }
        }
    }
}

#[cfg(test)]
mod rejection_accounting_tests {
    use super::*;

    /// A scripted provider fixture, not a network call or semantic evaluation.
    struct ScriptedRejectedResponse;
    impl VisionProvider for ScriptedRejectedResponse {
        fn identity(&self) -> (String, String, String) {
            (
                "SIMULATED".into(),
                "not-a-real-model".into(),
                "ledger-test-v1".into(),
            )
        }
        fn ready(&self) -> bool {
            true
        }
        fn estimate(&self, _: &VisionRequest) -> Result<(u64, f64)> {
            Ok((3000, 0.01))
        }
        fn decide(&self, request: &VisionRequest, _: &CancelToken) -> Result<VisionOutcome> {
            Err(AgentError::ModelResponseRejected {
                message: "Scripted incomplete response: selection rejected".into(),
                record: Box::new(ModelCall {
                    call_id: request.call_id.clone(),
                    backend: "SIMULATED".into(),
                    requested_model: "not-a-real-model".into(),
                    model: "not-a-real-snapshot".into(),
                    prompt_version: "ledger-test-v1".into(),
                    sample_ids: vec!["incorrect-provider-sample-list".into()],
                    provider_response_id: Some("scripted_response_123".into()),
                    request_id: Some("scripted_request_123".into()),
                    input_tokens: 80,
                    output_tokens: 20,
                    estimated_cost_usd: 0.001,
                    elapsed_ms: 0,
                    status: "response_rejected".into(),
                    // Even a broken fixture attaching a decision must not apply it.
                    decision: Some(VisionDecision::ProposePlan {
                        clips: vec![],
                        summary: "Do not apply this scripted decision".into(),
                    }),
                    error: Some("incomplete".into()),
                }),
            })
        }
    }

    fn reserved_task() -> (Task, VisionRequest) {
        let limits = Limits {
            max_output_tokens: 2000,
            ..Limits::default()
        };
        let record = ModelCall {
            call_id: "call-1".into(),
            backend: "SIMULATED".into(),
            requested_model: "not-a-real-model".into(),
            model: "not-a-real-model".into(),
            prompt_version: "ledger-test-v1".into(),
            sample_ids: vec![],
            provider_response_id: None,
            request_id: None,
            input_tokens: 1000,
            output_tokens: 2000,
            estimated_cost_usd: 0.01,
            elapsed_ms: 0,
            status: "reserved".into(),
            decision: None,
            error: None,
        };
        let task = Task {
            schema_version: 1,
            id: "scripted-ledger-only".into(),
            created_at: now(),
            intent: "No semantic evaluation".into(),
            target_duration_ms: 1000,
            assets: vec![],
            samples: vec![],
            model_calls: vec![record],
            plan: None,
            approval: None,
            status: TaskStatus::Analyzing,
            last_error: None,
            limits: limits.clone(),
            usage: Usage {
                model_calls: 1,
                input_tokens: 1000,
                output_tokens: 2000,
                estimated_cost_usd: 0.01,
                ..Usage::default()
            },
            cloud_consent: true,
            export: None,
            shards: vec![],
            history: vec![],
        };
        let request = VisionRequest {
            call_id: "call-1".into(),
            intent: task.intent.clone(),
            target_duration_ms: 1000,
            assets: vec![],
            samples: vec![],
            remaining_frames: limits.max_frames,
            limits,
            feedback: None,
        };
        (task, request)
    }

    #[test]
    fn scripted_rejected_response_retains_verified_trace_and_reconciles_usage_without_selection() {
        let (mut task, request) = reserved_task();
        let error = ScriptedRejectedResponse
            .decide(&request, &CancelToken::default())
            .err()
            .unwrap();
        assert!(account_provider_failure(
            &mut task, 0, &request, &error, 245
        ));
        assert_eq!(task.usage.model_calls, 1);
        assert_eq!(task.usage.input_tokens, 80);
        assert_eq!(task.usage.output_tokens, 20);
        assert!((task.usage.estimated_cost_usd - 0.001).abs() < 1e-9);
        let record = &task.model_calls[0];
        assert_eq!(record.status, "response_rejected");
        assert_eq!(record.model, "not-a-real-snapshot");
        assert_eq!(
            record.provider_response_id.as_deref(),
            Some("scripted_response_123")
        );
        assert_eq!(record.request_id.as_deref(), Some("scripted_request_123"));
        assert_eq!(record.elapsed_ms, 245);
        assert!(record.sample_ids.is_empty() && record.decision.is_none());
        assert!(task.plan.is_none() && task.approval.is_none());
    }

    #[test]
    fn unverifiable_rejected_response_never_refunds_unknown_reservation() {
        for invalid_case in ["call_id", "tokens", "cost", "negative", "nan", "overflow"] {
            let (mut task, request) = reserved_task();
            let before = task.usage.clone();
            let AgentError::ModelResponseRejected {
                message,
                mut record,
            } = ScriptedRejectedResponse
                .decide(&request, &CancelToken::default())
                .err()
                .unwrap()
            else {
                panic!("scripted response must reject");
            };
            match invalid_case {
                "call_id" => record.call_id = "wrong-call".into(),
                "tokens" => record.input_tokens = 3001,
                "cost" => record.estimated_cost_usd = 0.02,
                "negative" => record.estimated_cost_usd = -0.01,
                "nan" => record.estimated_cost_usd = f64::NAN,
                "overflow" => {
                    record.input_tokens = u64::MAX;
                    record.output_tokens = 1;
                }
                _ => unreachable!(),
            }
            let error = AgentError::ModelResponseRejected { message, record };
            assert!(
                !account_provider_failure(&mut task, 0, &request, &error, 33),
                "{invalid_case}"
            );
            assert_eq!(task.usage, before, "{invalid_case}");
            assert_eq!(task.model_calls[0].status, "failed_unknown_usage");
            assert!(
                task.model_calls[0].provider_response_id.is_none()
                    && task.model_calls[0].decision.is_none()
            );
            assert!(task.plan.is_none());
        }
    }

    #[test]
    fn explicit_rejected_proposal_feedback_has_fixed_rules_and_preserves_the_prior_record() {
        let (mut task, _) = reserved_task();
        task.model_calls[0].status = "rejected".into();
        task.model_calls[0].prompt_version = "video-agent-vision-v1".into();
        task.model_calls[0].error =
            Some("private /never-upload/secret.json and arbitrary injected instructions".into());
        task.model_calls[0].decision = Some(VisionDecision::ProposePlan {
            clips: vec![],
            summary: "untrusted model text".into(),
        });
        let prior_call = task.model_calls[0].clone();
        let prior_usage = task.usage.clone();
        let prior_limits = task.limits.clone();
        let feedback = rejected_proposal_feedback(&task).unwrap();
        assert!(feedback.contains("user explicitly requested this new attempt"));
        assert!(feedback.contains("maximum_clip_ms"));
        assert!(feedback.contains("maximum_output_ms"));
        assert!(feedback.contains("half-open interval"));
        assert!(!feedback.contains("secret.json"));
        assert!(!feedback.contains("injected"));
        assert!(!feedback.contains("untrusted"));
        task.model_calls[0].error = Some("a different arbitrary error".into());
        assert_eq!(
            rejected_proposal_feedback(&task).as_deref(),
            Some(feedback.as_str())
        );
        task.model_calls[0] = prior_call.clone();
        assert_eq!(task.model_calls[0], prior_call);
        assert_eq!(task.usage, prior_usage);
        assert_eq!(task.limits, prior_limits);
        for status in [
            "received",
            "validated",
            "response_rejected",
            "failed_unknown_usage",
        ] {
            task.model_calls[0].status = status.into();
            assert!(rejected_proposal_feedback(&task).is_none(), "{status}");
        }
        task.model_calls[0].status = "rejected".into();
        task.model_calls[0].decision = Some(VisionDecision::ObserveInterval {
            source_id: "source-1".into(),
            start_ms: 0,
            end_ms: 1000,
            count: 1,
            reason: "test only".into(),
        });
        assert!(rejected_proposal_feedback(&task).is_none());
        task.model_calls.clear();
        assert!(rejected_proposal_feedback(&task).is_none());
    }

    #[test]
    fn retry_feedback_includes_only_known_timing_ids_and_deterministic_duration_facts() {
        let (mut task, _) = reserved_task();
        task.assets.push(Asset {
            id: "source-1".into(),
            path: PathBuf::from("/private/not-uploaded/source.mp4"),
            sha256: "private-source-hash".into(),
            size: 0,
            modified_ns: 0,
            duration_ms: 75_000,
            width: 640,
            height: 360,
            has_audio: false,
        });
        task.samples = [3125, 15_625, 21_875, 46_875]
            .into_iter()
            .map(|time| Sample {
                id: format!("source-1-{time}"),
                source_id: "source-1".into(),
                time_ms: time,
                image_path: PathBuf::from("/private/not-uploaded/sample.jpg"),
                sha256: "unused".into(),
            })
            .collect();
        let private_text = "SECRET arbitrary instructions /private/not-uploaded/credential.json";
        let make_clip = |source: &str, start, end, ids| ClipProposal {
            source_id: source.into(),
            start_ms: start,
            end_ms: end,
            reason: private_text.into(),
            uncertainty: private_text.into(),
            evidence_ids: ids,
        };
        task.model_calls[0].status = "rejected".into();
        task.model_calls[0].error = Some(private_text.into());
        task.model_calls[0].decision = Some(VisionDecision::ProposePlan {
            clips: vec![
                make_clip(
                    "source-1",
                    3125,
                    15_625,
                    vec!["source-1-3125".into(), "source-1-15625".into()],
                ),
                make_clip(
                    "source-1",
                    21_875,
                    46_875,
                    vec!["source-1-21875".into(), "source-1-46875".into()],
                ),
                make_clip(private_text, 1, 101, vec![private_text.into()]),
            ],
            summary: private_text.into(),
        });
        let previous = task.model_calls[0].clone();
        let serialized = rejected_proposal_feedback(&task).unwrap();
        assert!(!serialized.contains("SECRET"));
        assert!(!serialized.contains("/private/"));
        assert!(!serialized.contains("private-source-hash"));
        let feedback: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        let clips = &feedback["previous_proposal_clips"];
        assert_eq!(clips[0]["duration_ms"], 12_500);
        assert_eq!(
            clips[0]["evidence_outside_clip_ids"],
            serde_json::json!(["source-1-15625"])
        );
        assert_eq!(clips[1]["start_ms"], 21_875);
        assert_eq!(clips[1]["end_ms"], 46_875);
        assert_eq!(clips[1]["duration_ms"], 25_000);
        assert_eq!(clips[1]["excess_over_maximum_clip_ms"], 10_000);
        assert_eq!(
            clips[1]["evidence_outside_clip_ids"],
            serde_json::json!(["source-1-46875"])
        );
        assert!(clips[2]["source_id"].is_null());
        assert_eq!(clips[2]["source_known"], false);
        assert_eq!(clips[2]["unrecognized_evidence_count"], 1);
        assert_eq!(feedback["proposed_total_duration_ms"], 37_600);
        assert_eq!(task.model_calls[0], previous);
    }

    #[test]
    fn overlong_proposals_remain_rejected_and_shorter_evidenced_plans_are_allowed() {
        let (mut task, _) = reserved_task();
        task.target_duration_ms = 20_000;
        task.limits.max_clip_ms = 15_000;
        task.limits.max_output_ms = 20_000;
        task.assets.push(Asset {
            id: "source-1".into(),
            path: PathBuf::from("not-read-by-pure-validator"),
            sha256: "not-semantic-evidence".into(),
            size: 0,
            modified_ns: 0,
            duration_ms: 75_000,
            width: 640,
            height: 360,
            has_audio: false,
        });
        task.samples.push(Sample {
            id: "sample-1".into(),
            source_id: "source-1".into(),
            time_ms: 37_500,
            image_path: PathBuf::from("not-read-by-pure-validator.jpg"),
            sha256: "test-only".into(),
        });
        let mut plan = EditPlan {
            revision: 1,
            clips: vec![Clip {
                id: "clip-1".into(),
                source_id: "source-1".into(),
                start_ms: 28_125,
                end_ms: 46_875,
                reason: "Validator fixture only".into(),
                uncertainty: "Not a semantic evaluation".into(),
                evidence_ids: vec!["sample-1".into()],
                human_modified: false,
            }],
            summary: "Validator fixture only".into(),
            audio_policy: AudioPolicy::Mute,
        };
        assert!(validate_plan(&task, &plan, true).is_err());
        plan.clips[0].end_ms = 43_125;
        assert!(validate_plan(&task, &plan, true).is_ok());
        let mut second = plan.clips[0].clone();
        second.id = "clip-2".into();
        second.start_ms = 45_000;
        second.end_ms = 60_000;
        second.evidence_ids = vec!["sample-2".into()];
        let mut sample = task.samples[0].clone();
        sample.id = "sample-2".into();
        sample.time_ms = 50_000;
        task.samples.push(sample);
        plan.clips.push(second);
        assert!(
            validate_plan(&task, &plan, true).is_err(),
            "summed duration stays bounded"
        );
        plan.clips.pop();
        plan.clips[0].end_ms = 37_500;
        assert!(
            validate_plan(&task, &plan, true).is_err(),
            "end_ms is exclusive for evidence"
        );
    }
}

#[cfg(test)]
mod integrity_load_tests {
    use super::*;

    // These ordinary tests cover authorization/hash boundaries with inert bytes.
    // They do NOT claim media decoding; the ignored FFmpeg suite covers that.
    fn fixture(root: &Path) -> (Workspace, Task) {
        let parent = fs::canonicalize(root).unwrap();
        let input = parent.join("source-integrity-fixture.bin");
        fs::write(&input, b"inert original source bytes").unwrap();
        let metadata = input.metadata().unwrap();
        let dir = parent.join("task");
        fs::create_dir(&dir).unwrap();
        let workspace = Workspace { dir };
        let asset = Asset {
            id: "source-1".into(),
            path: input.clone(),
            sha256: media::hash_file(&input).unwrap(),
            size: metadata.len(),
            modified_ns: modified_ns(&metadata),
            duration_ms: 2000,
            width: 16,
            height: 16,
            has_audio: false,
        };
        let plan = EditPlan {
            revision: 1,
            clips: vec![Clip {
                id: "clip-1".into(),
                source_id: asset.id.clone(),
                start_ms: 0,
                end_ms: 1000,
                reason: "MANUAL integrity fixture".into(),
                uncertainty: "Not a media or semantic test".into(),
                evidence_ids: vec![],
                human_modified: true,
            }],
            summary: "File integrity fixture only".into(),
            audio_policy: AudioPolicy::Mute,
        };
        let mut task = Task {
            schema_version: 1,
            id: "task".into(),
            created_at: now(),
            intent: "File-integrity test only".into(),
            target_duration_ms: 1000,
            assets: vec![asset],
            samples: vec![],
            model_calls: vec![],
            plan: Some(plan),
            approval: None,
            status: TaskStatus::Completed,
            last_error: None,
            limits: Limits::default(),
            usage: Usage::default(),
            cloud_consent: false,
            export: None,
            shards: vec![],
            history: vec![],
        };
        let digest = plan_digest(&task, task.plan.as_ref().unwrap()).unwrap();
        task.approval = Some(Approval {
            plan_digest: digest.clone(),
            revision: 1,
            confirmed_at: now(),
        });
        let exports = store::owned_dir(workspace.task_dir(), "exports").unwrap();
        let path = exports.join(format!("{digest}.mp4"));
        fs::write(
            &path,
            b"inert output bytes; real decoding tested separately",
        )
        .unwrap();
        task.export = Some(ExportRecord {
            plan_digest: digest,
            path: path.clone(),
            sha256: media::hash_file(&path).unwrap(),
            duration_ms: 1000,
            has_audio: false,
            verified: true,
            completed_at: Some(now()),
        });
        store::save(workspace.task_dir(), &task).unwrap();
        (workspace, task)
    }

    #[test]
    fn reopen_rejects_tampered_missing_or_symlink_output_without_marking_source_stale() {
        for case in ["modified", "missing", "symlink"] {
            let root = tempfile::tempdir().unwrap();
            let (workspace, task) = fixture(root.path());
            assert_eq!(workspace.load().unwrap().status, TaskStatus::Completed);
            assert!(
                !Workspace::snapshot(workspace.task_dir())
                    .unwrap()
                    .export
                    .unwrap()
                    .verified,
                "Progress snapshot cannot authorize output playback"
            );
            let output = &task.export.as_ref().unwrap().path;
            match case {
                "modified" => fs::write(output, b"changed artifact").unwrap(),
                "missing" => fs::remove_file(output).unwrap(),
                "symlink" => {
                    #[cfg(unix)]
                    {
                        fs::remove_file(output).unwrap();
                        std::os::unix::fs::symlink(&task.assets[0].path, output).unwrap();
                    }
                    #[cfg(not(unix))]
                    {
                        continue;
                    }
                }
                _ => unreachable!(),
            }
            let loaded = Workspace::open(workspace.task_dir())
                .unwrap()
                .load()
                .unwrap();
            assert_eq!(loaded.status, TaskStatus::Failed, "{case}");
            assert!(!loaded.export.unwrap().verified);
            assert!(
                loaded.approval.is_some(),
                "Output failure must not pretend original source identity changed"
            );
            assert!(loaded.last_error.unwrap().contains("Saved output"));
            assert!(loaded
                .history
                .iter()
                .any(|event| event.kind == "output_invalidated"));
        }
    }

    #[test]
    fn reopen_rejects_foreign_output_path_and_current_plan_or_approval_mismatch() {
        for case in ["foreign_path", "wrong_plan", "wrong_approval"] {
            let root = tempfile::tempdir().unwrap();
            let (workspace, mut task) = fixture(root.path());
            match case {
                "foreign_path" => {
                    let outside = fs::canonicalize(root.path()).unwrap().join("outside.mp4");
                    fs::copy(&task.export.as_ref().unwrap().path, &outside).unwrap();
                    task.export.as_mut().unwrap().path = outside;
                }
                "wrong_plan" => task.plan.as_mut().unwrap().clips[0].end_ms = 1100,
                "wrong_approval" => task.approval.as_mut().unwrap().revision = 2,
                _ => unreachable!(),
            }
            store::save(workspace.task_dir(), &task).unwrap();
            let loaded = Workspace::open(workspace.task_dir())
                .unwrap()
                .load()
                .unwrap();
            assert_eq!(loaded.status, TaskStatus::Failed, "{case}");
            assert!(!loaded.export.unwrap().verified);
            assert!(loaded.last_error.unwrap().contains("Saved output"));
        }
    }

    #[test]
    fn legacy_seek_only_sample_is_rejected_on_reopen_without_decoded_timecode_audit() {
        let root = tempfile::tempdir().unwrap();
        let (workspace, mut task) = fixture(root.path());
        let frames = store::owned_dir(workspace.task_dir(), "frames").unwrap();
        let image_path = frames.join("source-1-1000.jpg");
        fs::write(
            &image_path,
            b"inert sample fixture, not submitted to any model",
        )
        .unwrap();
        task.samples.push(Sample {
            id: "source-1-1000".into(),
            source_id: "source-1".into(),
            time_ms: 1000,
            sha256: media::hash_file(&image_path).unwrap(),
            image_path,
        });
        task.usage.frames_created = 1;
        event(
            &mut task,
            "sample_observed",
            serde_json::to_string(&FrameObservation {
                source_id: "source-1".into(),
                requested_time_ms: 666,
                decoded_time_ms: 1000,
                sample_id: "source-1-1000".into(),
            })
            .unwrap(),
        );
        store::save(workspace.task_dir(), &task).unwrap();
        assert!(Workspace::open(workspace.task_dir()).is_ok());
        task.history.retain(|event| event.kind != "sample_observed");
        store::save(workspace.task_dir(), &task).unwrap();
        let error = Workspace::open(workspace.task_dir()).unwrap_err();
        assert!(error
            .to_string()
            .contains("lack verified decoded-frame timecodes"));
    }
}
