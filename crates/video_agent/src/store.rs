//! Process-safe task state, separate from the legacy toolkit's configuration.
use crate::domain::*;
use chrono::Utc;
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) fn now() -> String {
    Utc::now().to_rfc3339()
}
pub(crate) fn invalid(message: impl Into<String>) -> AgentError {
    AgentError::Invalid(message.into())
}

pub fn validate_limits(l: &Limits) -> Result<()> {
    if !(1..=4).contains(&l.max_concurrency) {
        return Err(invalid(
            "Local frame extraction concurrency must be between 1 and 4",
        ));
    }
    if l.initial_frames == 0
        || l.initial_frames > l.max_frames
        || l.max_frames > 240
        || l.max_model_calls == 0
        || l.max_model_calls > 20
        || l.max_output_tokens < 128
        || l.max_output_tokens > 16000
        || l.max_total_tokens < u64::from(l.max_output_tokens)
        || l.max_total_tokens > 2_000_000
        || !l.max_estimated_cost_usd.is_finite()
        || l.max_estimated_cost_usd <= 0.0
        || l.max_estimated_cost_usd > 100.0
        || l.max_execution_secs == 0
        || l.max_execution_secs > 3600
        || l.process_timeout_secs == 0
        || l.process_timeout_secs > l.max_execution_secs
        || l.model_timeout_secs == 0
        || l.model_timeout_secs > l.max_execution_secs
        || l.max_sources == 0
        || l.max_sources > 8
        || l.max_clip_ms < 100
        || l.max_clip_ms > 120_000
        || l.max_output_ms < 100
        || l.max_output_ms > 600_000
    {
        return Err(invalid(
            "Invalid resource limits (frames, calls, tokens, cost, concurrency, or execution time)",
        ));
    }
    Ok(())
}

pub(crate) fn regular(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid("Expected a regular, non-symlink file"));
    }
    Ok(())
}

pub(crate) fn owned_dir(root: &Path, name: &str) -> Result<PathBuf> {
    let path = root.join(name);
    match fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            return Err(invalid("Managed task directory was replaced"))
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&path)?,
        Err(e) => return Err(e.into()),
    }
    if fs::canonicalize(&path)?.parent() != Some(root) {
        return Err(invalid("Managed path escaped task directory"));
    }
    Ok(path)
}

pub(crate) struct Guard {
    file: File,
}
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub(crate) fn lock(root: &Path) -> Result<Guard> {
    let path = root.join("task.lock");
    match fs::symlink_metadata(&path) {
        Ok(_) => regular(&path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    file.try_lock_exclusive().map_err(|_| {
        invalid("This task is busy in another operation or process; wait before retrying")
    })?;
    Ok(Guard { file })
}

pub(crate) fn read(root: &Path) -> Result<Task> {
    let path = root.join("task.json");
    regular(&path)?;
    if path.metadata()?.len() > 16 * 1024 * 1024 {
        return Err(invalid("Task state exceeds size limit"));
    }
    let mut task: Task = serde_json::from_reader(File::open(path)?)?;
    if task.schema_version != 1 {
        return Err(invalid("Unsupported task schema version"));
    }
    validate_limits(&task.limits)?;
    if task.assets.is_empty()
        || task.assets.len() > task.limits.max_sources
        || task.samples.len() > task.limits.max_frames as usize
    {
        return Err(invalid("Task state exceeds configured input/frame limits"));
    }
    if task.target_duration_ms < 100
        || task.target_duration_ms > task.limits.max_output_ms
        || task.usage.frames_created > task.limits.max_frames
        || task.usage.frames_created < task.samples.len() as u32
        || task.usage.model_calls > task.limits.max_model_calls
        || task.usage.model_calls as usize != task.model_calls.len()
        || !task.usage.estimated_cost_usd.is_finite()
        || task.usage.estimated_cost_usd < 0.0
        || task.usage.conservative_elapsed_ms > task.usage.elapsed_ms
        || task.usage.execution_reserved_ms > task.limits.max_execution_secs * 1000
        || task.usage.execution_accounting_version > 1
        || task.usage.peak_media_workers as usize > task.limits.max_concurrency
    {
        return Err(invalid("Invalid persisted task budgets or counters"));
    }
    for (index, asset) in task.assets.iter().enumerate() {
        if asset.id != format!("source-{}", index + 1)
            || asset.sha256.len() != 64
            || !asset.sha256.bytes().all(|c| c.is_ascii_hexdigit())
            || asset.duration_ms == 0
            || asset.width == 0
            || asset.height == 0
            || !asset.path.is_absolute()
        {
            return Err(invalid("Invalid persisted source identity"));
        }
    }
    let mut sample_ids = std::collections::HashSet::new();
    for sample in &task.samples {
        if sample.id != format!("{}-{}", sample.source_id, sample.time_ms)
            || !sample_ids.insert(&sample.id)
            || sample.sha256.len() != 64
            || !sample.sha256.bytes().all(|c| c.is_ascii_hexdigit())
            || !task
                .assets
                .iter()
                .any(|a| a.id == sample.source_id && sample.time_ms < a.duration_ms)
        {
            return Err(invalid("Invalid persisted sample identity"));
        }
        let has_decoded_timestamp = task
            .history
            .iter()
            .filter(|event| event.kind == "sample_observed")
            .any(|event| {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&event.detail) else {
                    return false;
                };
                value["sample_id"].as_str() == Some(sample.id.as_str())
                    && value["source_id"].as_str() == Some(sample.source_id.as_str())
                    && value["decoded_time_ms"].as_u64() == Some(sample.time_ms)
            });
        if !has_decoded_timestamp {
            return Err(invalid("Task samples lack verified decoded-frame timecodes from this release candidate; recreate this pre-release task before model use"));
        }
    }
    crate::execution::apply_snapshot(root, &mut task)?;
    Ok(task)
}

pub(crate) fn save(root: &Path, task: &Task) -> Result<()> {
    let path = root.join("task.json");
    if path.exists() {
        regular(&path)?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer_pretty(&mut temporary, task)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    File::open(root)?.sync_all()?;
    Ok(())
}

pub(crate) fn event(task: &mut Task, kind: &str, detail: impl Into<String>) {
    task.history.push(Event {
        at: now(),
        kind: kind.into(),
        detail: detail.into(),
    });
}
