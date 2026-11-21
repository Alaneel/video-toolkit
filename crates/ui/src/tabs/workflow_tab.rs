use crate::theme::{self, Icon, LINE, MUTED, SURFACE, TEXT};
use eframe::egui::{self, Color32, RichText, Ui};
use file_conversion::{CancelToken, JobState};
use file_workflows::{
    run_queue, tools, WorkflowEvent, WorkflowKind, WorkflowReport, WorkflowRequest,
};
use serde_json::{json, Map, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Studio {
    Pdf,
    Subtitles,
    Documents,
    Downloads,
}
impl Studio {
    pub fn title(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Subtitles => "Subtitles",
            Self::Documents => "Documents",
            Self::Downloads => "Downloads",
        }
    }
    pub fn icon(self) -> Icon {
        match self {
            Self::Pdf => Icon::Pdf,
            Self::Subtitles => Icon::Subtitles,
            Self::Documents => Icon::Document,
            Self::Downloads => Icon::Download,
        }
    }
    pub fn color(self) -> Color32 {
        match self {
            Self::Pdf => theme::HUE_PDF,
            Self::Subtitles => theme::HUE_SUBTITLE,
            Self::Documents => theme::HUE_DOCUMENT,
            Self::Downloads => theme::HUE_DOWNLOAD,
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            Self::Pdf => "Merge, split, compress, rotate and watermark PDFs.",
            Self::Subtitles => "Convert, shift and clean subtitle files.",
            Self::Documents => "Move text between Markdown, DOCX, HTML and EPUB.",
            Self::Downloads => "Save a file or media from a link you provide.",
        }
    }
    pub fn default_kind(self) -> WorkflowKind {
        match self {
            Self::Pdf => WorkflowKind::PdfMerge,
            Self::Subtitles => WorkflowKind::SubtitleConvert,
            Self::Documents => WorkflowKind::DocumentConvert,
            Self::Downloads => WorkflowKind::Download,
        }
    }
}

pub fn studio_for(kind: WorkflowKind) -> Studio {
    use WorkflowKind::*;
    match kind {
        SubtitleConvert | SubtitleShift | SubtitleFps | SubtitleClean => Studio::Subtitles,
        DocumentConvert => Studio::Documents,
        Download => Studio::Downloads,
        _ => Studio::Pdf,
    }
}

#[derive(Clone)]
struct Form {
    format: String,
    pages: String,
    chunk_size: u32,
    ranges: String,
    angle: i32,
    preset: String,
    text: String,
    opacity: f32,
    font_size: u32,
    title: String,
    author: String,
    subject: String,
    keywords: String,
    strip: bool,
    dpi: u32,
    offset_ms: i64,
    source_fps: f64,
    target_fps: f64,
    encoding: String,
    clean: bool,
    url: String,
    mode: String,
    filename: String,
    max_mb: u32,
}
impl Default for Form {
    fn default() -> Self {
        Self {
            format: "srt".into(),
            pages: String::new(),
            chunk_size: 1,
            ranges: String::new(),
            angle: 90,
            preset: "ebook".into(),
            text: String::new(),
            opacity: 0.2,
            font_size: 36,
            title: String::new(),
            author: String::new(),
            subject: String::new(),
            keywords: String::new(),
            strip: false,
            dpi: 150,
            offset_ms: 0,
            source_fps: 25.0,
            target_fps: 25.0,
            encoding: "auto".into(),
            clean: true,
            url: String::new(),
            mode: "direct".into(),
            filename: String::new(),
            max_mb: 2048,
        }
    }
}
impl Form {
    fn reset_for(&mut self, kind: WorkflowKind) {
        self.source_fps = 25.0;
        self.target_fps = if kind == WorkflowKind::SubtitleFps {
            23.976
        } else {
            25.0
        };
        self.angle = if kind == WorkflowKind::PdfWatermark {
            45
        } else {
            90
        };
        self.format = match kind {
            WorkflowKind::PdfToImages => "png",
            WorkflowKind::DocumentConvert => "pdf",
            WorkflowKind::Download => "original",
            _ => "srt",
        }
        .into();
        self.encoding = if kind == WorkflowKind::DocumentConvert {
            "utf-8"
        } else {
            "auto"
        }
        .into();
    }
    fn options(&self, kind: WorkflowKind) -> anyhow::Result<Value> {
        use WorkflowKind::*;
        let mut options = Map::new();
        match kind {
            PdfMerge | ImagesToPdf => {}
            PdfSplit => {
                if self.ranges.trim().is_empty() {
                    anyhow::ensure!(
                        (1..=5000).contains(&self.chunk_size),
                        "Pages per file must be between 1 and 5,000"
                    );
                    options.insert("chunk_size".into(), json!(self.chunk_size));
                } else {
                    let ranges = self
                        .ranges
                        .split(';')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>();
                    anyhow::ensure!(
                        !ranges.is_empty(),
                        "Enter page ranges separated by semicolons"
                    );
                    options.insert("ranges".into(), json!(ranges));
                }
            }
            PdfRotate => {
                options.insert("angle".into(), json!(self.angle));
            }
            PdfCompress => {
                options.insert("preset".into(), json!(self.preset));
            }
            PdfWatermark => {
                anyhow::ensure!(!self.text.trim().is_empty(), "Enter watermark text");
                anyhow::ensure!(
                    (6..=144).contains(&self.font_size),
                    "Watermark font size must be between 6 and 144"
                );
                options.extend(json!({"text":self.text,"opacity":self.opacity,"font_size":self.font_size,"angle":self.angle}).as_object().unwrap().clone());
            }
            PdfMetadata => {
                options.extend(json!({"title":self.title,"author":self.author,"subject":self.subject,"keywords":self.keywords,"strip":self.strip}).as_object().unwrap().clone());
            }
            PdfToImages => {
                anyhow::ensure!(
                    (36..=300).contains(&self.dpi),
                    "Resolution must be between 36 and 300 DPI"
                );
                options.extend(
                    json!({"format":self.format,"dpi":self.dpi})
                        .as_object()
                        .unwrap()
                        .clone(),
                );
            }
            PdfToText => {}
            SubtitleConvert | SubtitleShift | SubtitleFps | SubtitleClean => {
                options.insert("format".into(), json!(self.format));
                options.insert("encoding".into(), json!(self.encoding));
                match kind {
                    SubtitleShift => {
                        options.insert("offset_ms".into(), json!(self.offset_ms));
                    }
                    SubtitleFps => {
                        anyhow::ensure!(
                            self.source_fps.is_finite()
                                && self.source_fps > 0.0
                                && self.target_fps.is_finite()
                                && self.target_fps > 0.0,
                            "Frame rates must be positive"
                        );
                        options.extend(
                            json!({"source_fps":self.source_fps,"target_fps":self.target_fps})
                                .as_object()
                                .unwrap()
                                .clone(),
                        );
                    }
                    SubtitleClean => {
                        options.insert("clean".into(), json!(self.clean));
                    }
                    _ => {}
                }
                if kind != SubtitleFps && self.format.eq_ignore_ascii_case("sub") {
                    anyhow::ensure!(
                        self.target_fps.is_finite() && self.target_fps > 0.0,
                        "MicroDVD output FPS must be positive"
                    );
                    options.insert("target_fps".into(), json!(self.target_fps));
                }
            }
            DocumentConvert => {
                options.extend(
                    json!({"format":self.format,"encoding":self.encoding})
                        .as_object()
                        .unwrap()
                        .clone(),
                );
                if !self.title.trim().is_empty() {
                    options.insert("title".into(), json!(self.title));
                }
            }
            Download => {
                anyhow::ensure!(
                    (1..=102400).contains(&self.max_mb),
                    "Maximum size must be between 1 and 102,400 MB"
                );
                anyhow::ensure!(
                    self.url.starts_with("https://") || self.url.starts_with("http://"),
                    "Enter an HTTP or HTTPS link"
                );
                options.extend(
                    json!({"url":self.url.trim(),"mode":self.mode,"format":self.format,"max_mb":self.max_mb})
                        .as_object()
                        .unwrap()
                        .clone(),
                );
                if !self.filename.trim().is_empty() {
                    options.insert("filename".into(), json!(self.filename.trim()));
                }
            }
        }
        if matches!(kind, PdfRotate | PdfWatermark | PdfToImages | PdfToText)
            && !self.pages.trim().is_empty()
        {
            options.insert("pages".into(), json!(self.pages.trim()));
        }
        Ok(Value::Object(options))
    }
}

