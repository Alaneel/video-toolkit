use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use regex::Regex;
use thiserror::Error;
use walkdir::WalkDir;

use common::{check_ffmpeg, validate_time_range, verify_input_file, VideoToolkitError};

/// Errors specific to batch processing
#[derive(Error, Debug)]
pub enum BatchError {
    #[error("No input files found matching the pattern")]
    NoInputFiles,

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("Operation error: {0}")]
    OperationError(#[from] VideoToolkitError),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Regex error: {0}")]
    RegexError(#[from] regex::Error),

    #[error("Other error: {0}")]
    Other(String),
}

/// Result type for batch operations
pub type Result<T> = std::result::Result<T, BatchError>;

/// Supported batch operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchOperation {
    Clipper,
    GifConverter,
    GifTransparency,
    Splitter,
    Merger,
}

impl std::fmt::Display for BatchOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BatchOperation::Clipper => write!(f, "Video Clipper"),
            BatchOperation::GifConverter => write!(f, "GIF Converter"),
            BatchOperation::GifTransparency => write!(f, "GIF Transparency"),
            BatchOperation::Splitter => write!(f, "Video Splitter"),
            BatchOperation::Merger => write!(f, "Audio/Video Merger"),
        }
    }
}

/// Result of a single operation within a batch
#[derive(Debug)]
pub struct BatchItemResult {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub success: bool,
    pub error_message: Option<String>,
}

/// Configuration for batch clipping
#[derive(Debug, Clone)]
pub struct BatchClipperConfig {
    pub time_ranges: Vec<(String, String)>,
    pub output_dir: PathBuf,
    pub copy_codec: bool,
    pub suffix: Option<String>,
}

/// Configuration for batch GIF conversion
#[derive(Debug, Clone)]
pub struct BatchGifConverterConfig {
    pub width: Option<u32>,
    pub fps: u32,
    pub max_size_mb: f64,
    pub optimize: bool,
    pub output_dir: PathBuf,
}

/// Configuration for batch GIF transparency
#[derive(Debug, Clone)]
pub struct BatchGifTransparencyConfig {
    pub create_backup: bool,
}

/// Configuration for batch video splitting
#[derive(Debug, Clone)]
pub struct BatchSplitterConfig {
    pub output_dir: PathBuf,
    pub prefix: String,
    pub custom_encode: Option<String>,
    pub force: bool,
}

/// Configuration for batch audio/video merging
#[derive(Debug, Clone)]
pub struct BatchMergerConfig {
    pub audio_file: PathBuf,
    pub output_dir: PathBuf,
    pub use_shortest: bool,
    pub copy_codec: bool,
}

/// The main batch processor
pub struct BatchProcessor {
    operation: BatchOperation,
    input_pattern: Option<Regex>,
    parallel: bool,
    recursive: bool,

    // Operation-specific configurations
    clipper_config: Option<BatchClipperConfig>,
    gif_converter_config: Option<BatchGifConverterConfig>,
    gif_transparency_config: Option<BatchGifTransparencyConfig>,
    splitter_config: Option<BatchSplitterConfig>,
    merger_config: Option<BatchMergerConfig>,

    // Progress callback
    progress_callback: Option<Box<dyn Fn(usize, usize) + Send + Sync>>,
}

impl BatchProcessor {
    /// Create a new batch processor for a specific operation
    pub fn new(operation: BatchOperation) -> Self {
        Self {
            operation,
            input_pattern: None,
            parallel: true,
            recursive: false,
            clipper_config: None,
            gif_converter_config: None,
            gif_transparency_config: None,
            splitter_config: None,
            merger_config: None,
            progress_callback: None,
        }
    }

    /// Set a regex pattern to filter input files
    pub fn with_pattern(mut self, pattern: &str) -> Result<Self> {
        self.input_pattern = Some(Regex::new(pattern)?);
        Ok(self)
    }

    /// Enable or disable parallel processing
    pub fn with_parallel(mut self, parallel: bool) -> Self {
        self.parallel = parallel;
        self
    }

