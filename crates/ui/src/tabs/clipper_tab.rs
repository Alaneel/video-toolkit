use crate::theme;
use eframe::egui::{self, Ui};
use std::sync::{Arc, Mutex};
use std::thread;

use clipper::clip_video_with_format;
use common::{validate_time_range, VideoFormat};

pub struct ClipperTab {
    input_file: String,
    output_dir: String,
    time_ranges: Vec<String>,
    copy_codec: bool,
    output_format: VideoFormat,
    suffix: String,
    status: Arc<Mutex<String>>,
    processing: Arc<Mutex<bool>>,
}

impl ClipperTab {
    pub fn new(status: Arc<Mutex<String>>, processing: Arc<Mutex<bool>>) -> Self {
        Self {
            input_file: String::new(),
            output_dir: String::from("output_clips"),
            time_ranges: vec![String::new()],
            copy_codec: false,
            output_format: VideoFormat::MP4,
            suffix: String::new(),
            status,
            processing,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        theme::section(
            ui,
            "Source",
            Some("The video to cut and where the clips are saved."),
            |ui| {
                theme::field(ui, "Input video", |ui| {
                    if theme::path_input(ui, &mut self.input_file, "Choose a video file") {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Video Files", &common::get_supported_formats("clipper"))
                            .pick_file()
                        {
                            self.input_file = path.to_string_lossy().to_string();
                            if self.output_dir.is_empty() {
                                self.output_dir = "output_clips".to_string();
                            }
                        }
                    }
                });
                theme::field(ui, "Output folder", |ui| {
                    if theme::path_input(ui, &mut self.output_dir, "Folder for the clips") {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.output_dir = path.to_string_lossy().to_string();
                        }
                    }
                });
            },
        );

        theme::section(
            ui,
            "Time ranges",
            Some("One clip per range, written as START-END, for example 00:01:00-00:02:00."),
            |ui| {
                let mut remove_idx = None;
                let len = self.time_ranges.len();
                for i in 0..len {
                    let range = &mut self.time_ranges[i];
                    theme::field(ui, &format!("Clip {}", i + 1), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(range)
                                .margin(egui::vec2(10.0, 7.0))
                                .hint_text("00:00:00-00:00:10")
                                .font(egui::TextStyle::Monospace)
                                .desired_width((ui.available_width() - 96.0).max(120.0)),
                        );
                        if ui
                            .add_enabled(len > 1, theme::secondary_button("Remove"))
                            .clicked()
                        {
                            remove_idx = Some(i);
                        }
                    });
                }
                if let Some(idx) = remove_idx {
                    if self.time_ranges.len() > 1 {
                        self.time_ranges.remove(idx);
                    }
                }
                ui.horizontal(|ui| {
                    ui.add_space(168.0);
                    if ui.add(theme::secondary_button("+ Add range")).clicked() {
                        self.time_ranges.push(String::new());
                    }
                });
            },
        );

        theme::section(ui, "Output", None, |ui| {
            theme::field(ui, "Format", |ui| {
                egui::ComboBox::from_id_source("clipper_output_format")
                    .selected_text(self.output_format.to_string())
                    .show_ui(ui, |ui| {
                        for format in VideoFormat::all() {
                            ui.selectable_value(
                                &mut self.output_format,
                                format,
                                format.to_string(),
                            );
                        }
                    });
            });
            theme::field(ui, "File suffix", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.suffix)
                        .margin(egui::vec2(10.0, 7.0))
                        .hint_text("Optional, e.g. _clip")
                        .desired_width(240.0),
                );
            });
            theme::field(ui, "Speed", |ui| {
                ui.checkbox(
                    &mut self.copy_codec,
                    "Copy codec — faster, cuts on keyframes",
                );
            });
        });

        ui.add_space(4.0);
        let button = ui.add_enabled(
            !*self.processing.lock().unwrap(),
            theme::primary_button("Extract clips").min_size(egui::vec2(160.0, 40.0)),
        );

        if button.clicked() {
            if self.input_file.is_empty() {
                *self.status.lock().unwrap() =
                    "Error: Please select an input video file.".to_string();
                return;
            }

            // Parse time ranges
            let mut parsed_ranges = Vec::new();
            let mut invalid_found = false;

            for range in &self.time_ranges {
                if range.trim().is_empty() {
                    continue;
                }

                if let Some(parsed) = validate_time_range(range) {
                    parsed_ranges.push(parsed);
                } else {
                    *self.status.lock().unwrap() =
                        format!("Error: Invalid time range format: '{}'", range);
                    invalid_found = true;
                    break;
                }
            }

            if invalid_found || parsed_ranges.is_empty() {
                *self.status.lock().unwrap() = "Error: No valid time ranges provided.".to_string();
                return;
            }

            // Start processing in a separate thread
            *self.status.lock().unwrap() = "Processing video clips...".to_string();
            *self.processing.lock().unwrap() = true;

            // Clone values for thread
            let input_file = self.input_file.clone();
            let output_dir = self.output_dir.clone();
            let copy_codec = self.copy_codec;
            let output_format = self.output_format;
            let suffix = if self.suffix.is_empty() {
                None
            } else {
                Some(self.suffix.clone())
            };
            let status_clone = Arc::clone(&self.status);
            let processing_clone = Arc::clone(&self.processing);

            thread::spawn(move || {
                let result = clip_video_with_format(
                    &input_file,
                    &parsed_ranges,
                    &output_dir,
                    copy_codec,
                    suffix.as_deref(),
                    output_format,
                );

                match result {
                    Ok(true) => {
                        *status_clone.lock().unwrap() = format!(
                            "Successfully extracted all {} clip(s).",
                            parsed_ranges.len()
                        );
                    }
                    Ok(false) => {
                        *status_clone.lock().unwrap() = "Completed with some errors.".to_string();
                    }
                    Err(e) => {
                        *status_clone.lock().unwrap() = format!("Error: {}", e);
                    }
                }

                *processing_clone.lock().unwrap() = false;
            });
        }
    }
}
