//! Text subtitle workflows around a validated millisecond cue timeline.
//! Styles, positioning and karaoke are not preserved. Outputs are UTF-8.
use crate::WorkflowKind;
use anyhow::{anyhow, bail, Context, Result};
use encoding_rs::{Encoding, UTF_8};
use file_conversion::{CancelToken, Progress};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CUES: usize = 100_000;
// A finite bound also keeps integer/floating point transformations exact.
const MAX_TIMESTAMP_MS: i64 = 3_600_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Cue {
    start_ms: i64,
    end_ms: i64,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Srt,
    Vtt,
    Ass,
    Ssa,
    Sbv,
    Sub,
    Smi,
    Lrc,
    Txt,
    Csv,
    Json,
}

impl Format {
    fn parse(value: &str) -> Result<Self> {
        Ok(match value.to_ascii_lowercase().as_str() {
            "srt" => Self::Srt,
            "vtt" | "webvtt" => Self::Vtt,
            "ass" => Self::Ass,
            "ssa" => Self::Ssa,
            "sbv" => Self::Sbv,
            "sub" => Self::Sub,
            "smi" | "sami" => Self::Smi,
            "lrc" => Self::Lrc,
            "txt" => Self::Txt,
            "csv" => Self::Csv,
            "json" => Self::Json,
            _ => bail!("Unsupported subtitle format '{value}'"),
        })
    }
    fn extension(self) -> &'static str {
        match self {
            Self::Srt => "srt",
            Self::Vtt => "vtt",
            Self::Ass => "ass",
            Self::Ssa => "ssa",
            Self::Sbv => "sbv",
            Self::Sub => "sub",
            Self::Smi => "smi",
            Self::Lrc => "lrc",
            Self::Txt => "txt",
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Options {
    format: Option<String>,
    encoding: Option<String>,
    offset_ms: Option<i64>,
    source_fps: Option<f64>,
    target_fps: Option<f64>,
    clean: Option<CleanSetting>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CleanSetting {
    Enabled(bool),
    Settings(CleanOptions),
}

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CleanOptions {
    remove_empty: bool,
    remove_duplicates: bool,
    strip_markup: bool,
    normalize_whitespace: bool,
}
impl Default for CleanOptions {
    fn default() -> Self {
        Self {
            remove_empty: true,
            remove_duplicates: true,
            strip_markup: false,
            normalize_whitespace: true,
        }
    }
}

fn cancelled(cancel: &CancelToken) -> Result<()> {
    if cancel.is_cancelled() {
        bail!("Subtitle workflow cancelled");
    }
    Ok(())
}

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
        WorkflowKind::SubtitleConvert
            | WorkflowKind::SubtitleShift
            | WorkflowKind::SubtitleFps
            | WorkflowKind::SubtitleClean
    ) {
        bail!("Wrong workflow kind for subtitle processing");
    }
    cancelled(cancel)?;
    if inputs.is_empty() {
        bail!("Choose at least one subtitle input");
    }
    let options: Options = serde_json::from_value(if options.is_null() {
        serde_json::json!({})
    } else {
        options.clone()
    })
    .context("Invalid subtitle options (unknown options are rejected)")?;
    if kind == WorkflowKind::SubtitleConvert && options.format.is_none() {
        bail!("Subtitle conversion requires options.format");
    }
    if kind == WorkflowKind::SubtitleShift && options.offset_ms.is_none() {
        bail!("Subtitle shift requires options.offset_ms");
    }
    let fps_scale = if kind == WorkflowKind::SubtitleFps {
        Some(
            valid_fps(options.source_fps, "source_fps")?
                / valid_fps(options.target_fps, "target_fps")?,
        )
    } else {
        None
    };
    if let Some(fps) = options.source_fps {
        valid_fps(Some(fps), "source_fps")?;
    }
    if let Some(fps) = options.target_fps {
        valid_fps(Some(fps), "target_fps")?;
    }
    let cleaner = match &options.clean {
        Some(CleanSetting::Enabled(false)) => None,
        Some(CleanSetting::Enabled(true)) => Some(CleanOptions::default()),
        Some(CleanSetting::Settings(settings)) => Some(settings.clone()),
        None if kind == WorkflowKind::SubtitleClean => Some(CleanOptions::default()),
        None => None,
    };
    let mut outputs = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        cancelled(cancel)?;
        progress(Progress {
            fraction: index as f32 / inputs.len() as f32,
            message: format!("Reading subtitle {}", input.display()),
        });
        let source = Format::parse(
            input
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or(""),
        )?;
        let target = options
            .format
            .as_deref()
            .map(Format::parse)
            .transpose()?
            .unwrap_or(source);
        let text = read_text(input, options.encoding.as_deref(), cancel)?;
        let mut cues = parse(source, &text, options.source_fps)?;
        validate(&cues)?;
        if let Some(offset) = options.offset_ms {
            for cue in &mut cues {
                cue.start_ms = cue
                    .start_ms
                    .checked_add(offset)
                    .context("Subtitle shift overflows its timestamp")?
                    .max(0);
                cue.end_ms = cue
                    .end_ms
                    .checked_add(offset)
                    .context("Subtitle shift overflows its timestamp")?
                    .max(0);
            }
        }
        if let Some(scale) = fps_scale {
            for cue in &mut cues {
                cue.start_ms = scaled_timestamp(cue.start_ms, scale)?;
                cue.end_ms = scaled_timestamp(cue.end_ms, scale)?;
            }
        }
        if let Some(settings) = &cleaner {
            clean(&mut cues, settings);
        }
        validate(&cues)?;
        cancelled(cancel)?;
        let encoded = render(target, &cues, options.target_fps.or(options.source_fps))?;
        let mut filename = input
            .file_stem()
            .context("Subtitle input needs a filename")?
            .to_os_string();
        filename.push(format!(".converted.{}", target.extension()));
        let output = staging.join(filename);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .with_context(|| {
                format!("Cannot create unique subtitle output {}", output.display())
            })?;
        file.write_all(encoded.as_bytes())?;
        file.sync_all()?;
        cancelled(cancel)?;
        outputs.push(output);
        progress(Progress {
            fraction: (index + 1) as f32 / inputs.len() as f32,
            message: format!(
                "Converted {} text cues; styles/positioning are not preserved",
                cues.len()
            ),
        });
    }
    Ok(outputs)
}

