use crate::tabs::workflow_tab::{studio_for, Studio};
use crate::theme::{
    self, Icon, Tone, ACCENT, ACCENT_SOFT, DANGER, FAINT, LINE, LINE_STRONG, MUTED, SUCCESS,
    SUNKEN, SURFACE, TEXT, TEXT_2,
};
use eframe::egui::{self, Color32, RichText, Ui};
use file_conversion::{
    dependencies, output_requests, run_queue, CancelToken, ConversionRequest, DependencyStatus,
    JobState, QueueEvent, QueueReport, TargetFormat,
};
use file_workflows::WorkflowKind;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Icon, hue, title, detail, and what picking the card does.
type QuickCard<'a> = (
    Icon,
    Color32,
    &'a str,
    &'a str,
    Option<TargetFormat>,
    Option<WorkflowKind>,
);

const MAX_INPUTS: usize = 1_000;

#[derive(Clone, Copy, PartialEq)]
enum MediaFilter {
    All,
    Video,
    Audio,
    Image,
}

struct Workflow {
    title: &'static str,
    detail: &'static str,
    format: TargetFormat,
    category: MediaFilter,
}

const WORKFLOWS: &[Workflow] = &[
    Workflow {
        title: "Convert video",
        detail: "MP4, MKV, MOV and WebM",
        format: TargetFormat::Mp4,
        category: MediaFilter::Video,
    },
    Workflow {
        title: "MP4 → MP3",
        detail: "Keep just the sound from a video",
        format: TargetFormat::Mp3,
        category: MediaFilter::Audio,
    },
    Workflow {
        title: "HEIC to JPG",
        detail: "Convert iPhone photos to JPG",
        format: TargetFormat::Jpg,
        category: MediaFilter::Image,
    },
];

struct QueueEntry {
    input: PathBuf,
    output: PathBuf,
    state: JobState,
    progress: f32,
    message: String,
}

enum WorkerMessage {
    Event(QueueEvent),
    Finished(QueueReport),
    Panicked,
}

/// The GUI owns the editable form; a worker receives an immutable job snapshot.
pub struct ConvertTab {
    inputs: Vec<PathBuf>,
    output_directory: String,
    format: TargetFormat,
    workflow_search: String,
    media_filter: MediaFilter,
    workflow_navigation: Option<(WorkflowKind, Vec<PathBuf>)>,
    queue: Vec<QueueEntry>,
    receiver: Option<Receiver<WorkerMessage>>,
    worker: Option<thread::JoinHandle<()>>,
    cancel: Option<CancelToken>,
    dependency_status: Vec<DependencyStatus>,
    notice: Option<String>,
    status: Arc<Mutex<String>>,
    processing: Arc<Mutex<bool>>,
}

