// Plan subcommand: generate a recipe.pb describing how to assemble a Hybrid MP4
//
// Layout of the described MP4 (init + data concatenated):
//   [ftyp]
//   [free: lmc1 + original_init_0]
//   [free: lmc1 + original_init_1]
//   [moov with full stbl (co64 pointing into mdat)]
//   [mdat]
//     for each chunk_idx, stream_idx:
//       [lmc1_header(16)][original_chunk_file_bytes]
//
// The moov's sample tables reference actual sample data positions inside
// the original chunk files, skipping over fMP4 structural boxes (moof, mdat
// headers etc.) that are embedded verbatim.

mod binary;
mod boxes;
mod mp4_box;
mod parse;
mod types;

use std::fs;
use std::path::{Component, Path, PathBuf};

use prost::Message;
use tracing::{debug, info};

use self::boxes::*;
use self::mp4_box::make_free_header;
use self::parse::*;
use self::types::*;

use crate::proto;

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

/// Size of the lmc1 header placed before each embedded source file
const LMC1_HEADER_SIZE: usize = 16;

fn make_lmc1_header(stream_index: u8, file_index: u32, data: &[u8]) -> [u8; LMC1_HEADER_SIZE] {
    let crc = crc32fast::hash(data);
    let size = data.len() as u32;
    let mut header = [0u8; LMC1_HEADER_SIZE];
    header[0..4].copy_from_slice(b"lmc1");
    header[4] = stream_index;
    // file_index: 24-bit big-endian in bytes 5-7 (MSB set for init segments)
    header[5] = ((file_index >> 16) & 0xFF) as u8;
    header[6] = ((file_index >> 8) & 0xFF) as u8;
    header[7] = (file_index & 0xFF) as u8;
    // CRC-32 big-endian
    header[8..12].copy_from_slice(&crc.to_be_bytes());
    // Size big-endian
    header[12..16].copy_from_slice(&size.to_be_bytes());
    header
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

    // chunk_data_sizes[chunk_idx][stream_idx] = LMC1_HEADER_SIZE + file_size
    let mut chunk_data_sizes: Vec<Vec<usize>> = Vec::new();
    for chunk_idx in 0..num_chunks {
        let mut sizes = Vec::new();
        for stream_idx in 0..num_streams {
            let ref parsed = parsed_chunks[stream_idx][chunk_idx];
            sizes.push(LMC1_HEADER_SIZE + parsed.file_size);
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

        for chunk_idx in 0..num_chunks {
            let ref parsed = parsed_chunks[stream_idx][chunk_idx];
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
        }

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

    // --- free boxes embedding original init files ---
    for (stream_idx, (_source_name, init_data)) in init_file_data.iter().enumerate() {
        let lmc1 = make_lmc1_header(stream_idx as u8, 0x800000, init_data);
        let free_header = make_free_header(LMC1_HEADER_SIZE + init_data.len());

        // free_header + lmc1 as inline data
        let mut header_data = Vec::with_capacity(8 + LMC1_HEADER_SIZE);
        header_data.extend_from_slice(&free_header);
        header_data.extend_from_slice(&lmc1);
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

            // This chunk file's data starts at:
            let chunk_file_start = mdat_content_start + data_pos + LMC1_HEADER_SIZE as u64;

            // Each fragment within this chunk file becomes one stbl chunk
            for frag in &parsed.fragments {
                let sample_data_abs =
                    chunk_file_start + frag.moof_offset as u64 + frag.original_data_offset as u64;

                let stbl_chunk_idx = chunk_offset_cursor[stream_idx];
                sample_tables[stream_idx].chunk_offsets[stbl_chunk_idx] = sample_data_abs;
                chunk_offset_cursor[stream_idx] += 1;
            }

            data_pos += (LMC1_HEADER_SIZE + parsed.file_size) as u64;
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

    // --- mdat content: lmc1 headers + chunk files ---
    for chunk_idx in 0..num_chunks {
        for stream_idx in 0..num_streams {
            let ref parsed = parsed_chunks[stream_idx][chunk_idx];
            let chunk_rel = &chunk_file_rel_paths[stream_idx][chunk_idx];

            // Read chunk file to compute lmc1 header
            let chunk_abs_path = json_base_dir.join(chunk_rel);
            let chunk_data = fs::read(&chunk_abs_path).unwrap();

            // lmc1 header as inline data
            let lmc1 = make_lmc1_header(stream_idx as u8, chunk_idx as u32, &chunk_data);
            recipe_chunks.push(make_data_chunk(current_offset, lmc1.to_vec()));
            current_offset += LMC1_HEADER_SIZE as u64;

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
