use eframe::egui;
use egui::{Color32, RichText};
use std::sync::{Arc, Mutex};

use crate::tabs::{
    batch_tab::BatchTab, clipper_tab::ClipperTab, gif_converter_tab::GifConverterTab,
    gif_transparency_tab::GifTransparencyTab, merger_tab::MergerTab, plugins_tab::PluginsTab,
    profiles_tab::ProfilesTab, splitter_tab::SplitterTab,
};
use common::check_ffmpeg;

#[derive(PartialEq)]
pub enum Tab {
    Clipper,
    GifConverter,
    GifTransparency,
    Splitter,
    Merger,
    Batch,    // New tab
    Profiles, // New tab
    Plugins,  // New tab
}

pub struct VideoToolKitApp {
    active_tab: Tab,
    status: Arc<Mutex<String>>,
    processing: Arc<Mutex<bool>>,
    ffmpeg_available: bool,

    clipper_tab: ClipperTab,
    gif_converter_tab: GifConverterTab,
    gif_transparency_tab: GifTransparencyTab,
    splitter_tab: SplitterTab,
    merger_tab: MergerTab,
    batch_tab: BatchTab,       // New tab
    profiles_tab: ProfilesTab, // New tab
    plugins_tab: PluginsTab,   // New tab
}

impl Default for VideoToolKitApp {
    fn default() -> Self {
        let status = Arc::new(Mutex::new("Ready".to_string()));
        let processing = Arc::new(Mutex::new(false));

        Self {
            active_tab: Tab::Clipper,
            status: Arc::clone(&status),
            processing: Arc::clone(&processing),
            ffmpeg_available: check_ffmpeg(),

            clipper_tab: ClipperTab::new(Arc::clone(&status), Arc::clone(&processing)),
            gif_converter_tab: GifConverterTab::new(Arc::clone(&status), Arc::clone(&processing)),
            gif_transparency_tab: GifTransparencyTab::new(
                Arc::clone(&status),
                Arc::clone(&processing),
            ),
            splitter_tab: SplitterTab::new(Arc::clone(&status), Arc::clone(&processing)),
            merger_tab: MergerTab::new(Arc::clone(&status), Arc::clone(&processing)),
            batch_tab: BatchTab::new(Arc::clone(&status), Arc::clone(&processing)),
            profiles_tab: ProfilesTab::new(Arc::clone(&status), Arc::clone(&processing)),
            plugins_tab: PluginsTab::new(Arc::clone(&status), Arc::clone(&processing)),
        }
    }
}

impl eframe::App for VideoToolKitApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Video ToolKit")
                        .size(24.0)
                        .strong()
                        .color(Color32::from_rgb(99, 181, 255)),
                );
                ui.label(
                    RichText::new("Local-first media utilities powered by FFmpeg")
                        .color(Color32::from_gray(155)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, color) = if self.ffmpeg_available {
                        ("FFmpeg ready", Color32::from_rgb(89, 201, 143))
                    } else {
                        ("FFmpeg missing", Color32::from_rgb(255, 107, 107))
                    };
                    ui.label(RichText::new(label).color(color));
                });
            });
            ui.add_space(8.0);
        });

        egui::SidePanel::left("navigation")
            .resizable(false)
            .default_width(190.0)
            .show(ctx, |ui| {
                ui.add_space(12.0);
                ui.label(RichText::new("CREATE").small().strong().weak());
                ui.add_space(6.0);
                ui.selectable_value(&mut self.active_tab, Tab::Clipper, "Clip video");
                ui.selectable_value(&mut self.active_tab, Tab::GifConverter, "Convert to GIF");
                ui.selectable_value(
                    &mut self.active_tab,
                    Tab::GifTransparency,
                    "GIF transparency",
                );
                ui.selectable_value(&mut self.active_tab, Tab::Splitter, "Split video");
                ui.selectable_value(&mut self.active_tab, Tab::Merger, "Merge media");
                ui.add_space(18.0);
                ui.label(RichText::new("WORKFLOW").small().strong().weak());
                ui.add_space(6.0);
                ui.selectable_value(&mut self.active_tab, Tab::Batch, "Batch processing");
                ui.selectable_value(&mut self.active_tab, Tab::Profiles, "Profiles");
                ui.selectable_value(&mut self.active_tab, Tab::Plugins, "Plugins");
            });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if *self.processing.lock().unwrap() {
                    ui.spinner();
                }
                let status = self.status.lock().unwrap().clone();
                ui.label(RichText::new(status).small());
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(12.0);
            if !self.ffmpeg_available {
                egui::Frame::none()
                    .fill(Color32::from_rgb(72, 35, 42))
                    .rounding(8.0)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new("FFmpeg was not found in PATH. Install FFmpeg, then restart the app.").color(Color32::from_rgb(255, 190, 190)));
                    });
                return;
            }

            match self.active_tab {
                Tab::Clipper => self.clipper_tab.ui(ui),
                Tab::GifConverter => self.gif_converter_tab.ui(ui),
                Tab::GifTransparency => self.gif_transparency_tab.ui(ui),
                Tab::Splitter => self.splitter_tab.ui(ui),
                Tab::Merger => self.merger_tab.ui(ui),
                Tab::Batch => self.batch_tab.ui(ui),
                Tab::Profiles => self.profiles_tab.ui(ui),
                Tab::Plugins => self.plugins_tab.ui(ui),
            }

        });
    }
}
