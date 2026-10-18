use clap::{Parser, Subcommand};
use eframe::NativeOptions;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use batch_processing::{BatchProcessor, ProfileBatchConfig};
use clipper::{clip_video_with_format, parse_time_ranges};
use common::{check_ffmpeg, formats::*, get_supported_formats};
use gif_converter::{convert_video_to_gif, optimize_conversion_with_fps};
use gif_transparency::{batch_process_gifs, process_directory};
use merger::merge_audio_video_with_format;
use plugin_system::PluginManager;
use profile_system::{Profile, ProfileManager, ProfileType};
use splitter::split_video_with_format;
use ui::VideoToolKitApp;

#[derive(Parser)]
#[clap(author, version, about = "Video processing utilities")]
struct Cli {
    /// Use a custom profile storage directory (the directory containing type folders)
    #[clap(long, global = true)]
    profiles_dir: Option<PathBuf>,

    #[clap(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Open the local Video Agent desktop (vision selection, human review, safe export)
    Agent {
        /// Resume a previously saved Video Agent task directory
        #[clap(long)]
        workspace: Option<PathBuf>,
    },

    /// Inspect and exercise persistent Video Agent tasks without the desktop
    #[clap(version = env!("CARGO_PKG_VERSION"), propagate_version = true)]
    AgentTask(AgentTaskOptions),

    /// Extract specific time ranges from video files
    Clipper {
        /// Input video file path
        input: String,

        /// Time ranges to extract in format START-END (e.g., 00:01:00-00:02:00)
        #[clap(short, long, required = true, num_args = 1..)]
        ranges: Vec<String>,

        /// Output directory for video clips
        #[clap(short, long, default_value = "output_clips")]
        output_dir: String,

        /// Copy codec instead of re-encoding (faster but may be less precise)
        #[clap(long)]
        copy_codec: bool,

        /// Optional suffix to add to output filenames
        #[clap(short, long)]
        suffix: Option<String>,

        /// Output format (e.g., mp4, mkv, avi)
        #[clap(long, default_value = "mp4")]
        format: VideoFormat,
    },

    /// Convert videos (MP4, WebM, etc.) to optimized GIF format
    GifConverter {
        /// Input video file path (supports MP4, WebM, AVI, MOV, MKV)
        input: String,

        /// Output GIF file path
        #[clap(short, long)]
        output: Option<String>,

        /// Width to resize to (height will be adjusted automatically)
        #[clap(short, long)]
        width: Option<u32>,

        /// Frames per second for the output GIF
        #[clap(short, long, default_value = "10")]
        fps: u32,

        /// Maximum size of output GIF in MB
        #[clap(short, long, default_value = "5.0")]
        max_size: f64,

        /// Try multiple settings to achieve size target
        #[clap(long)]
        optimize: bool,
    },

    /// Make the GIF logical background color transparent
    GifTransparency {
        /// Input GIF files or directories
        #[clap(required = true)]
        inputs: Vec<PathBuf>,

        /// Process directories recursively
        #[clap(short, long)]
        recursive: bool,

        /// Create backup of original files
        #[clap(short, long)]
        backup: bool,
    },

    /// Make all GIFs in a directory transparent
    GifTransparencyDir {
        /// Directory containing GIF files
        directory: String,

        /// Process directories recursively
        #[clap(short, long)]
        recursive: bool,

        /// Create backup of original files
        #[clap(short, long)]
        backup: bool,
    },

    /// Split a video into equal vertical slices
    Splitter {
        /// Input video file path
        input: String,

        /// Output directory for video slices
        #[clap(short, long, default_value = "output_slices")]
        output_dir: String,

        /// Prefix for output filenames
        #[clap(short, long, default_value = "slice")]
        prefix: String,

        /// Custom FFmpeg encoding options (advanced users only)
        #[clap(long, allow_hyphen_values = true)]
        custom_encode: Option<String>,

        /// Process even if video dimensions are not 1920x1080
        #[clap(long)]
        force: bool,

        /// Output format (e.g., mp4, mkv, avi)
        #[clap(long, default_value = "mp4")]
        format: VideoFormat,
    },

    /// Merge video with audio
    Merger {
        /// Input video file path
        video: String,

        /// Input audio file path
        audio: String,

        /// Output file path
        #[clap(short, long)]
        output: String,

        /// End when shortest input stream ends
        #[clap(long)]
        shortest: bool,

        /// Copy codec without re-encoding (faster)
        #[clap(long)]
        copy_codec: bool,

        /// Output format (inferred from --output when omitted)
        #[clap(long)]
        format: Option<VideoFormat>,
    },

    /// Manage plugins
    #[clap(subcommand)]
    Plugin(PluginCommands),

    /// Manage profiles
    #[clap(subcommand)]
    Profile(ProfileCommands),

    /// Batch process multiple files
    #[clap(subcommand)]
    Batch(BatchCommands),

    /// List supported formats
    Formats {
        /// Operation to show formats for
        #[clap(long)]
        operation: Option<String>,
    },
}

#[derive(clap::Args)]
struct AgentTaskOptions {
    /// Emit one video-agent-cli/v1 JSON response, including structured errors
    #[clap(long, global = true)]
    json: bool,

    #[clap(subcommand)]
    command: AgentTaskCommands,
}

#[derive(Subcommand)]
enum AgentTaskCommands {
    /// List implemented Agent capabilities and boundaries without opening a task or calling tools
    Discover,

    /// Fingerprint local sources and create a private task; does not upload or call a model
    Create {
        #[clap(required = true, num_args = 1..)]
        inputs: Vec<PathBuf>,
        #[clap(long)]
        workspace_root: PathBuf,
        #[clap(long)]
        intent: String,
        #[clap(long, default_value = "30")]
        target_seconds: u64,
        /// Simultaneous local frame extractions; cloud requests and export stay sequential
        #[clap(long, visible_alias = "max-concurrency", default_value = "1", value_parser = clap::value_parser!(u8).range(1..=4))]
        frame_workers: u8,
    },
    /// Show current state, including exact plan digest; identifies changed inputs
    Show { workspace: PathBuf },
    /// Extract bounded timestamped JPEG samples, reusing completed samples
    Sample { workspace: PathBuf },
    /// Call the real OpenAI vision backend; requires credentials and explicit consent
    Analyze {
        workspace: PathBuf,
        #[clap(long)]
        allow_cloud: bool,
    },
    /// Create an explicitly manual draft for testing/review; NOT model selection
    ManualDraft { workspace: PathBuf },
    /// Save a human-edited plan JSON; always invalidates previous approval
    Edit {
        workspace: PathBuf,
        #[clap(long)]
        plan: PathBuf,
    },
    /// Confirm exactly the displayed plan digest, not a newer unseen version
    Approve {
        workspace: PathBuf,
        #[clap(long)]
        plan_digest: String,
    },
    /// Export the approved plan; reuses verified completed shards after interruption
    Export { workspace: PathBuf },

    /// Validate the current or supplied edit plan without saving or approving it
    Validate {
        workspace: PathBuf,
        /// Optional bounded EditPlan JSON file; omitted means the current saved plan
        #[clap(long)]
        plan: Option<PathBuf>,
    },
    /// Render a task-owned, unapproved whole-plan preview and objective checks
    Preview { workspace: PathBuf },
    /// Recheck objective preview quality; this is not a semantic or human score
    Quality { workspace: PathBuf },
    /// Inspect the latest preview record; does not approve or export it
    PreviewLatest { workspace: PathBuf },
    /// Read the local feedback trajectory; may contain private task metadata
    Trajectory { workspace: PathBuf },
    /// Preview the saved plan, then bounded supplied candidates; no cloud calls or approval
    Iterate {
        workspace: PathBuf,
        /// EditPlan JSON array; at most max-attempts minus one candidates (64 KiB total)
        #[clap(long)]
        candidates: PathBuf,
        #[clap(long, default_value = "3", value_parser = clap::value_parser!(u32).range(1..=3))]
        max_attempts: u32,
    },
    /// Render, sparsely review and revise previews with explicit cloud-upload consent
    SmartIterate {
        workspace: PathBuf,
        #[clap(long, default_value = "3", value_parser = clap::value_parser!(u32).range(1..=3))]
        max_rounds: u32,
        /// Allow rendered-preview JPEGs and bounded source samples to reach the model
        #[clap(long)]
        consent_preview_upload: bool,
    },
    /// Read persisted AI preview feedback; does not call a model or reverify media
    SmartStatus { workspace: PathBuf },
    /// Explicitly adopt this run's model candidate; still requires separate approval/export
    SmartAdopt {
        workspace: PathBuf,
        #[clap(long)]
        run_id: String,
    },
}

