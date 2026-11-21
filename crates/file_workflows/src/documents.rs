//! Document conversion is independent of PDF operations and media encoding.
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
    if !matches!(kind, WorkflowKind::DocumentConvert) {
        bail!("This operation is not a document conversion");
    }
    support::run_python(
        include_str!("../python/document_worker.py"),
        kind,
        inputs,
        staging,
        options,
        cancel,
        progress,
    )
}
