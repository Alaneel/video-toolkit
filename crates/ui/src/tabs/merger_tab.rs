use crate::theme;
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

        theme::section(ui, "Picture", None, |ui| {
            theme::field(ui, "Video", |ui| {
                if theme::path_input(ui, &mut self.video_file, "The video to keep") {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Video Files", &video_extensions)
                        .pick_file()
                    {
                        self.video_file = path.to_string_lossy().to_string();
                        if self.output_file.is_empty() {
                            let input_path = Path::new(&self.video_file);
                            if let Some(stem) = input_path.file_stem() {
                                let mut output_path = PathBuf::from(
                                    input_path.parent().unwrap_or_else(|| Path::new("")),
                                );
                                output_path.push(format!("{}_merged", stem.to_string_lossy()));
                                output_path.set_extension("mp4");
                                self.output_file = output_path.to_string_lossy().to_string();
                            }
                        }
                    }
                }
            });
        });

        theme::section(
            ui,
            "Soundtrack",
            Some("Replace the video's audio with a file or another video's audio."),
            |ui| {
                let current = if self.audio_source == AudioSource::File {
                    0
                } else {
                    1
                };
                if let Some(picked) = theme::segmented(ui, current, &["Audio file", "From a video"])
                {
                    self.audio_source = if picked == 0 {
                        AudioSource::File
                    } else {
                        AudioSource::Extract
                    };
                }
                ui.add_space(4.0);
                match self.audio_source {
                    AudioSource::File => {
                        theme::field(ui, "Audio file", |ui| {
                            if theme::path_input(ui, &mut self.audio_file, "AAC, MP3, WAV or M4A") {
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
                        theme::field(ui, "Source video", |ui| {
                            if theme::path_input(
                                ui,
                                &mut self.audio_extract_file,
                                "Video whose audio you want",
                            ) {
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
            },
        );

        theme::section(
            ui,
            "Output",
            Some("The format follows the file extension; no extension means .mp4."),
            |ui| {
                theme::field(ui, "Save as", |ui| {
                    if theme::path_input(
                        ui,
                        &mut self.output_file,
                        "Chosen automatically from the video",
                    ) {
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
                theme::field(ui, "Length", |ui| {
                    ui.checkbox(&mut self.use_shortest, "Stop when the shorter input ends");
                });
                theme::field(ui, "Speed", |ui| {
                    ui.checkbox(&mut self.copy_codec, "Copy streams without re-encoding");
                });
            },
        );

        ui.add_space(4.0);
        let button = ui.add_enabled(
            !*self.processing.lock().unwrap(),
            theme::primary_button("Merge").min_size(egui::vec2(160.0, 40.0)),
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