#[derive(Subcommand)]
enum PluginCommands {
    /// List available plugins
    List,

    /// Load a plugin from a file
    Load {
        /// Path to the plugin file
        path: String,
    },

    /// Run a plugin with parameters
    Run {
        /// Name of the plugin to run
        name: String,

        /// Parameters to pass to the plugin (key=value)
        #[clap(short, long)]
        params: Vec<String>,
    },

    /// Discover and load plugins from the default plugin directory
    Discover,
}

#[derive(Subcommand)]
enum ProfileCommands {
    /// List available profiles
    List {
        /// Profile type to list
        #[clap(long)]
        profile_type: Option<String>,
    },

    /// Show a specific profile
    Show {
        /// Name of the profile
        name: String,

        /// Type of the profile
        #[clap(long, required = true)]
        profile_type: String,
    },

    /// Create a new profile
    Create {
        /// Name of the profile
        name: String,

        /// Type of the profile
        #[clap(long, required = true)]
        profile_type: String,

        /// Description of the profile
        #[clap(long)]
        description: Option<String>,

        /// Parameters for the profile (key=value)
        #[clap(short, long, num_args = 1..)]
        params: Vec<String>,
    },

    /// Delete a profile
    Delete {
        /// Name of the profile
        name: String,

        /// Type of the profile
        #[clap(long, required = true)]
        profile_type: String,
    },

    /// Import a profile from a file
    Import {
        /// Path to the profile file
        path: String,
    },

    /// Export a profile to a file
    Export {
        /// Name of the profile
        name: String,

        /// Type of the profile
        #[clap(long, required = true)]
        profile_type: String,

        /// Path to export the profile to
        #[clap(short, long, required = true)]
        output: String,
    },
}

#[derive(Subcommand)]
enum BatchCommands {
    /// Batch process files with the clipper
    Clipper {
        /// Input files or directories
        #[clap(required = true)]
        inputs: Vec<PathBuf>,

        /// Process directories recursively
        #[clap(short, long)]
        recursive: bool,

        /// File pattern to match (regex)
        #[clap(short, long)]
        pattern: Option<String>,

        /// Output directory
        #[clap(short, long, default_value = "output_clips")]
        output_dir: String,

        /// Time ranges to extract (START-END)
        #[clap(short = 't', long, required = true, num_args = 1..)]
        ranges: Vec<String>,

        /// Copy codec instead of re-encoding
        #[clap(long)]
        copy_codec: bool,

        /// Optional suffix to add to output filenames
        #[clap(short, long)]
        suffix: Option<String>,

        /// Process files in parallel
        #[clap(long, default_value = "true")]
        parallel: bool,

        /// Output format (e.g., mp4, mkv, avi)
        #[clap(long, default_value = "mp4")]
        format: VideoFormat,
    },

    /// Batch convert videos to GIF
    GifConverter {
        /// Input files or directories
        #[clap(required = true)]
        inputs: Vec<PathBuf>,

        /// Process directories recursively
        #[clap(short, long)]
        recursive: bool,

        /// File pattern to match (regex)
        #[clap(short, long)]
        pattern: Option<String>,

        /// Output directory
        #[clap(short, long, default_value = "output_gifs")]
        output_dir: String,

        /// Width to resize to (height adjusted automatically)
        #[clap(short, long)]
        width: Option<u32>,

        /// Frames per second
        #[clap(short, long, default_value = "10")]
        fps: u32,

        /// Maximum size in MB
        #[clap(short, long, default_value = "5.0")]
        max_size: f64,

        /// Try multiple settings to achieve size target
        #[clap(long)]
        optimize: bool,

        /// Process files in parallel
        #[clap(long, default_value = "true")]
        parallel: bool,
    },

    /// Batch process GIFs for transparency
    GifTransparency {
        /// Input files or directories
        #[clap(required = true)]
        inputs: Vec<PathBuf>,

        /// Process directories recursively
        #[clap(short, long)]
        recursive: bool,

        /// File pattern to match (regex)
        #[clap(short, long)]
        pattern: Option<String>,

        /// Create backup of original files
        #[clap(short, long)]
        backup: bool,

        /// Process files in parallel
        #[clap(long, default_value = "true")]
        parallel: bool,
    },

    /// Batch split videos
    Splitter {
        /// Input files or directories
        #[clap(required = true)]
        inputs: Vec<PathBuf>,

        /// Process directories recursively
        #[clap(short, long)]
        recursive: bool,

        /// File pattern to match (regex)
        #[clap(short, long)]
        pattern: Option<String>,

        /// Output directory
        #[clap(short, long, default_value = "output_slices")]
        output_dir: String,

        /// Prefix for output filenames
        #[clap(short = 'n', long, default_value = "slice")]
        prefix: String,

        /// Custom FFmpeg encoding options
        #[clap(long, allow_hyphen_values = true)]
        custom_encode: Option<String>,

        /// Process even if video dimensions are not 1920x1080
        #[clap(long)]
        force: bool,

        /// Process files in parallel
        #[clap(long, default_value = "true")]
        parallel: bool,

        /// Output format (e.g., mp4, mkv, avi)
        #[clap(long, default_value = "mp4")]
        format: VideoFormat,
    },

    /// Batch merge videos with audio
    Merger {
        /// Input video files or directories
        #[clap(required = true)]
        inputs: Vec<PathBuf>,

        /// Process directories recursively
        #[clap(short, long)]
        recursive: bool,

        /// File pattern to match (regex)
        #[clap(short, long)]
        pattern: Option<String>,

        /// Input audio file to use for all videos
        #[clap(long, required = true)]
        audio: PathBuf,

        /// Output directory
        #[clap(short, long, default_value = "output_merged")]
        output_dir: String,

        /// End when shortest input stream ends
        #[clap(long)]
        shortest: bool,

        /// Copy codec without re-encoding
        #[clap(long)]
        copy_codec: bool,

        /// Process files in parallel
        #[clap(long, default_value = "true")]
        parallel: bool,

        /// Output format (e.g., mp4, mkv, avi)
        #[clap(long, default_value = "mp4")]
        format: VideoFormat,
    },

    /// Use a profile for batch processing
    WithProfile {
        /// Input files or directories
        #[clap(required = true)]
        inputs: Vec<PathBuf>,

        /// Process directories recursively
        #[clap(short, long, num_args = 0..=1, default_missing_value = "true", require_equals = true)]
        recursive: Option<bool>,

        /// File pattern to match (regex)
        #[clap(short, long)]
        pattern: Option<String>,

        /// Name of the profile to use
        #[clap(long, required = true)]
        profile: String,

        /// Type of the profile
        #[clap(long, required = true)]
        profile_type: String,

        /// Override the profile's output directory (except in-place GIF transparency)
        #[clap(short, long)]
        output_dir: Option<PathBuf>,

        /// Override parallel processing; use --parallel=false to run sequentially
        #[clap(long, num_args = 0..=1, default_missing_value = "true", require_equals = true)]
        parallel: Option<bool>,
    },
}

