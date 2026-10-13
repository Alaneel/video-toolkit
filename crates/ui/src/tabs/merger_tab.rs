use eframe::egui::{self, Ui};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use common::{resolve_video_output, VideoFormat};
use merger::merge_audio_video;

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
            copy_codec: false,
            status,
            processing,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let video_formats = VideoFormat::all();
        let video_extensions: Vec<&str> =
            video_formats.iter().map(VideoFormat::extension).collect();

        // Video input section
        ui.heading("Video Input");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.video_file);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Video Files", &video_extensions)
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
            "Use audio from another video",
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
                            .add_filter("Video Files", &video_extensions)
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
                    .add_filter("Video Files", &video_extensions)
                    .save_file()
                {
                    match resolve_video_output(&path, None) {
                        Ok((output, _)) => {
                            self.output_file = output.to_string_lossy().to_string();
                        }
                        Err(error) => {
                            *self.status.lock().unwrap() = format!("Error: {}", error);
                        }
                    }
                }
            }
        });
        ui.label("Output format follows the filename extension. No extension defaults to .mp4.");

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

            match resolve_video_output(Path::new(&self.output_file), None) {
                Ok((output, _)) => {
                    self.output_file = output.to_string_lossy().to_string();
                }
                Err(error) => {
                    *self.status.lock().unwrap() = format!("Error: {}", error);
                    return;
                }
            }

            // Start processing in a separate thread
            *self.status.lock().unwrap() = "Merging audio and video...".to_string();
            *self.processing.lock().unwrap() = true;

            // Clone values for thread
            let video_file = self.video_file.clone();
            let audio_source = match self.audio_source {
                AudioSource::File => self.audio_file.clone(),
                AudioSource::Extract => self.audio_extract_file.clone(),
            };
            let output_file = self.output_file.clone();
            let use_shortest = self.use_shortest;
            let copy_codec = self.copy_codec;
            let status_clone = Arc::clone(&self.status);
            let processing_clone = Arc::clone(&self.processing);

            thread::spawn(move || {
                let result = merge_audio_video(
                    &video_file,
                    &audio_source,
                    &output_file,
                    use_shortest,
                    copy_codec,
                );

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
