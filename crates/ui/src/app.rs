use eframe::egui;
use egui::{vec2, Color32, RichText, Sense, Stroke};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::tabs::{
    batch_tab::BatchTab,
    clipper_tab::ClipperTab,
    convert_tab::ConvertTab,
    gif_converter_tab::GifConverterTab,
    gif_transparency_tab::GifTransparencyTab,
    merger_tab::MergerTab,
    plugins_tab::PluginsTab,
    profiles_tab::ProfilesTab,
    splitter_tab::SplitterTab,
    workflow_tab::{Studio, WorkflowTab},
};
use crate::theme::{
    self, Icon, Tone, ACCENT, BG, FAINT, HOVER, LINE, MUTED, SUCCESS, SURFACE, TEXT, TEXT_2,
    WARNING,
};

#[derive(PartialEq)]
pub enum Tab {
    Convert,
    Workflows,
    Editors,
    Settings,
    Agent,
    Clipper,
    GifConverter,
    GifTransparency,
    Splitter,
    Merger,
    Batch,    // New tab
    Profiles, // New tab
    Plugins,  // New tab
}

impl Tab {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "convert" => Tab::Convert,
            "tools" | "workflows" => Tab::Workflows,
            "editors" => Tab::Editors,
            "settings" => Tab::Settings,
            "agent" => Tab::Agent,
            "clipper" => Tab::Clipper,
            "gif" => Tab::GifConverter,
            "transparency" => Tab::GifTransparency,
            "splitter" => Tab::Splitter,
            "merger" => Tab::Merger,
            "batch" => Tab::Batch,
            "profiles" => Tab::Profiles,
            "plugins" => Tab::Plugins,
            _ => return None,
        })
    }
}

pub struct VideoToolKitApp {
    active_tab: Tab,
    status: Arc<Mutex<String>>,
    processing: Arc<Mutex<bool>>,
    ffmpeg_available: bool,

    convert_tab: ConvertTab,
    workflow_tab: WorkflowTab,
    clipper_tab: ClipperTab,
    gif_converter_tab: GifConverterTab,
    gif_transparency_tab: GifTransparencyTab,
    splitter_tab: SplitterTab,
    merger_tab: MergerTab,
    batch_tab: BatchTab,       // New tab
    profiles_tab: ProfilesTab, // New tab
    plugins_tab: PluginsTab,   // New tab
    capture: Option<crate::capture::Capture>,
}

impl Default for VideoToolKitApp {
    fn default() -> Self {
        Self::with_profiles_directory(None)
    }
}

impl VideoToolKitApp {
    pub fn with_profiles_directory(directory: Option<&Path>) -> Self {
        let status = Arc::new(Mutex::new("Ready".to_string()));
        let processing = Arc::new(Mutex::new(false));

        Self {
            active_tab: crate::capture::Capture::page()
                .and_then(|page| Tab::from_name(&page))
                .unwrap_or(Tab::Convert),
            status: Arc::clone(&status),
            processing: Arc::clone(&processing),
            ffmpeg_available: file_conversion::dependencies()
                .iter()
                .any(|tool| tool.name == "FFmpeg" && tool.available),

            convert_tab: ConvertTab::new(Arc::clone(&status), Arc::clone(&processing)),
            workflow_tab: WorkflowTab::new(Arc::clone(&status), Arc::clone(&processing)),
            clipper_tab: ClipperTab::new(Arc::clone(&status), Arc::clone(&processing)),
            gif_converter_tab: GifConverterTab::new(Arc::clone(&status), Arc::clone(&processing)),
            gif_transparency_tab: GifTransparencyTab::new(
                Arc::clone(&status),
                Arc::clone(&processing),
            ),
            splitter_tab: SplitterTab::new(Arc::clone(&status), Arc::clone(&processing)),
            merger_tab: MergerTab::new(Arc::clone(&status), Arc::clone(&processing)),
            batch_tab: BatchTab::new(Arc::clone(&status), Arc::clone(&processing)),
            profiles_tab: ProfilesTab::with_directory(
                Arc::clone(&status),
                Arc::clone(&processing),
                directory,
            ),
            plugins_tab: PluginsTab::new(Arc::clone(&status), Arc::clone(&processing)),
            capture: crate::capture::Capture::from_env(),
        }
    }
}

