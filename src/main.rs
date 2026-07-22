use clap::{Parser, Subcommand};
use eframe::NativeOptions;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use batch_processing::BatchProcessor;
use clipper::{clip_video, parse_time_ranges};
use common::{check_ffmpeg, formats::*, get_supported_formats};
use gif_converter::{convert_video_to_gif, optimize_conversion};
use gif_transparency::{batch_process_gifs, process_directory};
use merger::merge_audio_video;
use plugin_system::PluginManager;
use profile_system::{Profile, ProfileManager, ProfileType};
use splitter::split_video;
use ui::VideoToolKitApp;

#[derive(Parser)]
#[clap(author, version, about = "Video processing utilities")]
struct Cli {
    #[clap(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Extract specific time ranges from MP4 video files
    Clipper {
        /// Input video file path
        input: String,

        /// Time ranges to extract in format START-END (e.g., 00:01:00-00:02:00)
        #[clap(short, long, required = true)]
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
        format: String,
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
        #[clap(long)]
        custom_encode: Option<String>,

        /// Process even if video dimensions are not 1920x1080
        #[clap(long)]
        force: bool,

        /// Output format (e.g., mp4, mkv, avi)
        #[clap(long, default_value = "mp4")]
        format: String,
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

        /// Output format (e.g., mp4, mkv, avi)
        #[clap(long, default_value = "mp4")]
        format: String,
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
        #[clap(short, long)]
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
        #[clap(short, long, required = true)]
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
        format: String,
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
        #[clap(short, long, default_value = "slice")]
        prefix: String,

        /// Custom FFmpeg encoding options
        #[clap(long)]
        custom_encode: Option<String>,

        /// Process even if video dimensions are not 1920x1080
        #[clap(long)]
        force: bool,

        /// Process files in parallel
        #[clap(long, default_value = "true")]
        parallel: bool,

        /// Output format (e.g., mp4, mkv, avi)
        #[clap(long, default_value = "mp4")]
        format: String,
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
        format: String,
    },

    /// Use a profile for batch processing
    WithProfile {
        /// Input files or directories
        #[clap(required = true)]
        inputs: Vec<PathBuf>,

        /// Process directories recursively
        #[clap(short, long)]
        recursive: bool,

        /// File pattern to match (regex)
        #[clap(short, long)]
        pattern: Option<String>,

        /// Name of the profile to use
        #[clap(long, required = true)]
        profile: String,

        /// Type of the profile
        #[clap(long, required = true)]
        profile_type: String,

        /// Process files in parallel
        #[clap(long, default_value = "true")]
        parallel: bool,
    },
}

fn main() -> Result<(), eframe::Error> {
    // Parse command-line arguments
    let cli = Cli::parse();

    // Keep metadata commands such as --help and --version available on machines
    // where FFmpeg has not been installed yet. The GUI renders its own setup hint.
    if cli.command.is_some() && !check_ffmpeg() {
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
            "Video-ToolKit",
            options,
            Box::new(|cc| {
                let mut visuals = egui::Visuals::dark();
                visuals.widgets.active.bg_fill = egui::Color32::from_rgb(38, 125, 190);
                visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(45, 68, 86);
                visuals.selection.bg_fill = egui::Color32::from_rgb(29, 104, 156);
                cc.egui_ctx.set_visuals(visuals);
                Box::new(VideoToolKitApp::default())
            }),
        );
    }

    // Otherwise, run the appropriate command-line tool
    match cli.command.unwrap() {
        Commands::Clipper {
            input,
            ranges,
            output_dir,
            copy_codec,
            suffix,
            format: _,
        } => {
            println!("Running clipper...");

            let time_ranges = parse_time_ranges(&ranges);
            if time_ranges.is_empty() {
                eprintln!("Error: No valid time ranges provided.");
                std::process::exit(1);
            }

            match clip_video(
                &input,
                &time_ranges,
                &output_dir,
                copy_codec,
                suffix.as_deref(),
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
                optimize_conversion(&input, &output, max_size, width)
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
            format: _,
        } => {
            println!("Running video splitter...");

            match split_video(
                &input,
                &output_dir,
                &prefix,
                custom_encode.as_deref(),
                force,
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
            format: _,
        } => {
            println!("Running audio/video merger...");

            match merge_audio_video(&video, &audio, &output, shortest, copy_codec) {
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
            handle_profile_command(profile_cmd);
        }

        Commands::Batch(batch_cmd) => {
            handle_batch_command(batch_cmd);
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

fn handle_profile_command(cmd: ProfileCommands) {
    let profile_manager = match ProfileManager::new() {
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
                if parts.len() == 2 {
                    parameters.insert(parts[0].to_string(), parts[1].to_string());
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
            match profile_manager.import_profile(Path::new(&path)) {
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

fn handle_batch_command(cmd: BatchCommands) {
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
            format: _,
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
            format: _,
        } => {
            println!("Running batch video splitter...");

            // Create processor
            let mut processor = BatchProcessor::create_splitter(
                Path::new(&output_dir),
                &prefix,
                custom_encode.as_deref(),
                force,
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

        BatchCommands::Merger {
            inputs,
            recursive,
            pattern,
            audio,
            output_dir,
            shortest,
            copy_codec,
            parallel,
            format: _,
        } => {
            println!("Running batch audio/video merger...");

            // Create processor
            let mut processor =
                BatchProcessor::create_merger(&audio, Path::new(&output_dir), shortest, copy_codec);

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
            inputs: _,
            recursive: _,
            pattern: _,
            profile,
            profile_type,
            parallel: _,
        } => {
            println!("Running batch processing with profile '{}'...", profile);

            // Load profile
            let profile_manager = match ProfileManager::new() {
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

            // TODO: Implement profile-based batch processing
            println!("Profile-based batch processing not fully implemented yet.");
            println!("Profile details:");
            println!("  Name: {}", profile.name);
            println!("  Type: {:?}", profile.profile_type);
            println!("  Parameters: {:?}", profile.parameters);
        }
    }
}