struct Job {
    kind: WorkflowKind,
    title: String,
    source: String,
    state: JobState,
    progress: f32,
    message: String,
    outputs: Vec<PathBuf>,
    show_all_outputs: bool,
}
enum Message {
    Event(WorkflowEvent),
    Finished(WorkflowReport),
    Panicked,
}

pub struct WorkflowTab {
    studio: Studio,
    kind: WorkflowKind,
    form: Form,
    inputs: Vec<PathBuf>,
    output_dir: String,
    pending: Vec<WorkflowRequest>,
    queue: Vec<Job>,
    receiver: Option<mpsc::Receiver<Message>>,
    worker: Option<thread::JoinHandle<()>>,
    cancel: Option<CancelToken>,
    notice: Option<String>,
    status: Arc<Mutex<String>>,
    processing: Arc<Mutex<bool>>,
}
impl WorkflowTab {
    pub fn new(status: Arc<Mutex<String>>, processing: Arc<Mutex<bool>>) -> Self {
        Self {
            studio: Studio::Pdf,
            kind: WorkflowKind::PdfMerge,
            form: Form::default(),
            inputs: Vec::new(),
            output_dir: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("Downloads")
                .join("converted")
                .display()
                .to_string(),
            pending: Vec::new(),
            queue: Vec::new(),
            receiver: None,
            worker: None,
            cancel: None,
            notice: None,
            status,
            processing,
        }
    }
    pub fn is_running(&self) -> bool {
        self.receiver.is_some()
    }
    pub fn open(&mut self, kind: WorkflowKind, inputs: Vec<PathBuf>) {
        if self.is_running() {
            return;
        }
        if self.studio != studio_for(kind) || !inputs.is_empty() {
            self.inputs.clear();
        }
        self.studio = studio_for(kind);
        self.kind = kind;
        self.form.reset_for(kind);
        self.import(inputs);
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Some(receiver) = &self.receiver {
            let next = receiver.try_recv();
            match next {
                Ok(Message::Event(event)) => {
                    if let Some(job) = self.queue.get_mut(event.index) {
                        job.state = event.state;
                        job.progress = if event.progress.is_finite() {
                            event.progress.clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        job.message = event.message;
                    }
                }
                Ok(Message::Finished(report)) => {
                    for result in &report.jobs {
                        if let Some(job) = self.queue.get_mut(result.index) {
                            job.state = result.state;
                            job.outputs = result.outputs.clone();
                            job.message = result.error.clone().unwrap_or_else(|| "Verified".into());
                            if result.state == JobState::Completed {
                                job.progress = 1.0;
                            }
                        }
                    }
                    self.notice = Some(if report.success() {
                        format!("{} complete", task_count(report.jobs.len()))
                    } else {
                        "Queue finished. Check the results for details.".into()
                    });
                    *self.status.lock().unwrap() = self.notice.clone().unwrap();
                    self.finish_worker();
                    break;
                }
                Ok(Message::Panicked) | Err(mpsc::TryRecvError::Disconnected) => {
                    for job in &mut self.queue {
                        if matches!(
                            job.state,
                            JobState::Queued | JobState::Running | JobState::Validating
                        ) {
                            job.state = JobState::Failed;
                            job.message = "Task worker stopped unexpectedly".into();
                        }
                    }
                    self.notice = Some("Task worker stopped unexpectedly".into());
                    *self.status.lock().unwrap() = self.notice.clone().unwrap();
                    *self.processing.lock().unwrap() = false;
                    self.finish_worker();
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if self.is_running() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
    fn finish_worker(&mut self) {
        self.receiver = None;
        self.cancel = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
    pub fn ui(&mut self, ui: &mut Ui) {
        let editable = !self.is_running() && !*self.processing.lock().unwrap();
        if editable {
            let dropped = ui.ctx().input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .filter_map(|f| f.path.clone())
                    .collect()
            });
            self.import(dropped);
        }
        egui::Frame::none()
            .inner_margin(egui::Margin {
                left: 0.0,
                right: 32.0,
                top: 0.0,
                bottom: 0.0,
            })
            .show(ui, |ui| {
                let width = ui.available_width();
                let height = ui.available_height().max(240.0);
                if width >= 900.0 {
                    let queue_width = 320.0;
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.allocate_ui_with_layout(
                            egui::vec2(width - queue_width, height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(width - queue_width);
                                self.scrolling_workspace(ui, editable, height);
                            },
                        );
                        ui.allocate_ui_with_layout(
                            egui::vec2(queue_width, height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(queue_width);
                                ui.add_space(32.0);
                                self.queue_ui(ui, height - 64.0);
                            },
                        );
                    });
                } else {
                    let form_height = (height * 0.6).clamp(160.0, (height - 172.0).max(160.0));
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, form_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            self.scrolling_workspace(ui, editable, form_height);
                        },
                    );
                    ui.add_space(12.0);
                    let queue_height = (height - form_height - 12.0).max(160.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, queue_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            self.queue_ui(ui, queue_height);
                        },
                    );
                }
            });
    }
    fn scrolling_workspace(&mut self, ui: &mut Ui, editable: bool, height: f32) {
        egui::ScrollArea::vertical()
            .id_source("workflow_form")
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| self.workspace(ui, editable));
    }

    fn workspace(&mut self, ui: &mut Ui, editable: bool) {
        egui::Frame::none()
            .inner_margin(egui::Margin {
                left: 40.0,
                right: 28.0,
                top: 32.0,
                bottom: 32.0,
            })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                theme::page_header(ui, None, self.studio.title(), self.studio.blurb());
                ui.add_enabled_ui(editable, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for studio in [
                            Studio::Pdf,
                            Studio::Subtitles,
                            Studio::Documents,
                            Studio::Downloads,
                        ] {
                            if theme::chip(
                                ui,
                                studio.title(),
                                self.studio == studio,
                                Some(studio.color()),
                            )
                            .clicked()
                                && self.studio != studio
                            {
                                self.open(studio.default_kind(), Vec::new());
                            }
                        }
                    });
                    ui.add_space(14.0);
                    let specs = tools();
                    let studio = self.studio;
                    theme::section(ui, "Tool", None, |ui| {
                        let current = specs.iter().find(|tool| tool.kind == self.kind);
                        ui.horizontal(|ui| {
                            theme::icon_tile(ui, studio.icon(), studio.color(), 34.0);
                            egui::ComboBox::from_id_source("workflow_operation")
                                .width(ui.available_width() - 4.0)
                                .selected_text(
                                    RichText::new(
                                        current
                                            .map(|tool| tool.title.as_str())
                                            .unwrap_or("Choose a tool"),
                                    )
                                    .font(theme::semibold(14.0)),
                                )
                                .show_ui(ui, |ui| {
                                    for tool in specs
                                        .iter()
                                        .filter(|tool| studio_for(tool.kind) == self.studio)
                                    {
                                        if ui
                                            .selectable_value(
                                                &mut self.kind,
                                                tool.kind,
                                                &tool.title,
                                            )
                                            .clicked()
                                        {
                                            self.form.reset_for(self.kind);
                                            ui.memory_mut(|memory| memory.close_popup());
                                        }
                                    }
                                });
                        });
                        if let Some(tool) = specs.iter().find(|tool| tool.kind == self.kind) {
                            theme::hint(ui, &tool.description);
                        }
                    });
                    if self.kind == WorkflowKind::Download {
                        theme::section(ui, "Link", None, |ui| self.link_ui(ui));
                    } else {
                        theme::section(ui, "Files", None, |ui| self.files_ui(ui));
                    }
                    self.inspector(ui);
                    theme::section(ui, "Output", None, |ui| {
                        output_picker(ui, &mut self.output_dir)
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let label = if self.pending.is_empty() {
                            "Run".to_string()
                        } else {
                            format!("Run queue ({})", self.pending.len())
                        };
                        if ui
                            .add(theme::primary_button(label).min_size(egui::vec2(140.0, 40.0)))
                            .clicked()
                        {
                            self.start(ui.ctx().clone());
                        }
                        if ui
                            .add(
                                theme::secondary_button("Add to queue")
                                    .min_size(egui::vec2(0.0, 40.0)),
                            )
                            .clicked()
                        {
                            match self.requests() {
                                Ok(requests) => {
                                    let count = requests.len();
                                    self.pending.extend(requests);
                                    self.notice =
                                        Some(format!("{} added to queue", task_count(count)));
                                }
                                Err(e) => self.notice = Some(e.to_string()),
                            }
                        }
                    });
                });
                if !editable && !self.is_running() {
                    ui.label(
                        RichText::new("Another task is running")
                            .size(12.0)
                            .color(MUTED),
                    );
                }
                if let Some(notice) = &self.notice {
                    ui.add_space(8.0);
                    ui.add(
                        egui::Label::new(RichText::new(notice).size(12.0).color(MUTED)).wrap(true),
                    );
                }
                ui.add_space(14.0);
                ui.collapsing(
                    RichText::new("Tool availability")
                        .font(theme::semibold(13.0))
                        .color(theme::TEXT_2),
                    |ui| {
                        for dependency in file_workflows::dependencies() {
                            ui.label(
                                RichText::new(format!(
                                    "{} · {}",
                                    dependency.name,
                                    if dependency.available {
                                        "Ready"
                                    } else {
                                        "Not installed"
                                    }
                                ))
                                .size(12.0)
                                .color(MUTED),
                            )
                            .on_hover_text(
                                dependency
                                    .path
                                    .map(|p| theme::display_path(&p))
                                    .unwrap_or_default(),
                            );
                        }
                    },
                );
            });
    }
    fn files_ui(&mut self, ui: &mut Ui) {
        if self.inputs.is_empty() {
            let hovered = ui.ctx().input(|input| !input.raw.hovered_files.is_empty());
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 176.0),
                egui::Sense::hover(),
            );
            let painter = ui.painter();
            painter.rect_filled(
                rect,
                theme::RADIUS_LG,
                if hovered {
                    theme::ACCENT_SOFT
                } else {
                    theme::SUNKEN
                },
            );
            theme::dashed_rect(
                painter,
                rect.shrink(0.5),
                theme::RADIUS_LG,
                egui::Stroke::new(
                    1.2_f32,
                    if hovered {
                        theme::ACCENT
                    } else {
                        theme::LINE_STRONG
                    },
                ),
            );
            let tile = egui::Rect::from_center_size(
                egui::pos2(rect.center().x, rect.top() + 38.0),
                egui::vec2(40.0, 40.0),
            );
            theme::paint_shadow(painter, tile, 11.0);
            painter.rect(tile, 11.0, SURFACE, egui::Stroke::new(1.0_f32, LINE));
            theme::paint_icon(
                painter,
                tile.shrink(11.0),
                self.studio.icon(),
                self.studio.color(),
            );
            painter.text(
                egui::pos2(rect.center().x, rect.top() + 80.0),
                egui::Align2::CENTER_CENTER,
                if hovered {
                    "Release to add your files"
                } else {
                    "Drop files or folders here"
                },
                theme::semibold(15.0),
                TEXT,
            );
            let y = rect.top() + 130.0;
            if ui
                .put(
                    egui::Rect::from_center_size(
                        egui::pos2(rect.center().x - 64.0, y),
                        egui::vec2(120.0, 34.0),
                    ),
                    theme::primary_button("Add files"),
                )
                .clicked()
            {
                if let Some(paths) = rfd::FileDialog::new().pick_files() {
                    self.import(paths);
                }
            }
            if ui
                .put(
                    egui::Rect::from_center_size(
                        egui::pos2(rect.center().x + 64.0, y),
                        egui::vec2(120.0, 34.0),
                    ),
                    theme::secondary_button("Add folder"),
                )
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.import(vec![path]);
                }
            }
            ui.advance_cursor_after_rect(rect);
            return;
        }
        ui.horizontal(|ui| {
            theme::badge(
                ui,
                &format!("{} selected", self.inputs.len()),
                theme::Tone::Accent,
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(theme::small_button("Clear")).clicked() {
                    self.inputs.clear();
                }
                if ui.add(theme::small_button("Add folder")).clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.import(vec![path]);
                    }
                }
                if ui.add(theme::small_button("Add files")).clicked() {
                    if let Some(paths) = rfd::FileDialog::new().pick_files() {
                        self.import(paths);
                    }
                }
            });
        });
        {
            let mut remove = None;
            egui::ScrollArea::vertical()
                .id_source("workflow_inputs")
                .max_height(155.0)
                .show(ui, |ui| {
                    for (index, path) in self.inputs.iter().enumerate() {
                        ui.horizontal(|ui| {
                            theme::icon_tile(ui, self.studio.icon(), self.studio.color(), 26.0);
                            let width = (ui.available_width() - 40.0).max(80.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(width, 26.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_width(width);
                                    ui.add(
                                        egui::Label::new(RichText::new(name(path)).size(13.0))
                                            .truncate(true),
                                    )
                                    .on_hover_text(path.display().to_string());
                                },
                            );
                            if ui
                                .add(theme::ghost_button("✕"))
                                .on_hover_text("Remove")
                                .clicked()
                            {
                                remove = Some(index);
                            }
                        });
                    }
                });
            if let Some(index) = remove {
                self.inputs.remove(index);
            }
            if matches!(
                self.kind,
                WorkflowKind::PdfMerge | WorkflowKind::ImagesToPdf
            ) {
                ui.label(
                    RichText::new("Files are combined in the order shown.")
                        .size(11.0)
                        .color(MUTED),
                );
                ui.horizontal(|ui| {
                    if ui.add(theme::small_button("Reverse order")).clicked() {
                        self.inputs.reverse();
                    }
                });
            }
        }
    }
    fn link_ui(&mut self, ui: &mut Ui) {
        theme::wide_input(ui, &mut self.form.url, "https://…");
        ui.horizontal(|ui| {
            if ui
                .selectable_value(&mut self.form.mode, "direct".into(), "Direct file")
                .clicked()
            {
                self.form.format = "original".into();
            }
            ui.selectable_value(&mut self.form.mode, "media".into(), "Video or audio");
        });
        ui.label(
            RichText::new(if self.form.mode == "direct" {
                "Download a file from its direct link."
            } else {
                "Download video or audio from a supported media link."
            })
            .size(12.0)
            .color(MUTED),
        );
        field(ui, "Filename (optional)", &mut self.form.filename);
    }
    fn inspector(&mut self, ui: &mut Ui) {
        use WorkflowKind::*;
        let kind = self.kind;
        theme::section(ui, "Options", None, |ui| {
            match kind {
                PdfMerge | ImagesToPdf => {
                    ui.label(
                        RichText::new("Combine the selected files into one PDF.")
                            .size(12.0)
                            .color(MUTED),
                    );
                }
                PdfSplit => {
                    ui.horizontal(|ui| {
                        ui.label("Pages per file");
                        ui.add(
                            egui::DragValue::new(&mut self.form.chunk_size).clamp_range(1..=5000),
                        );
                    });
                    field(ui, "Page ranges (optional)", &mut self.form.ranges);
                    ui.label(
                        RichText::new("Separate output files with ; — for example 1-2; 3-5")
                            .size(11.0)
                            .color(MUTED),
                    );
                }
                PdfRotate => {
                    ui.horizontal(|ui| {
                        ui.label("Rotation");
                        for angle in [90, 180, 270] {
                            ui.selectable_value(&mut self.form.angle, angle, format!("{angle}°"));
                        }
                    });
                }
                PdfCompress => {
                    choice(
                        ui,
                        "Quality",
                        &mut self.form.preset,
                        &["screen", "ebook", "printer", "prepress"],
                    );
                    ui.label(
                        RichText::new("Smaller files can reduce image quality.")
                            .size(11.0)
                            .color(MUTED),
                    );
                }
                PdfWatermark => {
                    field(ui, "Text", &mut self.form.text);
                    ui.horizontal(|ui| {
                        ui.label("Opacity");
                        ui.add(egui::Slider::new(&mut self.form.opacity, 0.05..=1.0));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Font size");
                        ui.add(egui::DragValue::new(&mut self.form.font_size).clamp_range(6..=144));
                        ui.label("Angle");
                        ui.add(egui::DragValue::new(&mut self.form.angle).clamp_range(-180..=180));
                    });
                }
                PdfMetadata => {
                    ui.checkbox(&mut self.form.strip, "Remove existing metadata");
                    field(ui, "Title", &mut self.form.title);
                    field(ui, "Author", &mut self.form.author);
                    field(ui, "Subject", &mut self.form.subject);
                    field(ui, "Keywords", &mut self.form.keywords);
                }
                PdfToImages => {
                    choice(ui, "Format", &mut self.form.format, &["png", "jpg"]);
                    ui.horizontal(|ui| {
                        ui.label("Resolution (DPI)");
                        ui.add(egui::DragValue::new(&mut self.form.dpi).clamp_range(36..=300));
                    });
                }
                PdfToText => {
                    ui.label(RichText::new("Extract selectable text. Scanned pages need OCR, which is not included.").size(12.0).color(MUTED));
                }
                SubtitleConvert | SubtitleShift | SubtitleFps | SubtitleClean => {
                    choice(
                        ui,
                        "Format",
                        &mut self.form.format,
                        &[
                            "srt", "vtt", "ass", "ssa", "sbv", "sub", "smi", "lrc", "txt", "csv",
                            "json",
                        ],
                    );
                    choice(
                        ui,
                        "Input encoding",
                        &mut self.form.encoding,
                        &[
                            "auto",
                            "utf-8",
                            "utf-16",
                            "gb18030",
                            "gbk",
                            "big5",
                            "shift_jis",
                        ],
                    );
                    ui.label(
                        RichText::new(
                            "Text and timing only. Styles and karaoke are not preserved.",
                        )
                        .size(11.0)
                        .color(MUTED),
                    );
                    if kind != SubtitleFps
                        && (self.form.format.eq_ignore_ascii_case("sub")
                            || self.inputs.iter().any(|input| is_microdvd(input)))
                    {
                        if self.inputs.iter().any(|input| is_microdvd(input)) {
                            ui.horizontal(|ui| {
                                ui.label("SUB input FPS (without header)");
                                ui.add(
                                    egui::DragValue::new(&mut self.form.source_fps)
                                        .speed(0.001)
                                        .clamp_range(1.0..=240.0),
                                );
                            });
                        }
                        if self.form.format.eq_ignore_ascii_case("sub") {
                            ui.horizontal(|ui| {
                                ui.label("SUB output FPS");
                                ui.add(
                                    egui::DragValue::new(&mut self.form.target_fps)
                                        .speed(0.001)
                                        .clamp_range(1.0..=240.0),
                                );
                            });
                        }
                        ui.label(RichText::new("Embedded FPS headers take precedence. Frame numbering changes without retiming.").size(11.0).color(MUTED));
                    }
                    match kind {
                        SubtitleShift => {
                            ui.horizontal(|ui| {
                                ui.label("Shift (milliseconds)");
                                ui.add(egui::DragValue::new(&mut self.form.offset_ms).speed(100));
                            });
                        }
                        SubtitleFps => {
                            ui.horizontal(|ui| {
                                ui.label("Source FPS");
                                ui.add(
                                    egui::DragValue::new(&mut self.form.source_fps)
                                        .speed(0.001)
                                        .clamp_range(1.0..=240.0),
                                );
                                ui.label("Target FPS");
                                ui.add(
                                    egui::DragValue::new(&mut self.form.target_fps)
                                        .speed(0.001)
                                        .clamp_range(1.0..=240.0),
                                );
                            });
                        }
                        SubtitleClean => {
                            ui.checkbox(&mut self.form.clean, "Remove styling and formatting tags");
                        }
                        _ => {}
                    }
                }
                DocumentConvert => {
                    choice(
                        ui,
                        "Format",
                        &mut self.form.format,
                        &["txt", "md", "html", "docx", "pdf", "epub"],
                    );
                    choice(
                        ui,
                        "Input encoding",
                        &mut self.form.encoding,
                        &["utf-8", "utf-16", "gb18030", "gbk", "big5", "shift_jis"],
                    );
                    field(ui, "Title (optional)", &mut self.form.title);
                }
                Download => {
                    ui.horizontal(|ui| {
                        ui.label("Maximum size (MB)");
                        ui.add(egui::DragValue::new(&mut self.form.max_mb).clamp_range(1..=102400));
                    });
                    if self.form.mode == "media" {
                        choice(
                            ui,
                            "Format",
                            &mut self.form.format,
                            &["original", "mp4", "mp3", "m4a", "webm"],
                        );
                    }
                }
            }
            if matches!(kind, PdfRotate | PdfWatermark | PdfToImages | PdfToText) {
                field(ui, "Pages (blank = all)", &mut self.form.pages);
                ui.label(RichText::new("Example: 1,3-5").size(11.0).color(MUTED));
            }
        });
    }
    fn import(&mut self, paths: Vec<PathBuf>) {
        let mut pending = paths.into_iter().rev().collect::<Vec<_>>();
        let mut visited = 0;
        let mut skipped = 0;
        while let Some(path) = pending.pop() {
            if self.inputs.len() >= 1000 || visited >= 20000 {
                self.notice =
                    Some("Import limit reached (1,000 files). Choose a smaller folder.".into());
                return;
            }
            visited += 1;
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                skipped += 1;
                continue;
            };
            if metadata.file_type().is_symlink() {
                skipped += 1;
                continue;
            }
            if metadata.is_dir() {
                if let Ok(entries) = std::fs::read_dir(path) {
                    let mut children = entries
                        .take(20000)
                        .filter_map(Result::ok)
                        .map(|e| e.path())
                        .collect::<Vec<_>>();
                    children.sort();
                    pending.extend(children.into_iter().rev());
                }
            } else if accepts(self.kind, &path) {
                if !self.inputs.contains(&path) {
                    self.inputs.push(path);
                }
            } else {
                skipped += 1;
            }
        }
        if skipped > 0 {
            self.notice = Some(format!("Skipped {skipped} unsupported or linked items"));
        }
    }
    fn requests(&self) -> anyhow::Result<Vec<WorkflowRequest>> {
        anyhow::ensure!(
            !self.output_dir.trim().is_empty(),
            "Choose an output folder"
        );
        let options = self.form.options(self.kind)?;
        if self.kind == WorkflowKind::Download {
            return Ok(vec![WorkflowRequest {
                kind: self.kind,
                inputs: Vec::new(),
                output_dir: PathBuf::from(&self.output_dir),
                options,
            }]);
        }
        anyhow::ensure!(!self.inputs.is_empty(), "Add files first");
        anyhow::ensure!(
            self.inputs.iter().all(|p| accepts(self.kind, p)),
            "Remove files that do not match this tool"
        );
        if matches!(
            self.kind,
            WorkflowKind::PdfMerge | WorkflowKind::ImagesToPdf
        ) {
            anyhow::ensure!(
                self.kind != WorkflowKind::PdfMerge || self.inputs.len() >= 2,
                "Add at least two PDFs to merge"
            );
            return Ok(vec![WorkflowRequest {
                kind: self.kind,
                inputs: self.inputs.clone(),
                output_dir: PathBuf::from(&self.output_dir),
                options,
            }]);
        }
        self.inputs
            .iter()
            .map(|input| {
                let mut options = options.clone();
                if matches!(
                    self.kind,
                    WorkflowKind::SubtitleConvert
                        | WorkflowKind::SubtitleShift
                        | WorkflowKind::SubtitleClean
                ) && is_microdvd(input)
                {
                    let source_fps = microdvd_header_fps(input).unwrap_or(self.form.source_fps);
                    anyhow::ensure!(
                        source_fps.is_finite() && source_fps > 0.0,
                        "MicroDVD source FPS must be positive"
                    );
                    options
                        .as_object_mut()
                        .unwrap()
                        .insert("source_fps".into(), json!(source_fps));
                }
                Ok(WorkflowRequest {
                    kind: self.kind,
                    inputs: vec![input.clone()],
                    output_dir: PathBuf::from(&self.output_dir),
                    options,
                })
            })
            .collect()
    }
    fn start(&mut self, ctx: egui::Context) {
        if self.is_running() || *self.processing.lock().unwrap() {
            self.notice = Some("Wait for the active task to finish".into());
            return;
        }
        let requests = if self.pending.is_empty() {
            match self.requests() {
                Ok(requests) => requests,
                Err(error) => {
                    self.notice = Some(error.to_string());
                    return;
                }
            }
        } else {
            std::mem::take(&mut self.pending)
        };
        {
            let mut busy = self.processing.lock().unwrap();
            if *busy {
                return;
            }
            *busy = true;
        }
        self.queue = requests
            .iter()
            .map(|request| Job {
                kind: request.kind,
                title: tools()
                    .into_iter()
                    .find(|tool| tool.kind == request.kind)
                    .map(|tool| tool.title)
                    .unwrap_or_else(|| "Conversion".into()),
                state: JobState::Queued,
                progress: 0.0,
                source: if request.kind == WorkflowKind::Download {
                    request.options["url"].as_str().unwrap_or("").to_string()
                } else {
                    request
                        .inputs
                        .iter()
                        .map(|p| name(p))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                message: String::new(),
                outputs: Vec::new(),
                show_all_outputs: false,
            })
            .collect();
        let cancel = CancelToken::default();
        self.cancel = Some(cancel.clone());
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.notice = None;
        let processing = self.processing.clone();
        *self.status.lock().unwrap() = "Running workflow queue".into();
        self.worker = Some(thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_queue(&requests, &cancel, |event| {
                    let _ = sender.send(Message::Event(event));
                    ctx.request_repaint();
                })
            }));
            *processing.lock().unwrap() = false;
            let _ = sender.send(match result {
                Ok(report) => Message::Finished(report),
                Err(_) => Message::Panicked,
            });
            ctx.request_repaint();
        }));
    }
    fn queue_ui(&mut self, ui: &mut Ui, height: f32) {
        egui::Frame::none()
            .fill(theme::SUNKEN)
            .stroke(egui::Stroke::new(1.0_f32, LINE))
            .rounding(theme::RADIUS_LG)
            .inner_margin(18.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height((height - 36.0).max(120.0));
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Queue")
                            .font(theme::semibold(15.0))
                            .color(TEXT),
                    );
                    if !self.pending.is_empty() {
                        theme::badge(
                            ui,
                            &format!("{} ready", task_count(self.pending.len())),
                            theme::Tone::Accent,
                        );
                    }
                });
                if !self.pending.is_empty()
                    && ui.add(theme::small_button("Clear pending")).clicked()
                {
                    self.pending.clear();
                }
                if self.is_running() {
                    let cancelling = self.cancel.as_ref().is_some_and(CancelToken::is_cancelled);
                    if ui
                        .add_enabled(
                            !cancelling,
                            theme::danger_button(if cancelling {
                                "Cancelling…"
                            } else {
                                "Cancel"
                            })
                            .min_size(egui::vec2(ui.available_width(), 32.0)),
                        )
                        .clicked()
                    {
                        if let Some(cancel) = &self.cancel {
                            cancel.cancel();
                        }
                    }
                }
                let remaining = ui.available_height().max(60.0);
                egui::ScrollArea::vertical()
                    .id_source("workflow_queue_results")
                    .max_height(remaining)
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.queue_results(ui));
            });
    }
    fn queue_results(&mut self, ui: &mut Ui) {
        if self.queue.is_empty() {
            ui.add_space(140.0);
            theme::empty_state(
                ui,
                Icon::Queue,
                "Nothing running yet",
                "Tasks you run appear here.",
            );
        }
        let mut error = None;
        for (index, job) in self.queue.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                theme::card()
                    .rounding(theme::RADIUS + 2.0)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            theme::icon_tile(
                                ui,
                                studio_for(job.kind).icon(),
                                studio_for(job.kind).color(),
                                26.0,
                            );
                            ui.label(RichText::new(&job.title).font(theme::semibold(13.0)));
                        });
                        ui.add(
                            egui::Label::new(RichText::new(&job.source).size(12.0).color(MUTED))
                                .truncate(true),
                        )
                        .on_hover_text(&job.source);
                        let active = matches!(job.state, JobState::Running | JobState::Validating);
                        ui.horizontal(|ui| {
                            theme::badge(
                                ui,
                                &format!("{:?}", job.state),
                                match job.state {
                                    JobState::Failed => theme::Tone::Danger,
                                    JobState::Completed => theme::Tone::Success,
                                    JobState::Running | JobState::Validating => theme::Tone::Accent,
                                    _ => theme::Tone::Neutral,
                                },
                            );
                            if active {
                                ui.spinner();
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(format!("{:.0}%", job.progress * 100.0))
                                                .size(11.0)
                                                .color(MUTED),
                                        );
                                    },
                                );
                            }
                        });
                        if active {
                            ui.add(
                                egui::ProgressBar::new(job.progress)
                                    .desired_height(4.0)
                                    .fill(theme::ACCENT),
                            );
                        }
                        if !job.message.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&job.message).size(11.0).color(MUTED),
                                )
                                .truncate(true),
                            )
                            .on_hover_text(&job.message);
                        }
                        let shown = if job.show_all_outputs {
                            job.outputs.len()
                        } else {
                            20
                        };
                        for output in job.outputs.iter().take(shown) {
                            ui.add(
                                egui::Label::new(RichText::new(name(output)).size(12.0))
                                    .truncate(true),
                            )
                            .on_hover_text(output.display().to_string());
                            ui.horizontal_wrapped(|ui| {
                                let label = if output
                                    .extension()
                                    .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
                                {
                                    "Preview"
                                } else {
                                    "Open"
                                };
                                if ui.add(theme::small_button(label)).clicked() {
                                    if let Err(e) = open_path(output) {
                                        error = Some(e.to_string());
                                    }
                                }
                                if ui.add(theme::small_button("Folder")).clicked() {
                                    if let Some(parent) = output.parent() {
                                        if let Err(e) = open_path(parent) {
                                            error = Some(e.to_string());
                                        }
                                    }
                                }
                                if ui.add(theme::small_button("Copy path")).clicked() {
                                    ui.output_mut(|out| {
                                        out.copied_text = output.display().to_string()
                                    });
                                }
                            });
                        }
                        if job.outputs.len() > 20 {
                            let label = if job.show_all_outputs {
                                "Show fewer outputs".to_string()
                            } else {
                                format!("Show all {} outputs", job.outputs.len())
                            };
                            if ui.add(theme::small_button(label)).clicked() {
                                job.show_all_outputs = !job.show_all_outputs;
                            }
                        }
                    });
            });
            ui.add_space(8.0);
        }
        if error.is_some() {
            self.notice = error;
        }
        if !self.is_running()
            && !self.queue.is_empty()
            && ui.add(button("Clear results", false)).clicked()
        {
            self.queue.clear();
        }
    }
}
impl Drop for WorkflowTab {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn is_microdvd(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sub"))
}

/// Inspect only a small text prefix; the backend still validates the full file.
fn microdvd_header_fps(path: &Path) -> Option<f64> {
    let mut prefix = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(4096)
        .read_to_end(&mut prefix)
        .ok()?;
    let text = if prefix.starts_with(&[0xff, 0xfe]) || prefix.starts_with(&[0xfe, 0xff]) {
        let little = prefix[0] == 0xff;
        let bytes = &prefix[2..];
        let units = bytes
            .iter()
            .step_by(2)
            .zip(bytes.iter().skip(1).step_by(2))
            .map(|(&first, &second)| {
                if little {
                    u16::from_le_bytes([first, second])
                } else {
                    u16::from_be_bytes([first, second])
                }
            })
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(&prefix).into_owned()
    };
    let first = text
        .trim_start_matches('\u{feff}')
        .lines()
        .find(|line| !line.trim().is_empty())?
        .trim();
    let fps = first.strip_prefix("{1}{1}")?.trim().parse::<f64>().ok()?;
    (fps.is_finite() && fps > 0.0).then_some(fps)
}

fn accepts(kind: WorkflowKind, path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    static EXTENSIONS: std::sync::OnceLock<std::collections::HashMap<WorkflowKind, Vec<String>>> =
        std::sync::OnceLock::new();
    EXTENSIONS
        .get_or_init(|| {
            tools()
                .into_iter()
                .map(|tool| (tool.kind, tool.input_extensions))
                .collect()
        })
        .get(&kind)
        .is_some_and(|extensions| {
            extensions
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(&ext))
        })
}
fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
fn field(ui: &mut Ui, label: &str, value: &mut String) {
    theme::field(ui, label, |ui| {
        theme::wide_input(ui, value, "");
    });
}
fn task_count(count: usize) -> String {
    format!("{count} {}", if count == 1 { "task" } else { "tasks" })
}

