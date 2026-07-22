use eframe::egui::{self, Ui};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use gif_transparency::{batch_process_gifs, process_directory};

pub struct GifTransparencyTab {
    input_paths: Vec<PathBuf>,
    directory_mode: bool,
    directory_path: String,
    recursive: bool,
    create_backup: bool,
    status: Arc<Mutex<String>>,
    processing: Arc<Mutex<bool>>,
}

impl GifTransparencyTab {
    pub fn new(status: Arc<Mutex<String>>, processing: Arc<Mutex<bool>>) -> Self {
        Self {
            input_paths: Vec::new(),
            directory_mode: true, // Default to directory mode
            directory_path: String::new(),
            recursive: true,
            create_backup: true,
            status,
            processing,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        // Mode selection
        ui.heading("Transparency Mode");
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.directory_mode, true, "Process Directory");
            ui.radio_value(&mut self.directory_mode, false, "Select Individual Files");
        });

        ui.separator();

        // Input paths based on mode
        if self.directory_mode {
            // Directory mode
            ui.heading("Input Directory");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.directory_path);
                if ui.button("Browse").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.directory_path = path.to_string_lossy().to_string();
                    }
                }
            });
        } else {
            // Individual files mode
            ui.heading("Input GIF Files");

            // Display selected files
            let mut to_remove = None;
            for (idx, path) in self.input_paths.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("{}. {}", idx + 1, path.to_string_lossy()));
                    if ui.button("Remove").clicked() {
                        to_remove = Some(idx);
                    }
                });
            }

            // Remove file if requested
            if let Some(idx) = to_remove {
                self.input_paths.remove(idx);
            }

            // Add file button
            if ui.button("Add GIF Files").clicked() {
                if let Some(paths) = rfd::FileDialog::new()
                    .add_filter("GIF Files", &["gif"])
                    .pick_files()
                {
                    self.input_paths.extend(paths);
                }
            }
        }

        ui.separator();

        // Options
        ui.heading("Options");
        ui.checkbox(&mut self.recursive, "Process subdirectories recursively");
        ui.checkbox(&mut self.create_backup, "Create backup of original files");

        ui.separator();

        // Process button
        let button_text = if self.directory_mode {
            "Process GIFs in Directory"
        } else {
            "Process Selected GIF Files"
        };

        let button = ui.add_enabled(
            !*self.processing.lock().unwrap(),
            egui::Button::new(button_text),
        );

        if button.clicked() {
            // Validate inputs
            if self.directory_mode && self.directory_path.is_empty() {
                *self.status.lock().unwrap() = "Error: Please select a directory.".to_string();
                return;
            }

            if !self.directory_mode && self.input_paths.is_empty() {
                *self.status.lock().unwrap() =
                    "Error: Please select at least one GIF file.".to_string();
                return;
            }

            // Start processing in a separate thread
            *self.status.lock().unwrap() = "Processing GIF files for transparency...".to_string();
            *self.processing.lock().unwrap() = true;

            // Clone values for thread
            let directory_mode = self.directory_mode;
            let directory_path = self.directory_path.clone();
            let input_paths = self.input_paths.clone();
            let recursive = self.recursive;
            let create_backup = self.create_backup;
            let status_clone = Arc::clone(&self.status);
            let processing_clone = Arc::clone(&self.processing);

            thread::spawn(move || {
                let result = if directory_mode {
                    process_directory(&directory_path, recursive, create_backup)
                } else {
                    batch_process_gifs(&input_paths, recursive, create_backup)
                };

                match result {
                    Ok((success_count, total_count)) => {
                        *status_clone.lock().unwrap() = format!(
                            "Successfully processed {}/{} GIF files",
                            success_count, total_count
                        );
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
