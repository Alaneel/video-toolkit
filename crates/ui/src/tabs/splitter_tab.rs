use crate::theme;
use eframe::egui::{self, Ui};
use std::sync::{Arc, Mutex};
use std::thread;

use common::VideoFormat;
use splitter::split_video_with_format;

pub struct SplitterTab {
    input_file: String,
    output_dir: String,
    prefix: String,
    encode_options: String,
    output_format: VideoFormat,
    force: bool,
    status: Arc<Mutex<String>>,
    processing: Arc<Mutex<bool>>,
}

impl SplitterTab {
    pub fn new(status: Arc<Mutex<String>>, processing: Arc<Mutex<bool>>) -> Self {
        Self {
            input_file: String::new(),
            output_dir: String::from("output_slices"),
            prefix: String::from("slice"),
            encode_options: String::new(),
            output_format: VideoFormat::MP4,
            force: false,
            status,
            processing,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        theme::section(
            ui,
            "Files",
            Some("Designed for 1920×1080 recordings; each slice is a full-height vertical crop."),
            |ui| {
                theme::field(ui, "Input video", |ui| {
                    if theme::path_input(ui, &mut self.input_file, "A 1920×1080 video") {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Video Files", &common::get_supported_formats("splitter"))
                            .pick_file()
                        {
                            self.input_file = path.to_string_lossy().to_string();
                            if self.output_dir.is_empty() {
                                self.output_dir = "output_slices".to_string();
                            }
                        }
                    }
                });
                theme::field(ui, "Output folder", |ui| {
                    if theme::path_input(ui, &mut self.output_dir, "Folder for the slices") {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.output_dir = path.to_string_lossy().to_string();
                        }
                    }
                });
            },
        );

        theme::section(ui, "Output", None, |ui| {
            theme::field(ui, "Format", |ui| {
                egui::ComboBox::from_id_source("splitter_output_format")
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
            theme::field(ui, "File name prefix", |ui| {
                theme::text_input(ui, &mut self.prefix);
            });
            theme::field(ui, "Other sizes", |ui| {
                ui.checkbox(&mut self.force, "Process videos that are not 1920×1080");
            });
            ui.collapsing(
                egui::RichText::new("Advanced")
                    .font(theme::semibold(13.0))
                    .color(theme::TEXT_2),
                |ui| {
                    theme::field(ui, "FFmpeg encode options", |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.encode_options)
                                .margin(egui::vec2(10.0, 7.0))
                                .font(egui::TextStyle::Monospace)
                                .hint_text("-c:v libx264 -crf 20")
                                .desired_width(f32::INFINITY),
                        );
                    });
                },
            );
        });

        ui.add_space(4.0);
        let button = ui.add_enabled(
            !*self.processing.lock().unwrap(),
            theme::primary_button("Split video").min_size(egui::vec2(160.0, 40.0)),
        );

        if button.clicked() {
            if self.input_file.is_empty() {
                *self.status.lock().unwrap() =
                    "Error: Please select an input video file.".to_string();
                return;
            }

            // Start processing in a separate thread
            *self.status.lock().unwrap() = "Processing video split...".to_string();
            *self.processing.lock().unwrap() = true;

            // Clone values for thread
            let input_file = self.input_file.clone();
            let output_dir = self.output_dir.clone();
            let prefix = self.prefix.clone();
            let encode_options = if self.encode_options.is_empty() {
                None
            } else {
                Some(self.encode_options.clone())
            };
            let force = self.force;
            let output_format = self.output_format;
            let status_clone = Arc::clone(&self.status);
            let processing_clone = Arc::clone(&self.processing);

            thread::spawn(move || {
                let result = split_video_with_format(
                    &input_file,
                    &output_dir,
                    &prefix,
                    encode_options.as_deref(),
                    force,
                    output_format,
                );

                match result {
                    Ok(true) => {
                        *status_clone.lock().unwrap() = format!(
                            "Successfully split video into 5 slices. Files saved in: {}",
                            output_dir
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