impl ConvertTab {
    pub fn new(status: Arc<Mutex<String>>, processing: Arc<Mutex<bool>>) -> Self {
        Self {
            inputs: Vec::new(),
            output_directory: std::env::current_dir()
                .ok()
                .filter(|path| path.parent().is_some())
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Downloads"))
                })
                .unwrap_or_else(std::env::temp_dir)
                .join("converted_media")
                .to_string_lossy()
                .into_owned(),
            format: TargetFormat::Mp4,
            workflow_search: String::new(),
            media_filter: MediaFilter::All,
            workflow_navigation: None,
            queue: Vec::new(),
            receiver: None,
            worker: None,
            cancel: None,
            dependency_status: dependencies(),
            notice: None,
            status,
            processing,
        }
    }

    pub fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn take_workflow(&mut self) -> Option<(WorkflowKind, Vec<PathBuf>)> {
        self.workflow_navigation.take()
    }

    /// Drain updates even while another tab is visible.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Some(receiver) = self.receiver.as_ref() {
            let next = receiver.try_recv();
            match next {
                Ok(WorkerMessage::Event(event)) => {
                    if let Some(entry) = self.queue.get_mut(event.index) {
                        entry.state = event.state;
                        entry.progress = if event.progress.is_finite() {
                            event.progress.clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        entry.message = event.message;
                    }
                }
                Ok(WorkerMessage::Finished(report)) => {
                    for result in &report.jobs {
                        if let Some(entry) = self.queue.get_mut(result.index) {
                            entry.state = result.state;
                            entry.output = result.output.clone();
                            if matches!(result.state, JobState::Completed) {
                                entry.progress = 1.0;
                                entry.message = "Output verified".to_string();
                            } else if let Some(error) = &result.error {
                                entry.message = error.clone();
                            }
                        }
                    }
                    let completed = report
                        .jobs
                        .iter()
                        .filter(|job| matches!(job.state, JobState::Completed))
                        .count();
                    let cancelled = report
                        .jobs
                        .iter()
                        .filter(|job| matches!(job.state, JobState::Cancelled))
                        .count();
                    let message = format!(
                        "Conversion queue finished: {} verified, {} cancelled, {} failed.",
                        completed,
                        cancelled,
                        report.jobs.len().saturating_sub(completed + cancelled)
                    );
                    *self.status.lock().unwrap() = message.clone();
                    self.notice = Some(message);
                    self.receiver = None;
                    self.cancel = None;
                    if let Some(worker) = self.worker.take() {
                        let _ = worker.join();
                    }
                    break;
                }
                Ok(WorkerMessage::Panicked) | Err(TryRecvError::Disconnected) => {
                    for entry in &mut self.queue {
                        if matches!(
                            entry.state,
                            JobState::Queued | JobState::Running | JobState::Validating
                        ) {
                            entry.state = JobState::Failed;
                            entry.message =
                                "The conversion worker stopped unexpectedly".to_string();
                        }
                    }
                    self.notice = Some("The conversion worker stopped unexpectedly".to_string());
                    *self.status.lock().unwrap() = self.notice.clone().unwrap();
                    *self.processing.lock().unwrap() = false;
                    self.receiver = None;
                    self.cancel = None;
                    if let Some(worker) = self.worker.take() {
                        let _ = worker.join();
                    }
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if self.is_running() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical()
            .id_source("conversion_page")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::none()
                    .inner_margin(egui::Margin {
                        left: 40.0,
                        right: 32.0,
                        top: 32.0,
                        bottom: 32.0,
                    })
                    .show(ui, |ui| self.page_ui(ui));
            });
    }

    fn page_ui(&mut self, ui: &mut Ui) {
        let available = ui.available_width();
        let editable = !self.is_running() && !*self.processing.lock().unwrap();
        if editable {
            let dropped = ui.ctx().input(|input| input.raw.dropped_files.clone());
            self.add_inputs(dropped.into_iter().filter_map(|file| file.path).collect());
        }
        if available >= 900.0 {
            let queue_width = 320.0;
            let gap = 28.0;
            let workspace_width = available - queue_width - gap;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.allocate_ui_with_layout(
                    egui::vec2(workspace_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.set_width(workspace_width);
                        self.workspace_ui(ui, editable);
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(queue_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.set_width(queue_width);
                        self.queue_ui(ui);
                    },
                );
            });
        } else {
            self.workspace_ui(ui, editable);
            ui.add_space(18.0);
            self.queue_ui(ui);
        }
    }

    fn workspace_ui(&mut self, ui: &mut Ui, editable: bool) {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        theme::page_header(
            ui,
            None,
            "Convert",
            "Search for a tool, drop your files, or start from a link.",
        );
        ui.add_enabled_ui(editable, |ui| {
            self.workflows_ui(ui);
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Source")
                        .font(theme::semibold(15.0))
                        .color(TEXT),
                );
                ui.add_space((ui.available_width() - 254.0).max(0.0));
                if theme::segmented(ui, 0, &["From files", "From a link"]) == Some(1) {
                    self.workflow_navigation = Some((WorkflowKind::Download, Vec::new()));
                }
            });
            ui.add_space(6.0);
            if self.inputs.is_empty() {
                self.drop_zone_ui(ui);
            } else {
                self.files_ui(ui);
            }
            if self.inputs.is_empty() {
                return;
            }
            ui.add_space(14.0);
            self.output_ui(ui);
            ui.add_space(14.0);
            let action = if self.is_running() {
                "Converting files…".to_string()
            } else if self.inputs.len() == 1 {
                "Convert 1 file".to_string()
            } else {
                format!("Convert {} files", self.inputs.len())
            };
            if theme::wide_primary(ui, !self.inputs.is_empty(), action).clicked() {
                self.start_queue(ui.ctx().clone());
            }
        });
        if !editable && !self.is_running() {
            ui.add_space(8.0);
            theme::hint(
                ui,
                "Another tool is working. Your files are ready when it finishes.",
            );
        }
        if let Some(notice) = &self.notice {
            ui.add_space(10.0);
            theme::well().show(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::hint(ui, notice);
            });
        }
        ui.add_space(14.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            for text in [
                "Folders include subfolders",
                "Originals stay unchanged",
                "Every output is verified",
            ] {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    theme::icon(ui, Icon::Check, 13.0, SUCCESS);
                    ui.label(RichText::new(text).size(12.5).color(MUTED));
                });
            }
        });
        ui.add_space(8.0);
        ui.collapsing(
            RichText::new("Advanced settings")
                .font(theme::semibold(13.0))
                .color(TEXT_2),
            |ui| {
                ui.add_enabled_ui(editable, |ui| {
                    theme::field(ui, "Output path", |ui| {
                        theme::wide_input(ui, &mut self.output_directory, "Folder for new files");
                    });
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Local conversion tools")
                        .font(theme::semibold(13.0))
                        .color(TEXT),
                );
                for dependency in &self.dependency_status {
                    let location = dependency
                        .path
                        .as_ref()
                        .map(|path| theme::display_path(path))
                        .unwrap_or_else(|| "Not installed".to_string());
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{} · {}", dependency.name, location))
                                .size(12.0)
                                .color(MUTED),
                        )
                        .wrap(true),
                    );
                }
                if ui.add(quiet_button("Check tools again")).clicked() {
                    self.dependency_status = dependencies();
                }
                theme::hint(
                    ui,
                    "Images use the first frame. JPEG replaces transparency with white.",
                );
            },
        );
    }

    fn workflows_ui(&mut self, ui: &mut Ui) {
        let focused_id = egui::Id::new("conversion_search_focused");
        let focused = ui
            .ctx()
            .data(|data| data.get_temp::<bool>(focused_id))
            .unwrap_or(false);
        let search = egui::Frame::none()
            .fill(SURFACE)
            .stroke(egui::Stroke::new(
                1.0_f32,
                if focused { ACCENT } else { LINE_STRONG },
            ))
            .rounding(theme::RADIUS_LG)
            .inner_margin(egui::Margin::symmetric(14.0, 11.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::icon(ui, Icon::Search, 17.0, MUTED);
                    let width = (ui.available_width()
                        - if self.workflow_search.is_empty() {
                            0.0
                        } else {
                            64.0
                        })
                    .max(80.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.workflow_search)
                            .frame(false)
                            .desired_width(width)
                            .font(egui::FontId::proportional(15.0))
                            .hint_text(
                                RichText::new("Try “merge PDFs”, “subtitles”, “HEIC to JPG”…")
                                    .color(FAINT),
                            ),
                    );
                    if !self.workflow_search.is_empty() && ui.add(quiet_button("Clear")).clicked() {
                        self.workflow_search.clear();
                    }
                    response.has_focus()
                })
                .inner
            });
        let has_focus = search.inner;
        ui.ctx()
            .data_mut(|data| data.insert_temp(focused_id, has_focus));
        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(5.0, 8.0);
            for (label, filter) in [
                ("All", MediaFilter::All),
                ("Video", MediaFilter::Video),
                ("Audio", MediaFilter::Audio),
                ("Images", MediaFilter::Image),
            ] {
                if theme::chip(ui, label, self.media_filter == filter, None).clicked() {
                    self.media_filter = filter;
                }
            }
            let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 30.0), egui::Sense::hover());
            ui.painter().line_segment(
                [
                    egui::pos2(rect.center().x, rect.top() + 7.0),
                    egui::pos2(rect.center().x, rect.bottom() - 7.0),
                ],
                egui::Stroke::new(1.0_f32, LINE),
            );
            for studio in [Studio::Pdf, Studio::Subtitles, Studio::Documents] {
                if theme::chip(ui, studio.title(), false, Some(studio.color()))
                    .on_hover_text(studio.blurb())
                    .clicked()
                {
                    self.workflow_navigation = Some((studio.default_kind(), Vec::new()));
                }
            }
        });
        ui.add_space(16.0);
        ui.label(
            RichText::new("Quick start")
                .font(theme::semibold(15.0))
                .color(TEXT),
        );
        ui.add_space(4.0);
        let search = self.workflow_search.trim().to_lowercase();
        let mut cards: Vec<QuickCard> = Vec::new();
        for workflow in WORKFLOWS {
            let matches_category =
                self.media_filter == MediaFilter::All || self.media_filter == workflow.category;
            let searchable = format!("{} {} {}", workflow.title, workflow.detail, workflow.format)
                .to_lowercase();
            if !matches_category || !searchable.contains(&search) {
                continue;
            }
            let (icon, hue) = match workflow.category {
                MediaFilter::Audio => (Icon::Audio, theme::HUE_AUDIO),
                MediaFilter::Image => (Icon::Image, theme::HUE_IMAGE),
                _ => (Icon::Video, theme::HUE_VIDEO),
            };
            cards.push((
                icon,
                hue,
                workflow.title,
                workflow.detail,
                Some(workflow.format),
                None,
            ));
        }
        let tools = file_workflows::tools();
        if self.media_filter == MediaFilter::All {
            for tool in &tools {
                let haystack =
                    format!("{} {} {}", tool.title, tool.category, tool.description).to_lowercase();
                if (!search.is_empty() && haystack.contains(&search))
                    || (search.is_empty() && tool.kind == WorkflowKind::PdfMerge)
                {
                    let studio = studio_for(tool.kind);
                    cards.push((
                        studio.icon(),
                        studio.color(),
                        tool.title.as_str(),
                        tool.description.as_str(),
                        None,
                        Some(tool.kind),
                    ));
                }
            }
        }
        if cards.is_empty() {
            theme::hint(
                ui,
                "No matching workflows. Try another search or choose a format below.",
            );
            return;
        }
        let columns = if ui.available_width() > 640.0 { 4 } else { 2 };
        let mut navigate = None;
        for row in cards.chunks(columns) {
            ui.columns(columns, |cols| {
                for (col, (icon, hue, title, detail, format, kind)) in cols.iter_mut().zip(row) {
                    let selected = format.is_some_and(|format| format == self.format);
                    if quick_card(col, *icon, *hue, title, detail, selected) {
                        if let Some(format) = format {
                            self.format = *format;
                        }
                        if let Some(kind) = kind {
                            navigate = Some(*kind);
                        }
                    }
                }
            });
        }
        if let Some(kind) = navigate {
            self.workflow_navigation = Some((kind, Vec::new()));
        }
    }

    fn drop_zone_ui(&mut self, ui: &mut Ui) {
        let hovered = ui.ctx().input(|input| !input.raw.hovered_files.is_empty());
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 248.0),
            egui::Sense::hover(),
        );
        let painter = ui.painter();
        painter.rect_filled(
            rect,
            theme::RADIUS_LG,
            if hovered { ACCENT_SOFT } else { SUNKEN },
        );
        theme::dashed_rect(
            painter,
            rect.shrink(0.5),
            theme::RADIUS_LG,
            egui::Stroke::new(1.2_f32, if hovered { ACCENT } else { LINE_STRONG }),
        );
        draw_drop_art(painter, egui::pos2(rect.center().x, rect.top() + 54.0));
        painter.text(
            egui::pos2(rect.center().x, rect.top() + 104.0),
            egui::Align2::CENTER_CENTER,
            if hovered {
                "Release to add your files"
            } else {
                "Drop files or folders here"
            },
            theme::semibold(17.0),
            TEXT,
        );
        painter.text(
            egui::pos2(rect.center().x, rect.top() + 128.0),
            egui::Align2::CENTER_CENTER,
            "Video, audio and images — one file or a whole folder.",
            egui::FontId::proportional(13.0),
            MUTED,
        );
        let button_y = rect.top() + 176.0;
        if ui
            .put(
                egui::Rect::from_center_size(
                    egui::pos2(rect.center().x - 64.0, button_y),
                    egui::vec2(120.0, 36.0),
                ),
                theme::primary_button("Add files"),
            )
            .clicked()
        {
            self.choose_files();
        }
        if ui
            .put(
                egui::Rect::from_center_size(
                    egui::pos2(rect.center().x + 64.0, button_y),
                    egui::vec2(120.0, 36.0),
                ),
                theme::secondary_button("Add folder"),
            )
            .clicked()
        {
            self.choose_folder_inputs();
        }
        ui.painter().text(
            egui::pos2(rect.center().x, rect.bottom() - 22.0),
            egui::Align2::CENTER_CENTER,
            "MP4 · MOV · MKV · MP3 · WAV · JPG · PNG · HEIC · WEBP",
            theme::semibold(10.5),
            FAINT,
        );
        ui.advance_cursor_after_rect(rect);
    }

    fn files_ui(&mut self, ui: &mut Ui) {
        theme::card().inner_margin(16.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Your files")
                        .font(theme::semibold(14.0))
                        .color(TEXT),
                );
                theme::badge(ui, &self.inputs.len().to_string(), Tone::Accent);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(quiet_button("Clear")).clicked() {
                        self.inputs.clear();
                    }
                    if ui.add(quiet_button("Add folder")).clicked() {
                        self.choose_folder_inputs();
                    }
                    if ui.add(quiet_button("Add files")).clicked() {
                        self.choose_files();
                    }
                });
            });
            ui.add_space(6.0);
            let mut remove = None;
            egui::ScrollArea::vertical()
                .id_source("conversion_inputs")
                .max_height(204.0)
                .show_rows(ui, 52.0, self.inputs.len(), |ui, rows| {
                    for index in rows {
                        let path = &self.inputs[index];
                        egui::Frame::none()
                            .fill(SUNKEN)
                            .rounding(theme::RADIUS)
                            .inner_margin(egui::Margin::symmetric(10.0, 7.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    file_icon(ui, path, 32.0);
                                    let name_width = (ui.available_width() - 40.0).max(80.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(name_width, 32.0),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.spacing_mut().item_spacing.y = 0.0;
                                            ui.set_width(name_width);
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(file_name(path))
                                                        .font(theme::semibold(13.0))
                                                        .color(TEXT),
                                                )
                                                .truncate(true),
                                            )
                                            .on_hover_text(path.display().to_string());
                                            ui.label(
                                                RichText::new(format!(
                                                    "{} · {}",
                                                    extension_label(path),
                                                    human_size(path)
                                                ))
                                                .size(11.5)
                                                .color(MUTED),
                                            );
                                        },
                                    );
                                    if ui
                                        .add(theme::ghost_button("✕"))
                                        .on_hover_text("Remove file")
                                        .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                });
                            });
                        ui.add_space(4.0);
                    }
                });
            if let Some(index) = remove {
                self.inputs.remove(index);
            }
            theme::hint(ui, "Drop more files or folders anywhere in this window.");
        });
    }

    fn output_ui(&mut self, ui: &mut Ui) {
        theme::card().inner_margin(16.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Convert to")
                        .font(theme::semibold(14.0))
                        .color(TEXT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_id_source("conversion_target_format")
                        .width(120.0)
                        .selected_text(
                            RichText::new(self.format.to_string().to_uppercase())
                                .font(theme::semibold(13.5))
                                .color(ACCENT),
                        )
                        .show_ui(ui, |ui| {
                            for format in TargetFormat::all() {
                                if ui
                                    .selectable_value(
                                        &mut self.format,
                                        *format,
                                        format.to_string().to_uppercase(),
                                    )
                                    .clicked()
                                {
                                    ui.memory_mut(|memory| memory.close_popup());
                                }
                            }
                        });
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                theme::icon(ui, Icon::Folder, 17.0, MUTED);
                let directory = PathBuf::from(&self.output_directory);
                let short_name = file_name(&directory);
                let label_width = (ui.available_width() - 84.0).max(60.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(label_width, 26.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_width(label_width);
                        ui.add(
                            egui::Label::new(RichText::new(short_name).size(13.5).color(TEXT))
                                .truncate(true),
                        )
                        .on_hover_text(&self.output_directory);
                    },
                );
                if ui.add(quiet_button("Choose…")).clicked() {
                    if let Some(directory) = rfd::FileDialog::new().pick_folder() {
                        self.output_directory = directory.to_string_lossy().into_owned();
                    }
                }
            });
            theme::hint(
                ui,
                "New files get a .converted suffix. Existing files are never overwritten.",
            );
        });
    }

    fn choose_files(&mut self) {
        let mut extensions = SUPPORTED_EXTENSIONS.to_vec();
        extensions.extend([
            "pdf", "srt", "vtt", "ass", "ssa", "sbv", "sub", "smi", "lrc", "txt", "md", "markdown",
            "html", "htm", "docx", "rtf", "odt", "epub",
        ]);
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter("Supported files", &extensions)
            .pick_files()
        {
            self.add_inputs(paths);
        }
    }

    fn choose_folder_inputs(&mut self) {
        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
            self.add_inputs(vec![folder]);
        }
    }

    fn add_inputs(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if paths.iter().all(|p| p.is_file()) {
            if let Some(kind) = paths.first().and_then(|p| workflow_input(p)) {
                if paths.iter().all(|p| {
                    workflow_input(p)
                        .is_some_and(|candidate| studio_for(candidate) == studio_for(kind))
                }) {
                    let kind = if kind == WorkflowKind::PdfSplit && paths.len() > 1 {
                        WorkflowKind::PdfMerge
                    } else {
                        kind
                    };
                    self.workflow_navigation = Some((kind, paths));
                    return;
                }
            }
        }
        let original_count = self.inputs.len();
        let mut workflow_candidates = Vec::new();
        let mut pending = paths.into_iter().rev().collect::<Vec<_>>();
        let mut rejected = 0;
        let mut visited = 0;
        let mut capped = false;
        while let Some(path) = pending.pop() {
            if self.inputs.len() >= MAX_INPUTS || visited >= 20_000 {
                capped = true;
                break;
            }
            visited += 1;
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                rejected += 1;
                continue;
            };
            if metadata.file_type().is_symlink() {
                rejected += 1;
                continue;
            }
            if metadata.is_dir() {
                match std::fs::read_dir(&path) {
                    Ok(entries) => {
                        let mut children = entries
                            .take(20_001)
                            .filter_map(Result::ok)
                            .map(|entry| entry.path())
                            .collect::<Vec<_>>();
                        if children.len() > 20_000 {
                            children.truncate(20_000);
                            capped = true;
                        }
                        children.sort();
                        pending.extend(children.into_iter().rev());
                    }
                    Err(_) => rejected += 1,
                }
            } else if metadata.is_file() && supported_input(&path) {
                if !self.inputs.contains(&path) {
                    self.inputs.push(path);
                }
            } else {
                if metadata.is_file()
                    && workflow_input(&path).is_some()
                    && workflow_candidates.len() < MAX_INPUTS
                {
                    workflow_candidates.push(path);
                }
                rejected += 1;
            }
        }
        if original_count == 0 && self.inputs.is_empty() && !workflow_candidates.is_empty() {
            let kind = workflow_input(&workflow_candidates[0]).unwrap();
            if workflow_candidates.iter().all(|p| {
                workflow_input(p).is_some_and(|candidate| studio_for(candidate) == studio_for(kind))
            }) {
                let kind = if kind == WorkflowKind::PdfSplit && workflow_candidates.len() > 1 {
                    WorkflowKind::PdfMerge
                } else {
                    kind
                };
                self.workflow_navigation = Some((kind, workflow_candidates));
                self.notice = None;
                return;
            }
        }
        if capped {
            self.notice = Some("Folder import reached its limit. A queue can contain up to 1,000 files; choose smaller folders to add the rest.".to_string());
        } else if rejected > 0 {
            self.notice = Some(format!(
                "Skipped {rejected} unsupported, linked or unavailable items."
            ));
        } else {
            self.notice = None;
        }
    }

    fn start_queue(&mut self, ctx: egui::Context) {
        if self.is_running() || *self.processing.lock().unwrap() {
            self.notice = Some("Wait for the active task to finish first.".to_string());
            return;
        }
        if self.inputs.is_empty() || self.output_directory.trim().is_empty() {
            self.notice = Some("Choose input files and an output folder first.".to_string());
            return;
        }
        let requests = match output_requests(
            &self.inputs,
            &PathBuf::from(&self.output_directory),
            self.format,
        ) {
            Ok(requests) => requests,
            Err(error) => {
                self.notice = Some(format!("Cannot start conversion: {:#}", error));
                return;
            }
        };

        // Claim the same task gate used by existing tabs, before starting work.
        {
            let mut processing = self.processing.lock().unwrap();
            if *processing {
                self.notice = Some("Wait for the active task to finish first.".to_string());
                return;
            }
            *processing = true;
        }
        self.queue = requests.iter().map(QueueEntry::from).collect();
        self.notice = None;
        *self.status.lock().unwrap() = format!("Converting {} files…", requests.len());
        let cancel = CancelToken::default();
        self.cancel = Some(cancel.clone());
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        let processing = Arc::clone(&self.processing);
        self.worker = Some(thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_queue(&requests, &cancel, |event| {
                    let _ = sender.send(WorkerMessage::Event(event));
                    ctx.request_repaint();
                })
            }));
            *processing.lock().unwrap() = false;
            let message = match result {
                Ok(report) => WorkerMessage::Finished(report),
                Err(_) => WorkerMessage::Panicked,
            };
            let _ = sender.send(message);
            ctx.request_repaint();
        }));
    }

    fn queue_ui(&mut self, ui: &mut Ui) {
        let running = self.is_running();
        let mut open_error = None;
        egui::Frame::none()
            .fill(SUNKEN)
            .stroke(egui::Stroke::new(1.0_f32, LINE))
            .rounding(theme::RADIUS_LG)
            .inner_margin(18.0)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                ui.set_min_height(560.0);
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Queue").font(theme::semibold(15.0)).color(TEXT));
                    if !self.queue.is_empty() {
                        theme::badge(ui, &self.queue.len().to_string(), Tone::Neutral);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !running && !self.queue.is_empty() && ui.add(quiet_button("Clear")).clicked() {
                            self.queue.clear();
                        }
                    });
                });
                ui.add_space(4.0);
                if self.queue.is_empty() {
                    ui.add_space(140.0);
                    theme::empty_state(
                        ui,
                        Icon::Queue,
                        "Nothing converting yet",
                        "Files you convert appear here.",
                    );
                    ui.add_space(140.0);
                } else {
                    let completed = self.queue.iter().filter(|entry| matches!(entry.state, JobState::Completed)).count();
                    let fraction = completed as f32 / self.queue.len().max(1) as f32;
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{completed} of {} complete", self.queue.len())).size(12.0).color(MUTED));
                    });
                    ui.add(egui::ProgressBar::new(fraction).desired_height(4.0).fill(ACCENT));
                    ui.add_space(6.0);
                    if running {
                        let cancelling = self.cancel.as_ref().map(CancelToken::is_cancelled).unwrap_or(false);
                        if ui.add_enabled(!cancelling, theme::danger_button(if cancelling { "Cancelling…" } else { "Cancel queue" })
                            .min_size(egui::vec2(ui.available_width(), 32.0))).clicked() {
                            if let Some(cancel) = &self.cancel { cancel.cancel(); }
                            *self.status.lock().unwrap() = "Cancelling conversion queue…".to_string();
                        }
                        ui.add_space(6.0);
                    }
                    egui::ScrollArea::vertical()
                        .id_source("conversion_queue_cards")
                        .max_height(480.0)
                        .show(ui, |ui| {
                            for (index, entry) in self.queue.iter().enumerate() {
                                ui.push_id(index, |ui| {
                                    theme::card()
                                        .inner_margin(12.0)
                                        .rounding(theme::RADIUS + 2.0)
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.horizontal(|ui| {
                                                file_icon(ui, &entry.input, 30.0);
                                                let width = (ui.available_width()).max(40.0);
                                                ui.allocate_ui_with_layout(egui::vec2(width, 30.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                                                    ui.spacing_mut().item_spacing.y = 0.0;
                                                    ui.set_width(width);
                                                    ui.add(egui::Label::new(RichText::new(file_name(&entry.input)).font(theme::semibold(12.5)).color(TEXT)).truncate(true))
                                                        .on_hover_text(entry.input.display().to_string());
                                                    ui.label(RichText::new(format!("{} → {}", extension_label(&entry.input), extension_label(&entry.output)))
                                                        .size(11.0).color(MUTED));
                                                });
                                            });
                                            ui.add_space(2.0);
                                            let tone = match entry.state {
                                                JobState::Completed => Tone::Success,
                                                JobState::Failed => Tone::Danger,
                                                JobState::Running | JobState::Validating => Tone::Accent,
                                                _ => Tone::Neutral,
                                            };
                                            ui.horizontal(|ui| {
                                                let state = if matches!(entry.state, JobState::Running | JobState::Validating) {
                                                    format!("{} · {:.0}%", state_label(&entry.state), entry.progress * 100.0)
                                                } else { state_label(&entry.state).to_string() };
                                                theme::badge(ui, &state, tone);
                                            });
                                            if matches!(entry.state, JobState::Running | JobState::Validating) {
                                                ui.add(egui::ProgressBar::new(entry.progress)
                                                    .desired_height(4.0)
                                                    .fill(ACCENT)
                                                    .animate(true));
                                            }
                                            if matches!(entry.state, JobState::Failed) && !entry.message.is_empty() {
                                                let summary = shorten(&entry.message, 120);
                                                ui.add(egui::Label::new(RichText::new(summary).size(11.5).color(DANGER)).wrap(true))
                                                    .on_hover_text(&entry.message);
                                            }
                                            if matches!(entry.state, JobState::Completed) {
                                                ui.add(egui::Label::new(RichText::new(file_name(&entry.output)).size(11.5).color(MUTED)).truncate(true))
                                                    .on_hover_text(format!("Verified output: {}", entry.output.display()));
                                                ui.horizontal(|ui| {
                                                    if ui.add(quiet_button("Show in folder")).clicked() {
                                                        if let Some(parent) = entry.output.parent() {
                                                            if let Err(error) = open_folder(parent) {
                                                                open_error = Some(format!("Could not open the output folder: {error}"));
                                                            }
                                                        }
                                                    }
                                                    if ui.add(quiet_button("Copy path")).clicked() {
                                                        ui.output_mut(|clipboard| clipboard.copied_text = entry.output.display().to_string());
                                                    }
                                                });
                                            }
                                        });
                                });
                                ui.add_space(6.0);
                            }
                        });
                }
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    theme::icon(ui, Icon::Shield, 14.0, SUCCESS);
                    ui.label(RichText::new("Outputs are verified before they're marked ready").size(11.5).color(MUTED));
                });
            });
        if open_error.is_some() {
            self.notice = open_error;
        }
    }
}

