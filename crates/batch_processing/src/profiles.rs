//! Validated, side-effect-free conversion of saved profiles into batch jobs.

use std::path::PathBuf;

use common::{validate_time_range, VideoFormat};
use profile_system::{Profile, ProfileType};
use regex::Regex;

use crate::{
    BatchClipperConfig, BatchError, BatchGifConverterConfig, BatchGifTransparencyConfig,
    BatchMergerConfig, BatchOperation, BatchProcessor, BatchSplitterConfig, Result,
};

/// The operation-specific settings in a validated profile.
#[derive(Debug, Clone)]
pub enum ProfileOperationConfig {
    Clipper(BatchClipperConfig),
    GifConverter(BatchGifConverterConfig),
    GifTransparency(BatchGifTransparencyConfig),
    Splitter(BatchSplitterConfig),
    Merger(BatchMergerConfig),
}

/// Shared batch settings plus the operation selected by a saved profile.
#[derive(Debug, Clone)]
pub struct ProfileBatchConfig {
    pub operation: ProfileOperationConfig,
    pub output_format: VideoFormat,
    pub recursive: bool,
    pub parallel: bool,
    pub pattern: Option<String>,
}

impl ProfileBatchConfig {
    /// Validate all profile parameters before looking up inputs or creating outputs.
    pub fn from_profile(profile: &Profile) -> Result<Self> {
        let operation_keys: &[&str] = match &profile.profile_type {
            ProfileType::Clipper => &[
                "time_ranges",
                "ranges",
                "output_dir",
                "copy_codec",
                "suffix",
                "output_format",
                "format",
            ],
            ProfileType::GifConverter => &[
                "width",
                "fps",
                "max_size_mb",
                "max_size",
                "optimize",
                "output_dir",
            ],
            ProfileType::GifTransparency => &["create_backup", "backup"],
            ProfileType::Splitter => &[
                "output_dir",
                "prefix",
                "encode_options",
                "custom_encode",
                "force",
                "output_format",
                "format",
            ],
            ProfileType::Merger => &[
                "audio_file",
                "audio",
                "output_dir",
                "use_shortest",
                "shortest",
                "copy_codec",
                "output_format",
                "format",
            ],
            ProfileType::Custom(name) => {
                return Err(parameter_error(
                    profile,
                    "profile_type",
                    name,
                    "custom profile types cannot be executed as batch operations",
                ));
            }
        };
        let parameters = Parameters(profile);
        // Report a stable first error even though serialized parameters use a map.
        let mut keys: Vec<_> = profile.parameters.keys().collect();
        keys.sort();
        for key in keys {
            if !["recursive", "parallel", "pattern"].contains(&key.as_str())
                && !operation_keys.contains(&key.as_str())
            {
                return Err(parameter_error(
                    profile,
                    key,
                    &profile.parameters[key],
                    "unknown parameter for this operation",
                ));
            }
        }

        let recursive = parameters.boolean(&["recursive"], false)?;
        let parallel = parameters.boolean(&["parallel"], true)?;
        // Whitespace is meaningful in regular expressions. Only an empty value
        // disables filtering; preserve the exact expression saved by the user.
        let pattern = parameters
            .get(&["pattern"])?
            .map(|(_, value)| value)
            .filter(|value| !value.is_empty())
            .map(String::from);
        if let Some(pattern) = &pattern {
            Regex::new(pattern).map_err(|error| {
                parameter_error(
                    profile,
                    "pattern",
                    pattern,
                    &format!("invalid regex: {error}"),
                )
            })?;
        }
        let output_format = match &profile.profile_type {
            ProfileType::Clipper | ProfileType::Splitter | ProfileType::Merger => {
                if let Some((key, value)) = parameters.get(&["output_format", "format"])? {
                    value.trim().parse::<VideoFormat>().map_err(|_| {
                        parameter_error(profile, key, value, "unsupported video output format")
                    })?
                } else {
                    VideoFormat::MP4
                }
            }
            _ => VideoFormat::MP4,
        };

        let operation = match &profile.profile_type {
            ProfileType::Clipper => {
                let (key, value) = parameters.required(&["time_ranges", "ranges"])?;
                let mut time_ranges = Vec::new();
                for group in value.split(',') {
                    if group.trim().is_empty() {
                        return Err(parameter_error(
                            profile,
                            key,
                            value,
                            "expected one or more START-END ranges separated by commas or spaces",
                        ));
                    }
                    for range in group.split_whitespace() {
                        time_ranges.push(validate_time_range(range).ok_or_else(|| {
                            parameter_error(
                                profile,
                                key,
                                value,
                                &format!("invalid START-END time range '{range}'"),
                            )
                        })?);
                    }
                }
                let suffix = parameters.optional(&["suffix"])?;
                if let Some(suffix) = &suffix {
                    validate_filename_component(profile, "suffix", suffix)?;
                }
                ProfileOperationConfig::Clipper(BatchClipperConfig {
                    time_ranges,
                    output_dir: parameters.directory("output_clips")?,
                    copy_codec: parameters.boolean(&["copy_codec"], false)?,
                    suffix,
                })
            }
            ProfileType::GifConverter => {
                let width = parameters
                    .get(&["width"])?
                    .filter(|(_, value)| !value.trim().is_empty())
                    .map(|(key, value)| parameters.positive_integer(key, value))
                    .transpose()?;
                let fps = match parameters.get(&["fps"])? {
                    Some((key, value)) => parameters.positive_integer(key, value)?,
                    None => 10,
                };
                let max_size_mb = match parameters.get(&["max_size_mb", "max_size"])? {
                    Some((key, value)) => value
                        .trim()
                        .parse::<f64>()
                        .ok()
                        .filter(|size| size.is_finite() && *size > 0.0)
                        .ok_or_else(|| {
                            parameter_error(
                                profile,
                                key,
                                value,
                                "expected a positive finite number",
                            )
                        })?,
                    None => 5.0,
                };
                ProfileOperationConfig::GifConverter(BatchGifConverterConfig {
                    width,
                    fps,
                    max_size_mb,
                    optimize: parameters.boolean(&["optimize"], false)?,
                    output_dir: parameters.directory("output_gifs")?,
                })
            }
            ProfileType::GifTransparency => {
                ProfileOperationConfig::GifTransparency(BatchGifTransparencyConfig {
                    create_backup: parameters.boolean(&["create_backup", "backup"], false)?,
                })
            }
            ProfileType::Splitter => {
                let prefix = match parameters.get(&["prefix"])? {
                    Some((key, value)) => {
                        validate_filename_component(profile, key, value.trim())?;
                        value.trim().to_string()
                    }
                    None => "slice".to_string(),
                };
                ProfileOperationConfig::Splitter(BatchSplitterConfig {
                    output_dir: parameters.directory("output_slices")?,
                    prefix,
                    custom_encode: parameters.optional(&["encode_options", "custom_encode"])?,
                    force: parameters.boolean(&["force"], false)?,
                })
            }
            ProfileType::Merger => {
                let (_, audio) = parameters.required(&["audio_file", "audio"])?;
                ProfileOperationConfig::Merger(BatchMergerConfig {
                    audio_file: PathBuf::from(audio.trim()),
                    output_dir: parameters.directory("output_merged")?,
                    use_shortest: parameters.boolean(&["use_shortest", "shortest"], false)?,
                    copy_codec: parameters.boolean(&["copy_codec"], false)?,
                })
            }
            ProfileType::Custom(_) => unreachable!("custom types were rejected above"),
        };
        Ok(Self {
            operation,
            output_format,
            recursive,
            parallel,
            pattern,
        })
    }

