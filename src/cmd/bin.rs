// Bin subcommand: read a recipe.pb and output the specified byte range of the described file
//
// Resolves file references relative to the recipe.pb's directory.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use prost::Message;
use tracing::info;

use crate::proto;

pub fn run(recipe_path_str: &str, start: Option<u64>, end: Option<u64>) {
    let start = start.unwrap_or(0);

    let recipe_path = PathBuf::from(recipe_path_str);
    let recipe_base_dir = recipe_path
        .parent()
        .unwrap_or(&PathBuf::from("."))
        .to_owned();

    let recipe_bytes = fs::read(&recipe_path).expect("Failed to read recipe.pb");
    let recipe_file =
        proto::RecipeFile::decode(recipe_bytes.as_slice()).expect("Failed to decode recipe.pb");

    let recipe_v1 = match recipe_file.recipe {
        Some(proto::recipe_file::Recipe::V1(v1)) => v1,
        None => panic!("recipe.pb has no recipe"),
    };

    // Determine end: if not specified, use the byte after the last chunk
    let total_size = recipe_v1
        .chunks
        .iter()
        .map(|c| c.offset + c.size)
        .max()
        .unwrap_or(0);
    let end = end.unwrap_or(total_size);

    assert!(start <= end, "start ({}) must be <= end ({})", start, end);

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut bytes_written: u64 = 0;
    let total_needed = end - start;

    for chunk in &recipe_v1.chunks {
        let chunk_start = chunk.offset;
        let chunk_end = chunk.offset + chunk.size;

        // Skip chunks entirely before or after the requested range
        if chunk_end <= start || chunk_start >= end {
            continue;
        }

        // Compute the overlap
        let read_start = if start > chunk_start {
            start - chunk_start
        } else {
            0
        };
        let read_end = if end < chunk_end {
            end - chunk_start
        } else {
            chunk.size
        };
        let read_len = read_end - read_start;

        match &chunk.content {
            Some(proto::chunk::Content::Data(data)) => {
                assert_eq!(
                    data.len() as u64, chunk.size,
                    "inline data size mismatch at offset {}: expected {}, got {}",
                    chunk.offset, chunk.size, data.len()
                );
                let actual_crc32 = crc32fast::hash(data);
                assert_eq!(
                    actual_crc32, chunk.crc32,
                    "CRC-32 mismatch for inline data at offset {}",
                    chunk.offset
                );
                out.write_all(&data[read_start as usize..read_end as usize])
                    .expect("Failed to write to stdout");
            }
            Some(proto::chunk::Content::File(file_path)) => {
                let resolved = recipe_base_dir.join(file_path);
                let mut file = fs::File::open(&resolved).unwrap_or_else(|e| {
                    panic!("Failed to open {}: {}", resolved.display(), e)
                });

                // Verify file size matches
                let file_len = file.metadata().unwrap().len();
                assert_eq!(
                    file_len, chunk.size,
                    "file size mismatch for {}: expected {}, got {}",
                    resolved.display(),
                    chunk.size,
                    file_len
                );

                if read_start > 0 {
                    file.seek(SeekFrom::Start(read_start)).unwrap();
                }

                // Read the needed portion and verify CRC on the full file
                // For efficiency, if we're reading the full file, verify CRC directly
                // Otherwise, we need to read the full file for CRC verification
                let full_data = {
                    let mut buf = Vec::with_capacity(chunk.size as usize);
                    file.seek(SeekFrom::Start(0)).unwrap();
                    file.read_to_end(&mut buf).unwrap();
                    buf
                };

                let actual_crc32 = crc32fast::hash(&full_data);
                assert_eq!(
                    actual_crc32, chunk.crc32,
                    "CRC-32 mismatch for file {} at offset {}",
                    resolved.display(),
                    chunk.offset
                );

                out.write_all(&full_data[read_start as usize..read_end as usize])
                    .expect("Failed to write to stdout");
            }
            None => {
                panic!("chunk at offset {} has no content", chunk.offset);
            }
        }

        bytes_written += read_len;
        if bytes_written >= total_needed {
            break;
        }
    }

    out.flush().unwrap();
    info!(
        bytes_written,
        start,
        end,
        "output complete"
    );
}