fn valid_fps(value: Option<f64>, name: &str) -> Result<f64> {
    let value = value.ok_or_else(|| anyhow!("Subtitle workflow requires options.{name}"))?;
    if !value.is_finite() || !(0.01..=1000.0).contains(&value) {
        bail!("{name} must be a finite FPS between 0.01 and 1000");
    }
    Ok(value)
}
fn scaled_timestamp(timestamp: i64, scale: f64) -> Result<i64> {
    let value = timestamp as f64 * scale;
    if !value.is_finite() || value < 0.0 || value > MAX_TIMESTAMP_MS as f64 {
        bail!("Subtitle transformation exceeds its 1000-hour timeline limit");
    }
    Ok(value.round() as i64)
}

fn read_text(input: &Path, label: Option<&str>, cancel: &CancelToken) -> Result<String> {
    let metadata =
        fs::metadata(input).with_context(|| format!("Cannot read {}", input.display()))?;
    if !metadata.is_file() || metadata.len() > MAX_INPUT_BYTES {
        bail!("Subtitle input must be a regular file no larger than 16 MiB");
    }
    let mut bytes = Vec::new();
    fs::File::open(input)?
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        bail!("Subtitle input exceeds 16 MiB");
    }
    cancelled(cancel)?;
    let bom = Encoding::for_bom(&bytes);
    let explicit = label.filter(|label| !label.eq_ignore_ascii_case("auto"));
    let encoding = if let Some(label) = explicit {
        Encoding::for_label(label.as_bytes())
            .ok_or_else(|| anyhow!("Unknown subtitle encoding '{label}'"))?
    } else {
        bom.map(|(encoding, _)| encoding).unwrap_or(UTF_8)
    };
    if let Some((detected, _)) = bom {
        if detected != encoding {
            bail!("Subtitle BOM conflicts with the requested encoding");
        }
    }
    let body = &bytes[bom.map(|(_, size)| size).unwrap_or(0)..];
    let decoded = encoding.decode_without_bom_handling_and_without_replacement(body)
        .ok_or_else(|| anyhow!("Subtitle is not valid {}; specify options.encoding (for example gb18030) for legacy text", encoding.name()))?;
    if decoded.contains('\0') {
        bail!(
            "Subtitle contains NUL bytes; binary subtitles and undetected UTF-16 are unsupported"
        );
    }
    Ok(decoded.replace("\r\n", "\n").replace('\r', "\n"))
}

