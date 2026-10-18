//! A local, human-in-the-loop desktop surface for the bounded Video Agent.
//!
//! Long-running inspection, model calls and media work stay off the egui thread.
//! The task on disk is authoritative; a dirty edit is never treated as approved.
use eframe::egui::{self, Color32, RichText};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
use video_agent::{
    AudioPolicy, CancelToken, Clip, EditPlan, Limits, OpenAiVision, Task, TaskStatus,
    VisionProvider, Workspace,
};

const ACCENT: Color32 = Color32::from_rgb(100, 208, 184);
const MUTED: Color32 = Color32::from_rgb(156, 166, 181);
const WARNING: Color32 = Color32::from_rgb(244, 193, 111);
const ERROR: Color32 = Color32::from_rgb(255, 143, 143);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Page {
    Sources,
    Observe,
    Edit,
    Result,
}

enum JobResult {
    Task(PathBuf, Box<Task>),
    Preview {
        path: PathBuf,
        source_id: String,
        time_ms: u64,
        task: Box<Task>,
    },
    Player(Child, String),
    SmartFeedback {
        path: PathBuf,
        task: Box<Task>,
        report: Option<Box<video_agent::smart_feedback::SmartReport>>,
        explicitly_adopted: bool,
    },
    Error {
        message: String,
        retained: Option<Box<Task>>,
    },
}

struct Job {
    label: String,
    started: Instant,
    cancel: CancelToken,
    receiver: Receiver<JobResult>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// The Agent is a separate native app, leaving the legacy toolkit unchanged.
pub struct VideoAgentApp {
    task_dir: Option<PathBuf>,
    task: Option<Task>,
    draft: Option<EditPlan>,
    dirty: bool,
    page: Page,
    inputs: Vec<PathBuf>,
    input_text: String,
    parent: String,
    resume_path: String,
    intent: String,
    target_seconds: u64,
    limits: Limits,
    consent: bool,
    job: Option<Job>,
    error: Option<String>,
    notice: String,
    textures: HashMap<PathBuf, std::result::Result<egui::TextureHandle, String>>,
    fonts_installed: bool,
    source_index: usize,
    preview_seconds: f64,
    preview_path: Option<PathBuf>,
    preview_caption: String,
    player: Option<Child>,
    provider_ready: bool,
    provider_model: String,
    provider_error: Option<String>,
    smart_upload_consent: bool,
    smart_report: Option<video_agent::smart_feedback::SmartReport>,
}

impl VideoAgentApp {
    pub fn new(workspace: Option<PathBuf>) -> Self {
        let mut app = Self {
            task_dir: None,
            task: None,
            draft: None,
            dirty: false,
            page: Page::Sources,
            inputs: Vec::new(),
            input_text: String::new(),
            parent: String::new(),
            resume_path: String::new(),
            intent: String::new(),
            target_seconds: 30,
            limits: Limits::default(),
            consent: false,
            job: None,
            error: None,
            notice: "Choose local footage. Nothing is uploaded until you explicitly consent."
                .into(),
            textures: HashMap::new(),
            fonts_installed: false,
            source_index: 0,
            preview_seconds: 0.0,
            preview_path: None,
            preview_caption: String::new(),
            player: None,
            provider_ready: false,
            provider_model: String::new(),
            provider_error: None,
            smart_upload_consent: false,
            smart_report: None,
        };
        app.refresh_provider();
        if let Some(path) = workspace {
            app.resume_path = path.display().to_string();
            app.open_workspace(path);
        }
        app
    }

    fn start_job<F>(&mut self, label: &str, work: F)
    where
        F: FnOnce(CancelToken) -> std::result::Result<JobResult, String> + Send + 'static,
    {
        if self.job.is_some() {
            return;
        }
        self.error = None;
        self.stop_player();
        let cancel = CancelToken::default();
        let thread_cancel = cancel.clone();
        let (sender, receiver) = mpsc::channel();
        let retained_path = self.task_dir.clone();
        let thread = std::thread::spawn(move || {
            let result = match work(thread_cancel) {
                Ok(result) => result,
                Err(message) => {
                    // Fingerprint checks can be expensive; failure recovery also stays on this worker.
                    let retained = retained_path
                        .and_then(|path| Workspace::snapshot(&path).ok())
                        .map(Box::new);
                    JobResult::Error { message, retained }
                }
            };
            // Child does not kill the process on drop. If the app closed, do not leak ffplay.
            if let Err(mpsc::SendError(JobResult::Player(mut child, _))) = sender.send(result) {
                let _ = child.kill();
                let _ = child.wait();
            }
        });
        self.job = Some(Job {
            label: label.into(),
            started: Instant::now(),
            cancel,
            receiver,
            thread: Some(thread),
        });
    }

    fn workspace_job<F>(&mut self, label: &str, work: F)
    where
        F: FnOnce(Workspace, CancelToken) -> video_agent::Result<Task> + Send + 'static,
    {
        let Some(path) = self.task_dir.clone() else {
            self.error = Some("Create or open a task first.".into());
            return;
        };
        self.start_job(label, move |cancel| {
            let workspace =
                Workspace::open_with_cancel(&path, &cancel).map_err(|e| e.to_string())?;
            let task = work(workspace, cancel).map_err(|e| e.to_string())?;
            Ok(JobResult::Task(path, Box::new(task)))
        });
    }

    fn open_workspace(&mut self, path: PathBuf) {
        self.start_job(
            "Reopening saved task and validating inputs",
            move |cancel| {
                let workspace =
                    Workspace::open_with_cancel(&path, &cancel).map_err(|e| e.to_string())?;
                let task = workspace
                    .load_with_cancel(&cancel)
                    .map_err(|e| e.to_string())?;
                Ok(JobResult::Task(
                    workspace.task_dir().to_path_buf(),
                    Box::new(task),
                ))
            },
        );
    }