    /// Convert validated settings into the shared execution engine.
    pub fn into_processor(self) -> Result<BatchProcessor> {
        let processor = match self.operation {
            ProfileOperationConfig::Clipper(config) => {
                BatchProcessor::new(BatchOperation::Clipper).with_clipper_config(config)
            }
            ProfileOperationConfig::GifConverter(config) => {
                BatchProcessor::new(BatchOperation::GifConverter).with_gif_converter_config(config)
            }
            ProfileOperationConfig::GifTransparency(config) => {
                BatchProcessor::new(BatchOperation::GifTransparency)
                    .with_gif_transparency_config(config)
            }
            ProfileOperationConfig::Splitter(config) => {
                BatchProcessor::new(BatchOperation::Splitter).with_splitter_config(config)
            }
            ProfileOperationConfig::Merger(config) => {
                BatchProcessor::new(BatchOperation::Merger).with_merger_config(config)
            }
        }
        .with_output_format(self.output_format)
        .with_recursive(self.recursive)
        .with_parallel(self.parallel);
        match self.pattern {
            Some(pattern) => processor.with_pattern(&pattern),
            None => Ok(processor),
        }
    }

    /// Override the output directory without creating it.
    pub fn with_output_directory(mut self, dir: PathBuf) -> Result<Self> {
        if dir.as_os_str().to_string_lossy().trim().is_empty() {
            return Err(BatchError::InvalidOperation(
                "Profile batch parameter 'output_dir' value '' must not be blank".to_string(),
            ));
        }
        match &mut self.operation {
            ProfileOperationConfig::Clipper(config) => config.output_dir = dir,
            ProfileOperationConfig::GifConverter(config) => config.output_dir = dir,
            ProfileOperationConfig::Splitter(config) => config.output_dir = dir,
            ProfileOperationConfig::Merger(config) => config.output_dir = dir,
            ProfileOperationConfig::GifTransparency(_) => {
                return Err(BatchError::InvalidOperation(format!(
                    "Profile batch parameter 'output_dir' value '{}' is not supported for GIF transparency; it edits the original GIF files",
                    dir.display()
                )));
            }
        }
        Ok(self)
    }
}

