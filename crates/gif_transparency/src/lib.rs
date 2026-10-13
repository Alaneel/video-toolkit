use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use walkdir::WalkDir;

use common::{Result, VideoToolkitError};

/// Checks if a file is a GIF by verifying its magic number
fn is_gif_file(path: &Path) -> bool {
    if let Ok(mut file) = File::open(path) {
        let mut buffer = [0; 6];
        if file.read_exact(&mut buffer).is_ok() {
            // Check for the GIF magic number (GIF87a or GIF89a)
            return buffer.starts_with(b"GIF87a") || buffer.starts_with(b"GIF89a");
        }
    }
    false
}

fn malformed_gif(message: &str) -> VideoToolkitError {
    VideoToolkitError::Other(format!("Malformed GIF: {message}"))
}

fn sub_blocks_end(data: &[u8], mut position: usize) -> Result<usize> {
    loop {
        let size = *data
            .get(position)
            .ok_or_else(|| malformed_gif("truncated data block"))? as usize;
        position += 1;
        if size == 0 {
            return Ok(position);
        }
        position = position
            .checked_add(size)
            .filter(|end| *end <= data.len())
            .ok_or_else(|| malformed_gif("data block exceeds file size"))?;
    }
}

fn add_background_transparency(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 14 || (!data.starts_with(b"GIF87a") && !data.starts_with(b"GIF89a")) {
        return Err(malformed_gif("missing a valid GIF header"));
    }

    let packed = data[10];
    let background_index = data[11];
    let global_table_size = if packed & 0x80 != 0 {
        3usize << ((packed & 0x07) + 1)
    } else {
        0
    };
    let mut position = 13usize
        .checked_add(global_table_size)
        .filter(|end| *end <= data.len())
        .ok_or_else(|| malformed_gif("truncated global color table"))?;

    let mut output = data[..position].to_vec();
    output[..6].copy_from_slice(b"GIF89a");
    let mut pending_transparency_index = None;
    let mut found_image = false;

    while position < data.len() {
        match data[position] {
            0x21 => {
                let label = *data
                    .get(position + 1)
                    .ok_or_else(|| malformed_gif("truncated extension"))?;
                if label == 0xF9 {
                    if data.get(position + 2) != Some(&4) || position + 8 > data.len() {
                        return Err(malformed_gif("invalid graphic control extension"));
                    }
                    let mut extension = data[position..position + 8].to_vec();
                    extension[3] |= 0x01;
                    extension[6] = background_index;
                    pending_transparency_index = Some(output.len() + 6);
                    output.extend_from_slice(&extension);
                    position += 8;
                } else {
                    let end = sub_blocks_end(data, position + 2)?;
                    output.extend_from_slice(&data[position..end]);
                    position = end;
                }
            }
            0x2C => {
                if position + 10 > data.len() {
                    return Err(malformed_gif("truncated image descriptor"));
                }
                let local_table = data[position + 9] & 0x80 != 0;
                let transparent_index = if local_table { 0 } else { background_index };
                if let Some(index_offset) = pending_transparency_index.take() {
                    output[index_offset] = transparent_index;
                } else {
                    output.extend_from_slice(&[
                        0x21,
                        0xF9,
                        0x04,
                        0x01,
                        0x00,
                        0x00,
                        transparent_index,
                        0x00,
                    ]);
                }

                let local_table_size = if local_table {
                    3usize << ((data[position + 9] & 0x07) + 1)
                } else {
                    0
                };
                let image_data_start = position + 10 + local_table_size;
                if image_data_start >= data.len() {
                    return Err(malformed_gif("truncated image data"));
                }
                let end = sub_blocks_end(data, image_data_start + 1)?;
                output.extend_from_slice(&data[position..end]);
                position = end;
                found_image = true;
            }
            0x3B => {
                output.push(0x3B);
                position += 1;
                if position != data.len() {
                    return Err(malformed_gif("unexpected bytes after trailer"));
                }
                break;
            }
            _ => return Err(malformed_gif("unknown block marker")),
        }
    }

    if !found_image || output.last() != Some(&0x3B) {
        return Err(malformed_gif("missing image data or trailer"));
    }
    Ok(output)
}