impl Drop for ConvertTab {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl From<&ConversionRequest> for QueueEntry {
    fn from(request: &ConversionRequest) -> Self {
        Self {
            input: request.input.clone(),
            output: request.output.clone(),
            state: JobState::Queued,
            progress: 0.0,
            message: "Waiting".to_string(),
        }
    }
}

fn state_label(state: &JobState) -> &'static str {
    match state {
        JobState::Queued => "Queued",
        JobState::Running => "Converting",
        JobState::Validating => "Validating output",
        JobState::Completed => "Verified",
        JobState::Failed => "Failed",
        JobState::Cancelled => "Cancelled",
    }
}

const SUPPORTED_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "mov", "webm", "avi", "flv", "wmv", "ts", "m4v", "mpeg", "mpg", "vob", "ogv",
    "mp3", "m4a", "aac", "wav", "flac", "ogg", "aif", "aiff", "png", "jpg", "jpeg", "webp", "tif",
    "tiff", "gif", "bmp", "heic", "heif",
];

fn supported_input(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SUPPORTED_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

fn workflow_input(path: &Path) -> Option<WorkflowKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "pdf" => WorkflowKind::PdfSplit,
        "srt" | "vtt" | "ass" | "ssa" | "sbv" | "sub" | "smi" | "lrc" => {
            WorkflowKind::SubtitleConvert
        }
        "txt" | "md" | "markdown" | "html" | "htm" | "docx" | "rtf" | "odt" | "epub" => {
            WorkflowKind::DocumentConvert
        }
        _ => return None,
    })
}

