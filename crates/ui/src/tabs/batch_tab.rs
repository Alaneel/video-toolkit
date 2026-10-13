use eframe::egui::{self, ComboBox, Ui};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use batch_processing::{
    BatchItemResult, BatchProcessor, ProfileBatchConfig, ProfileOperationConfig,
};
use common::VideoFormat;
use profile_system::{Profile, ProfileType};

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
    output_format: VideoFormat,
    loaded_profile: Option<String>,

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
            output_format: VideoFormat::MP4,
            loaded_profile: None,

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
            merger_copy_codec: false,

            status,
            processing: processing.clone(),
            results: Arc::new(Mutex::new(Vec::new())),
            progress: Arc::new(Mutex::new((0, 0))),
        }
    }

    /// Load validated settings without changing inputs or starting processing.
    pub fn apply_profile(&mut self, profile: &Profile) -> anyhow::Result<()> {
        if *self.processing.lock().unwrap() {
            anyhow::bail!("Wait for processing to finish before loading a profile");
        }
        let config = ProfileBatchConfig::from_profile(profile)?;

        // Parsing completes before any form field changes, so invalid profiles
        // leave the existing batch setup intact.
        match config.operation {
            ProfileOperationConfig::Clipper(settings) => {
                self.operation_type = BatchOperationType::Clipper;
                self.clipper_time_ranges = settings
                    .time_ranges
                    .into_iter()
                    .map(|(start, end)| format!("{}-{}", start, end))
                    .collect();
                self.clipper_output_dir = settings.output_dir.to_string_lossy().into_owned();
                self.clipper_copy_codec = settings.copy_codec;
                self.clipper_suffix = settings.suffix.unwrap_or_default();
            }
            ProfileOperationConfig::GifConverter(settings) => {
                self.operation_type = BatchOperationType::GifConverter;
                self.gif_output_dir = settings.output_dir.to_string_lossy().into_owned();
                self.gif_width = settings
                    .width
                    .map(|width| width.to_string())
                    .unwrap_or_default();
                self.gif_fps = settings.fps.to_string();
                self.gif_max_size = settings.max_size_mb.to_string();
                self.gif_optimize = settings.optimize;
            }
            ProfileOperationConfig::GifTransparency(settings) => {
                self.operation_type = BatchOperationType::GifTransparency;
                self.transparency_backup = settings.create_backup;
            }
            ProfileOperationConfig::Splitter(settings) => {
                self.operation_type = BatchOperationType::Splitter;
                self.splitter_output_dir = settings.output_dir.to_string_lossy().into_owned();
                self.splitter_prefix = settings.prefix;
                self.splitter_custom_encode = settings.custom_encode.unwrap_or_default();
                self.splitter_force = settings.force;
            }
            ProfileOperationConfig::Merger(settings) => {
                self.operation_type = BatchOperationType::Merger;
                self.merger_audio_file = settings.audio_file.to_string_lossy().into_owned();
                self.merger_output_dir = settings.output_dir.to_string_lossy().into_owned();
                self.merger_shortest = settings.use_shortest;
                self.merger_copy_codec = settings.copy_codec;
            }
        }
        self.output_format = config.output_format;
        self.recursive = config.recursive;
        self.parallel = config.parallel;
        self.pattern = config.pattern.unwrap_or_default();
        self.loaded_profile = Some(profile.name.clone());
        Ok(())
    }

    fn current_profile(&self) -> Profile {
        let mut parameters = HashMap::from([
            ("recursive".to_string(), self.recursive.to_string()),
            ("parallel".to_string(), self.parallel.to_string()),
            ("pattern".to_string(), self.pattern.clone()),
        ]);
        let profile_type = match self.operation_type {
            BatchOperationType::Clipper => {
                parameters.extend([
                    (
                        "time_ranges".to_string(),
                        self.clipper_time_ranges.join(","),
                    ),
                    ("output_dir".to_string(), self.clipper_output_dir.clone()),
                    (
                        "copy_codec".to_string(),
                        self.clipper_copy_codec.to_string(),
                    ),
                    ("suffix".to_string(), self.clipper_suffix.clone()),
                    ("output_format".to_string(), self.output_format.to_string()),
                ]);
                ProfileType::Clipper
            }
            BatchOperationType::GifConverter => {
                parameters.extend([
                    ("width".to_string(), self.gif_width.clone()),
                    ("fps".to_string(), self.gif_fps.clone()),
                    ("max_size_mb".to_string(), self.gif_max_size.clone()),
                    ("optimize".to_string(), self.gif_optimize.to_string()),
                    ("output_dir".to_string(), self.gif_output_dir.clone()),
                ]);
                ProfileType::GifConverter
            }
            BatchOperationType::GifTransparency => {
                parameters.insert(
                    "create_backup".to_string(),
                    self.transparency_backup.to_string(),
                );
                ProfileType::GifTransparency
            }
            BatchOperationType::Splitter => {
                parameters.extend([
                    ("output_dir".to_string(), self.splitter_output_dir.clone()),
                    ("prefix".to_string(), self.splitter_prefix.clone()),
                    (
                        "custom_encode".to_string(),
                        self.splitter_custom_encode.clone(),
                    ),
                    ("force".to_string(), self.splitter_force.to_string()),
                    ("output_format".to_string(), self.output_format.to_string()),
                ]);
                ProfileType::Splitter
            }
            BatchOperationType::Merger => {
                parameters.extend([
                    ("audio_file".to_string(), self.merger_audio_file.clone()),
                    ("output_dir".to_string(), self.merger_output_dir.clone()),
                    ("use_shortest".to_string(), self.merger_shortest.to_string()),
                    ("copy_codec".to_string(), self.merger_copy_codec.to_string()),
                    ("output_format".to_string(), self.output_format.to_string()),
                ]);
                ProfileType::Merger
            }
        };
        Profile::new(
            self.loaded_profile.as_deref().unwrap_or("Batch settings"),
            profile_type,
            parameters,
        )
    }

    fn validated_processor(&self) -> anyhow::Result<BatchProcessor> {
        Ok(ProfileBatchConfig::from_profile(&self.current_profile())?.into_processor()?)
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        ui.heading("Batch Processing");
        if let Some(name) = &self.loaded_profile {
            ui.label(format!(
                "Loaded profile: {} (settings can be adjusted below)",
                name
            ));
        }

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

        if matches!(
            self.operation_type,
            BatchOperationType::Clipper | BatchOperationType::Splitter | BatchOperationType::Merger
        ) {
            ui.horizontal(|ui| {
                ui.label("Output Format:");
                ComboBox::from_id_source("batch_output_format")
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
            self.start_batch_processing();
        }
    }

    fn start_batch_processing(&mut self) {
        if *self.processing.lock().unwrap() {
            *self.status.lock().unwrap() =
                "Error: Wait for processing to finish before starting another batch.".to_string();
            return;
        }
        if self.input_paths.is_empty() {
            *self.status.lock().unwrap() =
                "Error: Please add at least one input file or directory.".to_string();
            return;
        }
        let processor = match self.validated_processor() {
            Ok(processor) => processor,
            Err(error) => {
                *self.status.lock().unwrap() = format!("Error: {}", error);
                return;
            }
        };

        // Validate the complete form before changing job state or spawning work.
        *self.status.lock().unwrap() = format!(
            "Starting batch {} processing...",
            self.operation_type.display_name()
        );
        *self.processing.lock().unwrap() = true;
        self.results.lock().unwrap().clear();
        *self.progress.lock().unwrap() = (0, 0);

        let input_paths = self.input_paths.clone();
        let status = Arc::clone(&self.status);
        let processing = Arc::clone(&self.processing);
        let results = Arc::clone(&self.results);
        let progress = Arc::clone(&self.progress);
        thread::spawn(move || {
            let processor = processor.with_progress_callback(move |current, total| {
                *progress.lock().unwrap() = (current, total);
            });
            match processor.process(&input_paths) {
                Ok(batch_results) => {
                    let success_count =
                        batch_results.iter().filter(|result| result.success).count();
                    let total = batch_results.len();
                    *results.lock().unwrap() = batch_results;
                    *status.lock().unwrap() = format!(
                        "Batch processing complete: {}/{} files processed successfully.",
                        success_count, total
                    );
                }
                Err(error) => {
                    *status.lock().unwrap() = format!("Error during batch processing: {}", error);
                }
            }
            *processing.lock().unwrap() = false;
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use profile_system::ProfileType;
    use std::collections::HashMap;

    fn tab() -> BatchTab {
        let mut tab = BatchTab::new(
            Arc::new(Mutex::new("Ready".into())),
            Arc::new(Mutex::new(false)),
        );
        tab.input_paths = vec![PathBuf::from("selected-input.mp4")];
        tab
    }

    fn profile(profile_type: ProfileType, parameters: &[(&str, &str)]) -> Profile {
        Profile::new(
            "saved settings",
            profile_type,
            parameters
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<HashMap<_, _>>(),
        )
    }

    #[test]
    fn loading_clipper_maps_settings_and_preserves_inputs() {
        let mut tab = tab();
        let selected_inputs = tab.input_paths.clone();
        tab.apply_profile(&profile(
            ProfileType::Clipper,
            &[
                ("ranges", "0-00:05,00:10-00:15"),
                ("output_dir", "saved-clips"),
                ("copy_codec", "true"),
                ("suffix", "preview"),
                ("format", "mkv"),
                ("recursive", "true"),
                ("parallel", "false"),
                ("pattern", ".*\\.mp4$"),
            ],
        ))
        .unwrap();
        assert!(matches!(tab.operation_type, BatchOperationType::Clipper));
        assert_eq!(tab.clipper_time_ranges, ["0-00:05", "00:10-00:15"]);
        assert_eq!(tab.clipper_output_dir, "saved-clips");
        assert!(tab.clipper_copy_codec);
        assert_eq!(tab.clipper_suffix, "preview");
        assert_eq!(tab.output_format, VideoFormat::MKV);
        assert!(tab.recursive);
        assert!(!tab.parallel);
        assert_eq!(tab.pattern, ".*\\.mp4$");
        assert_eq!(tab.loaded_profile.as_deref(), Some("saved settings"));
        assert_eq!(tab.input_paths, selected_inputs);
        assert!(!*tab.processing.lock().unwrap());
    }

    #[test]
    fn loading_gif_maps_settings_and_clears_optional_values() {
        let mut tab = tab();
        tab.apply_profile(&profile(
            ProfileType::GifConverter,
            &[
                ("width", "640"),
                ("fps", "24"),
                ("max_size", "2.5"),
                ("optimize", "true"),
                ("output_dir", "saved-gifs"),
            ],
        ))
        .unwrap();
        assert!(matches!(
            tab.operation_type,
            BatchOperationType::GifConverter
        ));
        assert_eq!(tab.gif_width, "640");
        assert_eq!(tab.gif_fps, "24");
        assert_eq!(tab.gif_max_size, "2.5");
        assert!(tab.gif_optimize);
        assert_eq!(tab.gif_output_dir, "saved-gifs");
        tab.apply_profile(&profile(ProfileType::GifConverter, &[]))
            .unwrap();
        assert!(tab.gif_width.is_empty());
        assert_eq!(tab.gif_fps, "10");
        assert_eq!(tab.gif_max_size, "5");
        assert!(!tab.gif_optimize);
        assert_eq!(tab.gif_output_dir, "output_gifs");
    }

    #[test]
    fn loading_transparency_maps_backup_choice() {
        let mut tab = tab();
        tab.apply_profile(&profile(
            ProfileType::GifTransparency,
            &[("backup", "false")],
        ))
        .unwrap();
        assert!(matches!(
            tab.operation_type,
            BatchOperationType::GifTransparency
        ));
        assert!(!tab.transparency_backup);
    }

    #[test]
    fn loading_splitter_maps_all_settings() {
        let mut tab = tab();
        tab.apply_profile(&profile(
            ProfileType::Splitter,
            &[
                ("output_dir", "saved-slices"),
                ("prefix", "preview"),
                ("custom_encode", "-crf 25"),
                ("force", "true"),
                ("format", "webm"),
            ],
        ))
        .unwrap();
        assert!(matches!(tab.operation_type, BatchOperationType::Splitter));
        assert_eq!(tab.splitter_output_dir, "saved-slices");
        assert_eq!(tab.splitter_prefix, "preview");
        assert_eq!(tab.splitter_custom_encode, "-crf 25");
        assert!(tab.splitter_force);
        assert_eq!(tab.output_format, VideoFormat::WebM);
    }

    #[test]
    fn loading_merger_maps_all_settings() {
        let mut tab = tab();
        tab.apply_profile(&profile(
            ProfileType::Merger,
            &[
                ("audio", "soundtrack.wav"),
                ("output_dir", "saved-merged"),
                ("shortest", "false"),
                ("copy_codec", "true"),
                ("format", "mov"),
            ],
        ))
        .unwrap();
        assert!(matches!(tab.operation_type, BatchOperationType::Merger));
        assert_eq!(tab.merger_audio_file, "soundtrack.wav");
        assert_eq!(tab.merger_output_dir, "saved-merged");
        assert!(!tab.merger_shortest);
        assert!(tab.merger_copy_codec);
        assert_eq!(tab.output_format, VideoFormat::MOV);
    }

    #[test]
    fn invalid_or_custom_profiles_do_not_partially_change_batch_settings() {
        let mut tab = tab();
        let inputs = tab.input_paths.clone();
        for invalid in [
            profile(
                ProfileType::GifConverter,
                &[("fps", "0"), ("recursive", "false")],
            ),
            profile(ProfileType::Splitter, &[("pattern", "[")]),
            profile(ProfileType::Custom("extension".into()), &[]),
        ] {
            assert!(tab.apply_profile(&invalid).is_err());
            assert!(matches!(tab.operation_type, BatchOperationType::Clipper));
            assert_eq!(tab.input_paths, inputs);
            assert!(tab.recursive);
            assert!(tab.parallel);
            assert!(tab.pattern.is_empty());
            assert_eq!(tab.output_format, VideoFormat::MP4);
            assert_eq!(tab.gif_fps, "10");
            assert_eq!(tab.splitter_prefix, "slice");
            assert!(tab.loaded_profile.is_none());
        }
    }

    #[test]
    fn loading_during_processing_leaves_settings_intact() {
        let mut tab = tab();
        *tab.processing.lock().unwrap() = true;
        assert!(tab
            .apply_profile(&profile(ProfileType::GifTransparency, &[]))
            .is_err());
        assert!(matches!(tab.operation_type, BatchOperationType::Clipper));
        assert!(tab.loaded_profile.is_none());
        assert!(*tab.processing.lock().unwrap());
    }

    fn assert_form_rejected_before_job_start(tab: &mut BatchTab, parameter: &str) {
        *tab.progress.lock().unwrap() = (4, 5);
        *tab.results.lock().unwrap() = vec![BatchItemResult {
            input: PathBuf::from("previous-input.mp4"),
            output: Some(PathBuf::from("previous-output.mp4")),
            success: true,
            error_message: None,
        }];
        assert!(tab.validated_processor().is_err());
        tab.start_batch_processing();
        assert!(!*tab.processing.lock().unwrap());
        assert_eq!(*tab.progress.lock().unwrap(), (4, 5));
        assert_eq!(tab.results.lock().unwrap().len(), 1);
        assert!(tab.status.lock().unwrap().contains(parameter));
    }

    #[test]
    fn edited_numeric_gif_fields_use_shared_validation_before_starting() {
        for (parameter, invalid_values) in [
            ("width", &["0", "-1", "oops", "4294967296"][..]),
            ("fps", &["0", "-1", "oops", ""][..]),
            ("max_size_mb", &["0", "-1", "NaN", "inf", "1e309", ""][..]),
        ] {
            for invalid in invalid_values {
                let mut tab = tab();
                tab.operation_type = BatchOperationType::GifConverter;
                match parameter {
                    "width" => tab.gif_width = invalid.to_string(),
                    "fps" => tab.gif_fps = invalid.to_string(),
                    "max_size_mb" => tab.gif_max_size = invalid.to_string(),
                    _ => unreachable!(),
                }
                assert_form_rejected_before_job_start(&mut tab, parameter);
            }
        }
    }

    #[test]
    fn invalid_or_blank_clipper_rows_are_not_silently_skipped() {
        for ranges in [
            vec![],
            vec![""],
            vec!["0-00:05", " "],
            vec!["0-00:05", "broken"],
            vec!["00:10-00:05"],
        ] {
            let mut tab = tab();
            tab.clipper_time_ranges = ranges.into_iter().map(str::to_string).collect();
            assert_form_rejected_before_job_start(&mut tab, "time_ranges");
        }
    }

    #[test]
    fn invalid_regex_and_blank_audio_do_not_start_jobs() {
        let mut invalid_regex = tab();
        invalid_regex.operation_type = BatchOperationType::GifTransparency;
        invalid_regex.pattern = "[".to_string();
        assert_form_rejected_before_job_start(&mut invalid_regex, "pattern");

        let mut blank_audio = tab();
        blank_audio.operation_type = BatchOperationType::Merger;
        blank_audio.merger_audio_file = "  ".to_string();
        assert_form_rejected_before_job_start(&mut blank_audio, "audio_file");
    }

    #[test]
    fn all_five_operations_build_processors_from_current_form() {
        for profile_type in [
            ProfileType::Clipper,
            ProfileType::GifConverter,
            ProfileType::GifTransparency,
            ProfileType::Splitter,
            ProfileType::Merger,
        ] {
            let mut parameters: HashMap<_, _> =
                batch_processing::default_profile_parameters(&profile_type)
                    .into_iter()
                    .collect();
            if profile_type == ProfileType::Merger {
                parameters.insert("audio".to_string(), "soundtrack.wav".to_string());
            }
            let profile = Profile::new("round trip", profile_type, parameters);
            let mut tab = tab();
            tab.apply_profile(&profile).unwrap();
            assert!(tab.validated_processor().is_ok());
            assert_eq!(tab.current_profile().profile_type, profile.profile_type);
            assert!(!*tab.processing.lock().unwrap());
            assert_eq!(tab.loaded_profile.as_deref(), Some("round trip"));
        }
    }

    #[test]
    fn blank_optional_fields_stay_optional_in_current_form() {
        let mut tab = tab();
        tab.operation_type = BatchOperationType::GifConverter;
        tab.gif_width = "  ".to_string();
        tab.pattern.clear();
        let config = ProfileBatchConfig::from_profile(&tab.current_profile()).unwrap();
        assert!(config.pattern.is_none());
        match config.operation {
            ProfileOperationConfig::GifConverter(settings) => assert!(settings.width.is_none()),
            _ => panic!("expected GIF conversion"),
        }
        tab.operation_type = BatchOperationType::Splitter;
        tab.splitter_custom_encode = "  ".to_string();
        let config = ProfileBatchConfig::from_profile(&tab.current_profile()).unwrap();
        match config.operation {
            ProfileOperationConfig::Splitter(settings) => assert!(settings.custom_encode.is_none()),
            _ => panic!("expected splitting"),
        }
        tab.operation_type = BatchOperationType::Clipper;
        tab.clipper_time_ranges = vec!["0-00:05".to_string()];
        tab.clipper_suffix = "  ".to_string();
        let config = ProfileBatchConfig::from_profile(&tab.current_profile()).unwrap();
        match config.operation {
            ProfileOperationConfig::Clipper(settings) => assert!(settings.suffix.is_none()),
            _ => panic!("expected clipping"),
        }
        tab.pattern = "  ".to_string();
        assert_eq!(
            ProfileBatchConfig::from_profile(&tab.current_profile())
                .unwrap()
                .pattern
                .as_deref(),
            Some("  ")
        );
    }
}