/// Sidebar destination: page, icon, label.
type NavEntry = (Tab, Icon, &'static str);

struct EditorTool {
    tab: Tab,
    icon: Icon,
    hue: Color32,
    title: &'static str,
    description: &'static str,
}

const EDITOR_TOOLS: [EditorTool; 5] = [
    EditorTool {
        tab: Tab::Clipper,
        icon: Icon::Scissors,
        hue: theme::HUE_VIDEO,
        title: "Clip video",
        description: "Extract precise time ranges from a longer recording.",
    },
    EditorTool {
        tab: Tab::GifConverter,
        icon: Icon::Play,
        hue: theme::HUE_SUBTITLE,
        title: "Make a GIF",
        description: "Turn a video into a looping GIF with frame rate and size control.",
    },
    EditorTool {
        tab: Tab::GifTransparency,
        icon: Icon::Image,
        hue: theme::HUE_IMAGE,
        title: "GIF transparency",
        description: "Make the background of an animated GIF transparent.",
    },
    EditorTool {
        tab: Tab::Splitter,
        icon: Icon::Grid,
        hue: theme::HUE_DOWNLOAD,
        title: "Split video",
        description: "Divide a wide recording into equal vertical slices.",
    },
    EditorTool {
        tab: Tab::Merger,
        icon: Icon::Audio,
        hue: theme::HUE_AUDIO,
        title: "Merge media",
        description: "Pair a video with a new soundtrack and pick the output format.",
    },
];

impl Tab {
    fn same(&self, other: &Tab) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    fn copy(&self) -> Tab {
        Tab::from_name(self.key()).unwrap_or(Tab::Convert)
    }

    fn key(&self) -> &'static str {
        match self {
            Tab::Convert => "convert",
            Tab::Workflows => "tools",
            Tab::Editors => "editors",
            Tab::Settings => "settings",
            Tab::Agent => "agent",
            Tab::Clipper => "clipper",
            Tab::GifConverter => "gif",
            Tab::GifTransparency => "transparency",
            Tab::Splitter => "splitter",
            Tab::Merger => "merger",
            Tab::Batch => "batch",
            Tab::Profiles => "profiles",
            Tab::Plugins => "plugins",
        }
    }
}