    /// Enable or disable recursive directory traversal
    pub fn with_recursive(mut self, recursive: bool) -> Self {
        self.recursive = recursive;
        self
    }

    /// Set configuration for batch clipping
    pub fn with_clipper_config(mut self, config: BatchClipperConfig) -> Self {
        self.clipper_config = Some(config);
        self
    }

    /// Set configuration for batch GIF conversion
    pub fn with_gif_converter_config(mut self, config: BatchGifConverterConfig) -> Self {
        self.gif_converter_config = Some(config);
        self
    }

    /// Set configuration for batch GIF transparency
    pub fn with_gif_transparency_config(mut self, config: BatchGifTransparencyConfig) -> Self {
        self.gif_transparency_config = Some(config);
        self
    }

    /// Set configuration for batch video splitting
    pub fn with_splitter_config(mut self, config: BatchSplitterConfig) -> Self {
        self.splitter_config = Some(config);
        self
    }

    /// Set configuration for batch audio/video merging
    pub fn with_merger_config(mut self, config: BatchMergerConfig) -> Self {
        self.merger_config = Some(config);
        self
    }

    /// Set a progress callback function
    pub fn with_progress_callback<F>(mut self, callback: F) -> Self
    where
        F: Fn(usize, usize) + Send + Sync + 'static,
    {
        self.progress_callback = Some(Box::new(callback));
        self
    }

    /// Find all input files matching the criteria
    fn find_input_files(&self, input_paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();

        for path in input_paths {
            if path.is_file() {
                // Process a single file
                if self.matches_pattern(path) {
                    files.push(path.clone());
                }
            } else if path.is_dir() {
                // Process a directory
                let walker = if self.recursive {
                    WalkDir::new(path)
                } else {
                    WalkDir::new(path).max_depth(1)
                };

                for entry in walker.into_iter().filter_map(|e| e.ok()) {
                    let entry_path = entry.path();
                    if entry_path.is_file() && self.matches_pattern(entry_path) {
                        files.push(entry_path.to_path_buf());
                    }
                }
            }
        }

        if files.is_empty() {
            return Err(BatchError::NoInputFiles);
        }

        Ok(files)
    }

    /// Check if a file matches the pattern
    fn matches_pattern(&self, path: &Path) -> bool {
        if let Some(ref pattern) = self.input_pattern {
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                return pattern.is_match(file_name);
            }
            return false;
        }

