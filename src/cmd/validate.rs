// Validate subcommand: verify embedded source files in a combined MP4/ZIP
//
// This module is intentionally self-contained with NO dependencies on
// generation-side modules (generate, parse, helpers, etc.) so that it
// can serve as an independent audit tool.
//
// The output file is a polyglot MP4/ZIP. Source files are stored as ZIP
// entries (method 0 = stored) with ZIP64 extensions. This validator parses
// the ZIP central directory, cross-references with sources.json, and
// verifies CRC-32 and SHA-256 for each embedded file.

use std::collections::HashMap;
use std::fs;

use serde::Deserialize;
use sha2::{Digest, Sha256};

// ===== JSON types (duplicated intentionally for independence) =====

#[derive(Deserialize)]
struct SourcesOutput {
    files: Vec<SourceFile>,
}

#[derive(Deserialize)]
struct SourceFile {
    source: String,
    sha256: String,
    dest: SourceDest,
}

#[derive(Deserialize)]
struct SourceDest {
    #[allow(dead_code)]
    r#type: String,
    offset: u64,
    length: u64,
}

// ===== ZIP parsing helpers =====

struct ZipParsedEntry {
    filename: String,
    crc32: u32,
    uncompressed_size: u64,
    local_header_offset: u64,
    data_offset: u64,
}

fn read_u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(buf[off..off + 2].try_into().unwrap())
}

