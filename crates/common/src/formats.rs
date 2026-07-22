use std::fmt;
use std::path::Path;
use std::str::FromStr;

use thiserror::Error;

/// Error types for format operations
#[derive(Error, Debug)]
pub enum FormatError {
    #[error("Unknown format: {0}")]
    UnknownFormat(String),

    #[error("Unsupported format: {0}")]
    UnsupportedFormat(String),

    #[error("Invalid format for operation: {0}")]
    InvalidFormatForOperation(String),
}

/// Video container formats supported by the toolkit
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoFormat {
    MP4,
    MKV,
    AVI,
    MOV,
    WebM,
    FLV,
    WMV,
    TS,
    M4V,
    MPEG,
    VOB,
    OGV,
}

impl VideoFormat {
    /// Get the file extension for this format
    pub fn extension(&self) -> &str {
        match self {
            VideoFormat::MP4 => "mp4",
            VideoFormat::MKV => "mkv",
            VideoFormat::AVI => "avi",
            VideoFormat::MOV => "mov",
            VideoFormat::WebM => "webm",
            VideoFormat::FLV => "flv",
            VideoFormat::WMV => "wmv",
            VideoFormat::TS => "ts",
            VideoFormat::M4V => "m4v",
            VideoFormat::MPEG => "mpeg",
            VideoFormat::VOB => "vob",
            VideoFormat::OGV => "ogv",
        }
    }

    /// Check if the format is a web-friendly format
    pub fn is_web_friendly(&self) -> bool {
        matches!(
            self,
            VideoFormat::MP4 | VideoFormat::WebM | VideoFormat::OGV
        )
    }

    /// Check if the format supports transparency
    pub fn supports_transparency(&self) -> bool {
        matches!(self, VideoFormat::WebM)
    }

    /// Get the MIME type for this format
    pub fn mime_type(&self) -> &str {
        match self {
            VideoFormat::MP4 => "video/mp4",
            VideoFormat::MKV => "video/x-matroska",
            VideoFormat::AVI => "video/x-msvideo",
            VideoFormat::MOV => "video/quicktime",
            VideoFormat::WebM => "video/webm",
            VideoFormat::FLV => "video/x-flv",
            VideoFormat::WMV => "video/x-ms-wmv",
            VideoFormat::TS => "video/mp2t",
            VideoFormat::M4V => "video/x-m4v",
            VideoFormat::MPEG => "video/mpeg",
            VideoFormat::VOB => "video/x-vob",
            VideoFormat::OGV => "video/ogg",
        }
    }

    /// Get all supported video formats
    pub fn all() -> Vec<VideoFormat> {
        vec![
            VideoFormat::MP4,
            VideoFormat::MKV,
            VideoFormat::AVI,
            VideoFormat::MOV,
            VideoFormat::WebM,
            VideoFormat::FLV,
            VideoFormat::WMV,
            VideoFormat::TS,
            VideoFormat::M4V,
            VideoFormat::MPEG,
            VideoFormat::VOB,
            VideoFormat::OGV,
        ]
    }
}

impl fmt::Display for VideoFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.extension().to_uppercase())
    }
}

impl FromStr for VideoFormat {
    type Err = FormatError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "mp4" => Ok(VideoFormat::MP4),
            "mkv" => Ok(VideoFormat::MKV),
            "avi" => Ok(VideoFormat::AVI),
            "mov" => Ok(VideoFormat::MOV),
            "webm" => Ok(VideoFormat::WebM),
            "flv" => Ok(VideoFormat::FLV),
            "wmv" => Ok(VideoFormat::WMV),
            "ts" => Ok(VideoFormat::TS),
            "m4v" => Ok(VideoFormat::M4V),
            "mpeg" | "mpg" => Ok(VideoFormat::MPEG),
            "vob" => Ok(VideoFormat::VOB),
            "ogv" => Ok(VideoFormat::OGV),
            _ => Err(FormatError::UnknownFormat(s.to_string())),
        }
    }
}

/// Audio container formats supported by the toolkit
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioFormat {
    MP3,
    AAC,
    WAV,
    FLAC,
    OGG,
    M4A,
    WMA,
    AIFF,
}

impl AudioFormat {
    /// Get the file extension for this format
    pub fn extension(&self) -> &str {
        match self {
            AudioFormat::MP3 => "mp3",
            AudioFormat::AAC => "aac",
            AudioFormat::WAV => "wav",
            AudioFormat::FLAC => "flac",
            AudioFormat::OGG => "ogg",
            AudioFormat::M4A => "m4a",
            AudioFormat::WMA => "wma",
            AudioFormat::AIFF => "aiff",
        }
    }

    /// Check if the format is a web-friendly format
    pub fn is_web_friendly(&self) -> bool {
        matches!(self, AudioFormat::MP3 | AudioFormat::AAC | AudioFormat::OGG)
    }