        // If no pattern is set, match by extension based on operation
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            match self.operation {
                BatchOperation::Clipper | BatchOperation::Splitter | BatchOperation::Merger => {
                    ext.eq_ignore_ascii_case("mp4")
                        || ext.eq_ignore_ascii_case("avi")
                        || ext.eq_ignore_ascii_case("mov")
                        || ext.eq_ignore_ascii_case("mkv")
                }
                BatchOperation::GifConverter => {
                    ext.eq_ignore_ascii_case("mp4")
                        || ext.eq_ignore_ascii_case("avi")
                        || ext.eq_ignore_ascii_case("mov")
                        || ext.eq_ignore_ascii_case("mkv")
                }
                BatchOperation::GifTransparency => ext.eq_ignore_ascii_case("gif"),
            }
        } else {
            false
        }
    }

    /// Process the batch operation on the input files
    pub fn process(&self, input_paths: &[PathBuf]) -> Result<Vec<BatchItemResult>> {
        // Check if FFmpeg is installed
        if !check_ffmpeg() {
            return Err(BatchError::Other("FFmpeg not found".to_string()));
        }

        // Find input files
        let input_files = self.find_input_files(input_paths)?;
        let total_files = input_files.len();

        // Create a progress bar if there's no custom callback
        let progress_bar = if self.progress_callback.is_none() {
            let pb = ProgressBar::new(total_files as u64);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
                    .unwrap()
            );
            Some(pb)
        } else {
            None
        };

        // Process files
        let results = if self.parallel {
            // For thread-safe progress tracking
            let processed = Arc::new(Mutex::new(0));

            // Process in parallel using Rayon
            let results: Vec<BatchItemResult> = input_files
                .par_iter()
                .map(|file| {
                    let result = self.process_file(file);

                    // Update progress
                    if let Some(ref progress_bar) = progress_bar {
                        progress_bar.inc(1);
                    } else if let Some(ref callback) = self.progress_callback {
                        let mut count = processed.lock().unwrap();
                        *count += 1;
                        callback(*count, total_files);
                    }

                    result
                })
                .collect();

            results
        } else {
            // Process sequentially
            let mut results = Vec::with_capacity(total_files);
            for (i, file) in input_files.iter().enumerate() {
                let result = self.process_file(file);

                // Update progress
                if let Some(ref progress_bar) = progress_bar {
                    progress_bar.inc(1);
                } else if let Some(ref callback) = self.progress_callback {
                    callback(i + 1, total_files);
                }

                results.push(result);
            }

            results
        };

        // Finish the progress bar
        if let Some(pb) = progress_bar {
            pb.finish_with_message("Batch processing complete");
        }

        Ok(results)
    }

    /// Process a single file
    fn process_file(&self, input_file: &Path) -> BatchItemResult {
        match self.operation {
            BatchOperation::Clipper => self.process_clipper(input_file),
            BatchOperation::GifConverter => self.process_gif_converter(input_file),
            BatchOperation::GifTransparency => self.process_gif_transparency(input_file),
            BatchOperation::Splitter => self.process_splitter(input_file),
            BatchOperation::Merger => self.process_merger(input_file),
        }
    }

    /// Process a file with the clipper
    fn process_clipper(&self, input_file: &Path) -> BatchItemResult {
        let config = match &self.clipper_config {
            Some(config) => config,
            None => {
                return BatchItemResult {
                    input: input_file.to_path_buf(),
                    output: None,
                    success: false,
                    error_message: Some("Clipper configuration not set".to_string()),
                }
            }
        };

        // Verify the input file exists
        if let Err(e) = verify_input_file(&input_file.to_string_lossy()) {
            return BatchItemResult {
                input: input_file.to_path_buf(),
                output: None,
                success: false,
                error_message: Some(format!("Error verifying input file: {}", e)),
            };
        }

        // Create the output directory
        if let Err(e) = std::fs::create_dir_all(&config.output_dir) {
            return BatchItemResult {
                input: input_file.to_path_buf(),
                output: None,
                success: false,
                error_message: Some(format!("Error creating output directory: {}", e)),
            };
        }

        // Run the clipper
        match clipper::clip_video(
            &input_file.to_string_lossy(),
            &config.time_ranges,
            &config.output_dir.to_string_lossy(),
            config.copy_codec,
            config.suffix.as_deref(),
        ) {
            Ok(true) => BatchItemResult {
                input: input_file.to_path_buf(),
                output: Some(config.output_dir.clone()),
                success: true,
                error_message: None,
            },
            Ok(false) => BatchItemResult {
                input: input_file.to_path_buf(),
                output: Some(config.output_dir.clone()),
                success: false,
                error_message: Some("Some clips failed to process".to_string()),
            },
            Err(e) => BatchItemResult {
                input: input_file.to_path_buf(),
                output: None,
                success: false,
                error_message: Some(format!("Error: {}", e)),
            },
        }
    }

    /// Process a file with the GIF converter
    fn process_gif_converter(&self, input_file: &Path) -> BatchItemResult {
        let config = match &self.gif_converter_config {
            Some(config) => config,
            None => {
                return BatchItemResult {
                    input: input_file.to_path_buf(),
                    output: None,
                    success: false,
                    error_message: Some("GIF converter configuration not set".to_string()),
                }
            }
        };

        // Create output file path
        let file_stem = match input_file.file_stem() {
            Some(stem) => stem.to_string_lossy(),
            None => {
                return BatchItemResult {
                    input: input_file.to_path_buf(),
                    output: None,
                    success: false,
                    error_message: Some("Invalid input filename".to_string()),
                }
            }
        };

        let output_file = config.output_dir.join(format!("{}.gif", file_stem));

        // Create the output directory
        if let Err(e) = std::fs::create_dir_all(&config.output_dir) {
            return BatchItemResult {
                input: input_file.to_path_buf(),
                output: None,
                success: false,
                error_message: Some(format!("Error creating output directory: {}", e)),
            };
        }

        // Run the GIF converter
        let result = if config.optimize {
            gif_converter::optimize_conversion(
                &input_file.to_string_lossy(),
                &output_file.to_string_lossy(),
                config.max_size_mb,
                config.width,
            )
        } else {
            gif_converter::convert_video_to_gif(
                &input_file.to_string_lossy(),
                &output_file.to_string_lossy(),
                config.width,
                config.fps,
                config.max_size_mb,
            )
        };

        match result {
            Ok(true) => BatchItemResult {
                input: input_file.to_path_buf(),
                output: Some(output_file),
                success: true,
                error_message: None,
            },
            Ok(false) => BatchItemResult {
                input: input_file.to_path_buf(),
                output: Some(output_file),
                success: false,
                error_message: Some(format!(
                    "Output file exceeds size limit (> {}MB)",
                    config.max_size_mb
                )),
            },
            Err(e) => BatchItemResult {
                input: input_file.to_path_buf(),
                output: None,
                success: false,
                error_message: Some(format!("Error: {}", e)),
            },
        }
    }

    /// Process a file with the GIF transparency tool
    fn process_gif_transparency(&self, input_file: &Path) -> BatchItemResult {
        let _config = match &self.gif_transparency_config {
            Some(config) => config,
            None => {
                return BatchItemResult {
                    input: input_file.to_path_buf(),
                    output: None,
                    success: false,
                    error_message: Some("GIF transparency configuration not set".to_string()),
                }
            }
        };

        // Run the GIF transparency tool
        match gif_transparency::make_gif_transparent(input_file) {
            Ok(()) => BatchItemResult {
                input: input_file.to_path_buf(),
                output: Some(input_file.to_path_buf()), // The output is the same file
                success: true,
                error_message: None,
            },
            Err(e) => BatchItemResult {
                input: input_file.to_path_buf(),
                output: None,
                success: false,
                error_message: Some(format!("Error: {}", e)),
            },
        }
    }

    /// Process a file with the splitter
    fn process_splitter(&self, input_file: &Path) -> BatchItemResult {
        let config = match &self.splitter_config {
            Some(config) => config,
            None => {
                return BatchItemResult {
                    input: input_file.to_path_buf(),
                    output: None,
                    success: false,
                    error_message: Some("Splitter configuration not set".to_string()),
                }
            }
        };

        // Create the output directory
        if let Err(e) = std::fs::create_dir_all(&config.output_dir) {
            return BatchItemResult {
                input: input_file.to_path_buf(),
                output: None,
                success: false,
                error_message: Some(format!("Error creating output directory: {}", e)),
            };
        }

        // Run the splitter
        match splitter::split_video(
            &input_file.to_string_lossy(),
            &config.output_dir.to_string_lossy(),
            &config.prefix,
            config.custom_encode.as_deref(),
            config.force,
        ) {
            Ok(true) => BatchItemResult {
                input: input_file.to_path_buf(),
                output: Some(config.output_dir.clone()),
                success: true,
                error_message: None,
            },
            Ok(false) => BatchItemResult {
                input: input_file.to_path_buf(),
                output: Some(config.output_dir.clone()),
                success: false,
                error_message: Some("Some slices failed to process".to_string()),
            },
            Err(e) => BatchItemResult {
                input: input_file.to_path_buf(),
                output: None,
                success: false,
                error_message: Some(format!("Error: {}", e)),
            },
        }
    }

    /// Process a file with the merger
    fn process_merger(&self, input_file: &Path) -> BatchItemResult {
        let config = match &self.merger_config {
            Some(config) => config,
            None => {
                return BatchItemResult {
                    input: input_file.to_path_buf(),
                    output: None,
                    success: false,
                    error_message: Some("Merger configuration not set".to_string()),
                }
            }
        };

        // Create output file path
        let file_stem = match input_file.file_stem() {
            Some(stem) => stem.to_string_lossy(),
            None => {
                return BatchItemResult {
                    input: input_file.to_path_buf(),
                    output: None,
                    success: false,
                    error_message: Some("Invalid input filename".to_string()),
                }
            }
        };

        let output_file = config.output_dir.join(format!("{}_merged.mp4", file_stem));

        // Create the output directory
        if let Err(e) = std::fs::create_dir_all(&config.output_dir) {
            return BatchItemResult {
                input: input_file.to_path_buf(),
                output: None,
                success: false,
                error_message: Some(format!("Error creating output directory: {}", e)),
            };
        }

        // Run the merger
        match merger::merge_audio_video(
            &input_file.to_string_lossy(),
            &config.audio_file.to_string_lossy(),
            &output_file.to_string_lossy(),
            config.use_shortest,
            config.copy_codec,
        ) {
            Ok(()) => BatchItemResult {
                input: input_file.to_path_buf(),
                output: Some(output_file),
                success: true,
                error_message: None,
            },
            Err(e) => BatchItemResult {
                input: input_file.to_path_buf(),
                output: None,
                success: false,
                error_message: Some(format!("Error: {}", e)),
            },
        }
    }
}

