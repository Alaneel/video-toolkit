//! Typed PDF, subtitle, document and download jobs for all desktop/CLI/MCP entry points.
mod documents;
mod downloads;
mod pdf;
mod subtitles;
pub mod support;

use anyhow::{bail, Context, Result};
pub use file_conversion::{CancelToken, DependencyStatus, JobState, Progress};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowKind {
    PdfMerge,
    PdfSplit,
    PdfRotate,
    PdfCompress,
    PdfWatermark,
    PdfMetadata,
    PdfToImages,
    PdfToText,
    ImagesToPdf,
    SubtitleConvert,
    SubtitleShift,
    SubtitleFps,
    SubtitleClean,
    DocumentConvert,
    Download,
}

impl WorkflowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PdfMerge => "pdf_merge",
            Self::PdfSplit => "pdf_split",
            Self::PdfRotate => "pdf_rotate",
            Self::PdfCompress => "pdf_compress",
            Self::PdfWatermark => "pdf_watermark",
            Self::PdfMetadata => "pdf_metadata",
            Self::PdfToImages => "pdf_to_images",
            Self::PdfToText => "pdf_to_text",
            Self::ImagesToPdf => "images_to_pdf",
            Self::SubtitleConvert => "subtitle_convert",
            Self::SubtitleShift => "subtitle_shift",
            Self::SubtitleFps => "subtitle_fps",
            Self::SubtitleClean => "subtitle_clean",
            Self::DocumentConvert => "document_convert",
            Self::Download => "download",
        }
    }
}
impl std::fmt::Display for WorkflowKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
impl FromStr for WorkflowKind {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        serde_json::from_value(Value::String(value.replace('-', "_")))
            .context("Unknown workflow; use `workflows --json` for the tool list")
    }
}