impl BatchProcessor {
    /// Load a saved profile's settings into a configured batch processor.
    pub fn from_profile(profile: &Profile) -> Result<Self> {
        ProfileBatchConfig::from_profile(profile)?.into_processor()
    }
}

/// Editable profile templates shared by the CLI and desktop profile editor.
/// Empty values represent optional fields, except for the required merger audio path.
pub fn default_profile_parameters(profile_type: &ProfileType) -> Vec<(String, String)> {
    let operation: &[(&str, &str)] = match profile_type {
        ProfileType::Clipper => &[
            ("ranges", "0-00:15"),
            ("output_dir", "output_clips"),
            ("copy_codec", "false"),
            ("suffix", ""),
            ("format", "mp4"),
        ],
        ProfileType::GifConverter => &[
            ("width", ""),
            ("fps", "10"),
            ("max_size", "5.0"),
            ("optimize", "false"),
            ("output_dir", "output_gifs"),
        ],
        ProfileType::GifTransparency => &[("backup", "false")],
        ProfileType::Splitter => &[
            ("output_dir", "output_slices"),
            ("prefix", "slice"),
            ("custom_encode", ""),
            ("force", "false"),
            ("format", "mp4"),
        ],
        ProfileType::Merger => &[
            ("audio", ""),
            ("output_dir", "output_merged"),
            ("shortest", "false"),
            ("copy_codec", "false"),
            ("format", "mp4"),
        ],
        ProfileType::Custom(_) => return Vec::new(),
    };
    operation
        .iter()
        .chain(
            [
                ("recursive", "false"),
                ("parallel", "true"),
                ("pattern", ""),
            ]
            .iter(),
        )
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

struct Parameters<'a>(&'a Profile);

