use eframe::egui::{self, Ui};
use std::sync::{Arc, Mutex};
use std::thread;

use splitter::split_video;

pub struct SplitterTab {
    input_file: String,
    output_dir: String,
    prefix: String,
    encode_options: String,
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
            force: false,
            status,
            processing,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        // Input file section
        ui.heading("Input Video (should be 1920x1080)");
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
                        self.output_dir = "output_slices".to_string();
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

        // Options section
        ui.heading("Options");

        ui.horizontal(|ui| {
            ui.label("Filename Prefix:");
            ui.text_edit_singleline(&mut self.prefix);
        });

        ui.horizontal(|ui| {
            ui.label("Custom Encode Options:");
            ui.text_edit_singleline(&mut self.encode_options);
            ui.label("(advanced users only)");
        });

        ui.checkbox(
            &mut self.force,
            "Force (process even if video dimensions are not 1920x1080)",
        );

        // Execute button
        ui.add_space(10.0);
        let button = ui.add_enabled(
            !*self.processing.lock().unwrap(),
            egui::Button::new("Split Video"),
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
            let status_clone = Arc::clone(&self.status);
            let processing_clone = Arc::clone(&self.processing);

            thread::spawn(move || {
                let result = split_video(
                    &input_file,
                    &output_dir,
                    &prefix,
                    encode_options.as_deref(),
                    force,
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
