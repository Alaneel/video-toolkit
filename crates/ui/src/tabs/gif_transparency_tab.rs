use crate::theme::{self, Icon};
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
        theme::section(
            ui,
            "GIFs to process",
            Some("The logical background colour of each frame becomes transparent."),
            |ui| {
                let mode = if self.directory_mode { 0 } else { 1 };
                if let Some(picked) = theme::segmented(ui, mode, &["A folder", "Individual files"])
                {
                    self.directory_mode = picked == 0;
                }
                ui.add_space(4.0);
                if self.directory_mode {
                    theme::field(ui, "Folder", |ui| {
                        if theme::path_input(ui, &mut self.directory_path, "Folder containing GIFs")
                        {
                            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                self.directory_path = path.to_string_lossy().to_string();
                            }
                        }
                    });
                } else {
                    let mut to_remove = None;
                    if self.input_paths.is_empty() {
                        theme::well().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            theme::empty_state(
                                ui,
                                Icon::Image,
                                "No GIFs yet",
                                "Add one or more GIF files to process.",
                            );
                        });
                    }
                    for (idx, path) in self.input_paths.iter().enumerate() {
                        theme::well()
                            .inner_margin(egui::Margin::symmetric(12.0, 6.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    theme::icon_tile(ui, Icon::Image, theme::HUE_IMAGE, 26.0);
                                    let name = path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_default();
                                    ui.label(egui::RichText::new(name).font(theme::semibold(13.0)))
                                        .on_hover_text(path.to_string_lossy());
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.add(theme::small_button("Remove")).clicked() {
                                                to_remove = Some(idx);
                                            }
                                        },
                                    );
                                });
                            });
                    }
                    if let Some(idx) = to_remove {
                        self.input_paths.remove(idx);
                    }
                    if ui.add(theme::secondary_button("+ Add GIF files")).clicked() {
                        if let Some(paths) = rfd::FileDialog::new()
                            .add_filter("GIF Files", &["gif"])
                            .pick_files()
                        {
                            self.input_paths.extend(paths);
                        }
                    }
                }
            },
        );

        theme::section(ui, "Options", None, |ui| {
            ui.checkbox(&mut self.recursive, "Include subfolders");
            ui.checkbox(&mut self.create_backup, "Keep a backup of each original");
        });

        let button_text = if self.directory_mode {
            "Process folder"
        } else {
            "Process selected GIFs"
        };
        ui.add_space(4.0);
        let button = ui.add_enabled(
            !*self.processing.lock().unwrap(),
            theme::primary_button(button_text).min_size(egui::vec2(160.0, 40.0)),
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