impl<'a> Parameters<'a> {
    fn get(&self, keys: &[&'a str]) -> Result<Option<(&'a str, &'a str)>> {
        let mut present = keys.iter().filter_map(|key| {
            self.0
                .parameters
                .get(*key)
                .map(|value| (*key, value.as_str()))
        });
        let first = present.next();
        if let (Some((key, value)), Some((alias, alias_value))) = (first, present.next()) {
            return Err(parameter_error(
                self.0,
                key,
                value,
                &format!("ambiguous with alias '{alias}' value '{alias_value}'; supply only one"),
            ));
        }
        Ok(first)
    }

    fn required(&self, keys: &[&'a str]) -> Result<(&'a str, &'a str)> {
        let (key, value) = self.get(keys)?.ok_or_else(|| {
            parameter_error(
                self.0,
                keys[0],
                "<missing>",
                "required parameter is missing",
            )
        })?;
        if value.trim().is_empty() {
            return Err(parameter_error(self.0, key, value, "must not be blank"));
        }
        Ok((key, value))
    }

    fn optional(&self, keys: &[&'a str]) -> Result<Option<String>> {
        Ok(self
            .get(keys)?
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
            .map(String::from))
    }

    fn boolean(&self, keys: &[&'a str], default: bool) -> Result<bool> {
        match self.get(keys)? {
            Some((_, value)) if value.trim().eq_ignore_ascii_case("true") => Ok(true),
            Some((_, value)) if value.trim().eq_ignore_ascii_case("false") => Ok(false),
            Some((key, value)) => Err(parameter_error(
                self.0,
                key,
                value,
                "expected true or false",
            )),
            None => Ok(default),
        }
    }

    fn positive_integer(&self, key: &str, value: &str) -> Result<u32> {
        value
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| parameter_error(self.0, key, value, "expected a positive integer"))
    }

    fn directory(&self, default: &str) -> Result<PathBuf> {
        match self.get(&["output_dir"])? {
            Some((key, value)) if value.trim().is_empty() => {
                Err(parameter_error(self.0, key, value, "must not be blank"))
            }
            Some((_, value)) => Ok(PathBuf::from(value.trim())),
            None => Ok(PathBuf::from(default)),
        }
    }
}

fn parameter_error(profile: &Profile, key: &str, value: &str, reason: &str) -> BatchError {
    BatchError::InvalidOperation(format!(
        "Profile '{}' parameter '{}' value '{}': {}",
        profile.name, key, value, reason
    ))
}

fn validate_filename_component(profile: &Profile, key: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value
            .chars()
            .any(|ch| ch.is_control() || "/\\:<>\"|?*".contains(ch))
    {
        return Err(parameter_error(
            profile,
            key,
            value,
            "expected a nonempty filename component without path separators or control characters",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn profile(profile_type: ProfileType, parameters: &[(&str, &str)]) -> Profile {
        Profile::new(
            "batch-test",
            profile_type,
            parameters
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        )
    }

    fn error(profile_type: ProfileType, parameters: &[(&str, &str)], key: &str, value: &str) {
        let error = ProfileBatchConfig::from_profile(&profile(profile_type, parameters))
            .unwrap_err()
            .to_string();
        assert!(error.contains("batch-test"), "{error}");
        assert!(error.contains(key), "{error}");
        assert!(error.contains(value), "{error}");
    }

    #[test]
    fn clipper_profile_dispatches_all_settings() {
        let processor = BatchProcessor::from_profile(&profile(
            ProfileType::Clipper,
            &[
                ("time_ranges", "0-00:15, 20-30 00:40-00:45"),
                ("output_dir", "clips"),
                ("copy_codec", "TRUE"),
                ("suffix", "preview"),
                ("output_format", "mkv"),
                ("recursive", "true"),
                ("parallel", "false"),
                ("pattern", r"^holiday.*\.mov$"),
            ],
        ))
        .unwrap();
        assert_eq!(processor.operation, BatchOperation::Clipper);
        assert_eq!(processor.output_format, VideoFormat::MKV);
        assert!(processor.recursive);
        assert!(!processor.parallel);
        assert!(processor
            .input_pattern
            .as_ref()
            .unwrap()
            .is_match("holiday.mov"));
        let config = processor.clipper_config.unwrap();
        assert_eq!(
            config.time_ranges,
            [("0", "00:15"), ("20", "30"), ("00:40", "00:45")]
                .map(|(start, end)| (start.to_string(), end.to_string()))
        );
        assert_eq!(config.output_dir, PathBuf::from("clips"));
        assert!(config.copy_codec);
        assert_eq!(config.suffix.as_deref(), Some("preview"));
    }

    #[test]
    fn literal_spaces_in_regex_are_preserved_and_do_not_disable_filtering() {
        let processor = BatchProcessor::from_profile(&profile(
            ProfileType::GifTransparency,
            &[("pattern", " one")],
        ))
        .unwrap();
        let pattern = processor.input_pattern.unwrap();
        assert!(!pattern.is_match("one.gif"));
        assert!(pattern.is_match("prefix one.gif"));

        let processor = BatchProcessor::from_profile(&profile(
            ProfileType::GifTransparency,
            &[("pattern", " ")],
        ))
        .unwrap();
        let pattern = processor.input_pattern.unwrap();
        assert!(!pattern.is_match("one.gif"));
        assert!(pattern.is_match("one two.gif"));
    }

    #[test]
    fn gif_converter_profile_dispatches_all_settings() {
        let processor = BatchProcessor::from_profile(&profile(
            ProfileType::GifConverter,
            &[
                ("width", "320"),
                ("fps", "12"),
                ("max_size_mb", "1.5"),
                ("optimize", "True"),
                ("output_dir", "gifs"),
            ],
        ))
        .unwrap();
        assert_eq!(processor.operation, BatchOperation::GifConverter);
        let config = processor.gif_converter_config.unwrap();
        assert_eq!(config.width, Some(320));
        assert_eq!(config.fps, 12);
        assert_eq!(config.max_size_mb, 1.5);
        assert!(config.optimize);
        assert_eq!(config.output_dir, PathBuf::from("gifs"));
    }

    #[test]
    fn transparency_profile_dispatches_backup() {
        let processor = BatchProcessor::from_profile(&profile(
            ProfileType::GifTransparency,
            &[("create_backup", "true")],
        ))
        .unwrap();
        assert_eq!(processor.operation, BatchOperation::GifTransparency);
        assert!(processor.gif_transparency_config.unwrap().create_backup);
    }

    #[test]
    fn splitter_profile_dispatches_all_settings() {
        let processor = BatchProcessor::from_profile(&profile(
            ProfileType::Splitter,
            &[
                ("output_dir", "slices"),
                ("prefix", "tile"),
                ("encode_options", "-crf 18 -preset slow"),
                ("force", "true"),
                ("format", "webm"),
            ],
        ))
        .unwrap();
        assert_eq!(processor.operation, BatchOperation::Splitter);
        assert_eq!(processor.output_format, VideoFormat::WebM);
        let config = processor.splitter_config.unwrap();
        assert_eq!(config.output_dir, PathBuf::from("slices"));
        assert_eq!(config.prefix, "tile");
        assert_eq!(
            config.custom_encode.as_deref(),
            Some("-crf 18 -preset slow")
        );
        assert!(config.force);
    }

    #[test]
    fn merger_profile_dispatches_all_settings_without_checking_audio_exists() {
        let processor = BatchProcessor::from_profile(&profile(
            ProfileType::Merger,
            &[
                ("audio_file", "missing soundtrack.wav"),
                ("output_dir", "merged"),
                ("use_shortest", "true"),
                ("copy_codec", "true"),
                ("format", "mov"),
            ],
        ))
        .unwrap();
        assert_eq!(processor.operation, BatchOperation::Merger);
        assert_eq!(processor.output_format, VideoFormat::MOV);
        let config = processor.merger_config.unwrap();
        assert_eq!(config.audio_file, PathBuf::from("missing soundtrack.wav"));
        assert_eq!(config.output_dir, PathBuf::from("merged"));
        assert!(config.use_shortest);
        assert!(config.copy_codec);
    }

    #[test]
    fn minimal_profiles_use_documented_defaults() {
        let clip =
            ProfileBatchConfig::from_profile(&profile(ProfileType::Clipper, &[("ranges", "0-1")]))
                .unwrap();
        assert_eq!(clip.output_format, VideoFormat::MP4);
        assert!(!clip.recursive);
        assert!(clip.parallel);
        assert!(clip.pattern.is_none());
        let ProfileOperationConfig::Clipper(config) = clip.operation else {
            panic!("wrong operation")
        };
        assert_eq!(config.output_dir, PathBuf::from("output_clips"));
        assert!(!config.copy_codec);
        assert!(config.suffix.is_none());

        let gif = BatchProcessor::from_profile(&profile(ProfileType::GifConverter, &[])).unwrap();
        let config = gif.gif_converter_config.unwrap();
        assert!(config.width.is_none());
        assert_eq!(config.fps, 10);
        assert_eq!(config.max_size_mb, 5.0);
        assert!(!config.optimize);
        assert_eq!(config.output_dir, PathBuf::from("output_gifs"));

        let transparent =
            BatchProcessor::from_profile(&profile(ProfileType::GifTransparency, &[])).unwrap();
        assert!(!transparent.gif_transparency_config.unwrap().create_backup);

        let split = BatchProcessor::from_profile(&profile(ProfileType::Splitter, &[])).unwrap();
        let config = split.splitter_config.unwrap();
        assert_eq!(config.output_dir, PathBuf::from("output_slices"));
        assert_eq!(config.prefix, "slice");
        assert!(config.custom_encode.is_none());
        assert!(!config.force);

        let merge =
            BatchProcessor::from_profile(&profile(ProfileType::Merger, &[("audio", "sound.wav")]))
                .unwrap();
        let config = merge.merger_config.unwrap();
        assert_eq!(config.output_dir, PathBuf::from("output_merged"));
        assert!(!config.use_shortest);
        assert!(!config.copy_codec);
    }

    #[test]
    fn editor_defaults_round_trip_except_required_audio_placeholder() {
        for kind in [
            ProfileType::Clipper,
            ProfileType::GifConverter,
            ProfileType::GifTransparency,
            ProfileType::Splitter,
            ProfileType::Merger,
        ] {
            let mut parameters: HashMap<_, _> =
                default_profile_parameters(&kind).into_iter().collect();
            if kind == ProfileType::Merger {
                assert_eq!(parameters["audio"], "");
                let missing_audio = Profile::new("template", kind.clone(), parameters.clone());
                assert!(ProfileBatchConfig::from_profile(&missing_audio).is_err());
                parameters.insert("audio".to_string(), "sound.wav".to_string());
            }
            let template = Profile::new("template", kind, parameters);
            assert!(ProfileBatchConfig::from_profile(&template).is_ok());
        }
        assert!(default_profile_parameters(&ProfileType::Custom("effect".into())).is_empty());
    }

    #[test]
    fn aliases_are_supported_and_duplicate_aliases_are_rejected() {
        for (kind, first, second, value, required) in [
            (ProfileType::Clipper, "time_ranges", "ranges", "0-1", None),
            (
                ProfileType::GifConverter,
                "max_size_mb",
                "max_size",
                "2.0",
                None,
            ),
            (
                ProfileType::GifTransparency,
                "create_backup",
                "backup",
                "true",
                None,
            ),
            (
                ProfileType::Merger,
                "audio_file",
                "audio",
                "sound.wav",
                None,
            ),
            (
                ProfileType::Merger,
                "use_shortest",
                "shortest",
                "true",
                Some(("audio", "sound.wav")),
            ),
            (
                ProfileType::Splitter,
                "output_format",
                "format",
                "mkv",
                None,
            ),
            (
                ProfileType::Splitter,
                "encode_options",
                "custom_encode",
                "-crf 18",
                None,
            ),
        ] {
            for key in [first, second] {
                let mut values = vec![(key, value)];
                values.extend(required);
                assert!(ProfileBatchConfig::from_profile(&profile(kind.clone(), &values)).is_ok());
            }
            let mut values = vec![(first, value), (second, value)];
            values.extend(required);
            error(kind, &values, first, value);
        }
    }

    #[test]
    fn rejects_invalid_numbers_and_booleans() {
        for key in ["width", "fps"] {
            for value in ["0", "-1", "1.5", "NaN", "4294967296", "bad"] {
                error(ProfileType::GifConverter, &[(key, value)], key, value);
            }
        }
        for value in ["0", "-1", "NaN", "inf", "-inf", "bad"] {
            error(
                ProfileType::GifConverter,
                &[("max_size", value)],
                "max_size",
                value,
            );
        }
        for key in ["recursive", "parallel", "optimize"] {
            for value in ["yes", "1", "", "falsee"] {
                error(ProfileType::GifConverter, &[(key, value)], key, value);
            }
        }
    }

    #[test]
    fn rejects_missing_blank_or_malformed_required_parameters() {
        error(ProfileType::Clipper, &[], "time_ranges", "<missing>");
        error(ProfileType::Merger, &[], "audio_file", "<missing>");
        for value in ["", " "] {
            error(ProfileType::Clipper, &[("ranges", value)], "ranges", value);
            error(ProfileType::Merger, &[("audio", value)], "audio", value);
            error(
                ProfileType::Splitter,
                &[("output_dir", value)],
                "output_dir",
                value,
            );
            error(ProfileType::Splitter, &[("prefix", value)], "prefix", value);
        }
        for value in [
            "0",
            "1-0",
            "0-0",
            "0-bad",
            "0-1,,2-3",
            "0-1,",
            "0-1 invalid",
            "-1-3",
        ] {
            error(ProfileType::Clipper, &[("ranges", value)], "ranges", value);
        }
    }

    #[test]
    fn rejects_unknown_keys_invalid_formats_regex_and_custom_execution() {
        error(
            ProfileType::GifConverter,
            &[("format", "mp4")],
            "format",
            "mp4",
        );
        error(
            ProfileType::GifTransparency,
            &[("output_dir", "out")],
            "output_dir",
            "out",
        );
        error(
            ProfileType::Splitter,
            &[("prefixx", "tile")],
            "prefixx",
            "tile",
        );
        error(ProfileType::Splitter, &[("format", "gif")], "format", "gif");
        error(ProfileType::Splitter, &[("pattern", "[")], "pattern", "[");
        error(
            ProfileType::Custom("filter".into()),
            &[],
            "profile_type",
            "filter",
        );
    }

    #[test]
    fn filename_components_cannot_escape_output_directory() {
        for value in [
            "..", ".", "../clip", "a/b", r"a\b", "C:clip", "a\nclip", "a\0clip",
        ] {
            error(ProfileType::Splitter, &[("prefix", value)], "prefix", value);
            error(
                ProfileType::Clipper,
                &[("ranges", "0-1"), ("suffix", value)],
                "suffix",
                value,
            );
        }
    }

    #[test]
    fn parsing_and_overrides_do_not_create_output_directories() {
        static NEXT_PATH: AtomicU64 = AtomicU64::new(0);
        let output = std::env::temp_dir().join(format!(
            "video-toolkit-profile-parser-{}-{}",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(!output.exists());
        for kind in [
            ProfileType::Clipper,
            ProfileType::GifConverter,
            ProfileType::Splitter,
            ProfileType::Merger,
        ] {
            let mut parameters: HashMap<_, _> =
                default_profile_parameters(&kind).into_iter().collect();
            parameters.insert("output_dir".into(), output.to_string_lossy().into_owned());
            if kind == ProfileType::Merger {
                parameters.insert("audio".into(), "not-yet-present.wav".into());
            }
            ProfileBatchConfig::from_profile(&Profile::new("side-effect-test", kind, parameters))
                .unwrap()
                .with_output_directory(output.join("override"))
                .unwrap()
                .into_processor()
                .unwrap();
            assert!(!output.exists());
        }
        let transparency =
            ProfileBatchConfig::from_profile(&profile(ProfileType::GifTransparency, &[])).unwrap();
        assert!(transparency.with_output_directory(output.clone()).is_err());
        let splitter =
            ProfileBatchConfig::from_profile(&profile(ProfileType::Splitter, &[])).unwrap();
        assert!(splitter.with_output_directory(PathBuf::from(" ")).is_err());
        assert!(!output.exists());
    }

    #[test]
    fn output_directory_override_is_forwarded_to_operation() {
        for kind in [
            ProfileType::Clipper,
            ProfileType::GifConverter,
            ProfileType::Splitter,
            ProfileType::Merger,
        ] {
            let mut parameters: HashMap<_, _> =
                default_profile_parameters(&kind).into_iter().collect();
            if kind == ProfileType::Merger {
                parameters.insert("audio".into(), "sound.wav".into());
            }
            let processor = ProfileBatchConfig::from_profile(&Profile::new(
                "override",
                kind.clone(),
                parameters,
            ))
            .unwrap()
            .with_output_directory(PathBuf::from("chosen-output"))
            .unwrap()
            .into_processor()
            .unwrap();
            let dir = match kind {
                ProfileType::Clipper => processor.clipper_config.unwrap().output_dir,
                ProfileType::GifConverter => processor.gif_converter_config.unwrap().output_dir,
                ProfileType::Splitter => processor.splitter_config.unwrap().output_dir,
                ProfileType::Merger => processor.merger_config.unwrap().output_dir,
                _ => unreachable!(),
            };
            assert_eq!(dir, PathBuf::from("chosen-output"));
        }
    }
}