impl VideoToolKitApp {
    fn editors_active(&self) -> bool {
        matches!(
            self.active_tab,
            Tab::Editors
                | Tab::Clipper
                | Tab::GifConverter
                | Tab::GifTransparency
                | Tab::Splitter
                | Tab::Merger
        )
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.add_space(6.0);
        // Brand lockup.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, 9.0, TEXT);
            painter.rect_filled(
                egui::Rect::from_min_size(rect.min, vec2(32.0, 16.0)),
                egui::Rounding {
                    nw: 9.0,
                    ne: 9.0,
                    sw: 0.0,
                    se: 0.0,
                },
                Color32::from_rgb(40, 40, 52),
            );
            theme::paint_icon(painter, rect.shrink(9.5), Icon::Play, Color32::WHITE);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(
                    RichText::new("Video Toolkit")
                        .font(theme::semibold(14.5))
                        .color(TEXT),
                );
                ui.label(RichText::new("Local media studio").size(11.5).color(MUTED));
            });
        });
        ui.add_space(22.0);

        let groups: [(&str, &[NavEntry]); 2] = [
            (
                "Create",
                &[
                    (Tab::Convert, Icon::Home, "Convert"),
                    (Tab::Workflows, Icon::Document, "File tools"),
                    (Tab::Editors, Icon::Scissors, "Editors"),
                    (Tab::Agent, Icon::Sparkle, "Video Agent"),
                ],
            ),
            (
                "Automate",
                &[
                    (Tab::Batch, Icon::Queue, "Batches"),
                    (Tab::Profiles, Icon::Folder, "Profiles"),
                    (Tab::Plugins, Icon::Grid, "Plugins"),
                ],
            ),
        ];
        for (title, items) in groups {
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                theme::overline(ui, title);
            });
            ui.add_space(2.0);
            for (tab, symbol, label) in items {
                let selected = if matches!(tab, Tab::Editors) {
                    self.editors_active()
                } else {
                    self.active_tab.same(tab)
                };
                if nav_item(ui, *symbol, label, selected) {
                    self.active_tab = tab.copy();
                }
            }
            ui.add_space(18.0);
        }

        ui.add_space((ui.available_height() - 108.0).max(8.0));
        let (color, title, body) = if self.ffmpeg_available {
            (SUCCESS, "Engines ready", "FFmpeg found on this device")
        } else {
            (WARNING, "FFmpeg missing", "Install it to convert media")
        };
        let response = egui::Frame::none()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, LINE))
            .rounding(theme::RADIUS + 2.0)
            .inner_margin(egui::Margin::symmetric(12.0, 9.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.spacing_mut().interact_size.y = 16.0;
                theme::status_dot(ui, color, title);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(RichText::new(body).size(11.5).color(MUTED));
                });
            })
            .response
            .interact(Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Open engine settings");
        if response.clicked() {
            self.active_tab = Tab::Settings;
        }
        ui.add_space(8.0);
        if nav_item(
            ui,
            Icon::Settings,
            "Settings",
            matches!(self.active_tab, Tab::Settings),
        ) {
            self.active_tab = Tab::Settings;
        }
    }

    fn editors(&mut self, ui: &mut egui::Ui) {
        theme::page_header(
            ui,
            None,
            "Editors",
            "Focused tools for cutting, looping and combining media.",
        );
        theme::overline(ui, "Media");
        ui.add_space(2.0);
        let mut chosen = None;
        let columns = if ui.available_width() > 900.0 { 3 } else { 2 };
        for row in EDITOR_TOOLS.chunks(columns) {
            ui.columns(columns, |cols| {
                for (col, tool) in cols.iter_mut().zip(row) {
                    if tool_card(col, tool.icon, tool.hue, tool.title, tool.description) {
                        chosen = Some(tool.tab.copy());
                    }
                }
            });
            ui.add_space(4.0);
        }
        ui.add_space(18.0);
        theme::overline(ui, "Documents and files");
        ui.add_space(2.0);
        let studios = [
            Studio::Pdf,
            Studio::Subtitles,
            Studio::Documents,
            Studio::Downloads,
        ];
        let columns = if ui.available_width() > 900.0 { 4 } else { 2 };
        for row in studios.chunks(columns) {
            ui.columns(columns, |cols| {
                for (col, studio) in cols.iter_mut().zip(row) {
                    if tool_card(
                        col,
                        studio.icon(),
                        studio.color(),
                        studio.title(),
                        studio.blurb(),
                    ) {
                        self.workflow_tab.open(studio.default_kind(), Vec::new());
                        chosen = Some(Tab::Workflows);
                    }
                }
            });
            ui.add_space(4.0);
        }
        if let Some(tab) = chosen {
            self.active_tab = tab;
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        theme::page_header(
            ui,
            None,
            "Settings",
            "Local engines and how your files are handled.",
        );
        theme::constrained(ui, theme::FORM_WIDTH, |ui| {
            theme::section(
                ui,
                "Local engines",
                Some("Video and audio use FFmpeg; images use ImageMagick. Everything runs on this machine."),
                |ui| {
                    let dependencies: Vec<_> = file_conversion::dependencies()
                        .into_iter()
                        .chain(file_workflows::dependencies())
                        .collect();
                    for (index, dependency) in dependencies.iter().enumerate() {
                        if index > 0 {
                            ui.separator();
                        }
                        ui.horizontal(|ui| {
                            ui.set_min_height(34.0);
                            theme::icon_tile(
                                ui,
                                if dependency.available {
                                    Icon::Check
                                } else {
                                    Icon::Settings
                                },
                                if dependency.available { SUCCESS } else { WARNING },
                                28.0,
                            );
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.label(
                                    RichText::new(&dependency.name)
                                        .font(theme::semibold(13.5))
                                        .color(TEXT),
                                );
                                let location = dependency
                                    .path
                                    .as_ref()
                                    .map(|path| theme::display_path(path))
                                    .unwrap_or_else(|| "Not found on PATH".into());
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(location).monospace().size(11.0).color(MUTED),
                                    )
                                    .truncate(true),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if dependency.available {
                                        theme::badge(ui, "Ready", Tone::Success);
                                    } else {
                                        theme::badge(ui, "Not installed", Tone::Warning);
                                    }
                                },
                            );
                        });
                    }
                    ui.add_space(6.0);
                    if ui.add(theme::secondary_button("Check again")).clicked() {
                        self.ffmpeg_available = file_conversion::dependencies()
                            .iter()
                            .any(|tool| tool.name == "FFmpeg" && tool.available);
                        *self.status.lock().unwrap() = "Conversion tools checked".into();
                    }
                },
            );
            theme::section(ui, "Privacy", None, |ui| {
                for (title, body) in [
                    ("No account, no upload", "Conversion and editing run locally; nothing leaves this machine unless you download from a link."),
                    ("Originals stay untouched", "New files are written next to a .converted suffix and verified before they are marked ready."),
                ] {
                    ui.horizontal(|ui| {
                        theme::icon(ui, Icon::Shield, 18.0, ACCENT);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(RichText::new(title).font(theme::semibold(13.5)).color(TEXT));
                            theme::hint(ui, body);
                        });
                    });
                }
            });
            theme::section(ui, "About", None, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Video Toolkit").font(theme::semibold(13.5)));
                    theme::badge(ui, concat!("v", env!("CARGO_PKG_VERSION")), Tone::Neutral);
                });
                theme::hint(
                    ui,
                    "Local video, audio, image and document conversion, focused editors and a reviewed video Agent. Written in Rust with egui.",
                );
            });
        });
    }

    fn agent(&mut self, ui: &mut egui::Ui) {
        theme::page_header(
            ui,
            None,
            "Video Agent",
            "Describe the cut you want, review it, then export.",
        );
        theme::constrained(ui, theme::FORM_WIDTH, |ui| {
            theme::card().inner_margin(28.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    theme::icon_tile(ui, Icon::Sparkle, ACCENT, 52.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(
                            RichText::new("An editor that shows its work")
                                .font(theme::bold(19.0))
                                .color(TEXT),
                        );
                        theme::hint(
                            ui,
                            "The Agent samples moments from your footage, proposes a cut, and lets you adjust every segment before anything is exported.",
                        );
                    });
                });
                ui.add_space(18.0);
                ui.columns(3, |cols| {
                    for (col, (n, title, body)) in cols.iter_mut().zip([
                        ("1", "Explore", "Sampled frames and scenes, side by side."),
                        ("2", "Edit", "Accept, trim or drop each proposed segment."),
                        ("3", "Export", "Render and verify the final video locally."),
                    ]) {
                        theme::well().show(col, |ui| {
                            ui.set_width(ui.available_width());
                            ui.set_min_height(92.0);
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.label(RichText::new(n).font(theme::bold(18.0)).color(ACCENT));
                            ui.label(RichText::new(title).font(theme::semibold(13.5)).color(TEXT));
                            ui.add_space(-6.0);
                            ui.label(RichText::new(body).size(12.5).color(MUTED));
                            });
                        });
                    }
                });
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(theme::primary_button("Open Video Agent").min_size(vec2(180.0, 40.0)))
                        .clicked()
                    {
                        let result = std::env::current_exe().and_then(|exe| {
                            std::process::Command::new(exe).arg("agent").spawn()
                        });
                        *self.status.lock().unwrap() = match result {
                            Ok(_) => "Video Agent opened in a separate window".into(),
                            Err(error) => format!("Cannot open Video Agent: {error}"),
                        };
                    }
                    ui.label(RichText::new("Opens in its own window").size(12.5).color(MUTED));
                });
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.set_min_height(22.0);
            theme::status_dot(ui, SUCCESS, "Processing stays on this device");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let status = self.status.lock().unwrap().clone();
                let color = if status.starts_with("Error") || status.starts_with("Cannot") {
                    theme::DANGER
                } else {
                    MUTED
                };
                ui.add(
                    egui::Label::new(RichText::new(status).size(12.0).color(color)).truncate(true),
                );
                if *self.processing.lock().unwrap()
                    || self.convert_tab.is_running()
                    || self.workflow_tab.is_running()
                {
                    ui.add(egui::Spinner::new().size(12.0).color(ACCENT));
                }
            });
        });
    }

    fn tool_page(&mut self, ui: &mut egui::Ui) {
        let enabled = !self.convert_tab.is_running() && !self.workflow_tab.is_running();
        egui::ScrollArea::vertical()
            .id_source(("tool_content", std::mem::discriminant(&self.active_tab)))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::none()
                    .inner_margin(egui::Margin {
                        left: 40.0,
                        right: 40.0,
                        top: 32.0,
                        bottom: 40.0,
                    })
                    .show(ui, |ui| {
                        if !enabled {
                            theme::well().show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                theme::status_dot(
                                    ui,
                                    ACCENT,
                                    "A conversion is running. Open its page to follow or cancel it.",
                                );
                            });
                            ui.add_space(14.0);
                        }
                        ui.add_enabled_ui(enabled, |ui| self.tool_page_body(ui));
                    });
            });
    }

    fn tool_page_body(&mut self, ui: &mut egui::Ui) {
        let editor = |title: &str, description: &str, ui: &mut egui::Ui| {
            theme::page_header(ui, Some("Editors"), title, description)
        };
        let mut back = false;
        match self.active_tab {
            Tab::Editors => self.editors(ui),
            Tab::Settings => self.settings(ui),
            Tab::Agent => self.agent(ui),
            Tab::Clipper => {
                back = editor("Clip video", "Choose the moments you want to keep.", ui);
                theme::constrained(ui, theme::FORM_WIDTH, |ui| self.clipper_tab.ui(ui));
            }
            Tab::GifConverter => {
                back = editor("Make a GIF", "A little motion, ready to share.", ui);
                theme::constrained(ui, theme::FORM_WIDTH, |ui| self.gif_converter_tab.ui(ui));
            }
            Tab::GifTransparency => {
                back = editor(
                    "GIF transparency",
                    "Give your animation a clean background.",
                    ui,
                );
                theme::constrained(ui, theme::FORM_WIDTH, |ui| self.gif_transparency_tab.ui(ui));
            }
            Tab::Splitter => {
                back = editor(
                    "Split video",
                    "One wide recording, several new perspectives.",
                    ui,
                );
                theme::constrained(ui, theme::FORM_WIDTH, |ui| self.splitter_tab.ui(ui));
            }
            Tab::Merger => {
                back = editor(
                    "Merge media",
                    "Bring the picture and soundtrack together.",
                    ui,
                );
                theme::constrained(ui, theme::FORM_WIDTH, |ui| self.merger_tab.ui(ui));
            }
            Tab::Batch => {
                theme::page_header(
                    ui,
                    None,
                    "Batches",
                    "Apply one editing operation to many files at once.",
                );
                theme::constrained(ui, theme::FORM_WIDTH, |ui| self.batch_tab.ui(ui));
            }
            Tab::Profiles => {
                theme::page_header(
                    ui,
                    None,
                    "Profiles",
                    "Saved settings you can reuse in batches.",
                );
                theme::constrained(ui, theme::FORM_WIDTH, |ui| self.profiles_tab.ui(ui));
            }
            Tab::Plugins => {
                theme::page_header(
                    ui,
                    None,
                    "Plugins",
                    "Extend the toolkit with local executables.",
                );
                self.plugins_tab.ui(ui);
            }
            Tab::Convert | Tab::Workflows => {}
        }
        if back {
            self.active_tab = Tab::Editors;
        }
    }
}