fn validate(cues: &[Cue]) -> Result<()> {
    if cues.is_empty() {
        bail!("Subtitle has no timed text cues");
    }
    if cues.len() > MAX_CUES {
        bail!("Subtitle exceeds the 100,000 cue limit");
    }
    let mut previous = 0;
    for (index, cue) in cues.iter().enumerate() {
        if cue.start_ms < 0 || cue.end_ms <= cue.start_ms || cue.end_ms > MAX_TIMESTAMP_MS {
            bail!("Cue {} needs nonnegative start, end greater than start, and a timeline under 1000 hours (negative shifts clamp endpoints to zero)", index + 1);
        }
        if cue.start_ms < previous {
            bail!(
                "Subtitle cues are out of chronological order at cue {}",
                index + 1
            );
        }
        previous = cue.start_ms;
    }
    Ok(())
}
fn parse(format: Format, text: &str, fps: Option<f64>) -> Result<Vec<Cue>> {
    match format {
        Format::Srt | Format::Vtt => parse_blocks(text, format == Format::Vtt),
        Format::Sbv => parse_sbv(text),
        Format::Ass | Format::Ssa => parse_ass(text),
        Format::Sub => parse_microdvd(text, fps),
        Format::Smi => parse_sami(text),
        Format::Lrc => parse_lrc(text),
        Format::Txt | Format::Csv | Format::Json => bail!(
            "TXT, CSV and JSON are subtitle export formats only; choose a timed subtitle input"
        ),
    }
}
fn number(value: &str) -> Result<i64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("Expected a nonnegative timestamp number, got '{value}'");
    }
    value
        .parse()
        .context("Subtitle timestamp number is too large")
}
fn parse_clock(value: &str) -> Result<i64> {
    let fields: Vec<_> = value.trim().split(':').collect();
    if fields.len() != 2 && fields.len() != 3 {
        bail!("Invalid subtitle timestamp '{value}'");
    }
    let (hours, minutes, seconds) = if fields.len() == 3 {
        (number(fields[0])?, number(fields[1])?, fields[2])
    } else {
        (0, number(fields[0])?, fields[1])
    };
    if fields.len() == 3 && minutes >= 60 {
        bail!("Invalid minute value in '{value}'");
    }
    let parts: Vec<_> = seconds.split(['.', ',']).collect();
    if parts.len() > 2 {
        bail!("Invalid subtitle timestamp '{value}'");
    }
    let seconds = number(parts[0])?;
    if seconds >= 60 {
        bail!("Invalid seconds value in '{value}'");
    }
    let millis = if parts.len() == 2 {
        let fraction = parts[1];
        if fraction.is_empty() || fraction.len() > 3 {
            bail!("Timestamp fractions must have 1 to 3 decimal digits: '{value}'");
        }
        number(fraction)?
            .checked_mul(10_i64.pow(3 - fraction.len() as u32))
            .context("Timestamp overflow")?
    } else {
        0
    };
    hours
        .checked_mul(3_600_000)
        .and_then(|value| value.checked_add(minutes.checked_mul(60_000)?))
        .and_then(|value| value.checked_add(seconds.checked_mul(1000)?))
        .and_then(|value| value.checked_add(millis))
        .context("Subtitle timestamp overflow")
}
fn parse_blocks(text: &str, webvtt: bool) -> Result<Vec<Cue>> {
    if webvtt
        && !text.trim_start().lines().next().is_some_and(|line| {
            line == "WEBVTT" || line.starts_with("WEBVTT ") || line.starts_with("WEBVTT\t")
        })
    {
        bail!("WebVTT input is missing its WEBVTT header");
    }
    let mut cues = Vec::new();
    let separator = Regex::new(r"\n[ \t]*\n")?;
    for block in separator
        .split(text)
        .filter(|block| !block.trim().is_empty())
    {
        let block = block.trim_matches('\n');
        if webvtt
            && (block.starts_with("WEBVTT")
                || ["NOTE", "STYLE", "REGION"].iter().any(|keyword| {
                    block == *keyword
                        || block.starts_with(&format!("{keyword}\n"))
                        || block.starts_with(&format!("{keyword} "))
                        || block.starts_with(&format!("{keyword}\t"))
                }))
        {
            continue;
        }
        let lines: Vec<_> = block.lines().collect();
        let timing_index = if lines[0].contains("-->") { 0 } else { 1 };
        if !webvtt && timing_index == 1 {
            number(lines[0].trim()).context("Invalid SRT cue index")?;
        }
        let timing = lines
            .get(timing_index)
            .context("Subtitle block has no timing line")?;
        let (start, end) = timing
            .split_once("-->")
            .context("Subtitle block has no timing arrow")?;
        let end = if webvtt {
            end.split_whitespace()
                .next()
                .context("WebVTT cue has no end time")?
        } else {
            end.trim()
        };
        cues.push(Cue {
            start_ms: parse_clock(start)?,
            end_ms: parse_clock(end)?,
            text: lines[timing_index + 1..].join("\n"),
        });
    }
    Ok(cues)
}
fn parse_sbv(text: &str) -> Result<Vec<Cue>> {
    let mut cues = Vec::new();
    let separator = Regex::new(r"\n[ \t]*\n")?;
    for block in separator
        .split(text)
        .filter(|block| !block.trim().is_empty())
    {
        let mut lines = block.trim_matches('\n').lines();
        let (start, end) = lines
            .next()
            .context("SBV block is empty")?
            .split_once(',')
            .context("SBV timing must contain start,end")?;
        cues.push(Cue {
            start_ms: parse_clock(start)?,
            end_ms: parse_clock(end)?,
            text: lines.collect::<Vec<_>>().join("\n"),
        });
    }
    Ok(cues)
}
fn parse_ass(text: &str) -> Result<Vec<Cue>> {
    let mut in_events = false;
    let mut columns: Vec<String> = Vec::new();
    let mut cues = Vec::new();
    let overrides = Regex::new(r"\{\\[^}]*\}")?;
    let drawings = Regex::new(r"\\p[1-9]\d*")?;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_events = line.eq_ignore_ascii_case("[Events]");
            continue;
        }
        if !in_events || line.is_empty() || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.eq_ignore_ascii_case("format") {
            columns = value
                .split(',')
                .map(|value| value.trim().to_ascii_lowercase())
                .collect();
            continue;
        }
        if !key.eq_ignore_ascii_case("dialogue") {
            continue;
        }
        let start = columns
            .iter()
            .position(|value| value == "start")
            .context("ASS/SSA needs an Events Format with Start, End and final Text columns")?;
        let end = columns
            .iter()
            .position(|value| value == "end")
            .context("ASS/SSA Format has no End column")?;
        if columns.last().map(String::as_str) != Some("text") {
            bail!("ASS/SSA Text column must be last");
        }
        let fields: Vec<_> = value.trim_start().splitn(columns.len(), ',').collect();
        if fields.len() != columns.len() {
            bail!("ASS/SSA Dialogue does not match its Format columns");
        }
        if drawings.is_match(fields.last().unwrap()) {
            bail!(
                "ASS vector drawing cues are not text subtitles and cannot be converted faithfully"
            );
        }
        let text = overrides
            .replace_all(fields.last().unwrap(), "")
            .replace("\\N", "\n")
            .replace("\\n", "\n")
            .replace("\\h", "\u{a0}");
        cues.push(Cue {
            start_ms: parse_clock(fields[start])?,
            end_ms: parse_clock(fields[end])?,
            text,
        });
    }
    cues.sort_by_key(|cue| (cue.start_ms, cue.end_ms));
    Ok(cues)
}
fn parse_microdvd(text: &str, fps: Option<f64>) -> Result<Vec<Cue>> {
    let pattern = Regex::new(r"^\{(\d+)\}\{(\d+)\}(.*)$")?;
    let markup = Regex::new(r"\{[yYfFsScCoOhHpP]:[^}]*\}")?;
    let mut effective_fps = fps;
    let mut cues = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let fields = pattern.captures(line.trim()).context(
            "SUB input is not text MicroDVD; binary VobSub and other .sub dialects are unsupported",
        )?;
        if cues.is_empty() && &fields[1] == "1" && &fields[2] == "1" {
            if let Ok(header_fps) = fields[3].trim().parse::<f64>() {
                valid_fps(Some(header_fps), "MicroDVD FPS header")?;
                if let Some(explicit) = effective_fps {
                    if (explicit - header_fps).abs() > 0.001 {
                        bail!("MicroDVD FPS header conflicts with source_fps");
                    }
                }
                effective_fps = Some(header_fps);
                continue;
            }
        }
        let fps = valid_fps(
            effective_fps,
            "source_fps (or an embedded MicroDVD FPS header)",
        )?;
        cues.push(Cue {
            start_ms: scaled_timestamp(number(&fields[1])?, 1000.0 / fps)?,
            end_ms: scaled_timestamp(number(&fields[2])?, 1000.0 / fps)?,
            text: markup.replace_all(&fields[3], "").replace('|', "\n"),
        });
    }
    Ok(cues)
}
fn parse_sami(text: &str) -> Result<Vec<Cue>> {
    let sync = Regex::new(r#"(?is)<sync\b[^>]*\bstart\s*=\s*["']?(\d+)["']?[^>]*>"#)?;
    let matches: Vec<_> = sync.captures_iter(text).collect();
    if Regex::new(r"(?i)<sync\b")?.find_iter(text).count() != matches.len() {
        bail!("SAMI has a malformed SYNC timestamp");
    }
    let mut events = Vec::new();
    for (index, matched) in matches.iter().enumerate() {
        let begin = matched.get(0).unwrap().end();
        let end = matches
            .get(index + 1)
            .map(|matched| matched.get(0).unwrap().start())
            .unwrap_or(text.len());
        events.push((
            number(&matched[1])?,
            plain_html(&text[begin..end]).trim().to_string(),
        ));
    }
    start_only_events(events, false)
}
fn parse_lrc(text: &str) -> Result<Vec<Cue>> {
    let timestamp = Regex::new(r"^\[(\d+:\d{2}(?:[.,]\d{1,3})?)\]")?;
    let offset = Regex::new(r"(?i)^\[offset:([+-]?\d+)\]$")?;
    let metadata = Regex::new(r"(?i)^\[[a-z][a-z0-9]*:[^]]*\]$")?;
    let enhanced = Regex::new(r"<\d+:\d")?;
    let mut events = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(offset) = offset.captures(line) {
            if offset[1].parse::<i64>().context("Invalid LRC offset")? != 0 {
                bail!("Nonzero LRC offset metadata is unsupported; normalize it explicitly before conversion");
            }
            continue;
        }
        if metadata.is_match(line) {
            continue;
        }
        let mut remaining = line;
        let mut starts = Vec::new();
        while let Some(tag) = timestamp.captures(remaining) {
            starts.push(parse_clock(&tag[1])?);
            remaining = &remaining[tag.get(0).unwrap().end()..];
        }
        if starts.is_empty() {
            bail!("LRC text line has no timestamp: '{line}'");
        }
        if enhanced.is_match(remaining) {
            bail!("Enhanced LRC word timestamps are not supported");
        }
        for start in starts {
            events.push((start, remaining.to_string()));
        }
    }
    start_only_events(events, true)
}
fn start_only_events(mut events: Vec<(i64, String)>, sort: bool) -> Result<Vec<Cue>> {
    if sort {
        events.sort_by_key(|event| event.0);
    }
    if events.len() > MAX_CUES {
        bail!("Subtitle exceeds the 100,000 cue limit");
    }
    if events.windows(2).any(|events| events[0].0 > events[1].0) {
        bail!("Subtitle cues are out of chronological order");
    }
    let mut cues = Vec::new();
    let mut index = 0;
    while index < events.len() {
        let start = events[index].0;
        let mut next = index + 1;
        while next < events.len() && events[next].0 == start {
            next += 1;
        }
        let end = events.get(next).map(|event| event.0).unwrap_or(
            start
                .checked_add(2000)
                .context("Subtitle timestamp overflow")?,
        );
        for (_, text) in &events[index..next] {
            if !text.trim().is_empty() {
                cues.push(Cue {
                    start_ms: start,
                    end_ms: end,
                    text: text.clone(),
                });
            }
        }
        index = next;
    }
    Ok(cues)
}
fn plain_html(text: &str) -> String {
    static BREAKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());
    static PARAGRAPHS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</p\s*>").unwrap());
    static TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());
    let with_breaks = BREAKS.replace_all(text, "\n");
    let with_paragraphs = PARAGRAPHS.replace_all(&with_breaks, "\n");
    decode_entities(&TAGS.replace_all(&with_paragraphs, ""))
}
fn decode_entities(text: &str) -> String {
    static PATTERN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"&(#x[0-9a-fA-F]+|#\d+|amp|lt|gt|quot|apos|nbsp);").unwrap());
    PATTERN
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let entity = &captures[1];
            let character = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                value if value.starts_with("#x") => u32::from_str_radix(&value[2..], 16)
                    .ok()
                    .and_then(char::from_u32),
                value if value.starts_with('#') => {
                    value[1..].parse::<u32>().ok().and_then(char::from_u32)
                }
                _ => None,
            };
            character
                .map(|value| value.to_string())
                .unwrap_or_else(|| captures[0].to_string())
        })
        .into_owned()
}
fn clean(cues: &mut Vec<Cue>, options: &CleanOptions) {
    let mut seen = HashSet::new();
    cues.retain_mut(|cue| {
        if options.strip_markup {
            cue.text = plain_html(&cue.text);
        }
        if options.normalize_whitespace {
            cue.text = cue
                .text
                .lines()
                .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string();
        }
        if options.remove_empty && cue.text.trim().is_empty() {
            return false;
        }
        !options.remove_duplicates || seen.insert((cue.start_ms, cue.end_ms, cue.text.clone()))
    });
}
fn clock(timestamp: i64, separator: char) -> String {
    format!(
        "{:02}:{:02}:{:02}{separator}{:03}",
        timestamp / 3_600_000,
        timestamp / 60_000 % 60,
        timestamp / 1000 % 60,
        timestamp % 1000
    )
}
fn ass_clock(timestamp: i64) -> String {
    let centiseconds = (timestamp + 5) / 10;
    format!(
        "{}:{:02}:{:02}.{:02}",
        centiseconds / 360_000,
        centiseconds / 6000 % 60,
        centiseconds / 100 % 60,
        centiseconds % 100
    )
}
fn html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn render(format: Format, cues: &[Cue], fps: Option<f64>) -> Result<String> {
    let mut output = String::new();
    match format {
        Format::Srt | Format::Vtt => {
            if format == Format::Vtt {
                output.push_str("WEBVTT\n\n");
            }
            for (index, cue) in cues.iter().enumerate() {
                let separator = if format == Format::Srt { ',' } else { '.' };
                output.push_str(&format!(
                    "{}\n{} --> {}\n{}\n\n",
                    index + 1,
                    clock(cue.start_ms, separator),
                    clock(cue.end_ms, separator),
                    cue.text
                ));
            }
        }
        Format::Sbv => {
            for cue in cues {
                output.push_str(&format!(
                    "{},{}\n{}\n\n",
                    clock(cue.start_ms, '.'),
                    clock(cue.end_ms, '.'),
                    cue.text
                ));
            }
        }
        Format::Ass | Format::Ssa => {
            let plus = format == Format::Ass;
            output.push_str(if plus {
                "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,24,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,1,0,2,10,10,10,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n"
            } else {
                "[Script Info]\nScriptType: v4.00\n\n[V4 Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, TertiaryColour, BackColour, Bold, Italic, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, AlphaLevel, Encoding\nStyle: Default,Arial,24,16777215,255,0,0,0,0,1,1,0,2,10,10,10,0,1\n\n[Events]\nFormat: Marked, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n"
            });
            for cue in cues {
                if (cue.start_ms + 5) / 10 >= (cue.end_ms + 5) / 10 {
                    bail!("ASS/SSA centisecond precision collapses a cue; use SRT or VTT");
                }
                if cue.text.contains("{\\")
                    || ["\\N", "\\n", "\\h"]
                        .iter()
                        .any(|sequence| cue.text.contains(sequence))
                {
                    bail!("Literal ASS control sequences cannot be represented faithfully; use SRT or VTT");
                }
                let first = if plus { "0" } else { "Marked=0" };
                output.push_str(&format!(
                    "Dialogue: {first},{},{},Default,,0,0,0,,{}\n",
                    ass_clock(cue.start_ms),
                    ass_clock(cue.end_ms),
                    cue.text.replace('\n', "\\N")
                ));
            }
        }
        Format::Sub => {
            let fps = valid_fps(fps, "target_fps (or source_fps) for MicroDVD output")?;
            output.push_str(&format!("{{1}}{{1}}{fps:.6}\n"));
            let markup = Regex::new(r"\{[yYfFsScCoOhHpP]:")?;
            for cue in cues {
                let start = scaled_timestamp(cue.start_ms, fps / 1000.0)?;
                let end = scaled_timestamp(cue.end_ms, fps / 1000.0)?;
                if end <= start {
                    bail!("MicroDVD frame precision collapses a cue; use SRT or VTT");
                }
                if cue.text.contains('|') || markup.is_match(&cue.text) {
                    bail!("Literal MicroDVD pipes/style tags cannot be represented faithfully; use SRT or VTT");
                }
                output.push_str(&format!(
                    "{{{start}}}{{{end}}}{}\n",
                    cue.text.replace('\n', "|")
                ));
            }
        }
        Format::Smi => {
            if cues
                .windows(2)
                .any(|pair| pair[0].end_ms > pair[1].start_ms)
            {
                bail!("SAMI output cannot faithfully represent overlapping cues; use SRT or ASS");
            }
            output
                .push_str("<SAMI>\n<HEAD><TITLE>Video Toolkit subtitles</TITLE></HEAD>\n<BODY>\n");
            for cue in cues {
                output.push_str(&format!(
                    "<SYNC Start={}><P Class=ENCC>{}\n<SYNC Start={}><P Class=ENCC>&nbsp;\n",
                    cue.start_ms,
                    html(&cue.text).replace('\n', "<br>"),
                    cue.end_ms
                ));
            }
            output.push_str("</BODY>\n</SAMI>\n");
        }
        Format::Lrc => {
            for cue in cues {
                if cue.text.contains('\n') {
                    bail!("LRC cannot faithfully preserve multiline cues; choose SRT, VTT or ASS");
                }
                output.push_str(&format!(
                    "[{:02}:{:02}.{:03}]{}\n",
                    cue.start_ms / 60_000,
                    cue.start_ms / 1000 % 60,
                    cue.start_ms % 1000,
                    cue.text
                ));
            }
        }
        Format::Txt => {
            output = cues
                .iter()
                .map(|cue| cue.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            output.push('\n');
        }
        Format::Csv => {
            output.push_str("start_ms,end_ms,text\r\n");
            for cue in cues {
                output.push_str(&format!(
                    "{},{},\"{}\"\r\n",
                    cue.start_ms,
                    cue.end_ms,
                    cue.text.replace('"', "\"\"")
                ));
            }
        }
        Format::Json => {
            output = serde_json::to_string_pretty(
                &serde_json::json!({"schema":"video-toolkit/subtitles/v1","cues":cues}),
            )?;
            output.push('\n');
        }
    }
    Ok(output)
}
