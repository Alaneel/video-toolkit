use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use regex::Regex;
use thiserror::Error;
use walkdir::WalkDir;

use common::{
    check_ffmpeg, validate_time_range, verify_input_file, VideoFormat, VideoToolkitError,
};

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
    output_format: VideoFormat,
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
            output_format: VideoFormat::MP4,
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

    /// Select the output container for clip, split, and merge operations.
    pub fn with_output_format(mut self, format: VideoFormat) -> Self {
        self.output_format = format;
        self
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

        // Explicit files and directory traversal can discover the same source
        // more than once. Keep its first path for naming, but process it once.
        let mut canonical_sources = HashSet::new();
        let mut unique_files = Vec::new();
        for file in files {
            if canonical_sources.insert(std::fs::canonicalize(&file)?) {
                unique_files.push(file);
            }
        }

        Ok(unique_files)
    }

    /// Reject sources that would write the same filenames into a shared output directory.
    fn validate_output_collisions(&self, input_files: &[PathBuf]) -> Result<()> {
        if self.operation == BatchOperation::GifTransparency {
            return Ok(());
        }

        // Default macOS and Windows filesystems are case insensitive. Reject
        // these collisions on every platform so a batch stays portable.
        let mut output_stems: HashMap<String, &Path> = HashMap::new();
        for input in input_files {
            let stem = input
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            if let Some(previous) = output_stems.insert(stem.clone(), input) {
                return Err(BatchError::InvalidOperation(format!(
                    "Output filename collision between '{}' and '{}' (same filename stem '{}'). Rename one input or process them into separate output directories.",
                    previous.display(), input.display(), stem
                )));
            }
        }

        // Clip names also contain sanitized timestamps. Different source stems
        // can therefore overlap across ranges, and repeated ranges can collide
        // within a single source. Compare the exact names the clipper will use.
        if self.operation == BatchOperation::Clipper {
            if let Some(config) = &self.clipper_config {
                if config.time_ranges.is_empty() {
                    return Err(BatchError::InvalidOperation(
                        "No valid time ranges provided".to_string(),
                    ));
                }

                let ranges: Vec<(String, String)> = config
                    .time_ranges
                    .iter()
                    .map(|(start, end)| {
                        let range = format!("{start}-{end}");
                        validate_time_range(&range).ok_or_else(|| {
                            BatchError::InvalidOperation(format!("Invalid time range: {range}"))
                        })
                    })
                    .collect::<Result<_>>()?;
                let mut clip_outputs: HashMap<String, &Path> = HashMap::new();
                for input in input_files {
                    for (start, end) in &ranges {
                        let output = clipper::format_output_filename_with_format(
                            input,
                            start,
                            end,
                            &config.output_dir,
                            config.suffix.as_deref(),
                            self.output_format,
                        );
                        let key = output.to_string_lossy().to_lowercase();
                        if let Some(previous) = clip_outputs.insert(key, input) {
                            return Err(BatchError::InvalidOperation(format!(
                                "Output filename collision for '{}' between '{}' and '{}'. Rename inputs, remove overlapping or duplicate ranges, or process them into separate output directories.",
                                output.display(), previous.display(), input.display()
                            )));
                        }
                    }
                }
            }
        }

        Ok(())
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
                BatchOperation::Clipper
                | BatchOperation::Splitter
                | BatchOperation::Merger
                | BatchOperation::GifConverter => ext.parse::<VideoFormat>().is_ok(),
                BatchOperation::GifTransparency => ext.eq_ignore_ascii_case("gif"),
            }
        } else {
            false
        }
    }

    /// Process the batch operation on the input files
    pub fn process(&self, input_paths: &[PathBuf]) -> Result<Vec<BatchItemResult>> {
        // Discover and validate every destination before starting a media job.
        let input_files = self.find_input_files(input_paths)?;
        self.validate_output_collisions(&input_files)?;

        // Check if FFmpeg is installed
        if !check_ffmpeg() {
            return Err(BatchError::Other("FFmpeg not found".to_string()));
        }

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
        match clipper::clip_video_with_format(
            &input_file.to_string_lossy(),
            &config.time_ranges,
            &config.output_dir.to_string_lossy(),
            config.copy_codec,
            config.suffix.as_deref(),
            self.output_format,
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
        // Each source needs its own prefix so parallel jobs do not overwrite
        // the same five slices in the shared output directory.
        let source_stem = input_file.file_stem().unwrap_or_default().to_string_lossy();
        let output_prefix = format!("{}_{}", source_stem, config.prefix);
        match splitter::split_video_with_format(
            &input_file.to_string_lossy(),
            &config.output_dir.to_string_lossy(),
            &output_prefix,
            config.custom_encode.as_deref(),
            config.force,
            self.output_format,
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

        let output_file = config.output_dir.join(format!(
            "{}_merged.{}",
            file_stem,
            self.output_format.extension()
        ));

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
        match merger::merge_audio_video_with_format(
            &input_file.to_string_lossy(),
            &config.audio_file.to_string_lossy(),
            &output_file.to_string_lossy(),
            config.use_shortest,
            config.copy_codec,
            self.output_format,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "video-toolkit-batch-discovery-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"discovery fixture; no FFmpeg required").unwrap();
            path
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const VIDEO_OPERATIONS: [BatchOperation; 4] = [
        BatchOperation::Clipper,
        BatchOperation::Splitter,
        BatchOperation::Merger,
        BatchOperation::GifConverter,
    ];

    #[test]
    fn discovery_accepts_all_video_formats_uppercase_extensions_and_aliases() {
        let directory = TestDirectory::new();
        let mut expected: Vec<_> = VideoFormat::all()
            .iter()
            .map(|format| {
                directory.file(&format!(
                    "video-{}.{}",
                    format.extension(),
                    format.extension()
                ))
            })
            .collect();
        expected.push(directory.file("nested/uppercase.WEBM"));
        expected.push(directory.file("legacy-alias.mpg"));
        directory.file("not-a-video.txt");
        directory.file("not-a-video.gif");
        expected.sort();

        for operation in VIDEO_OPERATIONS {
            let processor = BatchProcessor::new(operation).with_recursive(true);
            let mut inputs = processor
                .find_input_files(std::slice::from_ref(&directory.0))
                .unwrap();
            inputs.sort();
            assert_eq!(inputs, expected, "incorrect discovery for {operation}");
            processor.validate_output_collisions(&inputs).unwrap();
        }
    }

    #[test]
    fn overlapping_inputs_and_canonical_aliases_are_processed_once() {
        let directory = TestDirectory::new();
        let input = directory.file("video.webm");
        std::fs::create_dir(directory.0.join("aliases")).unwrap();
        let alias = directory.0.join("aliases").join("..").join("video.webm");

        for operation in VIDEO_OPERATIONS {
            let processor = BatchProcessor::new(operation).with_recursive(true);
            let files = processor
                .find_input_files(&[input.clone(), directory.0.clone(), alias.clone()])
                .unwrap();
            assert_eq!(files, vec![input.clone()]);
            processor.validate_output_collisions(&files).unwrap();
        }
    }

    #[test]
    fn preflight_rejects_case_insensitive_stem_collisions_before_output_creation() {
        let directory = TestDirectory::new();
        let first = directory.file("left/Recording.mp4");
        let second = directory.file("right/recording.mkv");
        let output = directory.0.join("outputs-not-created");

        for operation in VIDEO_OPERATIONS {
            let processor = match operation {
                BatchOperation::Clipper => {
                    BatchProcessor::create_clipper(&["0-1".to_string()], &output, false, None)
                        .unwrap()
                }
                BatchOperation::Splitter => {
                    BatchProcessor::create_splitter(&output, "slice", None, false)
                }
                BatchOperation::Merger => {
                    BatchProcessor::create_merger(&first, &output, true, false)
                }
                BatchOperation::GifConverter => {
                    BatchProcessor::create_gif_converter(None, 10, 5.0, false, &output)
                }
                BatchOperation::GifTransparency => unreachable!(),
            };
            let files = processor
                .find_input_files(&[first.clone(), second.clone()])
                .unwrap();
            // Public process must stop at preflight without even requiring
            // FFmpeg, and without creating any configured output directory.
            let error = processor.process(&files).unwrap_err();
            assert!(matches!(error, BatchError::InvalidOperation(_)));
            let message = error.to_string();
            assert!(message.contains(&first.display().to_string()));
            assert!(message.contains(&second.display().to_string()));
            assert!(message.contains("Rename one input"));
            assert!(message.contains("separate output directories"));
            assert!(!output.exists());
        }

        assert_eq!(
            std::fs::read(first).unwrap(),
            b"discovery fixture; no FFmpeg required"
        );
        assert_eq!(
            std::fs::read(second).unwrap(),
            b"discovery fixture; no FFmpeg required"
        );
    }

    #[test]
    fn transparency_deduplicates_sources_but_allows_matching_stems_in_other_directories() {
        let directory = TestDirectory::new();
        let first = directory.file("left/recording.gif");
        let second = directory.file("right/recording.gif");
        let processor = BatchProcessor::new(BatchOperation::GifTransparency).with_recursive(true);
        let files = processor
            .find_input_files(&[first.clone(), directory.0.clone()])
            .unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.contains(&first));
        assert!(files.contains(&second));
        processor.validate_output_collisions(&files).unwrap();
    }

    #[test]
    fn clip_preflight_rejects_cross_range_collisions_between_different_stems() {
        let directory = TestDirectory::new();
        let first = directory.file("lesson_01.mp4");
        let second = directory.file("lesson.mp4");
        let output = directory.0.join("clips-not-created");
        let processor = BatchProcessor::create_clipper(
            &["02-01:03".to_string(), "01:02-01:03".to_string()],
            &output,
            false,
            Some("highlight"),
        )
        .unwrap()
        .with_output_format(VideoFormat::WebM);

        let error = processor
            .process(&[first.clone(), second.clone()])
            .unwrap_err();
        assert!(matches!(error, BatchError::InvalidOperation(_)));
        let message = error.to_string();
        assert!(message.contains("lesson_01_02-01_03_highlight.webm"));
        assert!(message.contains(&first.display().to_string()));
        assert!(message.contains(&second.display().to_string()));
        assert!(!output.exists());
    }

    #[test]
    fn clip_preflight_rejects_duplicate_ranges_after_timestamp_trimming() {
        let directory = TestDirectory::new();
        let input = directory.file("lesson.mp4");
        let output = directory.0.join("clips-not-created");
        let processor =
            BatchProcessor::new(BatchOperation::Clipper).with_clipper_config(BatchClipperConfig {
                time_ranges: vec![
                    ("0".to_string(), "1".to_string()),
                    (" 0 ".to_string(), " 1 ".to_string()),
                ],
                output_dir: output.clone(),
                copy_codec: false,
                suffix: None,
            });

        let error = processor.process(std::slice::from_ref(&input)).unwrap_err();
        assert!(matches!(error, BatchError::InvalidOperation(_)));
        let message = error.to_string();
        assert!(message.contains("lesson_0-1.mp4"));
        assert!(message.contains(&input.display().to_string()));
        assert!(message.contains("duplicate ranges"));
        assert!(!output.exists());
    }
}
