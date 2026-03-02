// Generation of MP4 boxes: moov, moof, sidx, ftyp

use super::binary::*;
use super::mp4_box::*;
use super::types::*;

// ===== Generate Combined moov =====

fn generate_mvhd(timescale: u32, next_track_id: u32) -> Vec<u8> {
    let mut data = Vec::new();
    write_u32_be(&mut data, 0); // creation_time
    write_u32_be(&mut data, 0); // modification_time
    write_u32_be(&mut data, timescale);
    write_u32_be(&mut data, 0); // duration
    write_u32_be(&mut data, 0x00010000); // rate = 1.0
    data.extend_from_slice(&[0x01, 0x00]); // volume = 1.0
    data.extend_from_slice(&[0u8; 10]); // reserved
    // Identity matrix
    for &v in &[0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
        write_u32_be(&mut data, v);
    }
    data.extend_from_slice(&[0u8; 24]); // pre_defined
    write_u32_be(&mut data, next_track_id);
    make_fullbox(b"mvhd", 0, 0, &data)
}

fn generate_trex(track: &TrackInfo) -> Vec<u8> {
    let mut data = Vec::new();
    write_u32_be(&mut data, track.new_track_id);
    write_u32_be(&mut data, track.trex_default_sample_description_index);
    write_u32_be(&mut data, track.trex_default_sample_duration);
    write_u32_be(&mut data, track.trex_default_sample_size);
    write_u32_be(&mut data, track.trex_default_sample_flags);
    make_fullbox(b"trex", 0, 0, &data)
}

pub fn generate_combined_moov(tracks: &[TrackInfo]) -> Vec<u8> {
    let max_track_id = tracks.iter().map(|t| t.new_track_id).max().unwrap_or(0);
    let movie_timescale = tracks[0].timescale;

    let mvhd = generate_mvhd(movie_timescale, max_track_id + 1);

    let trex_boxes: Vec<Vec<u8>> = tracks.iter().map(generate_trex).collect();
    let mvex_content: Vec<u8> = trex_boxes.into_iter().flatten().collect();
    let mvex = make_box(b"mvex", &mvex_content);

    let mut moov_content = mvhd;
    for track in tracks {
        moov_content.extend_from_slice(&track.trak_raw);
    }
    moov_content.extend_from_slice(&mvex);
    make_box(b"moov", &moov_content)
}

// ===== Generate Combined moof (one per track per chunk, with multiple truns) =====

/// Size of a single materialized trun for one fragment.
/// Materializes tfhd defaults into per-sample fields so multiple fragments
/// can share a single tfhd in the combined moof.
fn calc_materialized_trun_size(frag: &FragmentInfo) -> usize {
    let has_cts = frag.trun.flags & 0x000800 != 0;
    // Always materialize: duration(4) + size(4) + flags(4) + optional cts(4)
    let per_sample = 4 + 4 + 4 + if has_cts { 4 } else { 0 };
    // fullbox header(12) + sample_count(4) + data_offset(4) + samples
    12 + 4 + 4 + frag.trun.sample_count as usize * per_sample
}

/// Calculate the total size of a combined moof for one track's chunk (all fragments).
pub fn calc_chunk_moof_size(fragments: &[FragmentInfo]) -> usize {
    let mfhd_size: usize = 16; // 8 header + 4 version/flags + 4 seq

    // tfhd: 12 fullbox + 4 track_id + optional 4 sample_description_index
    let has_sdi = fragments[0].tfhd.flags & 0x000002 != 0;
    let tfhd_size: usize = 12 + 4 + if has_sdi { 4 } else { 0 };

    let tfdt_size: usize = fragments[0]
        .tfdt
        .as_ref()
        .map_or(0, |t| 12 + if t.version == 0 { 4 } else { 8 });

    let truns_total_size: usize = fragments.iter().map(calc_materialized_trun_size).sum();

    let traf_size = 8 + tfhd_size + tfdt_size + truns_total_size;
    8 + mfhd_size + traf_size
}

