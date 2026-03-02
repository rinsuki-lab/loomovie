// Validate subcommand: verify embedded source files in a combined MP4
//
// This module is intentionally self-contained with NO dependencies on
// generation-side modules (generate, parse, helpers, etc.) so that it
// can serve as an independent audit tool.

use std::fs;
use std::io::{Read, Seek, SeekFrom};

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

// ===== Validation =====

pub fn run(sources_json_path: &str, mp4_path: &str) {
    let json_str = fs::read_to_string(sources_json_path).expect("Failed to read sources.json");
    let sources: SourcesOutput =
        serde_json::from_str(&json_str).expect("Failed to parse sources.json");

    let mut mp4_file = fs::File::open(mp4_path).expect("Failed to open MP4 file");

    let mut errors = 0u64;
    let total = sources.files.len();

    for (i, file) in sources.files.iter().enumerate() {
        // Read lmc1 header (16 bytes immediately before the data offset)
        if file.dest.offset < 16 {
            eprintln!(
                "[{}] {}: offset {} too small for lmc1 header",
                i, file.source, file.dest.offset
            );
            errors += 1;
            continue;
        }

        let header_offset = file.dest.offset - 16;
        mp4_file.seek(SeekFrom::Start(header_offset)).unwrap();
        let mut header = [0u8; 16];
        mp4_file.read_exact(&mut header).unwrap();

        // Verify signature
        if &header[0..4] != b"lmc1" {
            eprintln!(
                "[{}] {}: invalid lmc1 signature: {:02x}{:02x}{:02x}{:02x}",
                i, file.source, header[0], header[1], header[2], header[3]
            );
            errors += 1;
            continue;
        }

        // Parse header fields
        let header_crc32 = u32::from_be_bytes(header[8..12].try_into().unwrap());
        let header_size = u32::from_be_bytes(header[12..16].try_into().unwrap());

        // Check size matches between lmc1 header and sources.json
        if header_size as u64 != file.dest.length {
            eprintln!(
                "[{}] {}: size mismatch: lmc1 header={}, sources.json={}",
                i, file.source, header_size, file.dest.length
            );
            errors += 1;
            continue;
        }

        // Read source data
        mp4_file.seek(SeekFrom::Start(file.dest.offset)).unwrap();
        let mut data = vec![0u8; file.dest.length as usize];
        mp4_file.read_exact(&mut data).unwrap();

        // Verify CRC-32
        let actual_crc32 = crc32fast::hash(&data);
        if actual_crc32 != header_crc32 {
            eprintln!(
                "[{}] {}: CRC-32 mismatch: lmc1 header={:08x}, actual={:08x}",
                i, file.source, header_crc32, actual_crc32
            );
            errors += 1;
            continue;
        }

        // Verify SHA-256
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let actual_sha256 = format!("{:x}", hasher.finalize());
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