fn default_options() -> Value {
    json!({})
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRequest {
    pub kind: WorkflowKind,
    #[serde(default)]
    pub inputs: Vec<PathBuf>,
    pub output_dir: PathBuf,
    #[serde(default = "default_options")]
    pub options: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowResult {
    pub outputs: Vec<PathBuf>,
    pub summary: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowEvent {
    pub index: usize,
    pub state: JobState,
    pub progress: f32,
    pub message: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowJobResult {
    pub index: usize,
    pub state: JobState,
    pub outputs: Vec<PathBuf>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowReport {
    pub jobs: Vec<WorkflowJobResult>,
}
impl WorkflowReport {
    pub fn success(&self) -> bool {
        !self.jobs.is_empty() && self.jobs.iter().all(|j| j.state == JobState::Completed)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolSpec {
    pub kind: WorkflowKind,
    pub title: String,
    pub category: String,
    pub description: String,
    pub input_extensions: Vec<String>,
    pub output_formats: Vec<String>,
    pub options_schema: Value,
}

pub fn tools() -> Vec<ToolSpec> {
    let pdf = ["pdf"];
    let sub = ["srt", "vtt", "ass", "ssa", "sbv", "sub", "smi", "lrc"];
    let subout = [
        "srt", "vtt", "ass", "ssa", "sbv", "sub", "smi", "lrc", "txt", "csv", "json",
    ];
    let docs = [
        "txt", "md", "markdown", "html", "htm", "docx", "rtf", "odt", "epub", "pdf",
    ];
    let images = [
        "png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "heic", "heif",
    ];
    type Definition<'a> = (
        WorkflowKind,
        &'a str,
        &'a str,
        &'a str,
        &'a [&'a str],
        &'a [&'a str],
        Value,
    );
    let definitions: [Definition<'_>; 15] = [
        (
            WorkflowKind::PdfMerge,
            "Merge PDFs",
            "PDF",
            "Combine PDF files in the order you choose.",
            &pdf,
            &["pdf"],
            json!({}),
        ),
        (
            WorkflowKind::PdfSplit,
            "Split PDF",
            "PDF",
            "Extract selected pages or split a PDF into smaller files.",
            &pdf,
            &["pdf"],
            json!({"pages":{"type":"string"},"ranges":{"type":"array","items":{"type":"string"}},"chunk_size":{"type":"integer","minimum":1}}),
        ),
        (
            WorkflowKind::PdfRotate,
            "Rotate PDF",
            "PDF",
            "Rotate all pages or a selected page range.",
            &pdf,
            &["pdf"],
            json!({"pages":{"type":"string"},"angle":{"type":"integer","enum":[90,180,270],"default":90}}),
        ),
        (
            WorkflowKind::PdfCompress,
            "Compress PDF",
            "PDF",
            "Reduce PDF size with a chosen image quality preset.",
            &pdf,
            &["pdf"],
            json!({"preset":{"type":"string","enum":["screen","ebook","printer","prepress"],"default":"ebook"}}),
        ),
        (
            WorkflowKind::PdfWatermark,
            "Watermark PDF",
            "PDF",
            "Add a text watermark to selected pages.",
            &pdf,
            &["pdf"],
            json!({"pages":{"type":"string"},"text":{"type":"string"},"opacity":{"type":"number","default":0.2},"font_size":{"type":"integer","default":36},"angle":{"type":"number","default":45}}),
        ),
        (
            WorkflowKind::PdfMetadata,
            "PDF metadata",
            "PDF",
            "Edit the title and author or remove document metadata.",
            &pdf,
            &["pdf"],
            json!({"title":{"type":"string"},"author":{"type":"string"},"subject":{"type":"string"},"keywords":{"type":"string"},"strip":{"type":"boolean","default":false}}),
        ),
        (
            WorkflowKind::PdfToImages,
            "PDF to images",
            "PDF",
            "Render selected pages as PNG or JPEG images.",
            &pdf,
            &["png", "jpg"],
            json!({"pages":{"type":"string"},"format":{"type":"string","enum":["png","jpg"],"default":"png"},"dpi":{"type":"integer","minimum":36,"maximum":300,"default":150}}),
        ),
        (
            WorkflowKind::PdfToText,
            "PDF to text",
            "PDF",
            "Extract searchable text. Image-only scans require OCR elsewhere.",
            &pdf,
            &["txt"],
            json!({"pages":{"type":"string"}}),
        ),
        (
            WorkflowKind::ImagesToPdf,
            "Images to PDF",
            "PDF",
            "Turn a set of images into an ordered PDF.",
            &images,
            &["pdf"],
            json!({}),
        ),
        (
            WorkflowKind::SubtitleConvert,
            "Convert subtitles",
            "Subtitles",
            "Convert timed captions between subtitle formats.",
            &sub,
            &subout,
            subtitle_schema(),
        ),
        (
            WorkflowKind::SubtitleShift,
            "Shift subtitle timing",
            "Subtitles",
            "Move captions forward or back by a time offset.",
            &sub,
            &subout,
            subtitle_schema(),
        ),
        (
            WorkflowKind::SubtitleFps,
            "Sync subtitle FPS",
            "Subtitles",
            "Retiming for a source and target frame rate.",
            &sub,
            &subout,
            subtitle_schema(),
        ),
        (
            WorkflowKind::SubtitleClean,
            "Clean subtitles",
            "Subtitles",
            "Remove empty or repeated cues and normalize text.",
            &sub,
            &subout,
            subtitle_schema(),
        ),
        (
            WorkflowKind::DocumentConvert,
            "Convert documents",
            "Documents",
            "Convert text, Markdown, HTML, Word and EPUB documents.",
            &docs,
            &["txt", "md", "html", "docx", "pdf", "epub"],
            json!({"format":{"type":"string","enum":["txt","md","html","docx","pdf","epub"]},"encoding":{"type":"string","default":"utf-8"},"title":{"type":"string"}}),
        ),
        (
            WorkflowKind::Download,
            "Download from URL",
            "Downloads",
            "Download a direct file or extract audio/video from a media link.",
            &[],
            &["original", "mp4", "mp3", "m4a", "webm"],
            json!({"url":{"type":"string"},"mode":{"type":"string","enum":["direct","media"],"default":"direct"},"filename":{"type":"string"},"format":{"type":"string","enum":["original","mp4","mp3","m4a","webm"],"default":"original"},"max_mb":{"type":"integer","default":2048}}),
        ),
    ];
    definitions.into_iter().map(|(kind,title,category,description,input,output,properties)| {
        let required:Vec<&str>=match kind {
            WorkflowKind::PdfWatermark=>vec!["text"],WorkflowKind::SubtitleConvert|WorkflowKind::DocumentConvert=>vec!["format"],
            WorkflowKind::SubtitleShift=>vec!["offset_ms"],WorkflowKind::SubtitleFps=>vec!["source_fps","target_fps"],
            WorkflowKind::Download=>vec!["url"],_=>vec![],
        };
        ToolSpec {
        kind,title:title.into(),category:category.into(),description:description.into(),input_extensions:input.iter().map(|s|(*s).into()).collect(),
        output_formats:output.iter().map(|s|(*s).into()).collect(),options_schema:json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
    }}).collect()
}
fn subtitle_schema() -> Value {
    json!({"format":{"type":"string","enum":["srt","vtt","ass","ssa","sbv","sub","smi","lrc","txt","csv","json"]},"encoding":{"type":"string","default":"auto"},"offset_ms":{"type":"integer","default":0},"source_fps":{"type":"number"},"target_fps":{"type":"number"},"clean":{"oneOf":[{"type":"boolean"},{"type":"object"}]}})
}

pub fn dependencies() -> Vec<DependencyStatus> {
    ["python3", "pandoc", "gs", "soffice", "yt-dlp", "curl"]
        .into_iter()
        .map(|name| {
            let path = if name == "python3" {
                support::find_python().ok()
            } else {
                support::find_tool(name)
            };
            DependencyStatus {
                name: name.into(),
                available: path.is_some(),
                path,
            }
        })
        .collect()
}

pub fn execute(
    request: &WorkflowRequest,
    cancel: &CancelToken,
    mut progress: impl FnMut(Progress),
) -> Result<WorkflowResult> {
    support::check_cancelled(cancel)?;
    if !request.options.is_object() {
        bail!("Workflow options must be a JSON object");
    }
    if request.inputs.len() > 1000 {
        bail!("Choose at most 1,000 input files");
    }
    let mut inputs = Vec::new();
    for path in &request.inputs {
        let path = path
            .canonicalize()
            .with_context(|| format!("Cannot open input: {}", path.display()))?;
        if !path.is_file() {
            bail!("Input must be a regular file: {}", path.display());
        }
        inputs.push(path);
    }
    if request.kind == WorkflowKind::Download {
        if !inputs.is_empty() {
            bail!("A URL download does not accept local input files");
        }
    } else if inputs.is_empty() {
        bail!("Choose input files before running this tool");
    }
    let mut stems = HashSet::new();
    if matches!(
        request.kind,
        WorkflowKind::DocumentConvert
            | WorkflowKind::SubtitleConvert
            | WorkflowKind::SubtitleShift
            | WorkflowKind::SubtitleFps
            | WorkflowKind::SubtitleClean
    ) {
        for path in &inputs {
            if !stems.insert(
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase(),
            ) {
                bail!("Input filenames would collide. Rename files or use separate jobs.");
            }
        }
    }
    let output_dir = if request.output_dir.is_absolute() {
        request.output_dir.clone()
    } else {
        std::env::current_dir()?.join(&request.output_dir)
    };
    std::fs::create_dir_all(&output_dir).context("Cannot create output directory")?;
    let output_dir = output_dir.canonicalize()?;
    let staging = tempfile::Builder::new()
        .prefix(".video-toolkit-workflow-")
        .tempdir_in(&output_dir)?;
    progress(Progress {
        fraction: 0.02,
        message: format!("Running {}", request.kind),
    });
    let mut inner_progress = |event: Progress| {
        progress(Progress {
            fraction: 0.04 + event.fraction.clamp(0.0, 1.0) * 0.86,
            message: event.message,
        })
    };
    let generated = match request.kind {
        WorkflowKind::PdfMerge
        | WorkflowKind::PdfSplit
        | WorkflowKind::PdfRotate
        | WorkflowKind::PdfCompress
        | WorkflowKind::PdfWatermark
        | WorkflowKind::PdfMetadata
        | WorkflowKind::PdfToImages
        | WorkflowKind::PdfToText
        | WorkflowKind::ImagesToPdf => pdf::run(
            request.kind,
            &inputs,
            staging.path(),
            &request.options,
            cancel,
            &mut inner_progress,
        )?,
        WorkflowKind::SubtitleConvert
        | WorkflowKind::SubtitleShift
        | WorkflowKind::SubtitleFps
        | WorkflowKind::SubtitleClean => subtitles::run(
            request.kind,
            &inputs,
            staging.path(),
            &request.options,
            cancel,
            &mut inner_progress,
        )?,
        WorkflowKind::DocumentConvert => documents::run(
            request.kind,
            &inputs,
            staging.path(),
            &request.options,
            cancel,
            &mut inner_progress,
        )?,
        WorkflowKind::Download => downloads::run(
            &inputs,
            staging.path(),
            &request.options,
            cancel,
            &mut inner_progress,
        )?,
    };
    support::check_cancelled(cancel)?;
    progress(Progress {
        fraction: 0.94,
        message: "Validating outputs".into(),
    });
    if generated.is_empty() {
        bail!("The tool produced no output files");
    }
    let stage = staging.path().canonicalize()?;
    let mut names = HashSet::new();
    let mut destinations = Vec::new();
    for path in &generated {
        if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
            bail!("Tool returned a symbolic link instead of an output file");
        }
        let path = path.canonicalize()?;
        if path.parent() != Some(stage.as_path()) || !path.is_file() || path.metadata()?.len() == 0
        {
            bail!(
                "Tool produced an invalid, empty or external output: {}",
                path.display()
            );
        }
        let name = path.file_name().context("Output has no filename")?;
        if !names.insert(name.to_string_lossy().to_lowercase()) {
            bail!("Tool produced duplicate output filenames");
        }
        let destination = output_dir.join(name);
        if std::fs::symlink_metadata(&destination).is_ok() {
            bail!("Output already exists: {}", destination.display());
        }
        destinations.push((path, destination));
    }
    let mut published = Vec::new();
    let result = (|| -> Result<()> {
        for (source, destination) in &destinations {
            support::check_cancelled(cancel)?;
            let mut temp = tempfile::Builder::new()
                .prefix(".video-toolkit-publish-")
                .tempfile_in(&output_dir)?;
            support::copy_cancelled(source, temp.as_file_mut(), cancel)?;
            temp.as_file().sync_all()?;
            support::check_cancelled(cancel)?;
            temp.persist_noclobber(destination).map_err(|e| {
                anyhow::anyhow!(
                    "Cannot save without overwriting {}: {e}",
                    destination.display()
                )
            })?;
            published.push(destination.clone());
        }
        Ok(())
    })();
    if let Err(error) = result {
        for path in &published {
            let _ = std::fs::remove_file(path);
        }
        return Err(error);
    }
    progress(Progress {
        fraction: 1.0,
        message: "Complete · outputs verified".into(),
    });
    Ok(WorkflowResult {
        summary: format!("{} verified output file(s)", published.len()),
        outputs: published,
    })
}

pub fn run_queue(
    requests: &[WorkflowRequest],
    cancel: &CancelToken,
    mut on_event: impl FnMut(WorkflowEvent),
) -> WorkflowReport {
    for index in 0..requests.len() {
        on_event(WorkflowEvent {
            index,
            state: JobState::Queued,
            progress: 0.0,
            message: "Queued".into(),
        });
    }
    let mut jobs = Vec::new();
    for (index, request) in requests.iter().enumerate() {
        let mut fraction = 0.0f32;
        let result = execute(request, cancel, |event| {
            if event.fraction >= 1.0 {
                return;
            }
            fraction = fraction.max(event.fraction.clamp(0.0, 1.0));
            let state = if event.message.starts_with("Validating") {
                JobState::Validating
            } else {
                JobState::Running
            };
            on_event(WorkflowEvent {
                index,
                state,
                progress: fraction,
                message: event.message,
            });
        });
        let (state, outputs, error) = match result {
            Ok(result) => (JobState::Completed, result.outputs, None),
            Err(error) => (
                if cancel.is_cancelled() {
                    JobState::Cancelled
                } else {
                    JobState::Failed
                },
                vec![],
                Some(format!("{error:#}")),
            ),
        };
        on_event(WorkflowEvent {
            index,
            state,
            progress: if state == JobState::Completed {
                1.0
            } else {
                fraction
            },
            message: error
                .clone()
                .unwrap_or_else(|| "Complete · outputs verified".into()),
        });
        jobs.push(WorkflowJobResult {
            index,
            state,
            outputs,
            error,
        });
    }
    WorkflowReport { jobs }
}

pub fn valid_filename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 220
        && Path::new(value).file_name().and_then(|n| n.to_str()) == Some(value)
        && !matches!(value, "." | "..")
        && !value
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unsafe_names_and_unknown_workflows() {
        for name in ["../a", "/a", "a/b", "a\\b", "..", "", "a\nb"] {
            assert!(!valid_filename(name));
        }
        assert!(valid_filename("my file 中文.pdf"));
        assert!("pdf_rotate".parse::<WorkflowKind>().is_ok());
        assert!("shell".parse::<WorkflowKind>().is_err());
    }
    #[test]
    fn pre_cancelled_job_does_not_create_output() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("out");
        let cancel = CancelToken::default();
        cancel.cancel();
        let report = run_queue(
            &[WorkflowRequest {
                kind: WorkflowKind::Download,
                inputs: vec![],
                output_dir: output.clone(),
                options: json!({"url":"https://example.com/a"}),
            }],
            &cancel,
            |_| {},
        );
        assert_eq!(report.jobs[0].state, JobState::Cancelled);
        assert!(!output.exists());
    }
}