/// Generate a combined moof for one track's chunk containing all fragments.
///
/// Produces: moof { mfhd, traf { tfhd, tfdt, trun_0, trun_1, ..., trun_N } }
///
/// Layout after this moof: [free_header(8)][lmc1_header(16)][original_file_bytes]
/// Each trun_k's data_offset = moof_size + pre_data_gap + frag_k.moof_offset + frag_k.original_data_offset
pub fn generate_chunk_moof(
    sequence_number: u32,
    new_track_id: u32,
    fragments: &[FragmentInfo],
    trex: &TrackInfo,
    moof_size: usize,
    pre_data_gap: usize,
) -> Vec<u8> {
    // mfhd
    let mut mfhd_data = Vec::new();
    write_u32_be(&mut mfhd_data, sequence_number);
    let mfhd = make_fullbox(b"mfhd", 0, 0, &mfhd_data);

    // tfhd: only track_id + default-base-is-moof, optionally sample_description_index.
    // Per-fragment defaults (duration/size/flags) are materialized into each trun.
    let has_sdi = fragments[0].tfhd.flags & 0x000002 != 0;
    let tfhd_flags: u32 = 0x020000 | if has_sdi { 0x000002 } else { 0 };
    let mut tfhd_data = Vec::new();
    write_u32_be(&mut tfhd_data, new_track_id);
    if has_sdi {
        write_u32_be(
            &mut tfhd_data,
            fragments[0].tfhd.sample_description_index.unwrap_or(1),
        );
    }
    let tfhd_box = make_fullbox(b"tfhd", 0, tfhd_flags, &tfhd_data);

    // tfdt: from first fragment
    let tfdt_box = fragments[0].tfdt.as_ref().map(|t| {
        let mut data = Vec::new();
        if t.version == 0 {
            write_u32_be(&mut data, t.base_media_decode_time as u32);
        } else {
            write_u64_be(&mut data, t.base_media_decode_time);
        }
        make_fullbox(b"tfdt", t.version, 0, &data)
    });

    // Build one trun per fragment with materialized per-sample fields
    let mut trun_boxes: Vec<Vec<u8>> = Vec::new();
    for fragment in fragments {
        let has_cts = fragment.trun.flags & 0x000800 != 0;
        // Always: data_offset + duration + size + flags; optionally cts
        // No first_sample_flags bit — we materialize it into per-sample flags
        let trun_flags: u32 =
            0x000001 | 0x000100 | 0x000200 | 0x000400 | if has_cts { 0x000800 } else { 0 };

        let new_data_offset = moof_size as i32
            + pre_data_gap as i32
            + fragment.moof_offset as i32
            + fragment.original_data_offset;

        let mut trun_data = Vec::new();
        write_u32_be(&mut trun_data, fragment.trun.sample_count);
        write_i32_be(&mut trun_data, new_data_offset);

        for (i, sample) in fragment.trun.samples.iter().enumerate() {
            // Duration: sample > tfhd default > trex default
            let duration = sample.duration.unwrap_or_else(|| {
                fragment
                    .tfhd
                    .default_sample_duration
                    .unwrap_or(trex.trex_default_sample_duration)
            });
            write_u32_be(&mut trun_data, duration);

            // Size: sample > tfhd default > trex default
            let size = sample.size.unwrap_or_else(|| {
                fragment
                    .tfhd
                    .default_sample_size
                    .unwrap_or(trex.trex_default_sample_size)
            });
            write_u32_be(&mut trun_data, size);

            // Flags: first_sample_flags (for i==0) > sample > tfhd default > trex default
            let flags = if i == 0 {
                fragment.trun.first_sample_flags.unwrap_or_else(|| {
                    sample.flags.unwrap_or_else(|| {
                        fragment
                            .tfhd
                            .default_sample_flags
                            .unwrap_or(trex.trex_default_sample_flags)
                    })
                })
            } else {
                sample.flags.unwrap_or_else(|| {
                    fragment
                        .tfhd
                        .default_sample_flags
                        .unwrap_or(trex.trex_default_sample_flags)
                })
            };
            write_u32_be(&mut trun_data, flags);

            // Composition time offset
            if has_cts {
                write_u32_be(
                    &mut trun_data,
                    sample.composition_time_offset_raw.unwrap_or(0),
                );
            }
        }

        let trun_box = make_fullbox(b"trun", fragment.trun.version, trun_flags, &trun_data);
        trun_boxes.push(trun_box);
    }

    // Assemble traf
    let mut traf_content = Vec::new();
    traf_content.extend_from_slice(&tfhd_box);
    if let Some(ref tfdt_b) = tfdt_box {
        traf_content.extend_from_slice(tfdt_b);
    }
    for trun_box in &trun_boxes {
        traf_content.extend_from_slice(trun_box);
    }
    let traf = make_box(b"traf", &traf_content);

    // Assemble moof
    let mut moof_content = Vec::new();
    moof_content.extend_from_slice(&mfhd);
    moof_content.extend_from_slice(&traf);
    let moof = make_box(b"moof", &moof_content);

    assert_eq!(moof.len(), moof_size, "chunk moof size mismatch");
    moof
}

// ===== Generate sidx =====

pub fn generate_sidx(
    reference_id: u32,
    timescale: u32,
    earliest_presentation_time: u64,
    first_offset: u64,
    references: &[SidxReference],
) -> Vec<u8> {
    let mut data = Vec::new();
    write_u32_be(&mut data, reference_id);
    write_u32_be(&mut data, timescale);
    write_u64_be(&mut data, earliest_presentation_time);
    write_u64_be(&mut data, first_offset);
    data.extend_from_slice(&[0u8; 2]); // reserved
    let ref_count = references.len() as u16;
    data.extend_from_slice(&ref_count.to_be_bytes());

    for r in references {
        let word1 = r.referenced_size & 0x7FFFFFFF;
        write_u32_be(&mut data, word1);
        write_u32_be(&mut data, r.subsegment_duration);
        let word3 = ((r.starts_with_sap as u32) << 31) | ((r.sap_type as u32 & 0x7) << 28);
        write_u32_be(&mut data, word3);
    }
    make_fullbox(b"sidx", 1, 0, &data)
}

// ===== Generate ftyp =====

pub fn generate_ftyp() -> Vec<u8> {
    let mut content = Vec::new();
    content.extend_from_slice(b"isom"); // major_brand
    write_u32_be(&mut content, 0x200); // minor_version
    content.extend_from_slice(b"isom");
    content.extend_from_slice(b"avc1");
    content.extend_from_slice(b"iso6");
    content.extend_from_slice(b"dash");
    content.extend_from_slice(b"msix");
    make_box(b"ftyp", &content)
}
