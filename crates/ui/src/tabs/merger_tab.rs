use eframe::egui::{self, Ui};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use merger::{extract_audio, merge_audio_video};

#[derive(PartialEq)]
pub enum AudioSource {
    File,
    Extract,
}

pub struct MergerTab {
    video_file: String,
    audio_source: AudioSource,
    audio_file: String,
    audio_extract_file: String,
    output_file: String,
    use_shortest: bool,
    copy_codec: bool,
    status: Arc<Mutex<String>>,
    processing: Arc<Mutex<bool>>,
}

impl MergerTab {
    pub fn new(status: Arc<Mutex<String>>, processing: Arc<Mutex<bool>>) -> Self {
        Self {
            video_file: String::new(),
            audio_source: AudioSource::File,
            audio_file: String::new(),
            audio_extract_file: String::new(),
            output_file: String::new(),
            use_shortest: true,
            copy_codec: true,
            status,
            processing,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        // Video input section
        ui.heading("Video Input");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.video_file);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Video Files", &["mp4", "avi", "mov", "mkv"])
                    .pick_file()
                {
                    self.video_file = path.to_string_lossy().to_string();

                    // Set default output file if not set
                    if self.output_file.is_empty() {
                        let input_path = Path::new(&self.video_file);
                        if let Some(stem) = input_path.file_stem() {
                            let mut output_path =
                                PathBuf::from(input_path.parent().unwrap_or_else(|| Path::new("")));
                            output_path.push(format!("{}_merged", stem.to_string_lossy()));
                            output_path.set_extension("mp4");
                            self.output_file = output_path.to_string_lossy().to_string();
                        }
                    }
                }
            }
        });

        // Audio source options
        ui.heading("Audio Source");
        ui.radio_value(&mut self.audio_source, AudioSource::File, "Use audio file");
        ui.radio_value(
            &mut self.audio_source,
            AudioSource::Extract,
            "Extract from video file",
        );

        // Audio input section (conditional based on audio source)
        match self.audio_source {
            AudioSource::File => {
                ui.horizontal(|ui| {
                    ui.label("Audio File:");
                    ui.text_edit_singleline(&mut self.audio_file);
                    if ui.button("Browse").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Audio Files", &["aac", "mp3", "wav", "m4a"])
                            .pick_file()
                        {
                            self.audio_file = path.to_string_lossy().to_string();
                        }
                    }
                });
            }
            AudioSource::Extract => {
                ui.horizontal(|ui| {
                    ui.label("Source Video:");
                    ui.text_edit_singleline(&mut self.audio_extract_file);
                    if ui.button("Browse").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Video Files", &["mp4", "avi", "mov", "mkv"])
                            .pick_file()
                        {
                            self.audio_extract_file = path.to_string_lossy().to_string();
                        }
                    }
                });
            }
        }

        // Output file section
        ui.heading("Output File");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.output_file);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("MP4 Files", &["mp4"])
                    .save_file()
                {
                    self.output_file = path.to_string_lossy().to_string();

                    // Make sure it has .mp4 extension
                    if !self.output_file.ends_with(".mp4") {
                        self.output_file.push_str(".mp4");
                    }
                }
            }
        });

        // Options section
        ui.heading("Options");
        ui.checkbox(
            &mut self.use_shortest,
            "Use -shortest flag (end when shortest input stream ends)",
        );
        ui.checkbox(
            &mut self.copy_codec,
            "Copy codec without re-encoding (faster)",
        );

        // Execute button
        ui.add_space(10.0);
        let button = ui.add_enabled(
            !*self.processing.lock().unwrap(),
            egui::Button::new("Merge Audio and Video"),
        );

        if button.clicked() {
            if self.video_file.is_empty() {
                *self.status.lock().unwrap() =
                    "Error: Please select an input video file.".to_string();
                return;
            }

            let audio_source_valid = match self.audio_source {
                AudioSource::File => !self.audio_file.is_empty(),
                AudioSource::Extract => !self.audio_extract_file.is_empty(),
            };

            if !audio_source_valid {
                *self.status.lock().unwrap() = "Error: Please select an audio source.".to_string();
                return;
            }

            if self.output_file.is_empty() {
                *self.status.lock().unwrap() = "Error: Please specify an output file.".to_string();
                return;
            }

            // Start processing in a separate thread
            *self.status.lock().unwrap() = "Merging audio and video...".to_string();
            *self.processing.lock().unwrap() = true;

            // Clone values for thread
            let video_file = self.video_file.clone();
            let audio_source = match self.audio_source {
                AudioSource::File => self.audio_file.clone(),
                AudioSource::Extract => {
                    // We'll extract to a temporary file
                    let temp_dir = Path::new(&self.output_file)
                        .parent()
                        .unwrap_or_else(|| Path::new(""));
                    let temp_audio = temp_dir
                        .join("temp_audio.aac")
                        .to_string_lossy()
                        .to_string();
                    temp_audio
                }
            };
            let audio_extract_file = self.audio_extract_file.clone();
            let output_file = self.output_file.clone();
            let use_shortest = self.use_shortest;
            let copy_codec = self.copy_codec;
            let is_extract = matches!(self.audio_source, AudioSource::Extract);
            let status_clone = Arc::clone(&self.status);
            let processing_clone = Arc::clone(&self.processing);

            thread::spawn(move || {
                let result = if is_extract {
                    // First extract audio
                    *status_clone.lock().unwrap() = "Extracting audio from video...".to_string();
                    match extract_audio(&audio_extract_file, &audio_source) {
                        Ok(_) => {
                            // Then merge
                            *status_clone.lock().unwrap() =
                                "Merging audio with video...".to_string();
                            let merge_result = merge_audio_video(
                                &video_file,
                                &audio_source,
                                &output_file,
                                use_shortest,
                                copy_codec,
                            );

                            // Clean up temporary file
                            let _ = std::fs::remove_file(&audio_source);

                            merge_result
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    // Directly merge with existing audio file
                    merge_audio_video(
                        &video_file,
                        &audio_source,
                        &output_file,
                        use_shortest,
                        copy_codec,
                    )
                };

                match result {
                    Ok(_) => {
                        *status_clone.lock().unwrap() = format!(
                            "Successfully merged audio and video. Output: {}",
                            output_file
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
