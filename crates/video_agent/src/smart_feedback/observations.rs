//! Content-bound, budgeted observations of actual rendered output and source samples.
use super::*;
use std::{collections::HashSet, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Published {
    key: String,
    bundle_sha256: String,
    image: PreviewImage,
}

fn directory(root: &Path, bundle_id: &str) -> Result<PathBuf> {
    let (attempt, digest) = bundle_id
        .split_once('-')
        .ok_or_else(|| invalid("Invalid observation bundle identity"))?;
    let attempt = attempt
        .parse::<u32>()
        .map_err(|_| invalid("Invalid observation attempt identity"))?;
    if !(1..=feedback::MAX_ATTEMPTS).contains(&attempt)
        || digest.len() != 64
        || !digest.bytes().all(|b| b.is_ascii_hexdigit())
        || bundle_id != format!("{attempt:02}-{digest}")
    {
        return Err(invalid("Escaped or invalid observation bundle identity"));
    }
    let parent = store::owned_dir(root, "feedback-frames")?;
    let path = parent.join(bundle_id);
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(invalid("Feedback observation directory was replaced"))
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&path)?,
        Err(e) => return Err(e.into()),
    }
    if fs::canonicalize(&path)?.parent() != Some(parent.as_path()) {
        return Err(invalid("Feedback observations escaped their task"));
    }
    Ok(path)
}

fn check_file(path: &Path, sha: &str, cancel: &CancelToken, clock: &Clock, cap: u64) -> Result<()> {
    store::regular(path)?;
    if path.metadata()?.len() > 1_048_576
        || media::hash_file_bounded(path, cancel, clock.timeout(cap)?)? != sha
    {
        return Err(invalid(
            "Feedback evidence changed or exceeds its image limit",
        ));
    }
    Ok(())
}

fn mapping(plan: &EditPlan, output_ms: u64) -> Result<(&Clip, u64)> {
    let mut start = 0u64;
    for clip in &plan.clips {
        let end = start
            .checked_add(clip.end_ms - clip.start_ms)
            .ok_or_else(|| invalid("Preview timeline overflow"))?;
        if output_ms >= start && output_ms < end {
            return Ok((clip, clip.start_ms + output_ms - start));
        }
        start = end;
    }
    Err(invalid(
        "Decoded preview observation falls outside the planned cut",
    ))
}