fn quiet_button(text: &str) -> egui::Button<'static> {
    theme::small_button(text)
}

/// Selectable shortcut card on the Convert page.
fn quick_card(
    ui: &mut Ui,
    icon: Icon,
    hue: Color32,
    title: &str,
    detail: &str,
    selected: bool,
) -> bool {
    let id = ui.next_auto_id();
    let hovered = ui
        .ctx()
        .data(|data| data.get_temp::<bool>(id))
        .unwrap_or(false);
    let (fill, stroke) = if selected {
        (SURFACE, egui::Stroke::new(1.5_f32, ACCENT))
    } else if hovered {
        (SUNKEN, egui::Stroke::new(1.0_f32, LINE_STRONG))
    } else {
        (SURFACE, egui::Stroke::new(1.0_f32, LINE))
    };
    let response = egui::Frame::none()
        .fill(fill)
        .stroke(stroke)
        .rounding(theme::RADIUS + 2.0)
        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                theme::icon_tile(ui, icon, hue, 34.0);
                let width = (ui.available_width() - 26.0).max(40.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 34.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.set_width(width);
                        ui.add(
                            egui::Label::new(
                                RichText::new(title).font(theme::semibold(13.5)).color(TEXT),
                            )
                            .truncate(true),
                        );
                        ui.add(
                            egui::Label::new(RichText::new(detail).size(12.0).color(MUTED))
                                .truncate(true),
                        );
                    },
                );
                if selected {
                    theme::icon(ui, Icon::Check, 14.0, ACCENT);
                }
            });
        })
        .response
        .interact(egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.ctx()
        .data_mut(|data| data.insert_temp(id, response.hovered()));
    response.clicked()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn extension_label(path: &Path) -> String {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "FILE".to_string())
}