    fn accept_task(&mut self, path: PathBuf, task: Task) {
        if self.task_dir.as_ref() != Some(&path) {
            self.textures.clear();
            self.preview_path = None;
            self.source_index = 0;
            self.preview_seconds = 0.0;
            self.smart_upload_consent = false;
            self.smart_report = None;
        }
        self.resume_path = path.display().to_string();
        self.task_dir = Some(path);
        self.intent = task.intent.clone();
        self.target_seconds = task.target_duration_ms / 1000;
        self.limits = task.limits.clone();
        self.refresh_provider();
        self.consent = task.cloud_consent;
        self.draft = task.plan.clone();
        self.dirty = false;
        self.page = if has_current_verified_export(&task) {
            Page::Result
        } else if task.plan.is_some() {
            Page::Edit
        } else {
            Page::Observe
        };
        self.notice = format!("Task {} · {}", task.id, task_status_label(&task));
        self.task = Some(task);
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let result = self.job.as_ref().and_then(|job| match job.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(JobResult::Error { message: "Background worker ended unexpectedly. Reopen the task to inspect retained stages.".into(), retained: None })
            }
            Err(mpsc::TryRecvError::Empty) => None,
        });
        if let Some(result) = result {
            self.job = None;
            match result {
                JobResult::Task(path, task) => self.accept_task(path, *task),
                JobResult::Preview {
                    path,
                    source_id,
                    time_ms,
                    task,
                } => {
                    self.preview_path = Some(path);
                    self.preview_caption = format!(
                        "Human-only preview · requested seek {source_id} @ {} (nearest decoded frame)",
                        timecode(time_ms)
                    );
                    self.task = Some(*task);
                    self.notice =
                        "Local preview frame ready. This frame is not sent to a model.".into();
                }
                JobResult::Player(child, message) => {
                    self.player = Some(child);
                    self.notice = message;
                }
                JobResult::SmartFeedback {
                    path,
                    task,
                    report,
                    explicitly_adopted,
                } => {
                    if self.task_dir.as_ref() == Some(&path) && self.dirty {
                        // Refreshing historical status must never discard unsaved human edits.
                        self.task = Some(*task);
                    } else {
                        self.accept_task(path, *task);
                    }
                    self.smart_report = report.map(|report| *report);
                    self.notice = if explicitly_adopted {
                        "Explicit adoption completed. Check the saved revision and its approval state before exporting."
                    } else {
                        "AI feedback records loaded. Candidates remain separate from your saved plan and still require human review."
                    }.into();
                }
                JobResult::Error { message, retained } => {
                    self.error = Some(message);
                    self.notice = "Operation stopped. Review the error; completed task stages remain available.".into();
                    if let Some(task) = retained {
                        self.consent = task.cloud_consent;
                        if !self.dirty {
                            self.draft = task.plan.clone();
                        }
                        self.task = Some(*task);
                    }
                }
            }
        }
        if let Some(player) = self.player.as_mut() {
            match player.try_wait() {
                Ok(Some(status)) => {
                    self.player = None;
                    if !status.success() {
                        self.error = Some("The preview player exited with an error. Check that ffplay is installed and a display/audio device is available.".into());
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    self.player = None;
                    self.error = Some(format!("Could not inspect preview player: {error}"));
                }
            }
        }
        if self.job.is_some() || self.player.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn stop_player(&mut self) {
        if let Some(mut player) = self.player.take() {
            let _ = player.kill();
            let _ = player.wait();
        }
    }

    fn refresh_provider(&mut self) {
        match OpenAiVision::from_env(&self.limits) {
            Ok(provider) => {
                self.provider_ready = provider.ready();
                self.provider_model = provider.identity().1;
                self.provider_error = None;
            }
            Err(error) => {
                self.provider_ready = false;
                self.provider_model = "configuration error".into();
                self.provider_error = Some(error.to_string());
            }
        }
    }

    fn can_run_task(&self) -> bool {
        self.job.is_none()
            && self
                .task
                .as_ref()
                .is_some_and(|task| has_execution_budget(&task.usage, &task.limits))
    }

    fn play_clip(&mut self, clip: Clip) {
        let Some(path) = self.task_dir.clone() else {
            return;
        };
        self.start_job("Opening source preview", move |cancel| {
            let workspace =
                Workspace::open_with_cancel(&path, &cancel).map_err(|e| e.to_string())?;
            workspace
                .spawn_preview_with_cancel(&clip, &cancel)
                .map(|child| {
                    JobResult::Player(
                        child,
                        "Playing the source interval in ffplay, including its original audio."
                            .into(),
                    )
                })
                .map_err(|e| e.to_string())
        });
    }

    fn smart_feedback_panel(&mut self, ui: &mut egui::Ui) {
        let Some(task) = self.task.clone() else {
            return;
        };
        if task.plan.is_none() {
            return;
        }
        let max_rounds = smart_round_limit(&task, self.smart_report.as_ref());
        ui.add_space(10.0);
        egui::CollapsingHeader::new("AI preview feedback · optional, separate consent")
            .default_open(true)
            .show(ui, |ui| {
                ui.label("Render → inspect sparse preview images → suggest a bounded revision. This does not change, approve or export your saved plan.");
                ui.label(RichText::new("Uploads: rendered-preview JPEGs, bounded source JPEG samples, your intent, current plan and objective checks. No audio or full video. Sparse vision can miss actions and timing; its verdict is not human acceptance.").small().color(WARNING));
                ui.label(RichText::new(format!(
                    "Run bound: {max_rounds} rounds. New runs default to 2; a loaded run keeps its original bound (1–3). Uses remaining task model/token/cost/time budgets and persistent preview attempts. Calls used: {} / {} · estimated cost used: ${:.4}.",
                    task.usage.model_calls, task.limits.max_model_calls,
                    task.usage.estimated_cost_usd
                )).small().color(MUTED));
                ui.add_enabled_ui(self.job.is_none(), |ui| {
                    ui.checkbox(
                        &mut self.smart_upload_consent,
                        "I separately allow these preview images and task details to be sent to the configured model",
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(
                        smart_run_allowed(&task, self.dirty, self.job.is_some(), self.smart_upload_consent),
                        egui::Button::new(format!("Run / resume AI feedback · up to {max_rounds} rounds")),
                    ).clicked() {
                        if let Some(path) = self.task_dir.clone() {
                            let consent = self.smart_upload_consent;
                            self.start_job("Rendering and reviewing bounded AI previews", move |cancel| {
                                let task = Workspace::snapshot(&path).map_err(|_| "Task metadata is unavailable; no feedback request was sent.".to_owned())?;
                                let provider = OpenAiVision::from_env(&task.limits).map_err(|_| "The private model configuration is unavailable. Check it locally; no credential is displayed.".to_owned())?;
                                let report = video_agent::smart_feedback::run(&path, &provider, max_rounds, consent, &cancel)
                                    .map_err(|_| "AI feedback stopped. Reload its saved status to inspect retained progress; no candidate was approved or exported.".to_owned())?;
                                let task = Workspace::snapshot(&path).map_err(|_| "Feedback was saved, but task metadata could not be reloaded.".to_owned())?;
                                Ok(JobResult::SmartFeedback { path, task: Box::new(task), report: Some(Box::new(report)), explicitly_adopted: false })
                            });
                        }
                    }
                    if ui.add_enabled(self.job.is_none(), egui::Button::new("Load saved feedback status")).clicked() {
                        if let Some(path) = self.task_dir.clone() {
                            self.start_job("Reading saved AI feedback, without model calls", move |_cancel| {
                                let report = video_agent::smart_feedback::status(&path).map_err(|_| "Saved feedback is unavailable or invalid. No model was called.".to_owned())?;
                                let task = Workspace::snapshot(&path).map_err(|_| "Task metadata is unavailable.".to_owned())?;
                                Ok(JobResult::SmartFeedback { path, task: Box::new(task), report: report.map(Box::new), explicitly_adopted: false })
                            });
                        }
                    }
                });
                if self.dirty {
                    ui.colored_label(WARNING, "Save or discard your human edits before starting feedback or adopting a candidate. Loading status will preserve unsaved edits.");
                }
                if task.samples.is_empty() {
                    ui.colored_label(WARNING, "Generate the local storyboard on Observed frames first. Feedback is disabled until source samples exist; no preview or model attempt is started here.");
                }
                // Own the snapshot while buttons may start a job and mutably borrow the app.
                let report = self.smart_report.take();
                if let Some(report) = report.as_ref() {
                    ui.label(RichText::new(format!("Saved run {} · phase {} · {} / {} rounds", report.run_id, report.phase, report.rounds.len(), report.max_rounds)).strong());
                    ui.label(RichText::new(format!("Base plan: {}", report.base_plan_digest)).small().color(MUTED));
                    if let Some(reason) = &report.stopped_reason {
                        ui.label(format!("Stop reason: {reason}"));
                    }
                    let current_matches = report.matches_current_plan && task.plan.as_ref().is_some_and(|plan| {
                        video_agent::workflow::plan_digest(&task, plan).is_ok_and(|digest| digest == report.base_plan_digest)
                    });
                    if !current_matches {
                        ui.colored_label(WARNING, "This is historical feedback for a different saved plan. It is not fresh media verification or permission to adopt/export.");
                    }
                    for round in &report.rounds {
                        egui::CollapsingHeader::new(format!("Round {} · objective checks {}", round.round, if round.preview.quality.objective_pass { "passed" } else { "need attention" }))
                            .id_source(("smart-feedback-round", &report.run_id, round.round))
                            .show(ui, |ui| {
                                ui.label(format!("Preview {} · {:.2}s planned · {:.2}s observed", round.preview.bundle_id, round.preview.quality.plan_duration_ms as f64 / 1000.0, round.preview.quality.observed_duration_ms as f64 / 1000.0));
                                for check in &round.preview.quality.checks {
                                    ui.label(format!("{} · {} · {}", if check.passed { "PASS" } else { "CHECK" }, check.name, check.detail));
                                }
                                if let Some(review) = &round.review {
                                    let real_review = round.model_call_id.as_ref().is_some_and(|id| task.model_calls.iter().any(|call| {
                                        &call.call_id == id && call.backend == "openai_responses"
                                            && call.prompt_version == video_agent::review::PROMPT_VERSION
                                            && call.provider_response_id.is_some()
                                    }));
                                    if !real_review {
                                        ui.colored_label(WARNING, "SIMULATED / NON-PRODUCTION REVIEW · This is not evidence of an actual model assessment.");
                                    }
                                    ui.label(RichText::new("Model review of sparse images — not a human score").color(WARNING));
                                    let verdict = match review.verdict {
                                        video_agent::review::ReviewVerdict::Accept => "The sampled images support this cut; human review still required",
                                        video_agent::review::ReviewVerdict::Revise => "The model suggests a bounded replacement cut",
                                        video_agent::review::ReviewVerdict::Uncertain => "The sampled images are insufficient for a reliable decision",
                                    };
                                    ui.label(RichText::new(verdict).strong());
                                    ui.label(&review.summary);
                                    for issue in &review.issues {
                                        ui.label(format!("{} · {}: {}", issue.clip_id, issue.kind, issue.detail));
                                        ui.label(RichText::new(format!("Observed evidence: {}", issue.evidence_ids.join(", "))).small().color(MUTED));
                                    }
                                    if !review.revised_clips.is_empty() {
                                        ui.label("Suggested source intervals (not yet adopted):");
                                        for clip in &review.revised_clips {
                                            ui.label(format!("{} · {} → {} · {}", clip.source_id, timecode(clip.start_ms), timecode(clip.end_ms), clip.reason));
                                            ui.label(RichText::new(format!("Uncertainty: {}", clip.uncertainty)).small().color(WARNING));
                                        }
                                    }
                                } else {
                                    ui.label("No persisted model review for this round. A preview alone is not a semantic assessment.");
                                }
                            });
                    }
                    if report.selected_plan.is_some() {
                        if current_matches {
                            ui.colored_label(ACCENT, "A reviewed candidate is available. Adoption is optional and does not confirm it for export.");
                        } else {
                            ui.colored_label(MUTED, "Historical selection only. This run cannot be adopted against the current saved plan.");
                        }
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui.add_enabled(self.can_run_task() && current_matches && !report.rounds.is_empty(), egui::Button::new("Re-check & play latest AI preview")).clicked() {
                            if let Some(path) = self.task_dir.clone() {
                                self.start_job("Rechecking and playing saved AI preview", move |cancel| {
                                    let child = video_agent::smart_feedback::spawn_preview(&path, &cancel)
                                        .map_err(|_| "AI preview could not be safely reverified or played. Check the task budget and local ffplay installation.".to_owned())?;
                                    Ok(JobResult::Player(child, "Playing a freshly checked what-if preview. This is not a final export or human approval.".into()))
                                });
                            }
                        }
                        if ui.add_enabled(self.can_run_task() && !self.dirty && current_matches && report.selected_plan.is_some(), egui::Button::new("Adopt reviewed candidate · still needs approval")).clicked() {
                            if let Some(path) = self.task_dir.clone() {
                                let run_id = report.run_id.clone();
                                self.start_job("Explicitly adopting reviewed AI candidate, without approval", move |cancel| {
                                    let task = video_agent::smart_feedback::adopt(&path, &run_id, &cancel)
                                        .map_err(|_| "Candidate was not adopted. Reload the saved plan and feedback; source/run/plan identity must still match.".to_owned())?;
                                    let report = video_agent::smart_feedback::status(&path)
                                        .map_err(|_| "Candidate was saved, but feedback status could not be reloaded. Reopen the task to inspect its current revision.".to_owned())?;
                                    Ok(JobResult::SmartFeedback { path, task: Box::new(task), report: report.map(Box::new), explicitly_adopted: true })
                                });
                            }
                        }
                    });
                    ui.label(RichText::new("Saved status is historical. Playback re-checks the core-owned preview; no model-provided path is opened. Adoption creates a new revision and revokes old approval; review and confirm it separately.").small().color(MUTED));
                } else {
                    ui.label(RichText::new("No feedback status loaded. Loading is local and does not send images or run a model.").small().color(MUTED));
                }
                self.smart_report = report;
            });
    }

    fn install_fonts(&mut self, ctx: &egui::Context) {
        if self.fonts_installed {
            return;
        }
        self.fonts_installed = true;
        let candidates = [
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/STHeiti Light.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        ];
        for path in candidates {
            if let Ok(bytes) = std::fs::read(path) {
                let mut fonts = egui::FontDefinitions::default();
                fonts
                    .font_data
                    .insert("local-cjk".into(), egui::FontData::from_owned(bytes));
                fonts
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .push("local-cjk".into());
                ctx.set_fonts(fonts);
                break;
            }
        }
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = Color32::from_rgb(19, 24, 33);
        style.visuals.window_fill = Color32::from_rgb(25, 31, 42);
        style.visuals.selection.bg_fill = Color32::from_rgb(35, 91, 83);
        style.visuals.hyperlink_color = ACCENT;
        ctx.set_style(style);
    }

    fn header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("video_agent_header").show(ctx, |ui| {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("VIDEO AGENT")
                        .size(23.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.label(RichText::new("Observe → choose → review → deliver").color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let configured = self.provider_ready;
                    ui.label(
                        RichText::new(if configured {
                            "OpenAI · configured"
                        } else {
                            "OpenAI · key not configured"
                        })
                        .small()
                        .color(if configured { ACCENT } else { WARNING }),
                    );
                });
            });
            ui.label(
                RichText::new(format!(
                    "Requested model: {} · OpenAI Responses · sparse source/preview JPEGs, not video/audio",
                    self.provider_model
                ))
                .small()
                .color(MUTED),
            );
            if matches!(self.provider_model.as_str(), video_agent::vision::LEGACY_MODEL | "gpt-4.1-mini") {
                ui.label(RichText::new("Legacy snapshot is deprecated; scheduled shutdown 2027-01-14. Configure and validate a replacement before then.").small().color(WARNING));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (page, label) in [
                    (Page::Sources, "1  Sources & intent"),
                    (Page::Observe, "2  Observed frames"),
                    (Page::Edit, "3  Human review"),
                    (Page::Result, "4  Verified output"),
                ] {
                    let available = page == Page::Sources || self.task.is_some();
                    if ui
                        .add_enabled(
                            available,
                            egui::SelectableLabel::new(self.page == page, label),
                        )
                        .clicked()
                    {
                        self.page = page;
                    }
                }
            });
            ui.add_space(7.0);
        });
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("video_agent_context").resizable(false).default_width(245.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(12.0);
                ui.label(RichText::new("LOCAL WORKSPACE").small().strong().color(MUTED));
                if let Some(task) = self.task.as_ref() {
                    ui.label(RichText::new(task_status_label(task)).strong().color(if task.status == TaskStatus::Stale || !has_execution_budget(&task.usage, &task.limits) { ERROR } else { ACCENT }));
                    ui.label(RichText::new(&task.intent).size(15.0));
                    ui.label(format!("Target: {:.1}s · {} source(s)", task.target_duration_ms as f64 / 1000.0, task.assets.len()));
                    ui.add_space(10.0);
                    ui.label(RichText::new("BUDGET ACCOUNTING / LIMIT").small().strong().color(MUTED));
                    ui.label(format!("Frame credits {} / {}", task.usage.frames_created, task.limits.max_frames))
                        .on_hover_text("Includes observation records and charged human-only preview attempts. Cancelled previews can remain charged; this is not the number of JPEGs sent to a model.");
                    ui.label(format!("Model attempts  {} / {}", task.usage.model_calls, task.limits.max_model_calls));
                    ui.label(format!("Tokens       {} / {}", task.usage.input_tokens + task.usage.output_tokens, task.limits.max_total_tokens));
                    ui.label(format!("Accounted    {:.1}s / {}s", task.usage.elapsed_ms as f64 / 1000.0, task.limits.max_execution_secs));
                    ui.label(RichText::new(execution_usage_summary(&task.usage)).small().color(MUTED));
                    ui.label(RichText::new(frame_worker_usage_summary(&task.usage, task.limits.max_concurrency)).small().color(MUTED));
                    ui.label(format!("Est. cost    ${:.4} / ${:.2}", task.usage.estimated_cost_usd, task.limits.max_estimated_cost_usd));
                    ui.label(RichText::new("Attempts include locally stopped requests. Tokens/cost retain unknown reservations after interruption; not-sent records refund them. Accounted tokens are not necessarily provider-reported usage.").small().color(MUTED));
                    let reported_input: u64 = task.model_calls.iter().filter(|call| call.provider_response_id.is_some()).map(|call| call.input_tokens).sum();
                    let reported_output: u64 = task.model_calls.iter().filter(|call| call.provider_response_id.is_some()).map(|call| call.output_tokens).sum();
                    ui.label(format!("Provider-reported: {reported_input} input / {reported_output} output tokens"));
                    ui.label(RichText::new("Cost uses configured price estimates, not an invoice or hard billing cap. Set provider account spend limits too.").small().color(MUTED));
                    ui.add_space(10.0);
                    for asset in &task.assets {
                        ui.separator();
                        ui.label(RichText::new(file_name(&asset.path)).strong());
                        ui.label(format!("{} · {}×{}", timecode(asset.duration_ms), asset.width, asset.height));
                        ui.label(if asset.has_audio { "Source audio present" } else { "Source has no audio" });
                        ui.label(RichText::new(format!("SHA-256 {}…", asset.sha256.chars().take(12).collect::<String>())).small().color(MUTED));
                    }
                } else {
                    ui.label("No task opened");
                    ui.label(RichText::new("Source videos stay read-only. Task data and finished media are saved separately.").color(MUTED));
                }
                ui.add_space(18.0);
                ui.separator();
                ui.label(RichText::new("RECOVERY").small().strong().color(MUTED));
                ui.label("Open an existing task folder:");
                ui.add(egui::TextEdit::singleline(&mut self.resume_path).hint_text("/path/to/task-folder").desired_width(f32::INFINITY));
                ui.add_enabled_ui(self.job.is_none(), |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Browse…").clicked() {
                            if let Some(path) = rfd::FileDialog::new().set_title("Choose saved Video Agent task").pick_folder() {
                                self.resume_path = path.display().to_string();
                            }
                        }
                        if ui.add_enabled(!self.resume_path.trim().is_empty(), egui::Button::new("Open task")).clicked() {
                            if self.dirty {
                                self.error = Some("Save or discard your unsaved plan edits before opening another task.".into());
                            } else {
                                self.open_workspace(PathBuf::from(self.resume_path.trim()));
                            }
                        }
                    });
                    if self.task_dir.is_some() && ui.button("Reload saved state").clicked() {
                        if self.dirty {
                            self.error = Some("Save or discard unsaved plan edits first.".into());
                        } else if let Some(path) = self.task_dir.clone() {
                            self.open_workspace(path);
                        }
                    }
                });
                ui.add_space(8.0);
                ui.label(RichText::new("Resume reuses completed sampling and export shards, not an arbitrary encoder position.").small().color(MUTED));
                ui.add_space(14.0);
                ui.label(RichText::new("PLATFORM").small().strong().color(MUTED));
                ui.label(RichText::new("macOS Apple Silicon is the first validation target. ffmpeg, ffprobe and ffplay must be available in PATH.").small().color(MUTED));
            });
        });
    }

    fn sources_page(&mut self, ui: &mut egui::Ui) {
        title(
            ui,
            "Start with footage, not a chat",
            "A bounded visual selection workflow with a real edit plan and a real media export.",
        );
        if self.task.is_some() {
            ui.label(RichText::new("This task's inputs and intent are immutable. Create a new task below to use different footage or a different request. The existing task remains on disk.").color(WARNING));
            ui.add_space(8.0);
        }
        ui.label(RichText::new("1  Choose local videos").strong());
        ui.horizontal(|ui| {
            if ui.button("Choose video files…").clicked() {
                if let Some(paths) = rfd::FileDialog::new()
                    .add_filter("Video", &["mp4", "mov", "mkv", "webm", "m4v", "avi"])
                    .pick_files()
                {
                    for path in paths {
                        if !self.inputs.contains(&path) {
                            self.inputs.push(path);
                        }
                    }
                }
            }
            ui.label(
                RichText::new("One source first; up to 3 files. Originals are never modified.")
                    .small()
                    .color(MUTED),
            );
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.input_text)
                    .hint_text("Or paste an absolute video path")
                    .desired_width(460.0),
            );
            if ui
                .add_enabled(
                    !self.input_text.trim().is_empty(),
                    egui::Button::new("Add path"),
                )
                .clicked()
            {
                let path = PathBuf::from(self.input_text.trim());
                if !self.inputs.contains(&path) {
                    self.inputs.push(path);
                }
                self.input_text.clear();
            }
        });
        let mut remove = None;
        for (index, path) in self.inputs.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{}  {}", index + 1, path.display()));
                if ui.small_button("Remove").clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            self.inputs.remove(index);
        }
        ui.add_space(12.0);
        ui.label(RichText::new("2  Describe the selection").strong());
        ui.add(egui::TextEdit::multiline(&mut self.intent).desired_rows(3).desired_width(f32::INFINITY)
            .hint_text("Example: Find ocean views and city night scenes. Suggest a roughly 30-second rough cut; let me review the shots before export."));
        ui.horizontal(|ui| {
            ui.label("Target length (seconds)");
            ui.add(egui::DragValue::new(&mut self.target_seconds).clamp_range(1..=120));
        });
        ui.add_space(12.0);
        ui.label(RichText::new("3  Save task data outside the source").strong());
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.parent)
                    .hint_text("Choose a private workspace parent folder")
                    .desired_width(480.0),
            );
            if ui.button("Choose folder…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Task data parent directory")
                    .pick_folder()
                {
                    self.parent = path.display().to_string();
                }
            }
        });
        ui.label(RichText::new("A unique task subfolder stores source fingerprints, sampled JPEGs, decisions, manual edits and verified outputs. It may contain private data: do not commit it.").small().color(MUTED));
        ui.add_space(12.0);
        egui::CollapsingHeader::new("Observation and execution budgets").show(ui, |ui| {
            ui.label(RichText::new("Saved when creating a new task. These controls do not change an already opened task's limits.").small().color(MUTED));
            egui::Grid::new("agent_budget_grid").num_columns(2).spacing([18.0, 8.0]).show(ui, |ui| {
                budget_row(ui, "Initial frames (total)", &mut self.limits.initial_frames, 1..=60);
                budget_row(ui, "Maximum frames (total)", &mut self.limits.max_frames, 1..=120);
                budget_row(ui, "Maximum model calls", &mut self.limits.max_model_calls, 1..=12);
                budget_row(ui, "Output tokens / call", &mut self.limits.max_output_tokens, 256..=8000);
                budget_row_u64(ui, "Total token budget", &mut self.limits.max_total_tokens, 1000..=500_000);
                ui.label("Estimated cost cap (USD)");
                ui.add(egui::DragValue::new(&mut self.limits.max_estimated_cost_usd).speed(0.01).clamp_range(0.01..=20.0)); ui.end_row();
                budget_row_u64(ui, "Execution budget (seconds)", &mut self.limits.max_execution_secs, 1..=3600);
                budget_row_u64(ui, "Process timeout (seconds)", &mut self.limits.process_timeout_secs, 1..=600);
                budget_row_u64(ui, "Model timeout (seconds)", &mut self.limits.model_timeout_secs, 1..=180);
                ui.label("Simultaneous frame extractions");
                ui.add(egui::DragValue::new(&mut self.limits.max_concurrency).clamp_range(1..=4)); ui.end_row();
            });
            ui.label(RichText::new("The worker limit controls local storyboard and follow-up frame extraction. Model requests, confirmation and export stay sequential. Sparse samples do not establish full video, event or audio understanding.").small().color(MUTED));
        });
        ui.add_space(12.0);
        let limits_error = video_agent::store::validate_limits(&self.limits).err();
        if let Some(error) = &limits_error {
            ui.label(RichText::new(format!("Adjust the budgets before creating a task: {error}. Each timeout must fit the execution budget; initial frames must fit the frame cap.")).color(ERROR));
        }
        let valid = !self.inputs.is_empty()
            && self.inputs.len() <= 3
            && !self.intent.trim().is_empty()
            && !self.parent.trim().is_empty()
            && limits_error.is_none();
        if ui
            .add_enabled(
                valid && self.job.is_none() && !self.dirty,
                egui::Button::new("Create task & inspect sources")
                    .fill(Color32::from_rgb(31, 91, 80)),
            )
            .clicked()
        {
            let parent = PathBuf::from(self.parent.trim());
            let inputs = self.inputs.clone();
            let intent = self.intent.trim().to_string();
            let duration = self.target_seconds * 1000;
            let limits = self.limits.clone();
            self.start_job(
                "Fingerprinting and inspecting local sources",
                move |cancel| {
                    cancel.check().map_err(|e| e.to_string())?;
                    let workspace = Workspace::create_with_cancel(
                        &parent, &inputs, intent, duration, limits, &cancel,
                    )
                    .map_err(|e| e.to_string())?;
                    let task =
                        Workspace::snapshot(workspace.task_dir()).map_err(|e| e.to_string())?;
                    Ok(JobResult::Task(
                        workspace.task_dir().to_path_buf(),
                        Box::new(task),
                    ))
                },
            );
        }
        if self.inputs.len() > 3 {
            ui.colored_label(ERROR, "This release accepts at most 3 source videos.");
        }
    }

    fn observe_page(&mut self, ui: &mut egui::Ui) {
        let Some(task) = self.task.clone() else {
            return;
        };
        title(ui, "Observed frames, explicit evidence", "All saved observations remain here. Each model request receives at most 16 time-distributed samples across observed sources; its history lists the exact image IDs sent. Unsent frames are not evidence for that request.");
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.can_run_task() && task.status != TaskStatus::Stale,
                    egui::Button::new(if task.samples.is_empty() {
                        "Generate initial storyboard"
                    } else {
                        "Resume / reuse sampling"
                    }),
                )
                .clicked()
            {
                self.workspace_job("Sampling source frames locally", |workspace, cancel| {
                    workspace.sample(&cancel)
                });
            }
            ui.label(format!(
                "{} sampled frame(s) · maximum {}",
                task.samples.len(),
                task.limits.max_frames
            ));
        });
        ui.add_space(8.0);
        card(ui, |ui| {
            ui.label(RichText::new("Cloud boundary").strong().color(ACCENT));
            ui.label("OpenAI receives your request, opaque source IDs, source durations, sample timestamps and the sampled JPEG images. It may request additional samples within this task's frame and call caps. Original video files, source audio and local paths are not uploaded.");
            let changed = ui
                .add_enabled(
                    self.can_run_task(),
                    egui::Checkbox::new(
                        &mut self.consent,
                        "I am allowed to share these sampled images and this request with OpenAI.",
                    ),
                )
                .changed();
            if changed {
                let consent = self.consent;
                self.workspace_job("Saving cloud consent", move |workspace, cancel| {
                    workspace.set_consent_with_cancel(consent, &cancel)
                });
            }
            if let Some(error) = self.provider_error.as_ref() {
                ui.label(
                    RichText::new(format!("Vision configuration error: {error}")).color(ERROR),
                );
            } else if !self.provider_ready {
                ui.label(RichText::new("New vision requests are unavailable: no local API credential is configured. Use OPENAI_API_KEY, VIDEO_AGENT_API_KEY or a private VIDEO_AGENT_CONFIG file outside Git. Never paste a key into this app, task files or the repository.").color(WARNING));
            }
            if ui
                .add_enabled(
                    self.job.is_none(),
                    egui::Button::new("Reload local model configuration"),
                )
                .clicked()
            {
                self.refresh_provider();
            }
            let saved_decision = task
                .model_calls
                .iter()
                .any(|call| call.status == "received" && call.decision.is_some());
            let enabled = self.can_run_task()
                && self.provider_error.is_none()
                && (self.provider_ready || saved_decision)
                && task.cloud_consent
                && !task.samples.is_empty()
                && task.status != TaskStatus::Stale
                && !self.dirty;
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::new(if saved_decision {
                        "Resume saved model decision"
                    } else {
                        "Ask vision model to observe & select"
                    })
                    .fill(Color32::from_rgb(31, 91, 80)),
                )
                .clicked()
            {
                self.workspace_job(
                    if saved_decision {
                        "Resuming the persisted model decision"
                    } else {
                        "Real OpenAI vision selection (bounded observations)"
                    },
                    |workspace, cancel| {
                        let limits = workspace.load_with_cancel(&cancel)?.limits;
                        let provider = OpenAiVision::from_env(&limits)?;
                        workspace.analyze(&provider, &cancel)
                    },
                );
            }
            if saved_decision {
                ui.label(RichText::new("A previously received decision can resume without another model call. Further requests still require credentials and stay within the remaining budgets.").small().color(MUTED));
            }
            ui.label(RichText::new("Model decisions are proposals, not exports. No model can execute shell commands or approve an edit plan.").small().color(MUTED));
        });
        ui.add_space(10.0);
        if task.samples.is_empty() {
            ui.label(RichText::new("No frames have been observed yet. Generate the storyboard before requesting visual selection.").color(MUTED));
        } else {
            let columns = ((ui.available_width() / 235.0).floor() as usize).clamp(1, 4);
            egui::Grid::new("agent_storyboard")
                .num_columns(columns)
                .spacing([12.0, 16.0])
                .show(ui, |ui| {
                    for (index, sample) in task.samples.iter().enumerate() {
                        ui.vertical(|ui| {
                            ui.set_width(216.0);
                            self.image(ui, &sample.image_path, egui::vec2(216.0, 128.0));
                            let source = task.assets.iter().find(|a| a.id == sample.source_id);
                            ui.label(
                                RichText::new(format!(
                                    "{} · {}",
                                    source
                                        .map(|a| file_name(&a.path))
                                        .unwrap_or_else(|| sample.source_id.clone()),
                                    timecode(sample.time_ms)
                                ))
                                .small()
                                .strong(),
                            );
                            ui.label(
                                RichText::new(format!("Evidence {}", sample.id))
                                    .small()
                                    .color(MUTED),
                            );
                        });
                        if (index + 1) % columns == 0 {
                            ui.end_row();
                        }
                    }
                });
        }
        ui.add_space(14.0);
        egui::CollapsingHeader::new("Manual workflow (not AI selection)").show(ui, |ui| {
            ui.label("You may create and adjust a manual draft to validate preview, approval, export and recovery without credentials. This does not validate visual understanding and is never labelled as model output.");
            if ui.add_enabled(self.can_run_task() && !task.samples.is_empty() && task.status != TaskStatus::Stale && !self.dirty, egui::Button::new("Create manual draft — no AI")).clicked() {
                self.workspace_job("Creating explicitly manual draft", |workspace, cancel| workspace.manual_draft_with_cancel(&cancel));
            }
        });
        self.audit_trail(ui, &task);
    }

    fn edit_page(&mut self, ui: &mut egui::Ui) {
        let Some(task) = self.task.clone() else {
            return;
        };
        title(ui, "Your cut. Your decision.", "Play the original intervals, adjust boundaries and order, then confirm this exact saved revision.");
        let Some(mut plan) = self.draft.clone() else {
            ui.label("There is no edit plan yet. Generate observations and ask the model to select, or explicitly create a manual draft.");
            if ui.button("Go to observed frames").clicked() {
                self.page = Page::Observe;
            }
            return;
        };
        let model_proposal = plan_has_model_origin(&task);
        if model_proposal {
            ui.label(
                RichText::new(
                    "Origin: real model proposal. Human modifications are labelled per clip.",
                )
                .color(ACCENT),
            );
        } else if video_agent::smart_feedback::is_adopted_model_plan(&task) {
            ui.label(
                RichText::new(
                    "Origin: simulated review provider — not a real model. No semantic-quality claim is established.",
                )
                .color(WARNING),
            );
        } else {
            ui.label(
                RichText::new(
                    "MANUAL / UNVERIFIED DRAFT · No real AI semantic selection has been verified for this plan.",
                )
                .color(WARNING),
            );
        }
        ui.label(&plan.summary);
        let total = plan
            .clips
            .iter()
            .map(|c| c.end_ms.saturating_sub(c.start_ms))
            .sum::<u64>();
        ui.label(format!(
            "Revision {} · {} clips · {:.2}s selected / {:.1}s requested",
            plan.revision,
            plan.clips.len(),
            total as f64 / 1000.0,
            task.target_duration_ms as f64 / 1000.0
        ));
        ui.label(RichText::new(format!("Per-clip cap: {:.1}s · total output cap: {:.1}s · boundaries are revalidated before export", task.limits.max_clip_ms as f64 / 1000.0, task.limits.max_output_ms as f64 / 1000.0)).small().color(MUTED));
        ui.add_space(8.0);
        let mut changed = false;
        let mut delete = None;
        let mut swap = None;
        let mut play = None;
        for index in 0..plan.clips.len() {
            let clip = &mut plan.clips[index];
            let source = task.assets.iter().find(|asset| asset.id == clip.source_id);
            ui.push_id(clip.id.clone(), |ui| {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{:02}   {}",
                                index + 1,
                                source
                                    .map(|a| file_name(&a.path))
                                    .unwrap_or_else(|| clip.source_id.clone())
                            ))
                            .strong()
                            .size(16.0),
                        );
                        if clip.human_modified {
                            ui.label(RichText::new("HUMAN EDIT").small().color(WARNING));
                        }
                        if ui
                            .add_enabled(self.can_run_task(), egui::Button::new("Play interval"))
                            .clicked()
                        {
                            play = Some(clip.clone());
                        }
                    });
                    ui.horizontal(|ui| {
                        if let Some(sample) = clip
                            .evidence_ids
                            .iter()
                            .find_map(|id| task.samples.iter().find(|s| s.id == *id))
                        {
                            self.image(ui, &sample.image_path, egui::vec2(190.0, 110.0));
                        }
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new("Selection reason")
                                    .small()
                                    .strong()
                                    .color(MUTED),
                            );
                            ui.label(&clip.reason);
                            ui.label(
                                RichText::new(format!("Uncertainty: {}", clip.uncertainty))
                                    .small()
                                    .color(WARNING),
                            );
                            let evidence = clip
                                .evidence_ids
                                .iter()
                                .map(|id| {
                                    task.samples
                                        .iter()
                                        .find(|s| s.id == *id)
                                        .map(|s| format!("{id} @ {}", timecode(s.time_ms)))
                                        .unwrap_or_else(|| format!("{id} (missing)"))
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            ui.label(
                                RichText::new(format!("Observed evidence: {evidence}"))
                                    .small()
                                    .color(MUTED),
                            );
                        });
                    });
                    ui.add_enabled_ui(self.can_run_task(), |ui| {
                        ui.horizontal(|ui| {
                            let mut start = clip.start_ms as f64 / 1000.0;
                            let mut end = clip.end_ms as f64 / 1000.0;
                            let max = source.map(|s| s.duration_ms as f64 / 1000.0).unwrap_or(end);
                            ui.label("In (s)");
                            let start_changed = ui
                                .add(
                                    egui::DragValue::new(&mut start)
                                        .speed(0.1)
                                        .max_decimals(3)
                                        .clamp_range(0.0..=max),
                                )
                                .changed();
                            ui.label("Out (s)");
                            let end_changed = ui
                                .add(
                                    egui::DragValue::new(&mut end)
                                        .speed(0.1)
                                        .max_decimals(3)
                                        .clamp_range(0.0..=max),
                                )
                                .changed();
                            if start_changed || end_changed {
                                if let (Some(start), Some(end)) =
                                    (seconds_to_ms(start), seconds_to_ms(end))
                                {
                                    clip.start_ms = start;
                                    clip.end_ms = end;
                                    clip.human_modified = true;
                                    changed = true;
                                }
                            }
                            if clip.end_ms <= clip.start_ms {
                                ui.colored_label(ERROR, "Out must follow in");
                            }
                            ui.label(format!(
                                "{} → {}",
                                timecode(clip.start_ms),
                                timecode(clip.end_ms)
                            ));
                        });
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(index > 0, egui::Button::new("Move earlier"))
                                .clicked()
                            {
                                swap = Some((index, index - 1));
                            }
                            if ui
                                .add_enabled(
                                    index + 1 < task_plan_len(&self.draft),
                                    egui::Button::new("Move later"),
                                )
                                .clicked()
                            {
                                swap = Some((index, index + 1));
                            }
                            if ui.button("Reject / delete clip").clicked() {
                                delete = Some(index);
                            }
                        });
                    });
                });
            });
            ui.add_space(8.0);
        }
        if let Some(index) = delete {
            plan.clips.remove(index);
            changed = true;
        }
        if let Some((a, b)) = swap {
            plan.clips.swap(a, b);
            plan.clips[a].human_modified = true;
            plan.clips[b].human_modified = true;
            changed = true;
        }
        if let Some(clip) = play {
            self.play_clip(clip);
        }
        ui.add_space(4.0);
        ui.add_enabled_ui(self.can_run_task(), |ui| {
            ui.horizontal(|ui| {
                ui.label("Output audio");
                changed |= ui
                    .radio_value(
                        &mut plan.audio_policy,
                        AudioPolicy::PreserveWithSilence,
                        "Keep source audio / silence for silent sources",
                    )
                    .changed();
                changed |= ui
                    .radio_value(&mut plan.audio_policy, AudioPolicy::Mute, "Mute")
                    .changed();
            });
        });
        ui.label(RichText::new("Audio is not analyzed by the vision model. Export normalizes to AAC stereo when keeping audio and inserts silence where a source has none. Playback previews use each source's original audio.").small().color(MUTED));
        self.source_preview(ui, &task, &mut plan, &mut changed);
        if changed {
            self.dirty = true;
        }
        self.draft = Some(plan.clone());
        ui.add_space(12.0);
        card(ui, |ui| {
            if self.dirty {
                ui.label(
                    RichText::new(
                        "UNSAVED EDITS · Previous confirmation is not valid for this draft.",
                    )
                    .color(WARNING),
                );
            } else if let Some(approval) = task
                .approval
                .as_ref()
                .filter(|_| approval_matches_saved_plan(&task))
            {
                ui.label(
                    RichText::new(format!(
                        "Confirmed revision {} · digest {}…",
                        approval.revision,
                        approval.plan_digest.chars().take(16).collect::<String>()
                    ))
                    .color(ACCENT),
                );
            } else {
                ui.label(
                    RichText::new(
                        "Not confirmed. Export is locked until you approve the saved plan.",
                    )
                    .color(WARNING),
                );
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.can_run_task() && self.dirty,
                        egui::Button::new("Save edits · revoke old confirmation"),
                    )
                    .clicked()
                {
                    self.workspace_job(
                        "Validating and saving human edits",
                        move |workspace, cancel| workspace.save_plan_with_cancel(plan, &cancel),
                    );
                }
                if ui
                    .add_enabled(
                        self.job.is_none() && self.dirty,
                        egui::Button::new("Discard unsaved edits"),
                    )
                    .clicked()
                {
                    self.draft = task.plan.clone();
                    self.dirty = false;
                }
                let approvable = self.can_run_task()
                    && !self.dirty
                    && !task.plan.as_ref().is_none_or(|p| p.clips.is_empty())
                    && task.status != TaskStatus::Stale;
                if ui
                    .add_enabled(
                        approvable,
                        egui::Button::new("Confirm this saved edit plan"),
                    )
                    .clicked()
                {
                    match self
                        .draft
                        .as_ref()
                        .map(|plan| video_agent::workflow::plan_digest(&task, plan))
                    {
                        Some(Ok(expected_digest)) => self.workspace_job(
                            "Binding approval to the exact edit plan",
                            move |workspace, cancel| {
                                workspace.approve_with_cancel(&expected_digest, &cancel)
                            },
                        ),
                        Some(Err(error)) => self.error = Some(error.to_string()),
                        None => self.error = Some("No rendered plan to confirm.".into()),
                    }
                }
            });
            ui.add_space(4.0);
            let approved = approval_matches_saved_plan(&task);
            if ui
                .add_enabled(
                    self.can_run_task()
                        && !self.dirty
                        && approved
                        && task.status != TaskStatus::Stale,
                    egui::Button::new("Export confirmed cut").fill(Color32::from_rgb(31, 91, 80)),
                )
                .clicked()
            {
                self.workspace_job(
                    "Exporting validated media · completed shards are recoverable",
                    |workspace, cancel| workspace.export(&cancel),
                );
            }
            ui.label(RichText::new("Export writes a separate MP4, checks duration/streams and decodes it before reporting success. Original media is read-only; existing output is not silently overwritten.").small().color(MUTED));
        });
        self.audit_trail(ui, &task);
    }

    fn source_preview(
        &mut self,
        ui: &mut egui::Ui,
        task: &Task,
        plan: &mut EditPlan,
        changed: &mut bool,
    ) {
        egui::CollapsingHeader::new("Inspect another source moment / add a human-selected clip").show(ui, |ui| {
            if task.assets.is_empty() { return; }
            self.source_index = self.source_index.min(task.assets.len() - 1);
            egui::ComboBox::from_id_source("agent_preview_source").selected_text(file_name(&task.assets[self.source_index].path)).show_ui(ui, |ui| {
                for (index, asset) in task.assets.iter().enumerate() {
                    if ui.selectable_value(&mut self.source_index, index, file_name(&asset.path)).changed() { self.preview_path = None; self.preview_seconds = 0.0; }
                }
            });
            let source = &task.assets[self.source_index];
            let max = source.duration_ms.saturating_sub(1) as f64 / 1000.0;
            if ui.add(egui::Slider::new(&mut self.preview_seconds, 0.0..=max).text("Source time (s)").max_decimals(3)).changed() { self.preview_path = None; }
            ui.horizontal(|ui| {
                if ui.add_enabled(self.can_run_task(), egui::Button::new("Show local frame")).clicked() {
                    if let Some(path) = self.task_dir.clone() {
                        let id = source.id.clone();
                        let time = seconds_to_ms(self.preview_seconds).unwrap_or(0);
                        self.start_job("Decoding a local human-preview frame", move |cancel| {
                            let workspace = Workspace::open_with_cancel(&path, &cancel).map_err(|e| e.to_string())?;
                            let path = workspace.preview_frame(&id, time, &cancel).map_err(|e| e.to_string())?;
                            let task = workspace.load_with_cancel(&cancel).map_err(|e| e.to_string())?;
                            Ok(JobResult::Preview { path, source_id: id, time_ms: time, task: Box::new(task) })
                        });
                    }
                }
                if ui.add_enabled(self.can_run_task() && source.duration_ms >= 100, egui::Button::new("Add up to 3s human clip here")).clicked() {
                    let start = seconds_to_ms(self.preview_seconds).unwrap_or(0).min(source.duration_ms.saturating_sub(100));
                    let end = (start + 3000.min(task.limits.max_clip_ms)).min(source.duration_ms);
                    let clip_id = unique_human_clip_id(plan);
                    plan.clips.push(Clip {
                        id: clip_id,
                        source_id: source.id.clone(), start_ms: start, end_ms: end,
                        reason: "MANUAL: selected by the user; not a model recommendation.".into(),
                        uncertainty: "Human-selected interval. No claim of AI observation outside recorded samples.".into(),
                        evidence_ids: Vec::new(), human_modified: true,
                    });
                    *changed = true;
                }
            });
            if let Some(path) = self.preview_path.clone() {
                self.image(ui, &path, egui::vec2(420.0, 236.0));
                ui.label(RichText::new(&self.preview_caption).small().color(MUTED));
            }
            ui.label(RichText::new("Human preview frames stay local and are not automatically added to model evidence. A human clip may extend beyond the model's observed points; it is explicitly marked as a human edit.").small().color(MUTED));
        });
    }

    fn result_page(&mut self, ui: &mut egui::Ui) {
        let Some(task) = self.task.clone() else {
            return;
        };
        title(
            ui,
            "A media artifact, not a success message",
            "Only verified, finalized output is presented as a deliverable.",
        );
        if let Some(export) = task
            .export
            .as_ref()
            .filter(|_| has_current_verified_export(&task))
        {
            card(ui, |ui| {
                ui.label(
                    RichText::new("VERIFIED MP4")
                        .strong()
                        .size(20.0)
                        .color(ACCENT),
                );
                ui.label(export.path.display().to_string());
                ui.label(format!(
                    "Duration: {:.3}s  ·  Audio: {}",
                    export.duration_ms as f64 / 1000.0,
                    if export.has_audio {
                        "AAC stereo retained / silence normalized"
                    } else {
                        "no audio stream (muted)"
                    }
                ));
                ui.label(
                    RichText::new(format!("SHA-256: {}", export.sha256))
                        .small()
                        .color(MUTED),
                );
                ui.label(
                    RichText::new(format!("Plan digest: {}", export.plan_digest))
                        .small()
                        .color(MUTED),
                );
                ui.label(
                    RichText::new(format!(
                        "Verified during export: {}",
                        export.completed_at.as_deref().unwrap_or("not finalized")
                    ))
                    .small()
                    .color(MUTED),
                );
                ui.horizontal(|ui| {
                    if ui.button("Play finished video").clicked() {
                        if let Err(error) = open_native(&export.path) {
                            self.error = Some(error);
                        }
                    }
                    if ui.button("Open output folder").clicked() {
                        if let Some(parent) = export.path.parent() {
                            if let Err(error) = open_native(parent) {
                                self.error = Some(error);
                            }
                        }
                    }
                    if ui.button("Review edit plan").clicked() {
                        self.page = Page::Edit;
                    }
                    if ui
                        .add_enabled(
                            self.can_run_task(),
                            egui::Button::new("Re-check saved output"),
                        )
                        .clicked()
                    {
                        self.workspace_job(
                            "Re-verifying saved output without duplicating it",
                            |workspace, cancel| workspace.export(&cancel),
                        );
                    }
                });
            });
            if !plan_has_model_origin(&task) {
                ui.add_space(8.0);
                let notice = if video_agent::smart_feedback::is_adopted_model_plan(&task) {
                    "Origin: simulated review provider — not a real model. This export is not evidence of real-model semantic quality."
                } else {
                    "No current real-model provenance is verified for this plan. Export checks establish the media workflow, not semantic quality."
                };
                ui.label(RichText::new(notice).color(WARNING));
            }
            ui.add_space(12.0);
            ui.label("A decodable file is not proof of correct selection. Evaluate relevance, omissions and reasons on the fixed task set separately.");
        } else {
            ui.label("No verified output yet. Review and confirm an edit plan, then export it.");
            if !task.shards.is_empty() {
                ui.label(format!("{} completed export shard(s) are persisted. Resume the confirmed export to reuse valid shards.", task.shards.len()));
            }
            if ui.button("Go to human review").clicked() {
                self.page = Page::Edit;
            }
        }
        if let Some(error) = task.last_error.as_ref() {
            card(ui, |ui| {
                ui.label(RichText::new(format!("Last persisted failure: {error}")).color(ERROR));
            });
        }
        self.audit_trail(ui, &task);
    }

    fn audit_trail(&mut self, ui: &mut egui::Ui, task: &Task) {
        ui.add_space(14.0);
        egui::CollapsingHeader::new("Recorded model calls and task history").show(ui, |ui| {
            if task.model_calls.is_empty() { ui.label("No model has been called. No semantic result is claimed."); }
            for call in &task.model_calls {
                card(ui, |ui| {
                    ui.label(RichText::new(format!("{} · {} · {}", call.call_id, call.backend, call.status)).strong());
                    ui.label(format!("Requested model: {} · Provider-reported model: {}", call.requested_model, if call.provider_response_id.is_some() { call.model.as_str() } else { "not received" }));
                    ui.label(format!("Prompt {} · observed {} samples · {:.2}s · {} input / {} output tokens · est. ${:.4}", call.prompt_version, call.sample_ids.len(), call.elapsed_ms as f64 / 1000.0, call.input_tokens, call.output_tokens, call.estimated_cost_usd));
                    ui.label(RichText::new(format!("Observed sample IDs: {}", call.sample_ids.join(", "))).small().color(MUTED));
                    if let Some(error) = call.error.as_ref() { ui.colored_label(ERROR, error); }
                    if let Some(decision) = call.decision.as_ref() {
                        ui.label(format!("Decision: {decision:?}"));
                    }
                });
            }
            for event in task.history.iter().rev().take(40) {
                ui.label(RichText::new(format!("{} · {} · {}", event.at, event.kind, event.detail)).small().color(MUTED));
            }
        });
    }

    fn image(&mut self, ui: &mut egui::Ui, path: &Path, maximum: egui::Vec2) {
        if !self.textures.contains_key(path) {
            // Bound cache and decoder allocation even when a local task has been hand-edited.
            if self.textures.len() >= 256 {
                self.textures.clear();
            }
            let loaded = read_thumbnail(path).map(|image| {
                let image = image.into_rgba8();
                let size = [image.width() as usize, image.height() as usize];
                ui.ctx().load_texture(
                    path.display().to_string(),
                    egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
                    egui::TextureOptions::LINEAR,
                )
            });
            self.textures.insert(path.to_path_buf(), loaded);
        }
        match self.textures.get(path) {
            Some(Ok(texture)) => {
                let original = texture.size_vec2();
                let ratio = (maximum.x / original.x).min(maximum.y / original.y);
                ui.add(egui::Image::new((texture.id(), original * ratio)).rounding(6.0));
            }
            Some(Err(error)) => {
                ui.colored_label(ERROR, error);
            }
            None => {}
        }
    }
}

