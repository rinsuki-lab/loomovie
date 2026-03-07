// Plan subcommand: generate a recipe.pb describing how to assemble a Hybrid MP4
//
// Layout of the described MP4 (init + data concatenated):
//   [ftyp]
//   [moov with full stbl (co64 pointing into mdat)]
//   [mdat]
//     for each chunk_idx, stream_idx:
//       [zip_local_header][original_chunk_file_bytes]
//   [free: deflated generated.m3u8 (master playlist)]
//   [free: deflated streams.N/generated.m3u8 (per-stream playlists)]
//   [free: streams.N/init.m4s]
//   [free: zip central directory + zip64 eocd + zip64 locator + eocd]
//
// The moov's sample tables reference actual sample data positions inside
// the original chunk files, skipping over fMP4 structural boxes (moof, mdat
// headers etc.) that are embedded verbatim.
//
// The file is also a valid ZIP archive. Media/init files use method 0 (stored),
// while m3u8 playlists use method 8 (deflated). Entry names use the pattern:
// streams.N/init.m4s, streams.N/chunks/chunk.NNNNNN.m4s, and generated.m3u8.

mod binary;
mod boxes;
mod hls;
mod mp4_box;
mod parse;
mod types;
mod zip;

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use flate2::write::DeflateEncoder;
use flate2::Compression;
use prost::Message;
use tracing::{debug, info};

use self::boxes::*;
use self::mp4_box::make_free_header;
use self::parse::*;
use self::types::*;

use crate::proto;

use self::zip::ZipFileEntry;

/// Compute a relative path from `base` to `target`.
/// Both paths should be absolute (canonicalized) for reliable results.
fn relative_path(base: &Path, target: &Path) -> PathBuf {
    let base_components: Vec<_> = base.components().collect();
    let target_components: Vec<_> = target.components().collect();

    // Find common prefix length
    let common_len = base_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut result = PathBuf::new();
    // Go up from base to common ancestor
    for _ in common_len..base_components.len() {
        result.push("..");
    }
    // Go down from common ancestor to target
    for component in &target_components[common_len..] {
        if let Component::Normal(c) = component {
            result.push(c);
        }
    }
    result
}

/// Create a recipe Chunk with inline data content
fn make_data_chunk(offset: u64, data: Vec<u8>) -> proto::Chunk {
    let crc32 = crc32fast::hash(&data);
    let size = data.len() as u64;
    proto::Chunk {
        offset,
        size,
        crc32,
        content: Some(proto::chunk::Content::Data(data)),
    }
}

/// Create a recipe Chunk referencing a file
fn make_file_chunk(offset: u64, file_path: String, file_data: &[u8]) -> proto::Chunk {
    let crc32 = crc32fast::hash(file_data);
    let size = file_data.len() as u64;
    proto::Chunk {
        offset,
        size,
        crc32,
        content: Some(proto::chunk::Content::File(file_path)),
    }
}