// Helper methods for creating common batch configurations
impl BatchProcessor {
    /// Create a clipper batch processor
    pub fn create_clipper(
        time_ranges_str: &[String],
        output_dir: &Path,
        copy_codec: bool,
        suffix: Option<&str>,
    ) -> Result<Self> {
        // Parse time ranges
        let mut time_ranges = Vec::new();
        for range_str in time_ranges_str {
            if let Some(range) = validate_time_range(range_str) {
                time_ranges.push(range);
            } else {
                return Err(BatchError::InvalidOperation(format!(
                    "Invalid time range: {}",
                    range_str
                )));
            }
        }

        if time_ranges.is_empty() {
            return Err(BatchError::InvalidOperation(
                "No valid time ranges provided".to_string(),
            ));
        }

        let config = BatchClipperConfig {
            time_ranges,
            output_dir: output_dir.to_path_buf(),
            copy_codec,
            suffix: suffix.map(String::from),
        };

        Ok(Self::new(BatchOperation::Clipper).with_clipper_config(config))
    }

    /// Create a GIF converter batch processor
    pub fn create_gif_converter(
        width: Option<u32>,
        fps: u32,
        max_size_mb: f64,
        optimize: bool,
        output_dir: &Path,
    ) -> Self {
        let config = BatchGifConverterConfig {
            width,
            fps,
            max_size_mb,
            optimize,
            output_dir: output_dir.to_path_buf(),
        };

        Self::new(BatchOperation::GifConverter).with_gif_converter_config(config)
    }

    /// Create a GIF transparency batch processor
    pub fn create_gif_transparency(create_backup: bool) -> Self {
        let config = BatchGifTransparencyConfig { create_backup };

        Self::new(BatchOperation::GifTransparency).with_gif_transparency_config(config)
    }

    /// Create a splitter batch processor
    pub fn create_splitter(
        output_dir: &Path,
        prefix: &str,
        custom_encode: Option<&str>,
        force: bool,
    ) -> Self {
        let config = BatchSplitterConfig {
            output_dir: output_dir.to_path_buf(),
            prefix: prefix.to_string(),
            custom_encode: custom_encode.map(String::from),
            force,
        };

        Self::new(BatchOperation::Splitter).with_splitter_config(config)
    }

    /// Create a merger batch processor
    pub fn create_merger(
        audio_file: &Path,
        output_dir: &Path,
        use_shortest: bool,
        copy_codec: bool,
    ) -> Self {
        let config = BatchMergerConfig {
            audio_file: audio_file.to_path_buf(),
            output_dir: output_dir.to_path_buf(),
            use_shortest,
            copy_codec,
        };

        Self::new(BatchOperation::Merger).with_merger_config(config)
    }
}
