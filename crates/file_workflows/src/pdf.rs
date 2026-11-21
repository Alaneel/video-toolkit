//! PDF workflows use an isolated Python worker; publication is owned by the queue.
use crate::{support, WorkflowKind};
use anyhow::{bail, Result};
use file_conversion::{CancelToken, Progress};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) fn run(
    kind: WorkflowKind,
    inputs: &[PathBuf],
    staging: &Path,
    options: &Value,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(Progress),
) -> Result<Vec<PathBuf>> {
    if !matches!(
        kind,
        WorkflowKind::PdfMerge
            | WorkflowKind::PdfSplit
            | WorkflowKind::PdfRotate
            | WorkflowKind::PdfCompress
            | WorkflowKind::PdfWatermark
            | WorkflowKind::PdfMetadata
            | WorkflowKind::PdfToImages
            | WorkflowKind::PdfToText
            | WorkflowKind::ImagesToPdf
    ) {
        bail!("This operation is not a PDF workflow");
    }
    support::run_python(
        include_str!("../python/pdf_worker.py"),
        kind,
        inputs,
        staging,
        options,
        cancel,
        progress,
    )
}