pub fn run(json_path_str: &str, recipe_path_str: &str) {
    let json_path = PathBuf::from(json_path_str);
    let json_base_dir = json_path.parent().unwrap_or(&PathBuf::from(".")).to_owned();

    let recipe_path = fs::canonicalize(PathBuf::from(recipe_path_str).parent().unwrap_or(Path::new(".")))
        .unwrap_or_else(|_| PathBuf::from(recipe_path_str).parent().unwrap_or(Path::new(".")).to_owned());
    let recipe_base_dir = recipe_path.clone();
    let recipe_out_path = recipe_base_dir.join(
        PathBuf::from(recipe_path_str)
            .file_name()
            .expect("recipe path must have a filename"),
    );

    let json_str = fs::read_to_string(&json_path).expect("Failed to read streams.json");
    let config: InputConfig = serde_json::from_str(&json_str).expect("Failed to parse JSON");

    let num_streams = config.streams.len();
    assert!(num_streams > 0, "No streams specified");

    let num_chunks = config.streams[0].chunks.len();
    for (i, stream) in config.streams.iter().enumerate() {
        assert_eq!(
            stream.chunks.len(),
            num_chunks,
            "Stream {} has {} chunks, expected {}",
            i,
            stream.chunks.len(),
            num_chunks
        );
    }

    info!(num_streams, num_chunks, "processing streams");

    // ===== Phase 1: Parse init segments =====
    let mut tracks: Vec<TrackInfo> = Vec::new();
    let mut init_file_data: Vec<(String, Vec<u8>)> = Vec::new();

    for (i, stream) in config.streams.iter().enumerate() {
        let init_path = json_base_dir.join(&stream.init);
        info!(path = %init_path.display(), "reading init segment");
        let init_data = fs::read(&init_path).expect("Failed to read init segment");
        let new_track_id = (i + 1) as u32;
        let track_info = parse_init_segment(&init_data, new_track_id);
        info!(
            track_id = new_track_id,
            timescale = track_info.timescale,
            handler = %std::str::from_utf8(&track_info.handler_type).unwrap_or("????"),
            "parsed init segment"
        );
        tracks.push(track_info);
        init_file_data.push((stream.init.clone(), init_data));
    }

    // ===== Phase 2: Parse all chunks =====
    // parsed_chunks[stream_idx][chunk_idx] = ChunkParseResult
    let mut parsed_chunks: Vec<Vec<ChunkParseResult>> = Vec::new();
    // chunk_file_rel_paths[stream_idx][chunk_idx] = path relative to json_base_dir
    let mut chunk_file_rel_paths: Vec<Vec<String>> = Vec::new();

    for (stream_idx, stream) in config.streams.iter().enumerate() {
        let init_parent = PathBuf::from(&stream.init)
            .parent()
            .map(|p| p.to_owned())
            .unwrap_or_default();

        let mut stream_parsed = Vec::new();
        let mut stream_rel_paths = Vec::new();
        for chunk_name in &stream.chunks {
            let chunk_path = json_base_dir.join(&init_parent).join(chunk_name);
            let chunk_data = fs::read(&chunk_path)
                .unwrap_or_else(|e| panic!("Failed to read chunk {}: {}", chunk_path.display(), e));
            let parsed = parse_chunk(&chunk_data);
            let relative_source = init_parent.join(chunk_name).to_string_lossy().to_string();
            stream_parsed.push(parsed);
            stream_rel_paths.push(relative_source);
        }
        parsed_chunks.push(stream_parsed);
        chunk_file_rel_paths.push(stream_rel_paths);
        info!(stream_idx, num_chunks, "parsed chunks for stream");
    }

    // ===== Phase 3: Collect sample tables & data layout =====

    // chunk_data_sizes[chunk_idx][stream_idx] = zip_header_size + file_size
    let mut chunk_data_sizes: Vec<Vec<usize>> = Vec::new();
    for chunk_idx in 0..num_chunks {
        let mut sizes = Vec::new();
        for stream_idx in 0..num_streams {
            let ref parsed = parsed_chunks[stream_idx][chunk_idx];
            let filename = zip::entry_name_chunk(stream_idx, chunk_idx);
            sizes.push(zip::local_file_header_size(filename.len()) + parsed.file_size);
        }
        chunk_data_sizes.push(sizes);
    }

    // Total mdat payload size (excluding mdat box header)
    let total_mdat_payload: u64 = chunk_data_sizes
        .iter()
        .flat_map(|v| v.iter())
        .map(|&s| s as u64)
        .sum();

    // Use large (64-bit) mdat box if payload + 8 > u32::MAX
    let mdat_header_size: u64 = if total_mdat_payload + 8 > u32::MAX as u64 {
        16 // extended size
    } else {
        8
    };

    // Build per-track sample tables (with placeholder chunk_offsets for now)
    let mut sample_tables: Vec<TrackSampleTable> = Vec::new();
    // chunk_durations_sec[stream_idx][chunk_idx] = duration in seconds (f64)
    let mut chunk_durations_sec: Vec<Vec<f64>> = Vec::new();

    for stream_idx in 0..num_streams {
        let track = &tracks[stream_idx];
        let mut st = TrackSampleTable {
            media_start_time: 0,
            total_duration: 0,
            sample_sizes: Vec::new(),
            sample_durations: Vec::new(),
            has_cts: false,
            cts_version: 0,
            cts_offsets: Vec::new(),
            sync_samples: Vec::new(),
            samples_per_chunk: Vec::new(),
            chunk_offsets: Vec::new(),
        };

        let mut sample_number: u32 = 0;
        let mut is_first_fragment = true;
        let mut stream_chunk_durations: Vec<f64> = Vec::new();

        for chunk_idx in 0..num_chunks {
            let ref parsed = parsed_chunks[stream_idx][chunk_idx];
            let mut chunk_duration_ticks: u64 = 0;
            for frag in &parsed.fragments {
                // Record base_media_decode_time from the very first fragment
                if is_first_fragment {
                    if let Some(ref tfdt) = frag.tfdt {
                        st.media_start_time = tfdt.base_media_decode_time;
                    }
                    is_first_fragment = false;
                }
                let frag_sample_count = frag.trun.sample_count;
                st.samples_per_chunk.push(frag_sample_count);
                st.chunk_offsets.push(0); // placeholder — filled in after moov size is known

                for (i, sample) in frag.trun.samples.iter().enumerate() {
                    sample_number += 1;

                    // Duration: sample > tfhd default > trex default
                    let duration = sample.duration.unwrap_or_else(|| {
                        frag.tfhd
                            .default_sample_duration
                            .unwrap_or(track.trex_default_sample_duration)
                    });
                    st.sample_durations.push(duration);
                    st.total_duration += duration as u64;
                    chunk_duration_ticks += duration as u64;

                    // Size: sample > tfhd default > trex default
                    let size = sample.size.unwrap_or_else(|| {
                        frag.tfhd
                            .default_sample_size
                            .unwrap_or(track.trex_default_sample_size)
                    });
                    st.sample_sizes.push(size);

                    // Flags (for sync sample detection)
                    let flags = if i == 0 {
                        frag.trun.first_sample_flags.unwrap_or_else(|| {
                            sample.flags.unwrap_or_else(|| {
                                frag.tfhd
                                    .default_sample_flags
                                    .unwrap_or(track.trex_default_sample_flags)
                            })
                        })
                    } else {
                        sample.flags.unwrap_or_else(|| {
                            frag.tfhd
                                .default_sample_flags
                                .unwrap_or(track.trex_default_sample_flags)
                        })
                    };

                    // Check sync: sample_depends_on==2 means I-frame, sample_is_non_sync==0 means sync
                    let sample_depends_on = (flags >> 24) & 0x3;
                    let sample_is_non_sync = (flags >> 16) & 0x1;
                    if sample_depends_on == 2 || sample_is_non_sync == 0 {
                        st.sync_samples.push(sample_number);
                    }

                    // Composition time offset
                    if let Some(raw) = sample.composition_time_offset_raw {
                        if !st.has_cts {
                            st.has_cts = true;
                            st.cts_version = frag.trun.version; // v0=unsigned, v1=signed
                            // Back-fill zeros for samples already processed
                            st.cts_offsets
                                .resize(st.sample_sizes.len() - 1, 0);
                        }
                        if frag.trun.version >= 1 {
                            st.cts_offsets.push(raw as i32);
                        } else {
                            st.cts_offsets.push(raw as i32);
                        }
                    } else if st.has_cts {
                        st.cts_offsets.push(0);
                    }
                }
            }
            stream_chunk_durations.push(chunk_duration_ticks as f64 / track.timescale as f64);
        }
        chunk_durations_sec.push(stream_chunk_durations);

        info!(
            stream_idx,
            samples = st.sample_sizes.len(),
            chunks = st.chunk_offsets.len(),
            sync_samples = st.sync_samples.len(),
            duration = st.total_duration,
            "collected sample table"
        );

        sample_tables.push(st);
    }

    // ===== Phase 4: Build recipe chunks =====

    let mut recipe_chunks: Vec<proto::Chunk> = Vec::new();
    let mut current_offset: u64 = 0;

    // --- ftyp ---
    let ftyp = generate_ftyp();
    recipe_chunks.push(make_data_chunk(current_offset, ftyp.clone()));
    current_offset += ftyp.len() as u64;

    // (init files are now placed after mdat, as ZIP-only entries)

    // Generate moov with placeholder offsets to determine its size
    let moov_placeholder = generate_hybrid_moov(&tracks, &sample_tables);
    let moov_size = moov_placeholder.len();

    let init_size = current_offset as usize + moov_size;

    // Now compute real chunk offsets
    // mdat content starts at init_size + mdat_header_size
    let mdat_content_start = init_size as u64 + mdat_header_size;

    let mut data_pos: u64 = 0; // offset within mdat content
    let mut chunk_offset_cursor: Vec<usize> = vec![0; num_streams]; // per-stream stbl chunk index

    for chunk_idx in 0..num_chunks {
        for stream_idx in 0..num_streams {
            let ref parsed = parsed_chunks[stream_idx][chunk_idx];
            let filename = zip::entry_name_chunk(stream_idx, chunk_idx);
            let zip_hdr_size = zip::local_file_header_size(filename.len()) as u64;

            // This chunk file's data starts at:
            let chunk_file_start = mdat_content_start + data_pos + zip_hdr_size;

            // Each fragment within this chunk file becomes one stbl chunk
            for frag in &parsed.fragments {
                let sample_data_abs =
                    chunk_file_start + frag.moof_offset as u64 + frag.original_data_offset as u64;

                let stbl_chunk_idx = chunk_offset_cursor[stream_idx];
                sample_tables[stream_idx].chunk_offsets[stbl_chunk_idx] = sample_data_abs;
                chunk_offset_cursor[stream_idx] += 1;
            }

            data_pos += zip_hdr_size + parsed.file_size as u64;
        }
    }

    // Regenerate moov with real offsets
    let moov = generate_hybrid_moov(&tracks, &sample_tables);
    assert_eq!(
        moov.len(),
        moov_size,
        "moov size changed after filling offsets"
    );

    // --- moov ---
    recipe_chunks.push(make_data_chunk(current_offset, moov));
    current_offset += moov_size as u64;

    // --- mdat header ---
    let mdat_header = if mdat_header_size == 16 {
        let total_mdat_box_size = mdat_header_size + total_mdat_payload;
        let mut buf = Vec::with_capacity(16);
        buf.extend_from_slice(&1u32.to_be_bytes()); // size=1 (use extended)
        buf.extend_from_slice(b"mdat");
        buf.extend_from_slice(&total_mdat_box_size.to_be_bytes());
        buf
    } else {
        let total_mdat_box_size = (mdat_header_size + total_mdat_payload) as u32;
        let mut buf = Vec::with_capacity(8);
        buf.extend_from_slice(&total_mdat_box_size.to_be_bytes());
        buf.extend_from_slice(b"mdat");
        buf
    };
    recipe_chunks.push(make_data_chunk(current_offset, mdat_header.clone()));
    current_offset += mdat_header.len() as u64;

    // --- mdat content: zip local headers + chunk files ---
    let mut zip_file_entries: Vec<ZipFileEntry> = Vec::new();
    for chunk_idx in 0..num_chunks {
        for stream_idx in 0..num_streams {
            let ref parsed = parsed_chunks[stream_idx][chunk_idx];
            let chunk_rel = &chunk_file_rel_paths[stream_idx][chunk_idx];

            let chunk_abs_path = json_base_dir.join(chunk_rel);
            let chunk_data = fs::read(&chunk_abs_path).unwrap();

            let filename = zip::entry_name_chunk(stream_idx, chunk_idx);
            let crc = crc32fast::hash(&chunk_data);
            let file_size = chunk_data.len() as u64;
            let zip_header = zip::make_local_file_header(filename.as_bytes(), crc, 0, file_size, file_size);

            // Record ZIP entry for central directory
            zip_file_entries.push(ZipFileEntry {
                filename: filename.into_bytes(),
                crc32: crc,
                compressed_size: file_size,
                uncompressed_size: file_size,
                compression_method: 0,
                local_header_offset: current_offset,
            });

            // ZIP local header as inline data
            let zip_header_len = zip_header.len() as u64;
            recipe_chunks.push(make_data_chunk(current_offset, zip_header));
            current_offset += zip_header_len;

            // chunk file as file reference
            let chunk_abs_canon = fs::canonicalize(&chunk_abs_path)
                .expect("Failed to canonicalize chunk path");
            let chunk_rel_to_recipe = relative_path(&recipe_base_dir, &chunk_abs_canon)
                .to_string_lossy()
                .to_string();
            recipe_chunks.push(make_file_chunk(
                current_offset,
                chunk_rel_to_recipe,
                &chunk_data,
            ));
            current_offset += parsed.file_size as u64;
        }

        if chunk_idx % 100 == 0 && chunk_idx > 0 {
            debug!(chunk_idx, num_chunks, "progress");
        }
    }

    // --- Trailing ZIP entries (wrapped in free boxes for MP4 compatibility) ---
    // Order: generated.m3u8, streams.N/generated.m3u8, streams.N/init.m4s, then CD

    // Helper: emit a small file as a free-box-wrapped ZIP entry with deflate compression
    let emit_zip_entry_deflated = |recipe_chunks: &mut Vec<proto::Chunk>,
                                        zip_file_entries: &mut Vec<ZipFileEntry>,
                                        current_offset: &mut u64,
                                        entry_name: &[u8],
                                        data: &[u8]| {
        let crc = crc32fast::hash(data);
        let uncompressed_size = data.len() as u64;
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(data).expect("deflate write failed");
        let compressed = encoder.finish().expect("deflate finish failed");
        let compressed_size = compressed.len() as u64;
        let zip_header = zip::make_local_file_header(entry_name, crc, 8, compressed_size, uncompressed_size);
        let free_header = make_free_header(zip_header.len() + compressed.len());

        let zip_local_header_offset = *current_offset + 8;
        zip_file_entries.push(ZipFileEntry {
            filename: entry_name.to_vec(),
            crc32: crc,
            compressed_size,
            uncompressed_size,
            compression_method: 8,
            local_header_offset: zip_local_header_offset,
        });

        let mut chunk = Vec::with_capacity(8 + zip_header.len() + compressed.len());
        chunk.extend_from_slice(&free_header);
        chunk.extend_from_slice(&zip_header);
        chunk.extend_from_slice(&compressed);
        recipe_chunks.push(make_data_chunk(*current_offset, chunk.clone()));
        *current_offset += chunk.len() as u64;
    };

    // Build per-stream info for HLS playlist generation
    let stream_infos: Vec<hls::StreamInfo> = (0..num_streams).map(|stream_idx| {
        let chunk_sizes: Vec<u64> = parsed_chunks[stream_idx].iter().map(|c| c.file_size as u64).collect();
        let bandwidth = hls::compute_peak_bandwidth(&chunk_sizes, &chunk_durations_sec[stream_idx]);
        hls::StreamInfo {
            codecs: &config.streams[stream_idx].codecs,
            bandwidth,
            is_video: &tracks[stream_idx].handler_type == b"vide",
            is_audio: &tracks[stream_idx].handler_type == b"soun",
        }
    }).collect();

    // 1) Generate master playlist (generated.m3u8)
    {
        let playlist_bytes = hls::generate_master_playlist(&stream_infos);
        emit_zip_entry_deflated(
            &mut recipe_chunks,
            &mut zip_file_entries,
            &mut current_offset,
            b"generated.m3u8",
            &playlist_bytes,
        );
    }

    // 2) Generate per-stream media playlists (streams.N/generated.m3u8)
    for stream_idx in 0..num_streams {
        let playlist_bytes = hls::generate_media_playlist(&chunk_durations_sec[stream_idx]);
        let playlist_name = format!("streams.{}/generated.m3u8", stream_idx);
        emit_zip_entry_deflated(
            &mut recipe_chunks,
            &mut zip_file_entries,
            &mut current_offset,
            playlist_name.as_bytes(),
            &playlist_bytes,
        );
    }

    // 3) Embed original init files as ZIP entries (streams.N/init.m4s)
    for (stream_idx, (_source_name, init_data)) in init_file_data.iter().enumerate() {
        let filename = zip::entry_name_init(stream_idx);
        let crc = crc32fast::hash(init_data);
        let file_size = init_data.len() as u64;
        let zip_header = zip::make_local_file_header(filename.as_bytes(), crc, 0, file_size, file_size);
        let free_header = make_free_header(zip_header.len() + init_data.len());

        let zip_local_header_offset = current_offset + 8;
        zip_file_entries.push(ZipFileEntry {
            filename: filename.into_bytes(),
            crc32: crc,
            compressed_size: file_size,
            uncompressed_size: file_size,
            compression_method: 0,
            local_header_offset: zip_local_header_offset,
        });

        // free_header + zip_header as inline data
        let mut header_data = Vec::with_capacity(8 + zip_header.len());
        header_data.extend_from_slice(&free_header);
        header_data.extend_from_slice(&zip_header);
        recipe_chunks.push(make_data_chunk(current_offset, header_data.clone()));
        current_offset += header_data.len() as u64;

        // init file as file reference
        let init_abs_path = fs::canonicalize(json_base_dir.join(_source_name))
            .expect("Failed to canonicalize init path");
        let init_rel_to_recipe = relative_path(&recipe_base_dir, &init_abs_path)
            .to_string_lossy()
            .to_string();
        recipe_chunks.push(make_file_chunk(
            current_offset,
            init_rel_to_recipe,
            init_data,
        ));
        current_offset += init_data.len() as u64;
    }

    // --- ZIP central directory and end records (wrapped in free box for MP4 compatibility) ---
    let cd_offset = current_offset + 8; // 8 bytes for free box header
    let zip_end_data = zip::make_end_records(&zip_file_entries, cd_offset);
    let free_header = make_free_header(zip_end_data.len());
    let mut zip_end_chunk = Vec::with_capacity(8 + zip_end_data.len());
    zip_end_chunk.extend_from_slice(&free_header);
    zip_end_chunk.extend_from_slice(&zip_end_data);
    recipe_chunks.push(make_data_chunk(current_offset, zip_end_chunk.clone()));
    current_offset += zip_end_chunk.len() as u64;

    // ===== Phase 5: Write recipe.pb =====
    let recipe_file = proto::RecipeFile {
        recipe: Some(proto::recipe_file::Recipe::V1(proto::RecipeV1 {
            chunks: recipe_chunks,
        })),
    };

    let recipe_bytes = recipe_file.encode_to_vec();

    if let Some(parent) = recipe_out_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).expect("Failed to create output directory");
        }
    }

    fs::write(&recipe_out_path, &recipe_bytes).expect("Failed to write recipe.pb");
    info!(
        path = %recipe_out_path.display(),
        size = recipe_bytes.len(),
        total_output_size = current_offset,
        num_chunks = recipe_file.recipe.as_ref().map(|r| match r {
            proto::recipe_file::Recipe::V1(v1) => v1.chunks.len(),
        }).unwrap_or(0),
        "wrote recipe"
    );
}