fn read_u32_le(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

fn read_u64_le(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(buf[off..off + 8].try_into().unwrap())
}

/// Scan backward from EOF for the EOCD signature (0x06054b50).
fn find_eocd(data: &[u8]) -> Option<usize> {
    // EOCD is at least 22 bytes. Max comment = 65535, so scan at most 22+65535 bytes.
    let start = data.len().saturating_sub(22 + 65535);
    for i in (start..data.len().saturating_sub(21)).rev() {
        if read_u32_le(data, i) == 0x06054b50 {
            return Some(i);
        }
    }
    None
}

/// Parse EOCD (and ZIP64 EOCD if present) to get CD offset, size, and entry count.
fn parse_eocd(data: &[u8], eocd_offset: usize) -> (u64, u64, u64) {
    let total_entries_16 = read_u16_le(data, eocd_offset + 8);
    let cd_size_32 = read_u32_le(data, eocd_offset + 12);
    let cd_offset_32 = read_u32_le(data, eocd_offset + 16);

    let need_zip64 = total_entries_16 == 0xFFFF
        || cd_size_32 == 0xFFFFFFFF
        || cd_offset_32 == 0xFFFFFFFF;

    if need_zip64 {
        // ZIP64 EOCD locator is 20 bytes before EOCD
        let locator_offset = eocd_offset - 20;
        assert_eq!(
            read_u32_le(data, locator_offset),
            0x07064b50,
            "ZIP64 EOCD locator signature mismatch"
        );
        let zip64_eocd_offset = read_u64_le(data, locator_offset + 8) as usize;
        assert_eq!(
            read_u32_le(data, zip64_eocd_offset),
            0x06064b50,
            "ZIP64 EOCD signature mismatch"
        );
        let total_entries = read_u64_le(data, zip64_eocd_offset + 32);
        let cd_size = read_u64_le(data, zip64_eocd_offset + 40);
        let cd_offset = read_u64_le(data, zip64_eocd_offset + 48);
        (cd_offset, cd_size, total_entries)
    } else {
        (
            cd_offset_32 as u64,
            cd_size_32 as u64,
            total_entries_16 as u64,
        )
    }
}

/// Parse central directory entries and compute data offsets from local headers.
fn parse_central_directory(
    file_data: &[u8],
    cd_offset: u64,
    cd_size: u64,
    total_entries: u64,
) -> Vec<ZipParsedEntry> {
    let cd = &file_data[cd_offset as usize..(cd_offset + cd_size) as usize];
    let mut entries = Vec::new();
    let mut pos = 0usize;

    for _ in 0..total_entries {
        assert_eq!(
            read_u32_le(cd, pos),
            0x02014b50,
            "Invalid central directory entry signature"
        );
        let crc32 = read_u32_le(cd, pos + 16);
        let compressed_size_32 = read_u32_le(cd, pos + 20);
        let uncompressed_size_32 = read_u32_le(cd, pos + 24);
        let filename_len = read_u16_le(cd, pos + 28) as usize;
        let extra_len = read_u16_le(cd, pos + 30) as usize;
        let comment_len = read_u16_le(cd, pos + 32) as usize;
        let local_offset_32 = read_u32_le(cd, pos + 42);

        let filename =
            String::from_utf8_lossy(&cd[pos + 46..pos + 46 + filename_len]).to_string();

        let extra_start = pos + 46 + filename_len;
        let extra_data = &cd[extra_start..extra_start + extra_len];

        // Parse ZIP64 extra field if present
        let mut uncompressed_size = uncompressed_size_32 as u64;
        let mut _compressed_size = compressed_size_32 as u64;
        let mut local_header_offset = local_offset_32 as u64;

        let mut epos = 0;
        while epos + 4 <= extra_data.len() {
            let header_id = read_u16_le(extra_data, epos);
            let data_size = read_u16_le(extra_data, epos + 2) as usize;
            if header_id == 0x0001 {
                let mut zpos = epos + 4;
                if uncompressed_size_32 == 0xFFFFFFFF && zpos + 8 <= epos + 4 + data_size {
                    uncompressed_size = read_u64_le(extra_data, zpos);
                    zpos += 8;
                }
                if compressed_size_32 == 0xFFFFFFFF && zpos + 8 <= epos + 4 + data_size {
                    _compressed_size = read_u64_le(extra_data, zpos);
                    zpos += 8;
                }
                if local_offset_32 == 0xFFFFFFFF && zpos + 8 <= epos + 4 + data_size {
                    local_header_offset = read_u64_le(extra_data, zpos);
                }
                break;
            }
            epos += 4 + data_size;
        }

        // Compute data offset from local file header
        assert_eq!(
            read_u32_le(file_data, local_header_offset as usize),
            0x04034b50,
            "Invalid local file header signature at offset {}",
            local_header_offset
        );
        let local_filename_len =
            read_u16_le(file_data, local_header_offset as usize + 26) as u64;
        let local_extra_len = read_u16_le(file_data, local_header_offset as usize + 28) as u64;
        let data_offset = local_header_offset + 30 + local_filename_len + local_extra_len;

        entries.push(ZipParsedEntry {
            filename,
            crc32,
            uncompressed_size,
            local_header_offset,
            data_offset,
        });

        pos += 46 + filename_len + extra_len + comment_len;
    }

    entries
}

// ===== Validation =====

pub fn run(sources_json_path: &str, mp4_path: &str) {
    let json_str = fs::read_to_string(sources_json_path).expect("Failed to read sources.json");
    let sources: SourcesOutput =
        serde_json::from_str(&json_str).expect("Failed to parse sources.json");

    let file_data = fs::read(mp4_path).expect("Failed to read MP4/ZIP file");

    // Parse ZIP structure from end of file
    let eocd_offset = find_eocd(&file_data).expect("Failed to find ZIP End of Central Directory");
    let (cd_offset, cd_size, total_entries) = parse_eocd(&file_data, eocd_offset);
    let zip_entries = parse_central_directory(&file_data, cd_offset, cd_size, total_entries);

    info_zip_entries(&zip_entries);

    // Build map: data_offset → zip_entry index
    let mut entry_by_offset: HashMap<u64, usize> = HashMap::new();
    for (idx, entry) in zip_entries.iter().enumerate() {
        entry_by_offset.insert(entry.data_offset, idx);
    }

    let mut errors = 0u64;
    let total = sources.files.len();

    for (i, file) in sources.files.iter().enumerate() {
        // Find matching ZIP entry by data offset
        let zip_entry = match entry_by_offset.get(&file.dest.offset) {
            Some(&idx) => &zip_entries[idx],
            None => {
                eprintln!(
                    "[{}] {}: no matching ZIP entry at data offset {}",
                    i, file.source, file.dest.offset
                );
                errors += 1;
                continue;
            }
        };

        // Verify size
        if zip_entry.uncompressed_size != file.dest.length {
            eprintln!(
                "[{}] {}: size mismatch: ZIP header={}, sources.json={}",
                i, file.source, zip_entry.uncompressed_size, file.dest.length
            );
            errors += 1;
            continue;
        }

        // Read source data and verify CRC-32
        let data =
            &file_data[file.dest.offset as usize..(file.dest.offset + file.dest.length) as usize];

        let actual_crc32 = crc32fast::hash(data);
        if actual_crc32 != zip_entry.crc32 {
            eprintln!(
                "[{}] {}: CRC-32 mismatch: ZIP header={:08x}, actual={:08x}",
                i, file.source, zip_entry.crc32, actual_crc32
            );
            errors += 1;
            continue;
        }

        // Verify SHA-256
        let actual_sha256 = format!("{:x}", Sha256::digest(data));
        if actual_sha256 != file.sha256 {
            eprintln!("[{}] {}: SHA-256 mismatch", i, file.source);
            eprintln!("  sources.json: {}", file.sha256);
            eprintln!("  actual:       {}", actual_sha256);
            errors += 1;
            continue;
        }
    }

    if errors > 0 {
        eprintln!("{} errors out of {} files", errors, total);
        std::process::exit(1);
    } else {
        println!("{} files validated successfully", total);
    }
}

fn info_zip_entries(entries: &[ZipParsedEntry]) {
    eprintln!("ZIP entries found: {}", entries.len());
    for entry in entries {
        eprintln!(
            "  {} (size={}, crc32={:08x}, local_hdr=0x{:x}, data=0x{:x})",
            entry.filename,
            entry.uncompressed_size,
            entry.crc32,
            entry.local_header_offset,
            entry.data_offset,
        );
    }
}
