# WebM to GIF Conversion

Video Toolkit now includes specific support for converting WebM videos to optimized GIF format. WebM is a modern video format commonly used on the web and social media platforms.

## Features

- Convert WebM videos to optimized GIF format
- Automatically resize to appropriate dimensions
- Control FPS, file size, and optimization parameters
- Available via both GUI and command-line interfaces

## Using WebM to GIF Conversion

### Via GUI

1. Launch the Video Toolkit application
2. Select the "Convert to GIF" tab
3. Click "Browse" and select your WebM file
4. Set your preferred output options (width, FPS, max size)
5. Click "Convert to GIF"

### Via Command Line

```bash
# Basic WebM to GIF conversion
cargo run --release -- gif-converter --input video.webm --output output.gif

# Control optimization parameters
cargo run --release -- gif-converter --input video.webm --width 320 --fps 15 --max-size 2.5 --optimize

# Batch processing multiple WebM files
cargo run --release -- batch gif-converter ./webm_files --recursive --pattern "\.webm$" --width 320 --fps 10