fn human_size(path: &Path) -> String {
    let Ok(metadata) = std::fs::metadata(path) else {
        return "Unavailable".to_string();
    };
    let bytes = metadata.len() as f64;
    if bytes >= 1_000_000_000.0 {
        format!("{:.1} GB", bytes / 1_000_000_000.0)
    } else if bytes >= 1_000_000.0 {
        format!("{:.1} MB", bytes / 1_000_000.0)
    } else if bytes >= 1_000.0 {
        format!("{:.1} KB", bytes / 1_000.0)
    } else {
        format!("{} B", metadata.len())
    }
}

fn file_icon(ui: &mut Ui, path: &Path, size: f32) {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (icon, color) = if [
        "png", "jpg", "jpeg", "webp", "tif", "tiff", "gif", "bmp", "heic", "heif",
    ]
    .contains(&extension.as_str())
    {
        (Icon::Image, theme::HUE_IMAGE)
    } else if ["mp3", "m4a", "aac", "wav", "flac", "ogg", "aif", "aiff"]
        .contains(&extension.as_str())
    {
        (Icon::Audio, theme::HUE_AUDIO)
    } else {
        (Icon::Video, theme::HUE_VIDEO)
    };
    theme::icon_tile(ui, icon, color, size);
}

fn shorten(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max_chars).collect::<String>())
    }
}

