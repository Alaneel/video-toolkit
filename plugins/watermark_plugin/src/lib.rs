use std::any::Any;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use plugin_system::{ParameterInfo, ParameterType, Plugin, PluginMetadata, PLUGIN_API_VERSION};

/// Watermark Plugin - Adds a text watermark to videos
pub struct WatermarkPlugin {
    metadata: PluginMetadata,
}

impl Default for WatermarkPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl WatermarkPlugin {
    pub fn new() -> Self {
        Self {
            metadata: PluginMetadata {
                name: "watermark_plugin".to_string(),
                version: "0.1.0".to_string(),
                author: "Video-ToolKit Team".to_string(),
                description: "Adds a text watermark to videos".to_string(),
                api_version: PLUGIN_API_VERSION,
            },
        }
    }
}

impl Plugin for WatermarkPlugin {
    fn metadata(&self) -> PluginMetadata {
        self.metadata.clone()
    }

    fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Check if FFmpeg is available
        let ffmpeg_check = Command::new("ffmpeg").arg("-version").output();

        if ffmpeg_check.is_err() {
            return Err(
                "FFmpeg not found. Please install FFmpeg and make sure it's in your PATH.".into(),
            );
        }

        Ok(())
    }

    fn execute(&self, params: HashMap<String, String>) -> Result<(), Box<dyn std::error::Error>> {
        // Get parameters
        let input_file = params
            .get("input_file")
            .ok_or("Input file parameter is missing")?;

        let output_file = params
            .get("output_file")
            .ok_or("Output file parameter is missing")?;

        let watermark_text = params
            .get("watermark_text")
            .ok_or("Watermark text parameter is missing")?;

        let default_position = "bottom_right".to_string();
        let default_font_size = "24".to_string();
        let default_font_color = "white".to_string();

        let position = params.get("position").unwrap_or(&default_position);
        let font_size = params.get("font_size").unwrap_or(&default_font_size);
        let font_color = params.get("font_color").unwrap_or(&default_font_color);

        // Verify input file exists
        if !Path::new(input_file).exists() {
            return Err(format!("Input file does not exist: {}", input_file).into());
        }

        // Create output directory if it doesn't exist
        if let Some(parent) = Path::new(output_file).parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        // Determine text position coordinates
        let position_coords = match position.as_str() {
            "top_left" => "10:10",
            "top_right" => "main_w-text_w-10:10",
            "bottom_left" => "10:main_h-text_h-10",
            "bottom_right" => "main_w-text_w-10:main_h-text_h-10",
            "center" => "main_w/2-text_w/2:main_h/2-text_h/2",
            _ => "main_w-text_w-10:main_h-text_h-10", // Default to bottom right
        };

        // Create FFmpeg command
        let drawtext_filter = format!(
            "drawtext=text='{}':fontsize={}:fontcolor={}:x={}:y={}",
            watermark_text,
            font_size,
            font_color,
            position_coords.split(':').next().unwrap(),
            position_coords.split(':').nth(1).unwrap()
        );

        // Execute FFmpeg command
        let output = Command::new("ffmpeg")
            .args([
                "-i",
                input_file,
                "-vf",
                &drawtext_filter,
                "-c:a",
                "copy",
                "-y", // Overwrite output file if it exists
                output_file,
            ])
            .output()?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(format!("FFmpeg command failed: {}", error).into());
        }

        // Verify output file was created
        if !Path::new(output_file).exists() {
            return Err("Failed to create output file".into());
        }

        Ok(())
    }

    fn get_parameter_info(&self) -> Vec<ParameterInfo> {
        vec![
            ParameterInfo {
                name: "input_file".to_string(),
                description: "Path to the input video file".to_string(),
                required: true,
                default_value: None,
                parameter_type: ParameterType::FilePath,
            },
            ParameterInfo {
                name: "output_file".to_string(),
                description: "Path to save the output video file".to_string(),
                required: true,
                default_value: None,
                parameter_type: ParameterType::FilePath,
            },
            ParameterInfo {
                name: "watermark_text".to_string(),
                description: "Text to use as watermark".to_string(),
                required: true,
                default_value: Some("© Video-ToolKit".to_string()),
                parameter_type: ParameterType::String,
            },
            ParameterInfo {
                name: "position".to_string(),
                description: "Position of the watermark (top_left, top_right, bottom_left, bottom_right, center)".to_string(),
                required: false,
                default_value: Some("bottom_right".to_string()),
                parameter_type: ParameterType::String,
            },
            ParameterInfo {
                name: "font_size".to_string(),
                description: "Font size for the watermark text".to_string(),
                required: false,
                default_value: Some("24".to_string()),
                parameter_type: ParameterType::Integer,
            },
            ParameterInfo {
                name: "font_color".to_string(),
                description: "Font color for the watermark text".to_string(),
                required: false,
                default_value: Some("white".to_string()),
                parameter_type: ParameterType::String,
            },
        ]
    }

    fn shutdown(&self) -> Result<(), Box<dyn std::error::Error>> {
        // Nothing to clean up
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// Export the plugin
plugin_system::export_plugin!(WatermarkPlugin);
