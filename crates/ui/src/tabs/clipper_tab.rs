use eframe::egui::{self, Ui};
use std::sync::{Arc, Mutex};
use std::thread;

use clipper::clip_video;
use common::validate_time_range;

pub struct ClipperTab {
    input_file: String,
    output_dir: String,
    time_ranges: Vec<String>,
    copy_codec: bool,
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
            suffix: String::new(),
            status,
            processing,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        // Input file section
        ui.heading("Input Video");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.input_file);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Video Files", &["mp4", "avi", "mov", "mkv"])
                    .pick_file()
                {
                    self.input_file = path.to_string_lossy().to_string();

                    // Set default output dir if not set
                    if self.output_dir.is_empty() {
                        self.output_dir = "output_clips".to_string();
                    }
                }
            }
        });

        // Output directory section
        ui.heading("Output Directory");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.output_dir);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.output_dir = path.to_string_lossy().to_string();
                }
            }
        });

        // Time ranges section
        ui.heading("Time Ranges (format: START-END, e.g., 00:01:00-00:02:00)");

        let mut remove_idx = None;
        let len = self.time_ranges.len();
        for i in 0..len {
            let range = &mut self.time_ranges[i];
            ui.horizontal(|ui| {
                ui.label(format!("Range {}:", i + 1));
                ui.text_edit_singleline(range);
                if ui.button("Remove").clicked() && len > 1 {
                    remove_idx = Some(i);
                }
            });
        }

        // Remove range if needed
        if let Some(idx) = remove_idx {
            if self.time_ranges.len() > 1 {
                self.time_ranges.remove(idx);
            }
        }

        // Add new range button
        if ui.button("Add Time Range").clicked() {
            self.time_ranges.push(String::new());
        }

        // Options section
        ui.heading("Options");
        ui.checkbox(&mut self.copy_codec, "Copy codec (faster but less precise)");

        ui.horizontal(|ui| {
            ui.label("Suffix:");
            ui.text_edit_singleline(&mut self.suffix);
        });

        // Execute button
        ui.add_space(10.0);
        let button = ui.add_enabled(
            !*self.processing.lock().unwrap(),
            egui::Button::new("Extract Clips"),
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
            let suffix = if self.suffix.is_empty() {
                None
            } else {
                Some(self.suffix.clone())
            };
            let status_clone = Arc::clone(&self.status);
            let processing_clone = Arc::clone(&self.processing);

            thread::spawn(move || {
                let result = clip_video(
                    &input_file,
                    &parsed_ranges,
                    &output_dir,
                    copy_codec,
                    suffix.as_deref(),
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
