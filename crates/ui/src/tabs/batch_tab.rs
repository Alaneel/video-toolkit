use eframe::egui::{self, ComboBox, Ui};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use batch_processing::{BatchItemResult, BatchProcessor};

#[derive(PartialEq, Clone, Copy)]
pub enum BatchOperationType {
    Clipper,
    GifConverter,
    GifTransparency,
    Splitter,
    Merger,
}

impl BatchOperationType {
    fn display_name(&self) -> &'static str {
        match self {
            BatchOperationType::Clipper => "Video Clipper",
            BatchOperationType::GifConverter => "GIF Converter",
            BatchOperationType::GifTransparency => "GIF Transparency",
            BatchOperationType::Splitter => "Video Splitter",
            BatchOperationType::Merger => "Audio/Video Merger",
        }
    }
}

pub struct BatchTab {
    // General batch settings
    operation_type: BatchOperationType,
    input_paths: Vec<PathBuf>,
    recursive: bool,
    pattern: String,
    parallel: bool,

    // Operation-specific settings

    // Clipper settings
    clipper_time_ranges: Vec<String>,
    clipper_output_dir: String,
    clipper_copy_codec: bool,
    clipper_suffix: String,

    // GIF converter settings
    gif_output_dir: String,
    gif_width: String,
    gif_fps: String,
    gif_max_size: String,
    gif_optimize: bool,

    // GIF transparency settings
    transparency_backup: bool,

    // Splitter settings
    splitter_output_dir: String,
    splitter_prefix: String,
    splitter_custom_encode: String,
    splitter_force: bool,

    // Merger settings
    merger_audio_file: String,
    merger_output_dir: String,
    merger_shortest: bool,
    merger_copy_codec: bool,

    // Processing state
    status: Arc<Mutex<String>>,
    processing: Arc<Mutex<bool>>,
    results: Arc<Mutex<Vec<BatchItemResult>>>,
    progress: Arc<Mutex<(usize, usize)>>,
}