impl eframe::App for VideoToolKitApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.convert_tab.poll(ctx);
        self.workflow_tab.poll(ctx);
        if let Some(capture) = &mut self.capture {
            capture.update(ctx);
        }
        egui::SidePanel::left("navigation")
            .resizable(false)
            .exact_width(232.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::none()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(14.0, 18.0))
                    .stroke(Stroke::new(1.0, LINE)),
            )
            .show(ctx, |ui| self.sidebar(ui));
        egui::TopBottomPanel::bottom("status")
            .show_separator_line(false)
            .frame(
                egui::Frame::none()
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0, LINE))
                    .inner_margin(egui::Margin::symmetric(20.0, 7.0)),
            )
            .show(ctx, |ui| self.status_bar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(SURFACE))
            .show(ctx, |ui| match self.active_tab {
                Tab::Convert => self.convert_tab.ui(ui),
                Tab::Workflows => self.workflow_tab.ui(ui),
                _ => self.tool_page(ui),
            });
        if let Some((kind, inputs)) = self.convert_tab.take_workflow() {
            self.workflow_tab.open(kind, inputs);
            self.active_tab = Tab::Workflows;
        }
        if let Some(profile) = self.profiles_tab.take_batch_profile() {
            match self.batch_tab.apply_profile(&profile) {
                Ok(()) => {
                    self.active_tab = Tab::Batch;
                    *self.status.lock().unwrap() =
                        format!("Profile '{}' loaded. Add files to begin.", profile.name);
                }
                Err(error) => {
                    *self.status.lock().unwrap() = format!("Cannot load profile: {error}")
                }
            }
        }
    }
}