/// Marks the GIF logical background color as transparent for every frame.
pub fn make_gif_transparent(file_path: &Path) -> Result<()> {
    make_gif_transparent_with_backup(file_path, false)
}

/// Validate first and preserve the first backup when transparency is applied again.
pub fn make_gif_transparent_with_backup(file_path: &Path, create_backup: bool) -> Result<()> {
    let data = fs::read(file_path).map_err(VideoToolkitError::IoError)?;
    let transparent = add_background_transparency(&data)?;
    if create_backup {
        let backup_path = file_path.with_extension("gif.bak");
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup_path)
        {
            Ok(mut backup) => {
                if let Err(error) = backup.write_all(&data).and_then(|()| backup.sync_all()) {
                    drop(backup);
                    let _ = fs::remove_file(backup_path);
                    return Err(VideoToolkitError::IoError(error));
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists && backup_path.is_file() => {}
            Err(error) => return Err(VideoToolkitError::IoError(error)),
        }
    }
    fs::write(file_path, transparent).map_err(VideoToolkitError::IoError)
}

/// Process multiple GIF files in batch, making them transparent
pub fn batch_process_gifs(
    input_paths: &[PathBuf],
    recursive: bool,
    create_backup: bool,
) -> Result<(usize, usize)> {
    // Collect all GIF files
    let mut gif_files = Vec::new();

    for path in input_paths {
        if path.is_dir() && recursive {
            // Recursively walk directory
            for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
                let entry_path = entry.path();
                if entry_path.is_file()
                    && entry_path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("gif"))
                    && is_gif_file(entry_path)
                {
                    gif_files.push(entry_path.to_owned());
                }
            }
        } else if path.is_file()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("gif"))
            && is_gif_file(path)
        {
            gif_files.push(path.to_owned());
        }
    }

    if gif_files.is_empty() {
        return Err(VideoToolkitError::Other("No GIF files found".to_string()));
    }

    let total_files = gif_files.len();
    let progress_bar = ProgressBar::new(total_files as u64);
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
            )
            .unwrap(),
    );

    // Process files in parallel
    let results: Vec<Result<()>> = gif_files
        .par_iter()
        .map(|file_path| {
            let result = make_gif_transparent_with_backup(file_path, create_backup);
            progress_bar.inc(1);
            result
        })
        .collect();

    progress_bar.finish_with_message("GIF processing complete");

    // Count successful operations
    let success_count = results.iter().filter(|r| r.is_ok()).count();

    Ok((success_count, total_files))
}

/// Find and process all GIFs in a directory
pub fn process_directory(
    dir_path: &str,
    recursive: bool,
    create_backup: bool,
) -> Result<(usize, usize)> {
    let path = Path::new(dir_path);

    if !path.exists() {
        return Err(VideoToolkitError::Other(format!(
            "Directory not found: {}",
            dir_path
        )));
    }

    if !path.is_dir() {
        return Err(VideoToolkitError::Other(format!(
            "Not a directory: {}",
            dir_path
        )));
    }

    batch_process_gifs(&[path.to_path_buf()], recursive, create_backup)
}

#[cfg(test)]
mod tests {
    use super::add_background_transparency;

    const ONE_PIXEL_GIF: &[u8] = &[
        b'G', b'I', b'F', b'8', b'7', b'a', 1, 0, 1, 0, 0x80, 0, 0, 0, 0, 0, 255, 255, 255, 0x2C,
        0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x4C, 0x01, 0, 0x3B,
    ];

    #[test]
    fn adds_a_valid_transparency_extension() {
        let output = add_background_transparency(ONE_PIXEL_GIF).unwrap();
        assert!(output.starts_with(b"GIF89a"));
        assert_eq!(output.last(), Some(&0x3B));
        assert!(output
            .windows(8)
            .any(|block| block == [0x21, 0xF9, 0x04, 0x01, 0, 0, 0, 0]));
    }

    #[test]
    fn remains_idempotent_and_valid() {
        let once = add_background_transparency(ONE_PIXEL_GIF).unwrap();
        let twice = add_background_transparency(&once).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn rejects_truncated_input() {
        assert!(add_background_transparency(b"GIF89a").is_err());
    }
}