impl BatchTab {
    pub fn new(status: Arc<Mutex<String>>, processing: Arc<Mutex<bool>>) -> Self {
        Self {
            operation_type: BatchOperationType::Clipper,
            input_paths: Vec::new(),
            recursive: true,
            pattern: String::new(),
            parallel: true,

            clipper_time_ranges: vec![String::new()],
            clipper_output_dir: String::from("output_clips"),
            clipper_copy_codec: false,
            clipper_suffix: String::new(),

            gif_output_dir: String::from("output_gifs"),
            gif_width: String::new(),
            gif_fps: String::from("10"),
            gif_max_size: String::from("5.0"),
            gif_optimize: true,

            transparency_backup: true,

            splitter_output_dir: String::from("output_slices"),
            splitter_prefix: String::from("slice"),
            splitter_custom_encode: String::new(),
            splitter_force: false,

            merger_audio_file: String::new(),
            merger_output_dir: String::from("output_merged"),
            merger_shortest: true,
            merger_copy_codec: true,

            status,
            processing: processing.clone(),
            results: Arc::new(Mutex::new(Vec::new())),
            progress: Arc::new(Mutex::new((0, 0))),
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        ui.heading("Batch Processing");

        // Operation type selection
        ui.horizontal(|ui| {
            ui.label("Operation Type:");
            ComboBox::from_id_source("batch_operation_type")
                .selected_text(self.operation_type.display_name())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.operation_type,
                        BatchOperationType::Clipper,
                        "Video Clipper",
                    );
                    ui.selectable_value(
                        &mut self.operation_type,
                        BatchOperationType::GifConverter,
                        "GIF Converter",
                    );
                    ui.selectable_value(
                        &mut self.operation_type,
                        BatchOperationType::GifTransparency,
                        "GIF Transparency",
                    );
                    ui.selectable_value(
                        &mut self.operation_type,
                        BatchOperationType::Splitter,
                        "Video Splitter",
                    );
                    ui.selectable_value(
                        &mut self.operation_type,
                        BatchOperationType::Merger,
                        "Audio/Video Merger",
                    );
                });
        });

        ui.separator();

        // Input files section
        ui.heading("Input Files");

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

        // Add file/directory buttons
        ui.horizontal(|ui| {
            if ui.button("Add Files").clicked() {
                if let Some(paths) = rfd::FileDialog::new().pick_files() {
                    self.input_paths.extend(paths);
                }
            }

            if ui.button("Add Directory").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.input_paths.push(path);
                }
            }
        });

        // Batch settings
        ui.heading("Batch Settings");

        ui.checkbox(&mut self.recursive, "Process directories recursively");
        ui.checkbox(&mut self.parallel, "Process files in parallel");

        ui.horizontal(|ui| {
            ui.label("Filename pattern (regex):");
            ui.text_edit_singleline(&mut self.pattern);
        });

        ui.separator();

        // Operation-specific settings
        match self.operation_type {
            BatchOperationType::Clipper => self.clipper_settings_ui(ui),
            BatchOperationType::GifConverter => self.gif_converter_settings_ui(ui),
            BatchOperationType::GifTransparency => self.gif_transparency_settings_ui(ui),
            BatchOperationType::Splitter => self.splitter_settings_ui(ui),
            BatchOperationType::Merger => self.merger_settings_ui(ui),
        }

        ui.separator();

        // Process button and progress
        let processing = *self.processing.lock().unwrap();

        if processing {
            // Show progress
            let (current, total) = *self.progress.lock().unwrap();
            ui.label(format!("Processing file {} of {}", current, total));
            ui.add(
                egui::ProgressBar::new(if total > 0 {
                    current as f32 / total as f32
                } else {
                    0.0
                })
                .show_percentage(),
            );
        } else {
            // Show results if available
            let results = self.results.lock().unwrap();
            if !results.is_empty() {
                let success_count = results.iter().filter(|r| r.success).count();
                ui.label(format!(
                    "Processed {} files: {} succeeded, {} failed",
                    results.len(),
                    success_count,
                    results.len() - success_count
                ));

                if results.len() - success_count > 0 {
                    ui.collapsing("Show errors", |ui| {
                        for result in results.iter().filter(|r| !r.success) {
                            if let Some(ref error) = result.error_message {
                                ui.label(format!("{}: {}", result.input.display(), error));
                            }
                        }
                    });
                }
            }
        }

        // Process button
        let button = ui.add_enabled(!processing, egui::Button::new("Start Batch Processing"));

        if button.clicked() {
            // Validate inputs
            if self.input_paths.is_empty() {
                *self.status.lock().unwrap() =
                    "Error: Please add at least one input file or directory.".to_string();
                return;
            }

            // Operation-specific validation
            match self.operation_type {
                BatchOperationType::Clipper => {
                    let has_valid_ranges = self
                        .clipper_time_ranges
                        .iter()
                        .any(|r| !r.trim().is_empty());

                    if !has_valid_ranges {
                        *self.status.lock().unwrap() =
                            "Error: Please add at least one time range.".to_string();
                        return;
                    }
                }
                BatchOperationType::Merger => {
                    if self.merger_audio_file.is_empty() {
                        *self.status.lock().unwrap() =
                            "Error: Please select an audio file.".to_string();
                        return;
                    }
                }
                _ => {}
            }

            // Start processing
            *self.status.lock().unwrap() = format!(
                "Starting batch {} processing...",
                self.operation_type.display_name()
            );
            *self.processing.lock().unwrap() = true;
            self.results.lock().unwrap().clear();
            *self.progress.lock().unwrap() = (0, 0);

            // Clone values for thread
            let operation_type = self.operation_type;
            let input_paths = self.input_paths.clone();
            let recursive = self.recursive;
            let pattern = self.pattern.clone();
            let parallel = self.parallel;

            // Operation-specific clones
            let clipper_time_ranges = self.clipper_time_ranges.clone();
            let clipper_output_dir = self.clipper_output_dir.clone();
            let clipper_copy_codec = self.clipper_copy_codec;
            let clipper_suffix = self.clipper_suffix.clone();

            let gif_output_dir = self.gif_output_dir.clone();
            let gif_width_str = self.gif_width.clone();
            let gif_fps_str = self.gif_fps.clone();
            let gif_max_size_str = self.gif_max_size.clone();
            let gif_optimize = self.gif_optimize;

            let transparency_backup = self.transparency_backup;

            let splitter_output_dir = self.splitter_output_dir.clone();
            let splitter_prefix = self.splitter_prefix.clone();
            let splitter_custom_encode = self.splitter_custom_encode.clone();
            let splitter_force = self.splitter_force;

            let merger_audio_file = self.merger_audio_file.clone();
            let merger_output_dir = self.merger_output_dir.clone();
            let merger_shortest = self.merger_shortest;
            let merger_copy_codec = self.merger_copy_codec;

            let status_clone = Arc::clone(&self.status);
            let processing_clone = Arc::clone(&self.processing);
            let results_clone: Arc<Mutex<Vec<batch_processing::BatchItemResult>>> =
                Arc::clone(&self.results);
            let progress_clone = Arc::clone(&self.progress);

            thread::spawn(move || {
                // Create batch processor based on operation type
                let mut processor = match operation_type {
                    BatchOperationType::Clipper => {
                        // Create processor for clipper
                        match BatchProcessor::create_clipper(
                            &clipper_time_ranges,
                            Path::new(&clipper_output_dir),
                            clipper_copy_codec,
                            if clipper_suffix.is_empty() {
                                None
                            } else {
                                Some(&clipper_suffix)
                            },
                        ) {
                            Ok(p) => p,
                            Err(e) => {
                                *status_clone.lock().unwrap() = format!("Error: {}", e);
                                *processing_clone.lock().unwrap() = false;
                                return;
                            }
                        }
                    }
                    BatchOperationType::GifConverter => {
                        // Parse GIF converter settings
                        let width = if gif_width_str.is_empty() {
                            None
                        } else {
                            match gif_width_str.parse::<u32>() {
                                Ok(w) => Some(w),
                                Err(_) => {
                                    *status_clone.lock().unwrap() =
                                        "Error: Width must be a positive integer.".to_string();
                                    *processing_clone.lock().unwrap() = false;
                                    return;
                                }
                            }
                        };

                        let fps = match gif_fps_str.parse::<u32>() {
                            Ok(f) => f,
                            Err(_) => {
                                *status_clone.lock().unwrap() =
                                    "Error: FPS must be a positive integer.".to_string();
                                *processing_clone.lock().unwrap() = false;
                                return;
                            }
                        };

                        let max_size = match gif_max_size_str.parse::<f64>() {
                            Ok(s) => s,
                            Err(_) => {
                                *status_clone.lock().unwrap() =
                                    "Error: Max size must be a positive number.".to_string();
                                *processing_clone.lock().unwrap() = false;
                                return;
                            }
                        };

                        // Create processor for GIF converter
                        BatchProcessor::create_gif_converter(
                            width,
                            fps,
                            max_size,
                            gif_optimize,
                            Path::new(&gif_output_dir),
                        )
                    }
                    BatchOperationType::GifTransparency => {
                        // Create processor for GIF transparency
                        BatchProcessor::create_gif_transparency(transparency_backup)
                    }
                    BatchOperationType::Splitter => {
                        // Create processor for splitter
                        BatchProcessor::create_splitter(
                            Path::new(&splitter_output_dir),
                            &splitter_prefix,
                            if splitter_custom_encode.is_empty() {
                                None
                            } else {
                                Some(&splitter_custom_encode)
                            },
                            splitter_force,
                        )
                    }
                    BatchOperationType::Merger => {
                        // Create processor for merger
                        BatchProcessor::create_merger(
                            Path::new(&merger_audio_file),
                            Path::new(&merger_output_dir),
                            merger_shortest,
                            merger_copy_codec,
                        )
                    }
                };

                // Configure processor
                processor = processor.with_recursive(recursive).with_parallel(parallel);

                if !pattern.is_empty() {
                    processor = match processor.with_pattern(&pattern) {
                        Ok(p) => p,
                        Err(e) => {
                            *status_clone.lock().unwrap() =
                                format!("Error: Invalid pattern - {}", e);
                            *processing_clone.lock().unwrap() = false;
                            return;
                        }
                    };
                }

                // Add progress callback
                processor = processor.with_progress_callback(move |current, total| {
                    *progress_clone.lock().unwrap() = (current, total);
                });

                // Process files
                let process_result = processor.process(&input_paths);

                match process_result {
                    Ok(batch_results) => {
                        // Store results
                        let success_count = batch_results.iter().filter(|r| r.success).count();
                        *results_clone.lock().unwrap() = batch_results;

                        // Update status
                        *status_clone.lock().unwrap() = format!(
                            "Batch processing complete: {}/{} files processed successfully.",
                            success_count,
                            results_clone.lock().unwrap().len()
                        );
                    }
                    Err(e) => {
                        *status_clone.lock().unwrap() =
                            format!("Error during batch processing: {}", e);
                    }
                }

                *processing_clone.lock().unwrap() = false;
            });
        }
    }

    fn clipper_settings_ui(&mut self, ui: &mut Ui) {
        ui.heading("Clipper Settings");

        // Output directory
        ui.horizontal(|ui| {
            ui.label("Output Directory:");
            ui.text_edit_singleline(&mut self.clipper_output_dir);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.clipper_output_dir = path.to_string_lossy().to_string();
                }
            }
        });

        // Time ranges
        ui.label("Time Ranges (format: START-END, e.g., 00:01:00-00:02:00):");

        let mut remove_idx = None;
        let mut ranges = self.clipper_time_ranges.clone();

        // First collect the user interactions without mutable references
        for i in 0..ranges.len() {
            let can_remove = ranges.len() > 1;
            let mut remove_this = false;

            ui.horizontal(|ui| {
                ui.label(format!("Range {}:", i + 1));
                ui.text_edit_singleline(&mut ranges[i]);
                if ui.button("Remove").clicked() && can_remove {
                    remove_this = true;
                }
            });

            if remove_this {
                remove_idx = Some(i);
            }
        }

        self.clipper_time_ranges = ranges;

        // Remove range if needed
        if let Some(idx) = remove_idx {
            if self.clipper_time_ranges.len() > 1 {
                self.clipper_time_ranges.remove(idx);
            }
        }

        // Add new range button
        if ui.button("Add Time Range").clicked() {
            self.clipper_time_ranges.push(String::new());
        }

        // Options
        ui.checkbox(
            &mut self.clipper_copy_codec,
            "Copy codec (faster but less precise)",
        );

        ui.horizontal(|ui| {
            ui.label("Suffix:");
            ui.text_edit_singleline(&mut self.clipper_suffix);
        });
    }

    fn gif_converter_settings_ui(&mut self, ui: &mut Ui) {
        ui.heading("GIF Converter Settings");

        // Output directory
        ui.horizontal(|ui| {
            ui.label("Output Directory:");
            ui.text_edit_singleline(&mut self.gif_output_dir);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.gif_output_dir = path.to_string_lossy().to_string();
                }
            }
        });

        // Width, FPS, and size settings
        ui.horizontal(|ui| {
            ui.label("Width:");
            ui.text_edit_singleline(&mut self.gif_width);
            ui.label("(leave empty for auto)");
        });

        ui.horizontal(|ui| {
            ui.label("FPS:");
            ui.text_edit_singleline(&mut self.gif_fps);
        });

        ui.horizontal(|ui| {
            ui.label("Max Size (MB):");
            ui.text_edit_singleline(&mut self.gif_max_size);
        });

        // Optimization option
        ui.checkbox(
            &mut self.gif_optimize,
            "Optimize (try multiple settings to achieve size target)",
        );
    }

    fn gif_transparency_settings_ui(&mut self, ui: &mut Ui) {
        ui.heading("GIF Transparency Settings");

        // Backup option
        ui.checkbox(
            &mut self.transparency_backup,
            "Create backup of original files",
        );
    }

    fn splitter_settings_ui(&mut self, ui: &mut Ui) {
        ui.heading("Splitter Settings");

        // Output directory
        ui.horizontal(|ui| {
            ui.label("Output Directory:");
            ui.text_edit_singleline(&mut self.splitter_output_dir);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.splitter_output_dir = path.to_string_lossy().to_string();
                }
            }
        });

        // Prefix and encoding options
        ui.horizontal(|ui| {
            ui.label("Filename Prefix:");
            ui.text_edit_singleline(&mut self.splitter_prefix);
        });

        ui.horizontal(|ui| {
            ui.label("Custom Encode Options:");
            ui.text_edit_singleline(&mut self.splitter_custom_encode);
        });

        // Force option
        ui.checkbox(
            &mut self.splitter_force,
            "Force (process even if video dimensions are not 1920x1080)",
        );
    }

    fn merger_settings_ui(&mut self, ui: &mut Ui) {
        ui.heading("Merger Settings");

        // Audio file
        ui.horizontal(|ui| {
            ui.label("Audio File:");
            ui.text_edit_singleline(&mut self.merger_audio_file);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Audio Files", &["mp3", "aac", "wav", "m4a", "flac", "ogg"])
                    .pick_file()
                {
                    self.merger_audio_file = path.to_string_lossy().to_string();
                }
            }
        });

        // Output directory
        ui.horizontal(|ui| {
            ui.label("Output Directory:");
            ui.text_edit_singleline(&mut self.merger_output_dir);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.merger_output_dir = path.to_string_lossy().to_string();
                }
            }
        });

        // Options
        ui.checkbox(
            &mut self.merger_shortest,
            "Use -shortest flag (end when shortest input stream ends)",
        );
        ui.checkbox(
            &mut self.merger_copy_codec,
            "Copy codec without re-encoding (faster)",
        );
    }
}