fn choice(ui: &mut Ui, label: &str, value: &mut String, values: &[&str]) {
    theme::field(ui, label, |ui| {
        egui::ComboBox::from_id_source(label)
            .selected_text(value.to_uppercase())
            .show_ui(ui, |ui| {
                for option in values {
                    if ui
                        .selectable_value(value, (*option).to_string(), option.to_uppercase())
                        .clicked()
                    {
                        ui.memory_mut(|memory| memory.close_popup());
                    }
                }
            });
    });
}
fn button(label: &str, primary: bool) -> egui::Button<'static> {
    if primary {
        theme::primary_button(label)
    } else {
        theme::secondary_button(label)
    }
}
fn output_picker(ui: &mut Ui, output: &mut String) {
    ui.horizontal(|ui| {
        theme::icon(ui, Icon::Folder, 17.0, MUTED);
        let width = (ui.available_width() - 90.0).max(60.0);
        ui.allocate_ui_with_layout(
            egui::vec2(width, 28.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_width(width);
                ui.add(
                    egui::Label::new(RichText::new(name(Path::new(output))).size(13.5))
                        .truncate(true),
                )
                .on_hover_text(output.as_str());
            },
        );
        if ui.add(theme::small_button("Choose…")).clicked() {
            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                *output = folder.display().to_string();
            }
        }
    });
    ui.collapsing(RichText::new("Edit path").size(12.5).color(MUTED), |ui| {
        theme::wide_input(ui, output, "Output folder");
    });
}
fn open_path(path: &Path) -> std::io::Result<()> {
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
            "Opening files is unsupported",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tab() -> WorkflowTab {
        WorkflowTab::new(
            Arc::new(Mutex::new("Ready".into())),
            Arc::new(Mutex::new(false)),
        )
    }

    #[test]
    fn each_operation_emits_only_options_declared_by_its_backend_schema() {
        for spec in tools() {
            let mut form = Form::default();
            form.reset_for(spec.kind);
            form.text = "Watermark".into();
            form.url = "https://example.com/fixture.txt".into();
            let options = form.options(spec.kind).unwrap();
            for key in options.as_object().unwrap().keys() {
                assert!(
                    spec.options_schema["properties"].get(key).is_some(),
                    "{} sent unsupported field {key}",
                    spec.title
                );
            }
            if matches!(
                spec.kind,
                WorkflowKind::PdfMerge | WorkflowKind::ImagesToPdf
            ) {
                assert_eq!(options, json!({}));
            }
            if spec.kind == WorkflowKind::SubtitleShift {
                assert!(options.get("offset_ms").is_some());
                assert!(options.get("clean").is_none());
                assert!(options.get("source_fps").is_none());
            }
        }
    }
    #[test]
    fn merge_preserves_file_order_while_independent_conversions_become_separate_jobs() {
        let mut tab = tab();
        tab.kind = WorkflowKind::PdfMerge;
        tab.inputs = vec!["second.pdf".into(), "first.pdf".into()];
        let requests = tab.requests().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].inputs, tab.inputs);
        tab.kind = WorkflowKind::SubtitleConvert;
        tab.form.reset_for(tab.kind);
        tab.inputs = vec!["first.srt".into(), "second.vtt".into()];
        let requests = tab.requests().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].inputs, vec![PathBuf::from("first.srt")]);
        assert_eq!(requests[1].inputs, vec![PathBuf::from("second.vtt")]);
    }
    #[test]
    fn queued_request_keeps_its_options_and_destination_when_form_changes() {
        let mut tab = tab();
        tab.kind = WorkflowKind::SubtitleShift;
        tab.form.reset_for(tab.kind);
        tab.form.offset_ms = -750;
        tab.inputs = vec!["captions.srt".into()];
        tab.output_dir = "chosen-output".into();
        let requests = tab.requests().unwrap();
        tab.form.offset_ms = 4000;
        tab.form.format = "vtt".into();
        tab.output_dir = "other-output".into();
        tab.inputs.clear();
        assert_eq!(requests[0].options["offset_ms"], -750);
        assert_eq!(requests[0].options["format"], "srt");
        assert_eq!(requests[0].output_dir, PathBuf::from("chosen-output"));
        assert_eq!(requests[0].inputs, vec![PathBuf::from("captions.srt")]);
    }
    #[test]
    fn invalid_forms_fail_before_claiming_gate_or_starting_worker() {
        let mut tab = tab();
        tab.start(egui::Context::default());
        assert!(!tab.is_running());
        assert!(!*tab.processing.lock().unwrap());
        let mut form = Form::default();
        assert!(form.options(WorkflowKind::PdfWatermark).is_err());
        assert!(form.options(WorkflowKind::Download).is_err());
        form.source_fps = f64::NAN;
        assert!(form.options(WorkflowKind::SubtitleFps).is_err());
        tab.kind = WorkflowKind::PdfMerge;
        tab.inputs = vec!["single.pdf".into()];
        assert!(tab.requests().is_err());
        tab.kind = WorkflowKind::Download;
        tab.form.reset_for(tab.kind);
        tab.form.url = "https://example.com/file.txt".into();
        let request = tab.requests().unwrap();
        assert!(request[0].inputs.is_empty());
        assert_eq!(request[0].options["format"], "original");
    }
    #[test]
    fn busy_gate_preserves_pending_work_instead_of_starting_a_second_worker() {
        let mut tab = tab();
        tab.pending.push(WorkflowRequest {
            kind: WorkflowKind::Download,
            inputs: Vec::new(),
            output_dir: "out".into(),
            options: json!({"url":"https://example.com/a"}),
        });
        *tab.processing.lock().unwrap() = true;
        tab.start(egui::Context::default());
        assert!(!tab.is_running());
        assert_eq!(tab.pending.len(), 1);
        assert!(*tab.processing.lock().unwrap());
    }
    #[test]
    fn split_ranges_replace_chunk_size_in_the_adapter() {
        let form = Form {
            ranges: "1-2; 3; 5-7".into(),
            ..Form::default()
        };
        assert_eq!(
            form.options(WorkflowKind::PdfSplit).unwrap(),
            json!({"ranges":["1-2","3","5-7"]})
        );
    }
    #[test]
    fn microdvd_conversion_shift_and_clean_emit_frame_rate_options_without_retiming() {
        for kind in [
            WorkflowKind::SubtitleConvert,
            WorkflowKind::SubtitleShift,
            WorkflowKind::SubtitleClean,
        ] {
            let mut tab = tab();
            tab.kind = kind;
            tab.form.reset_for(kind);
            tab.form.format = "sub".into();
            tab.inputs = vec!["source.srt".into()];
            let requests = tab.requests().unwrap();
            assert_eq!(requests[0].kind, kind);
            assert_eq!(requests[0].options["target_fps"], 25.0);
            assert!(requests[0].options.get("source_fps").is_none());
            tab.form.format = "srt".into();
            tab.inputs = vec!["source-without-header.sub".into()];
            let requests = tab.requests().unwrap();
            assert_eq!(requests[0].kind, kind);
            assert_eq!(requests[0].options["source_fps"], 25.0);
            assert!(requests[0].options.get("target_fps").is_none());
            tab.inputs = vec!["plain.srt".into()];
            let requests = tab.requests().unwrap();
            assert!(requests[0].options.get("source_fps").is_none());
            assert!(requests[0].options.get("target_fps").is_none());
        }
    }
    #[test]
    fn embedded_microdvd_fps_takes_precedence_over_the_fallback_form_value() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(
            std::env::temp_dir().join(format!("ui-microdvd-{}-{nonce}.sub", std::process::id())),
        );
        std::fs::write(&fixture.0, b"{1}{1}29.97\n{30}{60}Caption\n").unwrap();
        let mut tab = tab();
        tab.kind = WorkflowKind::SubtitleConvert;
        tab.form.reset_for(tab.kind);
        tab.inputs = vec![fixture.0.clone()];
        let request = tab.requests().unwrap();
        assert_eq!(request[0].options["source_fps"], 29.97);
        assert_eq!(request[0].kind, WorkflowKind::SubtitleConvert);
        assert!(request[0].options.get("target_fps").is_none());
    }
    #[test]
    fn utf16_microdvd_prefix_handles_boms_and_odd_trailing_bytes() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(std::env::temp_dir().join(format!(
            "ui-microdvd-utf16-{}-{nonce}.sub",
            std::process::id()
        )));
        for little in [true, false] {
            let bom = if little { [0xff, 0xfe] } else { [0xfe, 0xff] };
            std::fs::write(&fixture.0, bom).unwrap();
            assert_eq!(microdvd_header_fps(&fixture.0), None);
            let mut encoded = bom.to_vec();
            for unit in "{1}{1}29.97".encode_utf16() {
                encoded.extend_from_slice(&if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            std::fs::write(&fixture.0, &encoded).unwrap();
            assert_eq!(microdvd_header_fps(&fixture.0), Some(29.97));
            // An incomplete final UTF-16 unit is ignored, preserving the header.
            encoded.push(0x7b);
            std::fs::write(&fixture.0, &encoded).unwrap();
            assert_eq!(microdvd_header_fps(&fixture.0), Some(29.97));
        }
    }
    #[test]
    fn pdf_values_outside_backend_limits_fail_before_a_worker_starts() {
        let form = Form {
            dpi: 301,
            ..Form::default()
        };
        assert!(form.options(WorkflowKind::PdfToImages).is_err());
        let form = Form {
            font_size: 145,
            text: "Watermark".into(),
            ..Form::default()
        };
        assert!(form.options(WorkflowKind::PdfWatermark).is_err());
        let form = Form {
            chunk_size: 5001,
            ..Form::default()
        };
        assert!(form.options(WorkflowKind::PdfSplit).is_err());
        let form = Form {
            dpi: 300,
            font_size: 144,
            chunk_size: 5000,
            text: "Watermark".into(),
            ..Form::default()
        };
        assert!(form.options(WorkflowKind::PdfToImages).is_ok());
        assert!(form.options(WorkflowKind::PdfWatermark).is_ok());
        assert!(form.options(WorkflowKind::PdfSplit).is_ok());
    }
    #[test]
    fn closing_studio_cancels_and_joins_its_worker() {
        let mut tab = tab();
        let cancel = CancelToken::default();
        let worker_cancel = cancel.clone();
        let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_observed = observed.clone();
        tab.cancel = Some(cancel.clone());
        tab.worker = Some(thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !worker_cancel.is_cancelled() && std::time::Instant::now() < deadline {
                thread::sleep(Duration::from_millis(2));
            }
            worker_observed.store(
                worker_cancel.is_cancelled(),
                std::sync::atomic::Ordering::SeqCst,
            );
        }));
        drop(tab);
        assert!(cancel.is_cancelled());
        assert!(observed.load(std::sync::atomic::Ordering::SeqCst));
    }
    #[test]
    fn lost_worker_marks_pending_jobs_failed_and_releases_task_gate() {
        let mut tab = tab();
        let (sender, receiver) = mpsc::channel();
        tab.receiver = Some(receiver);
        tab.queue.push(Job {
            kind: WorkflowKind::PdfSplit,
            title: "Split PDF".into(),
            source: "fixture.pdf".into(),
            state: JobState::Running,
            progress: 0.5,
            message: String::new(),
            outputs: Vec::new(),
            show_all_outputs: false,
        });
        *tab.processing.lock().unwrap() = true;
        drop(sender);
        tab.poll(&egui::Context::default());
        assert!(!tab.is_running());
        assert!(!*tab.processing.lock().unwrap());
        assert_eq!(tab.queue[0].state, JobState::Failed);
    }
}