pub(super) fn rendered(
    root: &Path,
    task: &mut Task,
    bundle: &PreviewBundle,
    plan: &EditPlan,
    cancel: &CancelToken,
    clock: &Clock,
) -> Result<Vec<PreviewImage>> {
    let total = plan
        .clips
        .iter()
        .map(|c| c.end_ms - c.start_ms)
        .sum::<u64>();
    // Very short cuts may have only a few 30-fps output frames. Do not seek
    // beyond their last decodable frame just to fill a fixed-size storyboard.
    let count = plan
        .clips
        .len()
        .clamp(3, 6)
        .min((total / 150).max(1) as usize);
    let directory = directory(root, &bundle.bundle_id)?;
    let mut images = Vec::new();
    for n in 0..count {
        cancel.check()?;
        let key = format!("{}:{n}", bundle.bundle_id);
        let path = directory.join(format!("{n:02}.jpg"));
        let mut previous = None;
        for event in task
            .history
            .iter()
            .filter(|e| e.kind == "feedback_frame_published")
        {
            let published: Published = serde_json::from_str(&event.detail)?;
            if published.key == key {
                if previous.is_some() {
                    return Err(invalid("Duplicate feedback observation identity"));
                }
                previous = Some(published);
            }
        }
        let reserved = task
            .history
            .iter()
            .any(|e| e.kind == "feedback_frame_reserved" && e.detail == key);
        if previous.is_some() && !reserved {
            return Err(invalid("Feedback observation lost its frame reservation"));
        }
        if !reserved {
            if task.usage.frames_created >= task.limits.max_frames {
                return Err(invalid(
                    "Shared frame budget exhausted before preview observation",
                ));
            }
            task.usage.frames_created += 1;
            store::event(task, "feedback_frame_reserved", key.clone());
            store::save(root, task)?;
        }
        if let Some(published) = &previous {
            if published.bundle_sha256 != bundle.sha256 || published.image.path != path {
                return Err(invalid(
                    "Feedback observation is not bound to this rendered bundle",
                ));
            }
            let (clip, source_ms) = mapping(plan, published.image.output_time_ms)?;
            if published.image.source_time_ms != source_ms
                || published.image.clip_id != clip.id
                || published.image.source_id != clip.source_id
            {
                return Err(invalid("Feedback observation timeline identity changed"));
            }
            if path.exists() {
                check_file(
                    &path,
                    &published.image.sha256,
                    cancel,
                    clock,
                    task.limits.process_timeout_secs,
                )?;
                images.push(published.image.clone());
                continue;
            }
        } else if fs::symlink_metadata(&path).is_ok() {
            return Err(invalid(
                "Untracked feedback image exists; refusing overwrite",
            ));
        }
        let temporary = tempfile::Builder::new()
            .prefix("extract-")
            .tempdir_in(&directory)?;
        let partial = temporary.path().join("observation.jpg");
        let seek = ((total as u128 * (2 * n + 1) as u128) / (2 * count) as u128) as u64;
        let actual = media::frame(
            &root.join(&bundle.media_path),
            seek,
            &partial,
            cancel,
            clock.timeout(task.limits.process_timeout_secs)?,
        )?;
        if partial.metadata()?.len() > 1_048_576 {
            return Err(invalid("Rendered observation exceeds image size limit"));
        }
        let sha256 = media::hash_file_bounded(
            &partial,
            cancel,
            clock.timeout(task.limits.process_timeout_secs)?,
        )?;
        let (clip, source_ms) = mapping(plan, actual)?;
        let image = PreviewImage {
            id: format!("rendered-{}-{n}", bundle.bundle_id),
            path: path.clone(),
            sha256,
            output_time_ms: actual,
            clip_id: clip.id.clone(),
            source_id: clip.source_id.clone(),
            source_time_ms: source_ms,
        };
        if let Some(previous) = previous {
            if previous.image != image {
                return Err(invalid(
                    "Regenerated rendered observation differs from its recorded content",
                ));
            }
        } else {
            let published = Published {
                key,
                bundle_sha256: bundle.sha256.clone(),
                image: image.clone(),
            };
            store::event(
                task,
                "feedback_frame_published",
                serde_json::to_string(&published)?,
            );
            store::save(root, task)?;
        }
        fs::hard_link(&partial, &path)?;
        images.push(image);
    }
    check_sources_during(task, cancel, clock)?;
    Ok(images)
}

pub(super) fn sources(
    root: &Path,
    task: &Task,
    plan: &EditPlan,
    cancel: &CancelToken,
    clock: &Clock,
) -> Result<Vec<Sample>> {
    if task.samples.is_empty() {
        return Err(invalid(
            "Create actual source observations before smart feedback",
        ));
    }
    let chosen = select_sources(&task.assets, &task.samples, plan);
    for sample in &chosen {
        verify_source(root, task, sample, cancel, clock)?;
    }
    Ok(chosen)
}

fn select_sources(assets: &[Asset], samples: &[Sample], plan: &EditPlan) -> Vec<Sample> {
    let mut sorted: Vec<_> = samples.iter().collect();
    sorted.sort_by(|a, b| (&a.source_id, a.time_ms, &a.id).cmp(&(&b.source_id, b.time_ms, &b.id)));
    if sorted.is_empty() {
        return vec![];
    }
    let mut chosen = Vec::new();
    let mut ids = HashSet::new();
    {
        let mut add = |sample: &Sample| {
            if chosen.len() < 8 && ids.insert(sample.id.clone()) {
                chosen.push(sample.clone());
            }
        };
        // Cover every admitted source before spending extra slots on endpoints.
        // Prefer an actual observation within that source's current cut.
        for asset in assets {
            let frames: Vec<_> = sorted.iter().filter(|s| s.source_id == asset.id).collect();
            let preferred = frames.iter().find(|sample| {
                plan.clips.iter().any(|clip| {
                    clip.source_id == asset.id
                        && sample.time_ms >= clip.start_ms
                        && sample.time_ms < clip.end_ms
                })
            });
            if let Some(sample) = preferred.or_else(|| frames.first()) {
                add(sample);
            }
        }
        for clip in &plan.clips {
            if let Some(sample) = clip
                .evidence_ids
                .iter()
                .find_map(|id| samples.iter().find(|s| &s.id == id))
            {
                add(sample);
            }
        }
        for asset in assets {
            if let Some(sample) = sorted
                .iter()
                .rev()
                .find(|sample| sample.source_id == asset.id)
            {
                add(sample);
            }
        }
        for n in 0..8 {
            add(sorted[n * (sorted.len() - 1) / 7]);
        }
    }
    chosen
}