impl eframe::App for VideoAgentApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.install_fonts(ctx);
        self.poll_job(ctx);
        self.header(ctx);
        egui::TopBottomPanel::bottom("video_agent_status").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if let Some(job) = self.job.as_ref() {
                    ui.spinner();
                    ui.label(format!("{} · {:.1}s", if job.cancel.is_cancelled() { "Cancellation requested; retaining completed stages" } else { &job.label }, job.started.elapsed().as_secs_f64()));
                    if ui.add_enabled(!job.cancel.is_cancelled(), egui::Button::new("Cancel operation")).clicked() {
                        job.cancel.cancel();
                        self.notice = "Cancellation requested. Waiting for the current bounded operation to stop; completed stages stay on disk.".into();
                    }
                } else {
                    ui.label(RichText::new(&self.notice).small().color(MUTED));
                }
                if self.player.is_some() && ui.button("Stop preview").clicked() { self.stop_player(); }
            });
            if let Some(path) = self.task_dir.as_ref() {
                ui.label(RichText::new(format!("Task data: {}", path.display())).small().color(MUTED));
            }
            ui.add_space(4.0);
        });
        self.sidebar(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().id_source(("video_agent_content", self.page, self.task.as_ref().map(|task| &task.id))).auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(12.0);
                if let Some(error) = self.error.clone() {
                    egui::Frame::none().fill(Color32::from_rgb(65, 34, 42)).rounding(8.0).inner_margin(12.0).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(error).color(ERROR));
                            if ui.small_button("Dismiss").clicked() { self.error = None; }
                        });
                    });
                    ui.add_space(10.0);
                }
                if self.task.as_ref().is_some_and(|task| task.status == TaskStatus::Stale) {
                    ui.colored_label(ERROR, "INPUT CHANGED · This task is stale. Create a new task from the changed source; old approval cannot authorize export.");
                    ui.add_space(10.0);
                }
                if let Some(task) = self.task.as_ref() {
                    if !has_execution_budget(&task.usage, &task.limits) {
                        ui.colored_label(ERROR, "EXECUTION BUDGET EXHAUSTED · Saved records remain readable, but further processing is locked. Reopening does not reverify sources or outputs.");
                        if let Some(error) = &task.last_error {
                            ui.label(RichText::new(format!("Saved task state: {error}")).small().color(WARNING));
                        }
                        ui.add_space(10.0);
                    }
                }
                match self.page {
                    Page::Sources => self.sources_page(ui),
                    Page::Observe => self.observe_page(ui),
                    Page::Edit => self.edit_page(ui),
                    Page::Result => self.result_page(ui),
                }
                if matches!(self.page, Page::Edit | Page::Result) {
                    self.smart_feedback_panel(ui);
                }
                ui.add_space(20.0);
            });
        });
    }
}

