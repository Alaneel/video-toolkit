use thiserror::Error;

#[derive(Error, Debug)]
pub enum VideoToolkitError {
    #[error(transparent)]
    InvalidFormat(#[from] crate::formats::FormatError),

    #[error("FFmpeg not found. Please install FFmpeg and make sure it's in your PATH.")]
    FFmpegNotFound,

    #[error("FFmpeg command failed: {0}")]
    FFmpegCommandFailed(String),

    #[error("Input file '{0}' not found")]
    InputFileNotFound(String),

    #[error("Output file was not created")]
    OutputFileNotCreated,

    #[error("Invalid timestamp format: {0}")]
    InvalidTimestamp(String),

    #[error("Invalid time range format: {0}")]
    InvalidTimeRange(String),

    #[error("Video dimensions are {0}x{1}, expected 1920x1080")]
    InvalidDimensions(u32, u32),

    #[error("Could not determine video dimensions")]
    DimensionsError,

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Error: {0}")]
    Other(String),
}

// Type alias for Result with our custom error type
pub type Result<T> = std::result::Result<T, VideoToolkitError>;