fn draw_drop_art(painter: &egui::Painter, center: egui::Pos2) {
    for (offset, lift, kind, color) in [
        (-46.0, 6.0, Icon::Video, theme::HUE_VIDEO),
        (0.0, 0.0, Icon::Image, theme::HUE_IMAGE),
        (46.0, 6.0, Icon::Audio, theme::HUE_AUDIO),
    ] {
        let tile =
            egui::Rect::from_center_size(center + egui::vec2(offset, lift), egui::vec2(40.0, 40.0));
        theme::paint_shadow(painter, tile, 11.0);
        painter.rect(tile, 11.0, SURFACE, egui::Stroke::new(1.0_f32, LINE));
        theme::paint_icon(painter, tile.shrink(11.0), kind, color);
    }
}

fn open_folder(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = std::process::Command::new("explorer");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(any(unix, target_os = "windows"))]
    {
        command
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Opening folders is not supported on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ImportDirectory(PathBuf);

    impl ImportDirectory {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "video-toolkit-import-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn file(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"import fixture; conversion is not started").unwrap();
            path
        }
    }

    impl Drop for ImportDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tab() -> ConvertTab {
        ConvertTab::new(
            Arc::new(Mutex::new("Ready".to_string())),
            Arc::new(Mutex::new(false)),
        )
    }

    #[test]
    fn existing_tool_prevents_conversion_start() {
        let mut tab = tab();
        *tab.processing.lock().unwrap() = true;
        tab.start_queue(egui::Context::default());
        assert!(!tab.is_running());
        assert!(*tab.processing.lock().unwrap());
        assert!(tab.queue.is_empty());
        assert!(tab.notice.as_ref().unwrap().contains("active task"));
    }

    #[test]
    fn invalid_form_does_not_claim_task_gate() {
        let mut tab = tab();
        tab.start_queue(egui::Context::default());
        assert!(!tab.is_running());
        assert!(!*tab.processing.lock().unwrap());
        assert!(tab.queue.is_empty());
        assert!(tab.notice.as_ref().unwrap().contains("input files"));
    }

    #[test]
    fn dropping_tab_requests_worker_cancellation() {
        let mut tab = tab();
        let cancel = CancelToken::default();
        tab.cancel = Some(cancel.clone());
        drop(tab);
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn unexpected_worker_exit_marks_pending_jobs_failed_and_releases_gate() {
        let mut tab = tab();
        let (sender, receiver) = mpsc::channel();
        tab.receiver = Some(receiver);
        tab.queue = vec![QueueEntry {
            input: PathBuf::from("input.mp4"),
            output: PathBuf::from("output.mp4"),
            state: JobState::Running,
            progress: 0.5,
            message: String::new(),
        }];
        *tab.processing.lock().unwrap() = true;
        drop(sender);
        tab.poll(&egui::Context::default());
        assert!(!tab.is_running());
        assert!(!*tab.processing.lock().unwrap());
        assert!(matches!(tab.queue[0].state, JobState::Failed));
        assert!(tab.queue[0].message.contains("unexpectedly"));
    }

    #[test]
    fn folder_import_descends_into_nested_folders_and_skips_unsupported_files() {
        let directory = ImportDirectory::new();
        let video = directory.file("clip.MP4");
        let image = directory.file("nested/图片 source.HEIC");
        let audio = directory.file("nested/deeper/sound.mp3");
        directory.file("notes.txt");
        directory.file("nested/deeper/document.pdf");
        let mut tab = tab();

        tab.add_inputs(vec![directory.0.clone()]);

        let imported: std::collections::HashSet<_> = tab.inputs.iter().cloned().collect();
        assert_eq!(
            imported,
            std::collections::HashSet::from([video, image, audio])
        );
        assert_eq!(tab.inputs.len(), 3);
        assert!(tab.notice.as_ref().unwrap().contains("Skipped 2"));
        assert!(!tab.is_running());
        assert!(!*tab.processing.lock().unwrap());
    }

    #[test]
    fn reimporting_a_folder_and_its_selected_files_does_not_duplicate_inputs() {
        let directory = ImportDirectory::new();
        let first = directory.file("first.png");
        let second = directory.file("nested/second.webp");
        let mut tab = tab();
        tab.add_inputs(vec![first.clone(), directory.0.clone(), second.clone()]);
        assert_eq!(tab.inputs.len(), 2);

        tab.add_inputs(vec![directory.0.clone(), first.clone(), second.clone()]);

        assert_eq!(tab.inputs.len(), 2);
        assert!(tab.inputs.contains(&first));
        assert!(tab.inputs.contains(&second));
        assert!(tab.notice.is_none());
    }

    #[test]
    fn an_existing_full_selection_rejects_more_imports_without_changing_it() {
        let directory = ImportDirectory::new();
        let additional = directory.file("one more.png");
        let mut tab = tab();
        tab.inputs = (0..MAX_INPUTS)
            .map(|index| PathBuf::from(format!("already-selected-{index}.png")))
            .collect();
        let original = tab.inputs.clone();

        tab.add_inputs(vec![additional.clone()]);

        assert_eq!(tab.inputs, original);
        assert!(!tab.inputs.contains(&additional));
        assert!(tab.notice.as_ref().unwrap().contains("1,000 files"));
        assert!(!tab.is_running());
    }

    #[cfg(unix)]
    #[test]
    fn folder_import_skips_linked_files_linked_directories_and_recursive_links() {
        let directory = ImportDirectory::new();
        let outside = ImportDirectory::new();
        let local = directory.file("local.png");
        let external = outside.file("outside.png");
        std::os::unix::fs::symlink(&external, directory.0.join("linked.png")).unwrap();
        std::os::unix::fs::symlink(&outside.0, directory.0.join("linked folder")).unwrap();
        std::os::unix::fs::symlink(&directory.0, directory.0.join("recursive folder")).unwrap();
        let mut tab = tab();

        tab.add_inputs(vec![directory.0.clone()]);

        assert_eq!(tab.inputs, vec![local]);
        assert!(tab.notice.as_ref().unwrap().contains("Skipped 3"));
        assert_eq!(
            std::fs::read(&external).unwrap(),
            b"import fixture; conversion is not started"
        );
        assert!(!tab.is_running());
    }
}