impl Drop for VideoAgentApp {
    fn drop(&mut self) {
        if let Some(mut job) = self.job.take() {
            job.cancel.cancel();
            // Let the cancellable process runner reap FFmpeg before the app process exits.
            // Closing the window must not leave a native encoder/player detached.
            if let Some(thread) = job.thread.take() {
                let deadline = Instant::now() + Duration::from_secs(2);
                while !thread.is_finished() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                if thread.is_finished() {
                    let _ = thread.join();
                }
            }
            while let Ok(result) = job.receiver.try_recv() {
                if let JobResult::Player(mut player, _) = result {
                    let _ = player.kill();
                    let _ = player.wait();
                }
            }
        }
        self.stop_player();
    }
}

fn title(ui: &mut egui::Ui, heading: &str, subtitle: &str) {
    ui.heading(RichText::new(heading).size(26.0));
    ui.label(RichText::new(subtitle).color(MUTED));
    ui.add_space(15.0);
}

fn card<R>(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::none()
        .fill(Color32::from_rgb(28, 35, 47))
        .rounding(9.0)
        .inner_margin(12.0)
        .show(ui, content)
        .inner
}

fn budget_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u32,
    range: std::ops::RangeInclusive<u32>,
) {
    ui.label(label);
    ui.add(egui::DragValue::new(value).clamp_range(range));
    ui.end_row();
}
fn budget_row_u64(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u64,
    range: std::ops::RangeInclusive<u64>,
) {
    ui.label(label);
    ui.add(egui::DragValue::new(value).clamp_range(range));
    ui.end_row();
}
fn task_plan_len(plan: &Option<EditPlan>) -> usize {
    plan.as_ref().map_or(0, |plan| plan.clips.len())
}
fn execution_usage_summary(usage: &video_agent::Usage) -> String {
    format!(
        "Measured {:.1}s · crash upper bounds {:.1}s (included). In-flight upper bound {:.1}s (reserved, not measured).",
        usage.elapsed_ms.saturating_sub(usage.conservative_elapsed_ms) as f64 / 1000.0,
        usage.conservative_elapsed_ms as f64 / 1000.0,
        usage.execution_reserved_ms as f64 / 1000.0,
    )
}
fn has_execution_budget(usage: &video_agent::Usage, limits: &Limits) -> bool {
    usage.elapsed_ms.saturating_add(5) <= limits.max_execution_secs.saturating_mul(1000)
}
fn smart_round_limit(
    task: &Task,
    report: Option<&video_agent::smart_feedback::SmartReport>,
) -> u32 {
    let current_digest = task
        .plan
        .as_ref()
        .and_then(|plan| video_agent::workflow::plan_digest(task, plan).ok());
    report
        .filter(|report| {
            report.matches_current_plan && current_digest.as_ref() == Some(&report.base_plan_digest)
        })
        .map(|report| report.max_rounds)
        .filter(|rounds| (1..=3).contains(rounds))
        .unwrap_or(2)
}
fn smart_run_allowed(task: &Task, dirty: bool, busy: bool, preview_consent: bool) -> bool {
    preview_consent
        && !dirty
        && !busy
        && task.status != TaskStatus::Stale
        && !task.samples.is_empty()
        && has_execution_budget(&task.usage, &task.limits)
        && task
            .plan
            .as_ref()
            .is_some_and(|plan| !plan.clips.is_empty())
}
fn task_status_label(task: &Task) -> &'static str {
    if task.status != TaskStatus::Stale && !has_execution_budget(&task.usage, &task.limits) {
        "Execution budget exhausted · saved records only"
    } else {
        status_label(task.status)
    }
}
fn frame_worker_usage_summary(usage: &video_agent::Usage, limit: usize) -> String {
    format!(
        "Frame workers: recorded peak {} / {} · returned-worker time {:.1}s (sum, not execution budget).",
        usage.peak_media_workers,
        limit,
        usage.media_worker_ms as f64 / 1000.0,
    )
}
fn unique_human_clip_id(plan: &EditPlan) -> String {
    let mut sequence = 1;
    loop {
        let candidate = format!("human-r{}-{sequence}", plan.revision);
        if !plan.clips.iter().any(|clip| clip.id == candidate) {
            return candidate;
        }
        sequence += 1;
    }
}
fn plan_has_model_origin(task: &Task) -> bool {
    if video_agent::smart_feedback::is_real_adopted_model_plan(task) {
        return true;
    }
    let model_is_latest = task
        .history
        .iter()
        .rev()
        .find(|event| event.kind == "manual_plan" || event.kind == "model_plan")
        .is_some_and(|event| event.kind == "model_plan");
    model_is_latest
        && task
            .model_calls
            .iter()
            .rev()
            .find(|call| {
                matches!(
                    call.decision,
                    Some(video_agent::VisionDecision::ProposePlan { .. })
                ) && call.status == "validated"
            })
            .is_some_and(|call| {
                call.backend == "openai_responses" && call.provider_response_id.is_some()
            })
}
fn has_current_verified_export(task: &Task) -> bool {
    task.status == TaskStatus::Completed
        && has_execution_budget(&task.usage, &task.limits)
        && task
            .plan
            .as_ref()
            .zip(task.export.as_ref())
            .is_some_and(|(plan, export)| {
                export.verified
                    && export.completed_at.is_some()
                    && export.path.is_file()
                    && video_agent::workflow::plan_digest(task, plan)
                        .is_ok_and(|digest| digest == export.plan_digest)
            })
}
fn approval_matches_saved_plan(task: &Task) -> bool {
    task.approval
        .as_ref()
        .zip(task.plan.as_ref())
        .is_some_and(|(approval, plan)| {
            approval.revision == plan.revision
                && video_agent::workflow::plan_digest(task, plan)
                    .is_ok_and(|digest| digest == approval.plan_digest)
        })
}
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}
fn read_thumbnail(path: &Path) -> std::result::Result<image::DynamicImage, String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| format!("Preview unavailable: {error}"))?;
    if !metadata.is_file() || metadata.len() > 1_048_576 {
        return Err("Preview rejected: expected a regular JPEG smaller than 1 MiB.".into());
    }
    let mut limits = image::io::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(16 * 1024 * 1024);
    let mut reader =
        image::io::Reader::open(path).map_err(|error| format!("Preview unavailable: {error}"))?;
    reader.set_format(image::ImageFormat::Jpeg);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|error| format!("Preview unavailable: {error}"))
}
fn timecode(milliseconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        milliseconds / 3_600_000,
        (milliseconds / 60_000) % 60,
        (milliseconds / 1000) % 60,
        milliseconds % 1000
    )
}
fn seconds_to_ms(seconds: f64) -> Option<u64> {
    if !seconds.is_finite() || seconds < 0.0 || seconds > (u64::MAX / 1000) as f64 {
        None
    } else {
        Some((seconds * 1000.0).round() as u64)
    }
}
fn status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Created => "Sources inspected",
        TaskStatus::Sampling => "Sampling in progress",
        TaskStatus::AwaitingConsent => "Ready for observation / consent",
        TaskStatus::Analyzing => "Model observation in progress",
        TaskStatus::Review => "Awaiting human review",
        TaskStatus::Exporting => "Export in progress / resumable",
        TaskStatus::Completed => "Verified output ready",
        TaskStatus::Cancelled => "Cancelled · retained stages can resume",
        TaskStatus::Failed => "Failed · inspect error and resume",
        TaskStatus::Stale => "Stale source · cannot export",
    }
}