impl Commands {
    fn requires_ffmpeg(&self) -> bool {
        matches!(
            self,
            Self::Clipper { .. }
                | Self::GifConverter { .. }
                | Self::Splitter { .. }
                | Self::Merger { .. }
        )
    }
}

fn main() -> Result<(), eframe::Error> {
    // Keep the machine-readable mode scoped to agent-task, including parse errors.
    let arguments: Vec<_> = std::env::args_os().collect();
    let cli = match Cli::try_parse_from(&arguments) {
        Ok(cli) => cli,
        Err(error) => {
            if !matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) && agent_json_requested(&arguments)
            {
                print_agent_json_error("parse", "Invalid", "Invalid Agent command arguments.");
                std::process::exit(2);
            }
            error.exit();
        }
    };

    // Metadata, profile storage, and native GIF operations do not need FFmpeg.
    // The batch processor checks availability for its selected operation.
    if cli.command.as_ref().is_some_and(Commands::requires_ffmpeg) && !check_ffmpeg() {
        eprintln!("Error: FFmpeg is not installed or not found in PATH. Please install FFmpeg.");
        std::process::exit(1);
    }

    // Run GUI if no subcommand is provided
    if cli.command.is_none() {
        let options = NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1080.0, 720.0])
                .with_min_inner_size([860.0, 600.0]),
            ..Default::default()
        };
        return eframe::run_native(
            "Video Toolkit",
            options,
            Box::new(move |cc| {
                let mut visuals = egui::Visuals::dark();
                visuals.widgets.active.bg_fill = egui::Color32::from_rgb(38, 125, 190);
                visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(45, 68, 86);
                visuals.selection.bg_fill = egui::Color32::from_rgb(29, 104, 156);
                cc.egui_ctx.set_visuals(visuals);
                Box::new(VideoToolKitApp::with_profiles_directory(
                    cli.profiles_dir.as_deref(),
                ))
            }),
        );
    }

    // Otherwise, run the appropriate command-line tool
    match cli.command.unwrap() {
        Commands::Agent { workspace } => {
            return ui::run_video_agent(workspace);
        }
        Commands::AgentTask(options) => {
            let command_name = options.command.name();
            let result = execute_agent_task(options.command);
            if options.json {
                match result {
                    Ok(output) => println!(
                        "{}",
                        serde_json::json!({
                            "schema": "video-agent-cli/v1", "ok": true,
                            "command": command_name, "result": output.result, "error": null
                        })
                    ),
                    Err(error) => {
                        let (code, message) = agent_error_contract(&error);
                        print_agent_json_error(command_name, code, message);
                        std::process::exit(1);
                    }
                }
            } else if let Err(error) = result.and_then(print_agent_text) {
                eprintln!("Video Agent: {error}");
                std::process::exit(1);
            }
        }
        Commands::Clipper {
            input,
            ranges,
            output_dir,
            copy_codec,
            suffix,
            format,
        } => {
            println!("Running clipper...");

            let time_ranges = parse_time_ranges(&ranges);
            if time_ranges.is_empty() || time_ranges.len() != ranges.len() {
                eprintln!("Error: Every time range must be valid and end after it starts.");
                std::process::exit(1);
            }

            match clip_video_with_format(
                &input,
                &time_ranges,
                &output_dir,
                copy_codec,
                suffix.as_deref(),
                format,
            ) {
                Ok(true) => println!("Successfully extracted all {} clip(s).", time_ranges.len()),
                Ok(false) => {
                    eprintln!("Completed with some errors.");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::GifConverter {
            input,
            output,
            width,
            fps,
            max_size,
            optimize,
        } => {
            println!("Running GIF converter...");

            // Determine output filename if not provided
            let output = match output {
                Some(o) => o,
                None => {
                    let input_path = std::path::Path::new(&input);
                    match input_path.file_stem() {
                        Some(stem) => {
                            let mut output_path = std::path::PathBuf::from(
                                input_path
                                    .parent()
                                    .unwrap_or_else(|| std::path::Path::new("")),
                            );
                            output_path.push(stem);
                            output_path.set_extension("gif");
                            output_path.to_string_lossy().to_string()
                        }
                        None => {
                            eprintln!("Error: Could not determine output filename.");
                            std::process::exit(1);
                        }
                    }
                }
            };

            let result = if optimize {
                optimize_conversion_with_fps(&input, &output, max_size, width, fps)
            } else {
                convert_video_to_gif(&input, &output, width, fps, max_size)
            };

            match result {
                Ok(true) => println!("Conversion successful! Output: {}", output),
                Ok(false) => {
                    eprintln!("Output file exceeds size limit (> {}MB).", max_size);
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::GifTransparency {
            inputs,
            recursive,
            backup,
        } => {
            println!("Processing GIF files for transparency...");

            match batch_process_gifs(&inputs, recursive, backup) {
                Ok((success_count, total_count)) => {
                    println!(
                        "Successfully processed {}/{} GIF files",
                        success_count, total_count
                    );
                    if success_count < total_count {
                        eprintln!(
                            "Failed to process {} GIF files",
                            total_count - success_count
                        );
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::GifTransparencyDir {
            directory,
            recursive,
            backup,
        } => {
            println!("Processing all GIFs in directory: {}", directory);

            match process_directory(&directory, recursive, backup) {
                Ok((success_count, total_count)) => {
                    println!(
                        "Successfully processed {}/{} GIF files",
                        success_count, total_count
                    );
                    if success_count < total_count {
                        eprintln!(
                            "Failed to process {} GIF files",
                            total_count - success_count
                        );
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Splitter {
            input,
            output_dir,
            prefix,
            custom_encode,
            force,
            format,
        } => {
            println!("Running video splitter...");

            match split_video_with_format(
                &input,
                &output_dir,
                &prefix,
                custom_encode.as_deref(),
                force,
                format,
            ) {
                Ok(true) => println!(
                    "Successfully split video into 5 slices. Files saved in: {}",
                    output_dir
                ),
                Ok(false) => {
                    eprintln!("Completed with some errors.");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Merger {
            video,
            audio,
            output,
            shortest,
            copy_codec,
            format,
        } => {
            println!("Running audio/video merger...");

            let (output, format) = match resolve_video_output(Path::new(&output), format) {
                Ok(resolved) => resolved,
                Err(error) => {
                    eprintln!("Error: {error}");
                    std::process::exit(1);
                }
            };
            let output = output.to_string_lossy();
            match merge_audio_video_with_format(
                &video, &audio, &output, shortest, copy_codec, format,
            ) {
                Ok(_) => println!("Successfully merged audio and video. Output: {}", output),
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Plugin(plugin_cmd) => {
            handle_plugin_command(plugin_cmd);
        }

        Commands::Profile(profile_cmd) => {
            handle_profile_command(profile_cmd, cli.profiles_dir.as_deref());
        }

        Commands::Batch(batch_cmd) => {
            handle_batch_command(batch_cmd, cli.profiles_dir.as_deref());
        }

        Commands::Formats { operation } => {
            if let Some(operation) = operation {
                let formats = get_supported_formats(&operation);
                if formats.is_empty() {
                    println!("No supported formats found for operation: {}", operation);
                } else {
                    println!(
                        "Supported formats for {}: {}",
                        operation,
                        formats.join(", ")
                    );
                }
            } else {
                // List all formats by category
                println!("Supported Video Formats:");
                for format in VideoFormat::all() {
                    println!("  .{} - {}", format.extension(), format.mime_type());
                }

                println!("\nSupported Audio Formats:");
                for format in AudioFormat::all() {
                    println!("  .{} - {}", format.extension(), format.mime_type());
                }

                println!("\nSupported Image Formats:");
                for format in ImageFormat::all() {
                    println!("  .{} - {}", format.extension(), format.mime_type());
                }
            }
        }
    }

    Ok(())
}

impl AgentTaskCommands {
    fn name(&self) -> &'static str {
        match self {
            Self::Discover => "discover",
            Self::Create { .. } => "create",
            Self::Show { .. } => "show",
            Self::Sample { .. } => "sample",
            Self::Analyze { .. } => "analyze",
            Self::ManualDraft { .. } => "manual-draft",
            Self::Edit { .. } => "edit",
            Self::Approve { .. } => "approve",
            Self::Export { .. } => "export",
            Self::Validate { .. } => "validate",
            Self::Preview { .. } => "preview",
            Self::Quality { .. } => "quality",
            Self::PreviewLatest { .. } => "preview-latest",
            Self::Trajectory { .. } => "trajectory",
            Self::Iterate { .. } => "iterate",
            Self::SmartIterate { .. } => "smart-iterate",
            Self::SmartStatus { .. } => "smart-status",
            Self::SmartAdopt { .. } => "smart-adopt",
        }
    }
}

fn agent_json_requested(arguments: &[std::ffi::OsString]) -> bool {
    let mut index = 1;
    while let Some(argument) = arguments.get(index) {
        if argument == "--profiles-dir" {
            index += 2;
            continue;
        }
        if argument
            .to_str()
            .is_some_and(|value| value.starts_with("--profiles-dir="))
        {
            index += 1;
            continue;
        }
        return argument == "agent-task"
            && arguments[index + 1..]
                .iter()
                .take_while(|value| *value != "--")
                .any(|value| value == "--json");
    }
    false
}

fn agent_error_contract(error: &video_agent::AgentError) -> (&'static str, &'static str) {
    use video_agent::AgentError;
    // Never echo raw provider/FFmpeg errors, JSON content, paths, or arguments in
    // machine-mode errors. Private task records retain the underlying failure.
    match error {
        AgentError::Invalid(_) => (
            "Invalid",
            "The Agent request failed validation or a safety check.",
        ),
        AgentError::ModelResponseRejected { .. } => (
            "ModelResponseRejected",
            "The model response was rejected; no plan was approved or exported.",
        ),
        AgentError::Cancelled => (
            "Cancelled",
            "The operation was cancelled; retained stages may be resumed.",
        ),
        AgentError::TimedOut => (
            "TimedOut",
            "The execution deadline or available task budget was exhausted.",
        ),
        AgentError::Io(_) => ("Io", "A required local file or tool could not be accessed."),
        AgentError::Json(_) => (
            "Json",
            "The bounded JSON input or saved task data was invalid.",
        ),
    }
}

fn print_agent_json_error(command: &str, code: &str, message: &str) {
    println!(
        "{}",
        serde_json::json!({
            "schema": "video-agent-cli/v1", "ok": false, "command": command,
            "result": null, "error": {"code": code, "message": message}
        })
    );
}

struct AgentTaskOutput {
    result: serde_json::Value,
    text_task: Option<video_agent::Task>,
    workspace_header: Option<PathBuf>,
    plan_digest: Option<String>,
}

impl AgentTaskOutput {
    fn report(value: impl serde::Serialize) -> video_agent::Result<Self> {
        Ok(Self {
            result: serde_json::to_value(value)?,
            text_task: None,
            workspace_header: None,
            plan_digest: None,
        })
    }

    fn task(
        task: video_agent::Task,
        workspace: PathBuf,
        show_workspace: bool,
    ) -> video_agent::Result<Self> {
        let digest = task
            .plan
            .as_ref()
            .map(|plan| video_agent::workflow::plan_digest(&task, plan))
            .transpose()?;
        Ok(Self {
            result: serde_json::json!({
                "workspace": workspace, "plan_digest": digest,
                "task": serde_json::to_value(&task)?
            }),
            text_task: Some(task),
            workspace_header: show_workspace.then_some(workspace),
            plan_digest: digest,
        })
    }
}

fn print_agent_text(output: AgentTaskOutput) -> video_agent::Result<()> {
    if let Some(workspace) = output.workspace_header {
        println!("Workspace: {}", workspace.display());
    }
    if let Some(digest) = output.plan_digest {
        println!("Plan digest: {digest}");
    }
    if let Some(task) = output.text_task {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&output.result)?);
    }
    Ok(())
}

fn read_agent_json<T: serde::de::DeserializeOwned>(path: &Path) -> video_agent::Result<T> {
    use std::io::Read;
    const MAX_JSON_BYTES: u64 = video_agent::feedback::MAX_PLAN_BYTES;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_JSON_BYTES {
        return Err(video_agent::AgentError::Invalid(
            "Input must be a bounded regular, non-symlink JSON file".into(),
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(video_agent::AgentError::Invalid(
            "JSON input exceeds its byte limit".into(),
        ));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn agent_discovery() -> serde_json::Value {
    use video_agent::feedback::{
        MAX_ATTEMPTS, MAX_CLIPS, MAX_MEDIA_BYTES, MAX_PLAN_BYTES, MAX_PREVIEW_MS,
    };
    serde_json::json!({
        "capability_schema": "video-agent-capabilities/v1",
        "commands": ["discover", "create", "show", "sample", "analyze", "manual-draft",
            "edit", "validate", "preview", "quality", "preview-latest", "trajectory",
            "iterate", "smart-iterate", "smart-status", "smart-adopt", "approve", "export"],
        "json_mode": "agent-task --json COMMAND",
        "structured_edit_plan": {
            "format": "EditPlan JSON", "maximum_input_bytes": MAX_PLAN_BYTES,
            "time_unit": "integer milliseconds", "interval": "half-open [start_ms, end_ms)",
            "paths_from_model": false, "shell_commands": false,
            "validation_limits": "Bound to the existing task limits and source fingerprints"
        },
        "dependencies": [
            {"tool": "ffmpeg", "external": true, "bundled": false,
             "purpose": "frame extraction, preview rendering, export and full-stream decoding"},
            {"tool": "ffprobe", "external": true, "bundled": false,
             "purpose": "local media stream and duration inspection"},
            {"tool": "ffplay", "external": true, "bundled": false, "optional": true,
             "purpose": "native desktop source-interval playback; not needed for headless preview rendering"}
        ],
        "dependency_installation_checked": false,
        "preview": {"whole_plan": true, "requires_approval": false, "is_final_export": false,
            "maximum_duration_ms": MAX_PREVIEW_MS, "maximum_clip_count": MAX_CLIPS,
            "maximum_media_bytes": MAX_MEDIA_BYTES, "maximum_plan_bytes": MAX_PLAN_BYTES,
            "persistent_attempt_limit": MAX_ATTEMPTS,
            "media_size_limit_scope": "post_generation_acceptance_not_os_quota"},
        "quality": {"objective_media_checks": true, "semantic": "not_evaluated",
            "human_scores": "not_generated"},
        "iteration": {"candidate_source": "explicit local EditPlan JSON array",
            "includes_current_saved_plan": true, "maximum_attempts": 3,
            "maximum_supplied_candidates": 2, "persistent_preview_attempt_limit": MAX_ATTEMPTS,
            "cloud_calls": false, "automatic_approval": false},
        "smart_feedback": {"maximum_rounds": 3, "desktop_default_maximum_rounds": 2,
            "desktop_resumes_saved_round_bound": true,
            "candidate_source": "model review of sparse rendered JPEGs and bounded source samples",
            "separate_preview_upload_consent_required": true,
            "resume_persisted_results": true, "status_is_historical_not_fresh_verification": true,
            "semantic_scope": "sparse_model_review_not_human_acceptance_or_full_video_understanding",
            "automatic_plan_adoption": false, "automatic_approval": false,
            "automatic_export": false, "explicit_adoption_command": "smart-adopt"},
        "cloud": {"commands": ["analyze", "smart-iterate"],
            "explicit_consent_required": true, "credentials_required": true,
            "uploads": {"analyze": "bounded labeled source JPEG observations, intent and source metadata",
                "smart-iterate": "rendered-preview JPEGs, bounded source JPEG samples, intent, current plan and objective quality"},
            "source_video_uploaded": false, "audio_uploaded": false,
            "sparse_samples_only": true},
        "safety": {"source_overwrite": false, "automatic_approval": false,
            "final_export_requires_exact_human_approval": true,
            "external_hub_installation": false, "trajectory_is_private_by_default": true}
    })
}

fn execute_agent_task(command: AgentTaskCommands) -> video_agent::Result<AgentTaskOutput> {
    use video_agent::{CancelToken, Limits, OpenAiVision, VisionProvider, Workspace};
    let cancel = CancelToken::default();
    let (task, workspace_path, show_workspace) = match command {
        AgentTaskCommands::Discover => return AgentTaskOutput::report(agent_discovery()),
        AgentTaskCommands::Create {
            inputs,
            workspace_root,
            intent,
            target_seconds,
            frame_workers,
        } => {
            let millis = target_seconds.checked_mul(1000).ok_or_else(|| {
                video_agent::AgentError::Invalid("Target duration overflow".into())
            })?;
            let limits = Limits {
                max_concurrency: usize::from(frame_workers),
                ..Limits::default()
            };
            let workspace = Workspace::create(&workspace_root, &inputs, intent, millis, limits)?;
            (workspace.load()?, workspace.task_dir().to_path_buf(), true)
        }
        AgentTaskCommands::Show { workspace } => {
            (Workspace::open(&workspace)?.load()?, workspace, false)
        }
        AgentTaskCommands::Sample { workspace } => (
            Workspace::open(&workspace)?.sample(&cancel)?,
            workspace,
            false,
        ),
        AgentTaskCommands::Analyze {
            workspace,
            allow_cloud,
        } => {
            let workspace = Workspace::open(&workspace)?;
            let task = workspace.load()?;
            let provider = OpenAiVision::from_env(&task.limits)?;
            if !provider.ready() {
                return Err(video_agent::AgentError::Invalid(
                    "Real vision is not configured. Set a local API credential; no simulation or network call was substituted.".into()));
            }
            if allow_cloud {
                workspace.set_consent(true)?;
            }
            (
                workspace.analyze(&provider, &cancel)?,
                workspace.task_dir().to_path_buf(),
                false,
            )
        }
        AgentTaskCommands::ManualDraft { workspace } => (
            Workspace::open(&workspace)?.manual_draft()?,
            workspace,
            false,
        ),
        AgentTaskCommands::Edit { workspace, plan } => {
            let plan = read_agent_json(&plan)?;
            (
                Workspace::open(&workspace)?.save_plan(plan)?,
                workspace,
                false,
            )
        }
        AgentTaskCommands::Approve {
            workspace,
            plan_digest,
        } => (
            Workspace::open(&workspace)?.approve(&plan_digest)?,
            workspace,
            false,
        ),
        AgentTaskCommands::Export { workspace } => (
            Workspace::open(&workspace)?.export(&cancel)?,
            workspace,
            false,
        ),
        AgentTaskCommands::Validate { workspace, plan } => {
            let plan = plan.as_deref().map(read_agent_json).transpose()?;
            return AgentTaskOutput::report(video_agent::feedback::validate(
                &workspace, plan, &cancel,
            )?);
        }
        AgentTaskCommands::Preview { workspace } => {
            return AgentTaskOutput::report(video_agent::feedback::capture(&workspace, &cancel)?);
        }
        AgentTaskCommands::Quality { workspace } => {
            return AgentTaskOutput::report(video_agent::feedback::quality(&workspace, &cancel)?);
        }
        AgentTaskCommands::PreviewLatest { workspace } => {
            return AgentTaskOutput::report(video_agent::feedback::latest(&workspace)?);
        }
        AgentTaskCommands::Trajectory { workspace } => {
            return AgentTaskOutput::report(video_agent::feedback::trajectory(&workspace)?);
        }
        AgentTaskCommands::Iterate {
            workspace,
            candidates,
            max_attempts,
        } => {
            let candidates = read_agent_json(&candidates)?;
            return AgentTaskOutput::report(video_agent::feedback::iterate(
                &workspace,
                candidates,
                max_attempts,
                &cancel,
            )?);
        }
        AgentTaskCommands::SmartIterate {
            workspace,
            max_rounds,
            consent_preview_upload,
        } => {
            // Refuse before opening a task or private provider configuration.
            if !consent_preview_upload {
                return Err(video_agent::AgentError::Invalid(
                    "Separate consent for rendered-preview uploads is required".into(),
                ));
            }
            let task = Workspace::snapshot(&workspace)?;
            let provider = OpenAiVision::from_env(&task.limits)?;
            return AgentTaskOutput::report(video_agent::smart_feedback::run(
                &workspace,
                &provider,
                max_rounds,
                consent_preview_upload,
                &cancel,
            )?);
        }
        AgentTaskCommands::SmartStatus { workspace } => {
            return AgentTaskOutput::report(video_agent::smart_feedback::status(&workspace)?);
        }
        AgentTaskCommands::SmartAdopt { workspace, run_id } => (
            video_agent::smart_feedback::adopt(&workspace, &run_id, &cancel)?,
            workspace,
            false,
        ),
    };
    AgentTaskOutput::task(task, workspace_path, show_workspace)
}

fn handle_plugin_command(cmd: PluginCommands) {
    let plugin_manager = match PluginManager::new() {
        Ok(manager) => manager,
        Err(e) => {
            eprintln!("Error creating plugin manager: {}", e);
            std::process::exit(1);
        }
    };

    match cmd {
        PluginCommands::List => {
            let metadata = plugin_manager.get_all_plugin_metadata();
            if metadata.is_empty() {
                println!("No plugins loaded.");
                return;
            }

            println!("Loaded plugins:");
            for meta in metadata {
                println!("  {} v{} by {}", meta.name, meta.version, meta.author);
                println!("    {}", meta.description);
            }
        }

        PluginCommands::Load { path } => {
            match plugin_manager.load_plugin(Path::new(&path)) {
                Ok(()) => {
                    println!("Plugin loaded successfully!");

                    // Display plugin info
                    let plugin_name = Path::new(&path).file_stem().unwrap().to_string_lossy();
                    if let Some(meta) =
                        plugin_manager.with_plugin(&plugin_name, |plugin| plugin.metadata())
                    {
                        println!("Name: {} v{}", meta.name, meta.version);
                        println!("Author: {}", meta.author);
                        println!("Description: {}", meta.description);

                        // Show parameters
                        let params = plugin_manager
                            .get_plugin_parameters(&plugin_name)
                            .unwrap_or_default();
                        if !params.is_empty() {
                            println!("Parameters:");
                            for param in params {
                                let required = if param.required { " (required)" } else { "" };
                                let default = match param.default_value {
                                    Some(ref v) => format!(" [default: {}]", v),
                                    None => String::new(),
                                };

                                println!(
                                    "  {}{}{} - {}",
                                    param.name, required, default, param.description
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error loading plugin: {}", e);
                    std::process::exit(1);
                }
            }
        }

        PluginCommands::Run { name, params } => {
            // Check if plugin exists
            if !plugin_manager.with_plugin(&name, |_| true).unwrap_or(false) {
                eprintln!("Plugin '{}' not found.", name);
                std::process::exit(1);
            }

            // Parse parameters
            let mut param_map = HashMap::new();
            for param in params {
                let parts: Vec<&str> = param.splitn(2, '=').collect();
                if parts.len() == 2 {
                    param_map.insert(parts[0].to_string(), parts[1].to_string());
                } else {
                    eprintln!("Invalid parameter format: {}. Expected key=value", param);
                    std::process::exit(1);
                }
            }

            // Check required parameters
            if let Some(param_info) = plugin_manager.get_plugin_parameters(&name) {
                for info in &param_info {
                    if info.required && !param_map.contains_key(&info.name) {
                        eprintln!("Missing required parameter: {}", info.name);
                        std::process::exit(1);
                    }
                }
            }

            // Execute the plugin
            match plugin_manager.execute_plugin(&name, param_map) {
                Ok(()) => println!("Plugin executed successfully!"),
                Err(e) => {
                    eprintln!("Error executing plugin: {}", e);
                    std::process::exit(1);
                }
            }
        }

        PluginCommands::Discover => {
            let results = plugin_manager.discover_plugins();

            let successes: Vec<_> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
            let failures: Vec<_> = results.iter().filter_map(|r| r.as_ref().err()).collect();

            println!("Discovered {} plugin(s).", successes.len());

            if !successes.is_empty() {
                println!("Successfully loaded plugins:");
                for meta in successes {
                    println!("  {} v{} by {}", meta.name, meta.version, meta.author);
                    println!("    {}", meta.description);
                }
            }

            if !failures.is_empty() {
                println!("Failed to load {} plugin(s):", failures.len());
                for error in failures {
                    println!("  Error: {}", error);
                }
            }
        }
    }
}

fn profile_manager(directory: Option<&Path>) -> profile_system::Result<ProfileManager> {
    match directory {
        Some(path) => ProfileManager::with_directory(path),
        None => ProfileManager::new(),
    }
}

fn validate_runnable_profile(profile: &Profile) -> batch_processing::Result<()> {
    if !matches!(profile.profile_type, ProfileType::Custom(_)) {
        ProfileBatchConfig::from_profile(profile)?;
    }
    Ok(())
}

fn handle_profile_command(cmd: ProfileCommands, profiles_directory: Option<&Path>) {
    let profile_manager = match profile_manager(profiles_directory) {
        Ok(manager) => manager,
        Err(e) => {
            eprintln!("Error creating profile manager: {}", e);
            std::process::exit(1);
        }
    };

    match cmd {
        ProfileCommands::List { profile_type } => {
            if let Some(type_str) = profile_type {
                // List profiles of a specific type
                let profile_type = match type_str.as_str() {
                    "clipper" => ProfileType::Clipper,
                    "gif_converter" => ProfileType::GifConverter,
                    "gif_transparency" => ProfileType::GifTransparency,
                    "splitter" => ProfileType::Splitter,
                    "merger" => ProfileType::Merger,
                    other => ProfileType::Custom(other.to_string()),
                };
                let profile_type_display = profile_type.clone();

                match profile_manager.list_profiles(profile_type) {
                    Ok(profiles) => {
                        if profiles.is_empty() {
                            println!("No profiles found for type: {:?}", profile_type_display);
                            return;
                        }

                        println!("Profiles for type {:?}:", profile_type_display);
                        for name in profiles {
                            println!("  {}", name);
                        }
                    }
                    Err(e) => {
                        eprintln!("Error listing profiles: {}", e);
                        std::process::exit(1);
                    }
                }
            } else {
                // List all profiles
                match profile_manager.list_all_profiles() {
                    Ok(all_profiles) => {
                        if all_profiles.is_empty() {
                            println!("No profiles found.");
                            return;
                        }

                        println!("Available profiles:");
                        for (profile_type, profiles) in all_profiles {
                            if !profiles.is_empty() {
                                println!("  {:?}:", profile_type);
                                for name in profiles {
                                    println!("    {}", name);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error listing profiles: {}", e);
                        std::process::exit(1);
                    }
                }
            }
        }

        ProfileCommands::Show { name, profile_type } => {
            let profile_type = match profile_type.as_str() {
                "clipper" => ProfileType::Clipper,
                "gif_converter" => ProfileType::GifConverter,
                "gif_transparency" => ProfileType::GifTransparency,
                "splitter" => ProfileType::Splitter,
                "merger" => ProfileType::Merger,
                other => ProfileType::Custom(other.to_string()),
            };

            match profile_manager.load_profile(&name, profile_type) {
                Ok(profile) => {
                    println!("Profile: {} ({:?})", profile.name, profile.profile_type);
                    if let Some(desc) = profile.description {
                        println!("Description: {}", desc);
                    }
                    println!("Created: {}", profile.created);
                    println!("Last modified: {}", profile.last_modified);
                    println!("Parameters:");
                    for (key, value) in profile.parameters {
                        println!("  {} = {}", key, value);
                    }
                }
                Err(e) => {
                    eprintln!("Error loading profile: {}", e);
                    std::process::exit(1);
                }
            }
        }

        ProfileCommands::Create {
            name,
            profile_type,
            description,
            params,
        } => {
            // Parse parameters
            let mut parameters = HashMap::new();
            for param in params {
                let parts: Vec<&str> = param.splitn(2, '=').collect();
                if parts.len() == 2 && !parts[0].trim().is_empty() {
                    let key = parts[0].trim().to_string();
                    if parameters
                        .insert(key.clone(), parts[1].to_string())
                        .is_some()
                    {
                        eprintln!("Error: Duplicate profile parameter '{key}'.");
                        std::process::exit(1);
                    }
                } else {
                    eprintln!("Invalid parameter format: {}. Expected key=value", param);
                    std::process::exit(1);
                }
            }

            // Create profile
            let profile_type = match profile_type.as_str() {
                "clipper" => ProfileType::Clipper,
                "gif_converter" => ProfileType::GifConverter,
                "gif_transparency" => ProfileType::GifTransparency,
                "splitter" => ProfileType::Splitter,
                "merger" => ProfileType::Merger,
                other => ProfileType::Custom(other.to_string()),
            };

            let mut profile = Profile::new(&name, profile_type, parameters);
            if let Some(desc) = description {
                profile = profile.with_description(&desc);
            }

            if let Err(error) = validate_runnable_profile(&profile) {
                eprintln!("Error: {error}");
                std::process::exit(1);
            }

            // Save profile
            match profile_manager.save_profile(&profile) {
                Ok(()) => println!("Profile '{}' created successfully!", name),
                Err(e) => {
                    eprintln!("Error creating profile: {}", e);
                    std::process::exit(1);
                }
            }
        }

        ProfileCommands::Delete { name, profile_type } => {
            let profile_type = match profile_type.as_str() {
                "clipper" => ProfileType::Clipper,
                "gif_converter" => ProfileType::GifConverter,
                "gif_transparency" => ProfileType::GifTransparency,
                "splitter" => ProfileType::Splitter,
                "merger" => ProfileType::Merger,
                other => ProfileType::Custom(other.to_string()),
            };

            match profile_manager.delete_profile(&name, profile_type) {
                Ok(()) => println!("Profile '{}' deleted successfully!", name),
                Err(e) => {
                    eprintln!("Error deleting profile: {}", e);
                    std::process::exit(1);
                }
            }
        }

        ProfileCommands::Import { path } => {
            let result = (|| -> anyhow::Result<Profile> {
                let profile = ProfileManager::read_profile(Path::new(&path))?;
                validate_runnable_profile(&profile)?;
                profile_manager.save_profile(&profile)?;
                Ok(profile)
            })();
            match result {
                Ok(profile) => println!("Profile '{}' imported successfully!", profile.name),
                Err(e) => {
                    eprintln!("Error importing profile: {}", e);
                    std::process::exit(1);
                }
            }
        }

        ProfileCommands::Export {
            name,
            profile_type,
            output,
        } => {
            let profile_type = match profile_type.as_str() {
                "clipper" => ProfileType::Clipper,
                "gif_converter" => ProfileType::GifConverter,
                "gif_transparency" => ProfileType::GifTransparency,
                "splitter" => ProfileType::Splitter,
                "merger" => ProfileType::Merger,
                other => ProfileType::Custom(other.to_string()),
            };

            match profile_manager.export_profile(&name, profile_type, Path::new(&output)) {
                Ok(()) => println!("Profile '{}' exported to '{}'!", name, output),
                Err(e) => {
                    eprintln!("Error exporting profile: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }
}

fn handle_batch_command(cmd: BatchCommands, profiles_directory: Option<&Path>) {
    match cmd {
        BatchCommands::Clipper {
            inputs,
            recursive,
            pattern,
            output_dir,
            ranges,
            copy_codec,
            suffix,
            parallel,
            format,
        } => {
            println!("Running batch clipper...");

            // Parse time ranges
            let time_ranges_result = parse_time_ranges(&ranges);
            if time_ranges_result.is_empty() {
                eprintln!("Error: No valid time ranges provided.");
                std::process::exit(1);
            }

            // Create processor
            let mut processor = match BatchProcessor::create_clipper(
                &ranges,
                Path::new(&output_dir),
                copy_codec,
                suffix.as_deref(),
            ) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Error configuring batch processor: {}", e);
                    std::process::exit(1);
                }
            };

            // Configure processor
            processor = processor
                .with_recursive(recursive)
                .with_parallel(parallel)
                .with_output_format(format);

            if let Some(pat) = pattern {
                processor = match processor.with_pattern(&pat) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Error setting pattern: {}", e);
                        std::process::exit(1);
                    }
                };
            }

            // Process files
            match processor.process(&inputs) {
                Ok(results) => {
                    let success_count = results.iter().filter(|r| r.success).count();
                    println!(
                        "Successfully processed {}/{} files.",
                        success_count,
                        results.len()
                    );

                    if success_count < results.len() {
                        eprintln!("Errors occurred during processing:");
                        for result in results.iter().filter(|r| !r.success) {
                            if let Some(ref error) = result.error_message {
                                eprintln!("  {}: {}", result.input.display(), error);
                            }
                        }
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error during batch processing: {}", e);
                    std::process::exit(1);
                }
            }
        }

        BatchCommands::GifConverter {
            inputs,
            recursive,
            pattern,
            output_dir,
            width,
            fps,
            max_size,
            optimize,
            parallel,
        } => {
            println!("Running batch GIF converter...");

            // Create processor
            let mut processor = BatchProcessor::create_gif_converter(
                width,
                fps,
                max_size,
                optimize,
                Path::new(&output_dir),
            );

            // Configure processor
            processor = processor.with_recursive(recursive).with_parallel(parallel);

            if let Some(pat) = pattern {
                processor = match processor.with_pattern(&pat) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Error setting pattern: {}", e);
                        std::process::exit(1);
                    }
                };
            }

            // Process files
            match processor.process(&inputs) {
                Ok(results) => {
                    let success_count = results.iter().filter(|r| r.success).count();
                    println!(
                        "Successfully processed {}/{} files.",
                        success_count,
                        results.len()
                    );

                    if success_count < results.len() {
                        eprintln!("Errors occurred during processing:");
                        for result in results.iter().filter(|r| !r.success) {
                            if let Some(ref error) = result.error_message {
                                eprintln!("  {}: {}", result.input.display(), error);
                            }
                        }
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error during batch processing: {}", e);
                    std::process::exit(1);
                }
            }
        }

        BatchCommands::GifTransparency {
            inputs,
            recursive,
            pattern,
            backup,
            parallel,
        } => {
            println!("Running batch GIF transparency processor...");

            // Create processor
            let mut processor = BatchProcessor::create_gif_transparency(backup);

            // Configure processor
            processor = processor.with_recursive(recursive).with_parallel(parallel);

            if let Some(pat) = pattern {
                processor = match processor.with_pattern(&pat) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Error setting pattern: {}", e);
                        std::process::exit(1);
                    }
                };
            }

            // Process files
            match processor.process(&inputs) {
                Ok(results) => {
                    let success_count = results.iter().filter(|r| r.success).count();
                    println!(
                        "Successfully processed {}/{} files.",
                        success_count,
                        results.len()
                    );

                    if success_count < results.len() {
                        eprintln!("Errors occurred during processing:");
                        for result in results.iter().filter(|r| !r.success) {
                            if let Some(ref error) = result.error_message {
                                eprintln!("  {}: {}", result.input.display(), error);
                            }
                        }
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error during batch processing: {}", e);
                    std::process::exit(1);
                }
            }
        }

        BatchCommands::Splitter {
            inputs,
            recursive,
            pattern,
            output_dir,
            prefix,
            custom_encode,
            force,
            parallel,
            format,
        } => {
            println!("Running batch video splitter...");

            // Create processor
            let mut processor = BatchProcessor::create_splitter(
                Path::new(&output_dir),
                &prefix,
                custom_encode.as_deref(),
                force,
            )
            .with_output_format(format);

            // Configure processor
            processor = processor.with_recursive(recursive).with_parallel(parallel);

            if let Some(pat) = pattern {
                processor = match processor.with_pattern(&pat) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Error setting pattern: {}", e);
                        std::process::exit(1);
                    }
                };
            }

            // Process files
            match processor.process(&inputs) {
                Ok(results) => {
                    let success_count = results.iter().filter(|r| r.success).count();
                    println!(
                        "Successfully processed {}/{} files.",
                        success_count,
                        results.len()
                    );

                    if success_count < results.len() {
                        eprintln!("Errors occurred during processing:");
                        for result in results.iter().filter(|r| !r.success) {
                            if let Some(ref error) = result.error_message {
                                eprintln!("  {}: {}", result.input.display(), error);
                            }
                        }
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error during batch processing: {}", e);
                    std::process::exit(1);
                }
            }
        }

        BatchCommands::Merger {
            inputs,
            recursive,
            pattern,
            audio,
            output_dir,
            shortest,
            copy_codec,
            parallel,
            format,
        } => {
            println!("Running batch audio/video merger...");

            // Create processor
            let mut processor =
                BatchProcessor::create_merger(&audio, Path::new(&output_dir), shortest, copy_codec)
                    .with_output_format(format);

            // Configure processor
            processor = processor.with_recursive(recursive).with_parallel(parallel);

            if let Some(pat) = pattern {
                processor = match processor.with_pattern(&pat) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Error setting pattern: {}", e);
                        std::process::exit(1);
                    }
                };
            }

            // Process files
            match processor.process(&inputs) {
                Ok(results) => {
                    let success_count = results.iter().filter(|r| r.success).count();
                    println!(
                        "Successfully processed {}/{} files.",
                        success_count,
                        results.len()
                    );

                    if success_count < results.len() {
                        eprintln!("Errors occurred during processing:");
                        for result in results.iter().filter(|r| !r.success) {
                            if let Some(ref error) = result.error_message {
                                eprintln!("  {}: {}", result.input.display(), error);
                            }
                        }
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error during batch processing: {}", e);
                    std::process::exit(1);
                }
            }
        }

        BatchCommands::WithProfile {
            inputs,
            recursive,
            pattern,
            profile,
            profile_type,
            output_dir,
            parallel,
        } => {
            println!("Running batch processing with profile '{}'...", profile);

            // Load profile
            let profile_manager = match profile_manager(profiles_directory) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("Error creating profile manager: {}", e);
                    std::process::exit(1);
                }
            };

            let profile_type = match profile_type.as_str() {
                "clipper" => ProfileType::Clipper,
                "gif_converter" => ProfileType::GifConverter,
                "gif_transparency" => ProfileType::GifTransparency,
                "splitter" => ProfileType::Splitter,
                "merger" => ProfileType::Merger,
                other => ProfileType::Custom(other.to_string()),
            };

            let profile = match profile_manager.load_profile(&profile, profile_type) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Error loading profile: {}", e);
                    std::process::exit(1);
                }
            };

            let result = (|| -> batch_processing::Result<BatchProcessor> {
                let mut config = ProfileBatchConfig::from_profile(&profile)?;
                if let Some(output_dir) = output_dir {
                    config = config.with_output_directory(output_dir)?;
                }
                if let Some(recursive) = recursive {
                    config.recursive = recursive;
                }
                if let Some(parallel) = parallel {
                    config.parallel = parallel;
                }
                if let Some(pattern) = pattern {
                    config.pattern = Some(pattern);
                }
                config.into_processor()
            })();
            let processor = match result {
                Ok(processor) => processor,
                Err(error) => {
                    eprintln!("Error configuring profile '{}': {error}", profile.name);
                    std::process::exit(1);
                }
            };
            match processor.process(&inputs) {
                Ok(results) => {
                    let success_count = results.iter().filter(|result| result.success).count();
                    println!(
                        "Successfully processed {success_count}/{} files using profile '{}'.",
                        results.len(),
                        profile.name
                    );
                    for result in results.iter().filter(|result| !result.success) {
                        eprintln!(
                            "  {}: {}",
                            result.input.display(),
                            result
                                .error_message
                                .as_deref()
                                .unwrap_or("Processing failed")
                        );
                    }
                    if success_count != results.len() {
                        std::process::exit(1);
                    }
                }
                Err(error) => {
                    eprintln!("Error during profile batch processing: {error}");
                    std::process::exit(1);
                }
            }
        }
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn all_commands_have_consistent_argument_definitions() {
        Cli::command().debug_assert();
    }

    #[test]
    fn frame_worker_limit_has_a_real_create_argument_and_bounded_alias() {
        let arguments = || {
            vec![
                "video-toolkit",
                "agent-task",
                "create",
                "local.mp4",
                "--workspace-root",
                "/unused-parser-fixture",
                "--intent",
                "Parser check only",
            ]
        };
        for (flag, value) in [
            ("--frame-workers", "1"),
            ("--frame-workers", "4"),
            ("--max-concurrency", "2"),
        ] {
            let mut args = arguments();
            args.extend([flag, value]);
            let parsed = Cli::try_parse_from(args).unwrap();
            let Some(Commands::AgentTask(AgentTaskOptions {
                command: AgentTaskCommands::Create { frame_workers, .. },
                ..
            })) = parsed.command
            else {
                panic!("Expected task creation arguments");
            };
            assert_eq!(frame_workers, value.parse::<u8>().unwrap());
        }
        for value in ["0", "5", "-1", "unlimited"] {
            let mut args = arguments();
            args.extend(["--frame-workers", value]);
            assert!(Cli::try_parse_from(args).is_err());
        }
        let parsed = Cli::try_parse_from(arguments()).unwrap();
        assert!(matches!(
            parsed.command,
            Some(Commands::AgentTask(AgentTaskOptions {
                command: AgentTaskCommands::Create {
                    frame_workers: 1,
                    ..
                },
                ..
            }))
        ));
    }

    #[test]
    fn json_is_scoped_to_agent_task_and_propagates_to_its_commands() {
        for arguments in [
            vec!["video-toolkit", "agent-task", "--json", "discover"],
            vec!["video-toolkit", "agent-task", "discover", "--json"],
        ] {
            let cli = Cli::try_parse_from(arguments).unwrap();
            assert!(matches!(
                cli.command,
                Some(Commands::AgentTask(AgentTaskOptions {
                    json: true,
                    command: AgentTaskCommands::Discover,
                }))
            ));
        }
        assert!(Cli::try_parse_from(["video-toolkit", "--json", "formats"]).is_err());
        assert!(Cli::try_parse_from(["video-toolkit", "formats", "--json"]).is_err());
    }

    #[test]
    fn json_parse_detection_does_not_claim_legacy_commands_or_positional_values() {
        let detect = |arguments: &[&str]| {
            let arguments: Vec<_> = arguments.iter().map(std::ffi::OsString::from).collect();
            agent_json_requested(&arguments)
        };
        assert!(detect(&["video-toolkit", "agent-task", "--json"]));
        assert!(detect(&[
            "video-toolkit",
            "--profiles-dir",
            "private",
            "agent-task",
            "--json",
            "discover"
        ]));
        assert!(!detect(&["video-toolkit", "formats", "--json"]));
        assert!(!detect(&[
            "video-toolkit",
            "--profiles-dir",
            "agent-task",
            "formats",
            "--json"
        ]));
        assert!(!detect(&[
            "video-toolkit",
            "agent-task",
            "show",
            "--",
            "--json"
        ]));
    }

    #[test]
    fn feedback_iteration_has_a_small_explicit_attempt_bound() {
        for (value, valid) in [("1", true), ("3", true), ("0", false), ("4", false)] {
            let parsed = Cli::try_parse_from([
                "video-toolkit",
                "agent-task",
                "iterate",
                "unused-workspace",
                "--candidates",
                "unused-candidates.json",
                "--max-attempts",
                value,
            ]);
            assert_eq!(parsed.is_ok(), valid);
        }
    }

    #[test]
    fn smart_feedback_rounds_are_bounded_and_preview_upload_consent_is_never_implicit() {
        for (value, valid) in [("1", true), ("3", true), ("0", false), ("4", false)] {
            let parsed = Cli::try_parse_from([
                "video-toolkit",
                "agent-task",
                "smart-iterate",
                "unused-workspace",
                "--max-rounds",
                value,
            ]);
            assert_eq!(parsed.is_ok(), valid);
            if let Ok(cli) = parsed {
                assert!(matches!(
                    cli.command,
                    Some(Commands::AgentTask(AgentTaskOptions {
                        command: AgentTaskCommands::SmartIterate {
                            consent_preview_upload: false,
                            ..
                        },
                        ..
                    }))
                ));
            }
        }
        let parsed = Cli::try_parse_from([
            "video-toolkit",
            "agent-task",
            "smart-iterate",
            "unused-workspace",
            "--consent-preview-upload",
        ])
        .unwrap();
        assert!(matches!(
            parsed.command,
            Some(Commands::AgentTask(AgentTaskOptions {
                command: AgentTaskCommands::SmartIterate {
                    max_rounds: 3,
                    consent_preview_upload: true,
                    ..
                },
                ..
            }))
        ));
    }

    #[test]
    fn machine_error_contract_has_stable_codes_and_no_raw_diagnostics() {
        use video_agent::AgentError;
        let json_error =
            serde_json::from_str::<serde_json::Value>("inert-private-marker").unwrap_err();
        let record = video_agent::ModelCall {
            call_id: "fixture".into(),
            backend: "SIMULATED-UNIT-TEST".into(),
            requested_model: String::new(),
            model: String::new(),
            prompt_version: String::new(),
            sample_ids: vec![],
            provider_response_id: None,
            request_id: None,
            input_tokens: 0,
            output_tokens: 0,
            estimated_cost_usd: 0.0,
            elapsed_ms: 0,
            status: "rejected".into(),
            decision: None,
            error: None,
        };
        let cases = [
            (
                AgentError::Invalid("inert-private-marker".into()),
                "Invalid",
            ),
            (AgentError::Cancelled, "Cancelled"),
            (AgentError::TimedOut, "TimedOut"),
            (
                AgentError::Io(std::io::Error::other("inert-private-marker")),
                "Io",
            ),
            (AgentError::Json(json_error), "Json"),
            (
                AgentError::ModelResponseRejected {
                    message: "inert-private-marker".into(),
                    record: Box::new(record),
                },
                "ModelResponseRejected",
            ),
        ];
        for (error, expected) in cases {
            let (code, message) = agent_error_contract(&error);
            assert_eq!(code, expected);
            assert!(!message.contains("inert-private-marker"));
        }
    }
}