    /// Get the MIME type for this format
    pub fn mime_type(&self) -> &str {
        match self {
            AudioFormat::MP3 => "audio/mpeg",
            AudioFormat::AAC => "audio/aac",
            AudioFormat::WAV => "audio/wav",
            AudioFormat::FLAC => "audio/flac",
            AudioFormat::OGG => "audio/ogg",
            AudioFormat::M4A => "audio/mp4",
            AudioFormat::WMA => "audio/x-ms-wma",
            AudioFormat::AIFF => "audio/aiff",
        }
    }

    /// Get all supported audio formats
    pub fn all() -> Vec<AudioFormat> {
        vec![
            AudioFormat::MP3,
            AudioFormat::AAC,
            AudioFormat::WAV,
            AudioFormat::FLAC,
            AudioFormat::OGG,
            AudioFormat::M4A,
            AudioFormat::WMA,
            AudioFormat::AIFF,
        ]
    }
}

impl fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.extension().to_uppercase())
    }
}

impl FromStr for AudioFormat {
    type Err = FormatError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "mp3" => Ok(AudioFormat::MP3),
            "aac" => Ok(AudioFormat::AAC),
            "wav" => Ok(AudioFormat::WAV),
            "flac" => Ok(AudioFormat::FLAC),
            "ogg" => Ok(AudioFormat::OGG),
            "m4a" => Ok(AudioFormat::M4A),
            "wma" => Ok(AudioFormat::WMA),
            "aiff" | "aif" => Ok(AudioFormat::AIFF),
            _ => Err(FormatError::UnknownFormat(s.to_string())),
        }
    }
}

/// Image formats supported by the toolkit
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    GIF,
    PNG,
    JPG,
    WEBP,
    BMP,
    TIFF,
}

impl ImageFormat {
    /// Get the file extension for this format
    pub fn extension(&self) -> &str {
        match self {
            ImageFormat::GIF => "gif",
            ImageFormat::PNG => "png",
            ImageFormat::JPG => "jpg",
            ImageFormat::WEBP => "webp",
            ImageFormat::BMP => "bmp",
            ImageFormat::TIFF => "tiff",
        }
    }

    /// Check if the format supports transparency
    pub fn supports_transparency(&self) -> bool {
        matches!(
            self,
            ImageFormat::GIF | ImageFormat::PNG | ImageFormat::WEBP
        )
    }

    /// Check if the format supports animation
    pub fn supports_animation(&self) -> bool {
        matches!(self, ImageFormat::GIF | ImageFormat::WEBP)
    }

    /// Get the MIME type for this format
    pub fn mime_type(&self) -> &str {
        match self {
            ImageFormat::GIF => "image/gif",
            ImageFormat::PNG => "image/png",
            ImageFormat::JPG => "image/jpeg",
            ImageFormat::WEBP => "image/webp",
            ImageFormat::BMP => "image/bmp",
            ImageFormat::TIFF => "image/tiff",
        }
    }

    /// Get all supported image formats
    pub fn all() -> Vec<ImageFormat> {
        vec![
            ImageFormat::GIF,
            ImageFormat::PNG,
            ImageFormat::JPG,
            ImageFormat::WEBP,
            ImageFormat::BMP,
            ImageFormat::TIFF,
        ]
    }
}

impl fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.extension().to_uppercase())
    }
}

impl FromStr for ImageFormat {
    type Err = FormatError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "gif" => Ok(ImageFormat::GIF),
            "png" => Ok(ImageFormat::PNG),
            "jpg" | "jpeg" => Ok(ImageFormat::JPG),
            "webp" => Ok(ImageFormat::WEBP),
            "bmp" => Ok(ImageFormat::BMP),
            "tif" | "tiff" => Ok(ImageFormat::TIFF),
            _ => Err(FormatError::UnknownFormat(s.to_string())),
        }
    }
}

/// Detect the format of a file based on its extension
pub fn detect_format(path: &Path) -> Option<FormatType> {
    let extension = path.extension().and_then(|e| e.to_str())?.to_lowercase();

    // Check video formats
    if let Ok(format) = VideoFormat::from_str(&extension) {
        return Some(FormatType::Video(format));
    }

    // Check audio formats
    if let Ok(format) = AudioFormat::from_str(&extension) {
        return Some(FormatType::Audio(format));
    }

    // Check image formats
    if let Ok(format) = ImageFormat::from_str(&extension) {
        return Some(FormatType::Image(format));
    }

    None
}

/// Enum representing all supported format types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatType {
    Video(VideoFormat),
    Audio(AudioFormat),
    Image(ImageFormat),
}

impl FormatType {
    /// Get the file extension for this format
    pub fn extension(&self) -> &str {
        match self {
            FormatType::Video(format) => format.extension(),
            FormatType::Audio(format) => format.extension(),
            FormatType::Image(format) => format.extension(),
        }
    }

    /// Get the MIME type for this format
    pub fn mime_type(&self) -> &str {
        match self {
            FormatType::Video(format) => format.mime_type(),
            FormatType::Audio(format) => format.mime_type(),
            FormatType::Image(format) => format.mime_type(),
        }
    }
}

impl fmt::Display for FormatType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatType::Video(format) => write!(f, "Video ({})", format),
            FormatType::Audio(format) => write!(f, "Audio ({})", format),
            FormatType::Image(format) => write!(f, "Image ({})", format),
        }
    }
}