fn open_native(path: &Path) -> std::result::Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = Command::new("xdg-open");
    command
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not open media/folder: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ai_feedback_resumes_the_saved_bound_only_for_the_same_plan() {
        let task = task_snapshot();
        assert_eq!(smart_round_limit(&task, None), 2);
        let mut report = video_agent::smart_feedback::SmartReport {
            schema: "unit-only".into(),
            run_id: "unit-only".into(),
            base_plan_digest: video_agent::workflow::plan_digest(
                &task,
                task.plan.as_ref().unwrap(),
            )
            .unwrap(),
            max_rounds: 3,
            rounds: vec![],
            phase: "unit-only".into(),
            stopped_reason: None,
            selected_plan: None,
            needs_human_review: true,
            task_plan_unchanged: true,
            matches_current_plan: true,
            semantic_evaluation: "unit-only-not-a-model-assessment".into(),
            verification_scope: "unit-only-not-media-verification".into(),
            events: vec![],
        };
        assert_eq!(smart_round_limit(&task, Some(&report)), 3);
        report.max_rounds = 1;
        assert_eq!(smart_round_limit(&task, Some(&report)), 1);
        report.max_rounds = 4;
        assert_eq!(smart_round_limit(&task, Some(&report)), 2);
        report.max_rounds = 3;
        report.matches_current_plan = false;
        assert_eq!(smart_round_limit(&task, Some(&report)), 2);
        report.matches_current_plan = true;
        report.base_plan_digest = "different-plan".into();
        assert_eq!(smart_round_limit(&task, Some(&report)), 2);
    }
    #[test]
    fn ai_preview_feedback_requires_its_own_consent_and_a_saved_usable_draft() {
        let mut task = task_snapshot();
        task.cloud_consent = true;
        task.plan.as_mut().unwrap().clips.push(Clip {
            id: "manual-unit-clip".into(),
            source_id: "unit-only".into(),
            start_ms: 0,
            end_ms: 1000,
            reason: "MANUAL unit fixture; no real media or provider".into(),
            uncertainty: "Not evaluated".into(),
            evidence_ids: vec![],
            human_modified: true,
        });
        assert!(!smart_run_allowed(&task, false, false, false));
        assert!(!smart_run_allowed(&task, false, false, true));
        task.samples.push(video_agent::Sample {
            id: "unit-only-0".into(),
            source_id: "unit-only".into(),
            time_ms: 0,
            image_path: PathBuf::from("not-read-by-ui-gate-test.jpg"),
            sha256: "unit-fixture-no-file".into(),
        });
        assert!(smart_run_allowed(&task, false, false, true));
        assert!(!smart_run_allowed(&task, true, false, true));
        assert!(!smart_run_allowed(&task, false, true, true));
        task.status = TaskStatus::Stale;
        assert!(!smart_run_allowed(&task, false, false, true));
        task.status = TaskStatus::Review;
        task.usage.elapsed_ms = task.limits.max_execution_secs * 1000;
        assert!(!smart_run_allowed(&task, false, false, true));
        task.usage.elapsed_ms = 0;
        task.plan = None;
        assert!(!smart_run_allowed(&task, false, false, true));
    }
    #[test]
    fn exhausted_execution_budget_locks_processing_without_hiding_saved_records() {
        let limits = Limits {
            max_execution_secs: 1,
            process_timeout_secs: 1,
            model_timeout_secs: 1,
            ..Limits::default()
        };
        let mut usage = video_agent::Usage::default();
        assert!(has_execution_budget(&usage, &limits));
        usage.elapsed_ms = 995;
        assert!(has_execution_budget(&usage, &limits));
        usage.elapsed_ms = 996;
        assert!(!has_execution_budget(&usage, &limits));
        usage.elapsed_ms = u64::MAX;
        assert!(!has_execution_budget(&usage, &limits));
        let mut task = task_snapshot();
        task.limits = limits;
        task.usage = usage;
        assert_eq!(
            task_status_label(&task),
            "Execution budget exhausted · saved records only"
        );
    }
    #[test]
    fn execution_usage_separates_measured_crash_and_pending_reservations() {
        let usage = video_agent::Usage {
            elapsed_ms: 13_000,
            conservative_elapsed_ms: 10_000,
            execution_reserved_ms: 5_000,
            ..Default::default()
        };
        assert_eq!(execution_usage_summary(&usage), "Measured 3.0s · crash upper bounds 10.0s (included). In-flight upper bound 5.0s (reserved, not measured).");
    }

    #[test]
    fn frame_worker_usage_distinguishes_peak_from_summed_duration() {
        let usage = video_agent::Usage {
            peak_media_workers: 2,
            media_worker_ms: 6500,
            ..Default::default()
        };
        assert_eq!(frame_worker_usage_summary(&usage, 4), "Frame workers: recorded peak 2 / 4 · returned-worker time 6.5s (sum, not execution budget).");
        assert_eq!(frame_worker_usage_summary(&video_agent::Usage::default(), 1), "Frame workers: recorded peak 0 / 1 · returned-worker time 0.0s (sum, not execution budget).");
    }
    #[test]
    fn source_timecodes_retain_hours_and_milliseconds() {
        assert_eq!(timecode(3_661_234), "01:01:01.234");
        assert_eq!(timecode(0), "00:00:00.000");
    }
    #[test]
    fn human_boundary_input_rejects_non_finite_or_negative_values() {
        assert_eq!(seconds_to_ms(2.345), Some(2345));
        assert_eq!(seconds_to_ms(f64::NAN), None);
        assert_eq!(seconds_to_ms(f64::INFINITY), None);
        assert_eq!(seconds_to_ms(-0.1), None);
    }

    fn task_snapshot() -> Task {
        Task {
            schema_version: 1,
            id: "unit-ui-task".into(),
            created_at: "unit-test".into(),
            intent: "Unit test; no real model or media claim".into(),
            target_duration_ms: 1000,
            assets: Vec::new(),
            samples: Vec::new(),
            model_calls: Vec::new(),
            plan: Some(EditPlan {
                revision: 1,
                clips: Vec::new(),
                summary: "Unit test".into(),
                audio_policy: AudioPolicy::Mute,
            }),
            approval: None,
            status: TaskStatus::Review,
            last_error: None,
            limits: Limits::default(),
            usage: video_agent::Usage::default(),
            cloud_consent: false,
            export: None,
            shards: Vec::new(),
            history: Vec::new(),
        }
    }

    #[test]
    fn approval_badge_requires_exact_digest_not_just_revision() {
        let mut task = task_snapshot();
        let digest =
            video_agent::workflow::plan_digest(&task, task.plan.as_ref().unwrap()).unwrap();
        task.approval = Some(video_agent::Approval {
            revision: 1,
            plan_digest: digest,
            confirmed_at: "unit-test".into(),
        });
        assert!(approval_matches_saved_plan(&task));
        task.plan.as_mut().unwrap().audio_policy = AudioPolicy::PreserveWithSilence;
        assert!(!approval_matches_saved_plan(&task));
    }

    #[test]
    fn pending_or_previous_revision_export_is_never_current_success() {
        let mut task = task_snapshot();
        let digest =
            video_agent::workflow::plan_digest(&task, task.plan.as_ref().unwrap()).unwrap();
        task.status = TaskStatus::Completed;
        task.export = Some(video_agent::ExportRecord {
            plan_digest: digest,
            path: std::env::current_exe().unwrap(),
            sha256: "unit-test-only".into(),
            duration_ms: 1000,
            has_audio: false,
            verified: true,
            completed_at: None,
        });
        assert!(!has_current_verified_export(&task));
        task.export.as_mut().unwrap().completed_at = Some("unit-test".into());
        assert!(has_current_verified_export(&task));
        task.usage.elapsed_ms = task.limits.max_execution_secs * 1000;
        assert!(!has_current_verified_export(&task), "An exhausted task cannot enable result playback even if an earlier verification flag remains set");
        task.usage.elapsed_ms = 0;
        task.plan.as_mut().unwrap().revision += 1;
        assert!(!has_current_verified_export(&task));
    }

    #[test]
    fn manual_replacement_does_not_inherit_old_ai_origin() {
        let mut task = task_snapshot();
        task.history.push(video_agent::Event {
            at: "unit-test".into(),
            kind: "model_plan".into(),
            detail: "Unit-test event, not a real call".into(),
        });
        // A history label alone must never pass as a received production model call.
        assert!(!plan_has_model_origin(&task));
        task.history.push(video_agent::Event {
            at: "unit-test".into(),
            kind: "manual_plan".into(),
            detail: "Explicit manual replacement".into(),
        });
        assert!(!plan_has_model_origin(&task));
    }

    #[test]
    fn a_smart_adoption_history_label_without_model_evidence_is_not_ai_origin() {
        let mut task = task_snapshot();
        let digest =
            video_agent::workflow::plan_digest(&task, task.plan.as_ref().unwrap()).unwrap();
        task.history.push(video_agent::Event {
            at: "unit-test".into(),
            kind: "smart_feedback_adopted".into(),
            detail: format!(
                "{{\"run_id\":\"unit-only\",\"base_plan_digest\":\"{digest}\",\"selected_plan_digest\":\"{digest}\",\"revision\":1,\"proposal_call_id\":\"not-a-model-call\"}}"
            ),
        });
        assert!(!plan_has_model_origin(&task));
    }
}
