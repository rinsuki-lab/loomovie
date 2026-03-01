mod binary;
mod generate;
mod helpers;
mod mp4_box;
mod parse;
mod types;

use std::{env, fs, io::Write, path::PathBuf};

use tracing::{debug, info};

use generate::*;
use helpers::*;
use mp4_box::make_free_header;
use parse::*;
use types::*;

fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: loomovie <streams.json> [output_dir]");
        std::process::exit(1);
    }

    let json_path = PathBuf::from(&args[1]);
    let output_dir = if args.len() > 2 {
        PathBuf::from(&args[2])
    } else {
        PathBuf::from(".")
    };

    let json_base_dir = json_path.parent().unwrap_or(&PathBuf::from(".")).to_owned();

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
    // parsed_chunks[stream_idx][chunk_idx] = (relative_source_path, ChunkParseResult)
    let mut parsed_chunks: Vec<Vec<(String, ChunkParseResult)>> = Vec::new();
    // chunk_file_paths[stream_idx][chunk_idx] = absolute path to chunk file
    let mut chunk_file_paths: Vec<Vec<PathBuf>> = Vec::new();

    for (stream_idx, stream) in config.streams.iter().enumerate() {
        let init_parent = PathBuf::from(&stream.init)
            .parent()
            .map(|p| p.to_owned())
            .unwrap_or_default();

        let mut stream_parsed = Vec::new();
        let mut stream_paths = Vec::new();
        for chunk_name in &stream.chunks {
            let chunk_path = json_base_dir.join(&init_parent).join(chunk_name);
            let chunk_data = fs::read(&chunk_path)
                .unwrap_or_else(|e| panic!("Failed to read chunk {}: {}", chunk_path.display(), e));
            let parsed = parse_chunk(&chunk_data);
            let relative_source = init_parent.join(chunk_name).to_string_lossy().to_string();
            stream_parsed.push((relative_source, parsed));
            stream_paths.push(chunk_path);
        }
        parsed_chunks.push(stream_parsed);
        chunk_file_paths.push(stream_paths);
        info!(stream_idx, num_chunks, "parsed chunks for stream");
    }

    // ===== Phase 3: Compute layout =====

    let ftyp = generate_ftyp();
    let moov = generate_combined_moov(&tracks);

    let mut sources: Vec<SourceFile> = Vec::new();

    // ----- Build out.init.m4s -----
    let mut init_out = Vec::new();
    init_out.extend_from_slice(&ftyp);

    // Embed original init files in free boxes
    for (source_name, init_data) in &init_file_data {
        let free_header = make_free_header(init_data.len());
        let original_data_offset = init_out.len() + free_header.len();
        init_out.extend_from_slice(&free_header);
        init_out.extend_from_slice(init_data);

        sources.push(SourceFile {
            source: source_name.clone(),
            dest: SourceDest {
                r#type: "init".into(),
                offset: original_data_offset as u64,
                length: init_data.len() as u64,
            },
        });
    }

    init_out.extend_from_slice(&moov);

    // ----- Compute segment sizes for sidx -----

    struct SegmentInfo {
        /// Per stream: the size of the combined moof (one per track per chunk)
        stream_chunk_moof_sizes: Vec<usize>,
        total_size: usize,
        duration: u64,
        starts_with_sap: bool,
    }

    let mut segment_infos: Vec<SegmentInfo> = Vec::new();

    for chunk_idx in 0..num_chunks {
        let mut stream_chunk_moof_sizes: Vec<usize> = Vec::new();
        let mut total_size: usize = 0;

        for stream_idx in 0..num_streams {
            let (_, ref parsed) = parsed_chunks[stream_idx][chunk_idx];

            // One combined moof per track per chunk (contains multiple truns)
            let chunk_moof_size = calc_chunk_moof_size(&parsed.fragments);

            // Layout per stream chunk:
            // [combined_moof][free_header(8)][original_file_bytes]
            let chunk_contribution = chunk_moof_size + 8 + parsed.file_size;
            total_size += chunk_contribution;

            stream_chunk_moof_sizes.push(chunk_moof_size);
        }

        // Duration and SAP are based on the first (reference) track's fragments
        let ref_parsed = &parsed_chunks[0][chunk_idx].1;
        let duration = total_chunk_duration(
            &ref_parsed.fragments,
            tracks[0].trex_default_sample_duration,
        );
        let starts_with_sap = is_first_sample_sap(
            &ref_parsed.fragments[0].trun,
            &ref_parsed.fragments[0].tfhd,
            tracks[0].trex_default_sample_flags,
        );

        segment_infos.push(SegmentInfo {
            stream_chunk_moof_sizes,
            total_size,
            duration,
            starts_with_sap,
        });
    }

    let sidx_references: Vec<SidxReference> = segment_infos
        .iter()
        .map(|seg| SidxReference {
            referenced_size: seg.total_size as u32,
            subsegment_duration: seg.duration as u32,
            starts_with_sap: seg.starts_with_sap,
            sap_type: if seg.starts_with_sap { 1 } else { 0 },
        })
        .collect();

    let sidx = generate_sidx(
        tracks[0].new_track_id,
        tracks[0].timescale,
        0,
        0,
        &sidx_references,
    );

    init_out.extend_from_slice(&sidx);

    // ----- Write out.init.m4s -----
    fs::create_dir_all(&output_dir).expect("Failed to create output directory");
    let init_path = output_dir.join("out.init.m4s");
    fs::write(&init_path, &init_out).expect("Failed to write out.init.m4s");
    info!(
        path = %init_path.display(),
        size = init_out.len(),
        "wrote init segment"
    );

    // ----- Build and write out.data.m4s -----
    let data_path = output_dir.join("out.data.m4s");
    let mut data_out = fs::File::create(&data_path).expect("Failed to create out.data.m4s");
    let mut data_offset: u64 = 0;
    let mut global_seq: u32 = 1;

    for chunk_idx in 0..num_chunks {
        let seg = &segment_infos[chunk_idx];

        for stream_idx in 0..num_streams {
            let (ref source_name, ref parsed) = parsed_chunks[stream_idx][chunk_idx];
            let chunk_moof_size = seg.stream_chunk_moof_sizes[stream_idx];

            // Generate a single combined moof for all fragments of this track in this chunk.
            // Layout: [combined_moof][free_header(8)][original_file]
            //
            // Each trun_k inside the moof uses default-base-is-moof, so data_offset_k is
            // from the moof start to the first sample data of fragment k:
            //   data_offset_k = moof_size + 8 + frag_k.moof_offset + frag_k.original_data_offset
            let combined_moof = generate_chunk_moof(
                global_seq,
                tracks[stream_idx].new_track_id,
                &parsed.fragments,
                &tracks[stream_idx],
                chunk_moof_size,
            );
            data_out.write_all(&combined_moof).unwrap();
            global_seq += 1;

            // Write free box header that covers the entire original chunk file
            let free_header = make_free_header(parsed.file_size);
            data_out.write_all(&free_header).unwrap();

            // Write original chunk file data (all bytes, unchanged)
            let chunk_path = &chunk_file_paths[stream_idx][chunk_idx];
            let chunk_data = fs::read(chunk_path).unwrap();

            // Record where the original file bytes are stored
            let orig_start = data_offset + chunk_moof_size as u64 + 8;
            sources.push(SourceFile {
                source: source_name.clone(),
                dest: SourceDest {
                    r#type: "data".into(),
                    offset: orig_start,
                    length: chunk_data.len() as u64,
                },
            });

            data_out.write_all(&chunk_data).unwrap();

            let written = chunk_moof_size + 8 + chunk_data.len();
            data_offset += written as u64;
        }

        if chunk_idx % 100 == 0 && chunk_idx > 0 {
            debug!(chunk_idx, num_chunks, "progress");
        }
    }

    data_out.flush().unwrap();
    info!(
        path = %data_path.display(),
        size = data_offset,
        "wrote data segment"
    );

    // ----- Write out.sources.json -----
    let sources_output = SourcesOutput { files: sources };
    let sources_json = serde_json::to_string_pretty(&sources_output).unwrap();
    let sources_path = output_dir.join("out.sources.json");
    fs::write(&sources_path, &sources_json).expect("Failed to write out.sources.json");
    info!(path = %sources_path.display(), "wrote sources");

    info!("done! to test: cat out.init.m4s out.data.m4s > out.mp4 && ffplay out.mp4");
}