fn nav_item(ui: &mut egui::Ui, symbol: Icon, label: &str, selected: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, selected, label));
    let painter = ui.painter();
    if selected {
        theme::paint_shadow(painter, rect, theme::RADIUS);
        painter.rect(rect, theme::RADIUS, SURFACE, Stroke::new(1.0, LINE));
    } else if response.hovered() {
        painter.rect_filled(rect, theme::RADIUS, HOVER);
    }
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 21.0, rect.center().y),
        vec2(16.0, 16.0),
    );
    theme::paint_icon(
        painter,
        icon_rect,
        symbol,
        if selected { ACCENT } else { MUTED },
    );
    painter.text(
        egui::pos2(rect.left() + 40.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        if selected {
            theme::semibold(13.5)
        } else {
            egui::FontId::proportional(13.5)
        },
        if selected { TEXT } else { TEXT_2 },
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn tool_card(
    ui: &mut egui::Ui,
    symbol: Icon,
    hue: Color32,
    title: &str,
    description: &str,
) -> bool {
    let id = ui.next_auto_id();
    let hovered = ui.ctx().animate_bool(
        id,
        ui.ctx()
            .data(|data| data.get_temp::<bool>(id))
            .unwrap_or(false),
    );
    let frame = egui::Frame::none()
        .fill(SURFACE)
        .stroke(Stroke::new(
            1.0,
            if hovered > 0.0 {
                theme::LINE_STRONG
            } else {
                LINE
            },
        ))
        .rounding(theme::RADIUS_LG)
        .inner_margin(20.0)
        .shadow(egui::epaint::Shadow {
            extrusion: 10.0 * hovered,
            color: Color32::from_black_alpha((14.0 * hovered) as u8),
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(128.0);
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                theme::icon_tile(ui, symbol, hue, 36.0);
                ui.add_space(4.0);
                ui.label(RichText::new(title).font(theme::semibold(15.0)).color(TEXT));
                ui.add_space(-6.0);
                ui.add(
                    egui::Label::new(RichText::new(description).size(13.0).color(MUTED)).wrap(true),
                );
            });
        });
    let response = frame
        .response
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.ctx()
        .data_mut(|data| data.insert_temp(id, response.hovered()));
    if response.hovered() {
        let r = response.rect;
        theme::paint_icon(
            ui.painter(),
            egui::Rect::from_center_size(r.right_top() + vec2(-26.0, 30.0), vec2(14.0, 14.0)),
            Icon::Arrow,
            FAINT,
        );
    }
    response.clicked()
}
