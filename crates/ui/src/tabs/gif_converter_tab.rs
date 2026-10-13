use eframe::egui::{self, Ui};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use gif_converter::{convert_video_to_gif, optimize_conversion_with_fps};

pub struct GifConverterTab {
    input_file: String,
    output_file: String,
    width: String,
    fps: String,
    max_size: String,
    optimize: bool,
    status: Arc<Mutex<String>>,
    processing: Arc<Mutex<bool>>,
}

impl GifConverterTab {
    pub fn new(status: Arc<Mutex<String>>, processing: Arc<Mutex<bool>>) -> Self {
        Self {
            input_file: String::new(),
            output_file: String::new(),
            width: String::new(),
            fps: String::from("10"),
            max_size: String::from("5.0"),
            optimize: true,
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
                    .add_filter("Video Files", &["mp4", "webm", "avi", "mov", "mkv"])
                    .pick_file()
                {
                    self.input_file = path.to_string_lossy().to_string();

                    // Set default output file if not set
                    if self.output_file.is_empty() {
                        let input_path = Path::new(&self.input_file);
                        if let Some(stem) = input_path.file_stem() {
                            let mut output_path =
                                PathBuf::from(input_path.parent().unwrap_or_else(|| Path::new("")));
                            output_path.push(stem);
                            output_path.set_extension("gif");
                            self.output_file = output_path.to_string_lossy().to_string();
                        }
                    }
                }
            }
        });

        // Output file section
        ui.heading("Output GIF");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.output_file);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("GIF Files", &["gif"])
                    .save_file()
                {
                    self.output_file = path.to_string_lossy().to_string();

                    // Make sure it has .gif extension
                    if !self.output_file.ends_with(".gif") {
                        self.output_file.push_str(".gif");
                    }
                }
            }
        });

        // Options section
        ui.heading("Conversion Options");

        ui.horizontal(|ui| {
            ui.label("Width:");
            ui.text_edit_singleline(&mut self.width);
            ui.label("(leave empty for auto)");
        });

        ui.horizontal(|ui| {
            ui.label("FPS:");
            ui.text_edit_singleline(&mut self.fps);
        });

        ui.horizontal(|ui| {
            ui.label("Max Size (MB):");
            ui.text_edit_singleline(&mut self.max_size);
        });

        ui.checkbox(
            &mut self.optimize,
            "Optimize (try multiple settings to achieve size target)",
        );

        // Execute button
        ui.add_space(10.0);
        let button = ui.add_enabled(
            !*self.processing.lock().unwrap(),
            egui::Button::new("Convert to GIF"),
        );

        if button.clicked() {
            if self.input_file.is_empty() {
                *self.status.lock().unwrap() =
                    "Error: Please select an input video file.".to_string();
                return;
            }

            if self.output_file.is_empty() {
                *self.status.lock().unwrap() =
                    "Error: Please specify an output GIF file.".to_string();
                return;
            }

            // Parse options
            let width = if self.width.is_empty() {
                None
            } else {
                match self.width.parse::<u32>() {
                    Ok(w) => Some(w),
                    Err(_) => {
                        *self.status.lock().unwrap() =
                            "Error: Width must be a positive integer.".to_string();
                        return;
                    }
                }
            };

            let fps = match self.fps.parse::<u32>() {
                Ok(f) => f,
                Err(_) => {
                    *self.status.lock().unwrap() =
                        "Error: FPS must be a positive integer.".to_string();
                    return;
                }
            };

            let max_size = match self.max_size.parse::<f64>() {
                Ok(s) => s,
                Err(_) => {
                    *self.status.lock().unwrap() =
                        "Error: Max size must be a positive number.".to_string();
                    return;
                }
            };

            // Start processing in a separate thread
            *self.status.lock().unwrap() = "Converting video to GIF...".to_string();
            *self.processing.lock().unwrap() = true;

            // Clone values for thread
            let input_file = self.input_file.clone();
            let output_file = self.output_file.clone();
            let optimize = self.optimize;
            let status_clone = Arc::clone(&self.status);
            let processing_clone = Arc::clone(&self.processing);

            thread::spawn(move || {
                let result = if optimize {
                    optimize_conversion_with_fps(&input_file, &output_file, max_size, width, fps)
                } else {
                    convert_video_to_gif(&input_file, &output_file, width, fps, max_size)
                };

                match result {
                    Ok(true) => {
                        *status_clone.lock().unwrap() = "Conversion successful!".to_string();
                    }
                    Ok(false) => {
                        *status_clone.lock().unwrap() =
                            format!("Output file exceeds size limit (> {}MB).", max_size);
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