fn verify_source(
    root: &Path,
    task: &Task,
    sample: &Sample,
    cancel: &CancelToken,
    clock: &Clock,
) -> Result<()> {
    if !task.samples.contains(sample) {
        return Err(invalid(
            "Review source evidence is not an actual task observation",
        ));
    }
    let frames = store::owned_dir(root, "frames")?;
    if sample.image_path.parent() != Some(frames.as_path())
        || sample.image_path != frames.join(format!("{}.jpg", sample.id))
        || fs::canonicalize(&sample.image_path)?.parent() != Some(frames.as_path())
    {
        return Err(invalid("Source review image escaped task frames"));
    }
    check_file(
        &sample.image_path,
        &sample.sha256,
        cancel,
        clock,
        task.limits.process_timeout_secs,
    )
}

pub(super) fn verify_request_images(
    root: &Path,
    task: &Task,
    request: &ReviewRequest,
    cancel: &CancelToken,
    clock: &Clock,
) -> Result<()> {
    if plan_digest(task, &request.plan)? != request.plan_digest {
        return Err(invalid("Review request plan fingerprint changed"));
    }
    for source in &request.source_samples {
        verify_source(root, task, source, cancel, clock)?;
    }
    for image in &request.preview_frames {
        let record = task
            .history
            .iter()
            .filter(|e| e.kind == "feedback_frame_published")
            .map(|e| serde_json::from_str::<Published>(&e.detail))
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .find(|p| {
                p.image.id == image.id
                    && p.image == *image
                    && p.bundle_sha256 == request.preview_sha256
            })
            .ok_or_else(|| invalid("Rendered review image has no published content binding"))?;
        let bundle_id = record
            .key
            .rsplit_once(':')
            .map(|(id, _)| id)
            .ok_or_else(|| invalid("Malformed preview observation identity"))?;
        let folder = directory(root, bundle_id)?;
        if image.path.parent() != Some(folder.as_path())
            || fs::canonicalize(&image.path)?.parent() != Some(folder.as_path())
        {
            return Err(invalid("Rendered review image escaped its task"));
        }
        check_file(
            &image.path,
            &image.sha256,
            cancel,
            clock,
            task.limits.process_timeout_secs,
        )?;
    }
    check_sources_during(task, cancel, clock)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn eight_source_pool_covers_late_sources_and_prefers_current_interval() {
        let assets: Vec<_> = (1..=8)
            .map(|n| Asset {
                id: format!("source-{n}"),
                path: "inert-not-read.mp4".into(),
                sha256: "a".repeat(64),
                size: 0,
                modified_ns: 0,
                duration_ms: 3000,
                width: 160,
                height: 90,
                has_audio: false,
            })
            .collect();
        let samples: Vec<_> = assets
            .iter()
            .flat_map(|asset| {
                [0, 1500].map(|time| Sample {
                    id: format!("{}-{time}", asset.id),
                    source_id: asset.id.clone(),
                    time_ms: time,
                    image_path: "inert-not-read.jpg".into(),
                    sha256: "b".repeat(64),
                })
            })
            .collect();
        let plan = EditPlan {
            revision: 1,
            clips: vec![Clip {
                id: "current".into(),
                source_id: "source-8".into(),
                start_ms: 1000,
                end_ms: 2000,
                reason: "MANUAL fixture".into(),
                uncertainty: "fixture only".into(),
                evidence_ids: vec![],
                human_modified: true,
            }],
            summary: "Fixture".into(),
            audio_policy: AudioPolicy::Mute,
        };
        let selected = select_sources(&assets, &samples, &plan);
        assert_eq!(selected.len(), 8);
        for asset in &assets {
            assert!(selected.iter().any(|sample| sample.source_id == asset.id));
        }
        assert!(selected.iter().any(|sample| sample.id == "source-8-1500"));
        assert!(!selected.iter().any(|sample| sample.id == "source-8-0"));
        assert_eq!(selected, select_sources(&assets, &samples, &plan));
    }
}
