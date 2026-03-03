// Parsing of init segments and chunk files

use tracing::{debug, trace};

use super::binary::*;
use super::mp4_box::*;
use super::types::*;

// ===== Init Segment Parsing =====

pub fn parse_init_segment(data: &[u8], new_track_id: u32) -> TrackInfo {
    let moov_info = find_box(data, b"moov").expect("moov not found in init segment");
    let moov_content = box_content(data, &moov_info);

    let trak_info = find_box(moov_content, b"trak").expect("trak not found in moov");
    let trak_content = box_content(moov_content, &trak_info);

    // ---- Extract tkhd and patch track_id ----
    let tkhd_info = find_box(trak_content, b"tkhd").expect("tkhd not found in trak");
    let mut tkhd_raw = box_raw(trak_content, &tkhd_info).to_vec();
    let tkhd_content = box_content(trak_content, &tkhd_info);
    let (tkhd_version, _, _) = fullbox_parse(tkhd_content);
    // track_id field offset within the tkhd box
    let track_id_within_box =
        tkhd_info.header_size + 4 + if tkhd_version == 0 { 8 } else { 16 };
    tkhd_raw[track_id_within_box..track_id_within_box + 4]
        .copy_from_slice(&new_track_id.to_be_bytes());

    // ---- Extract mdia children ----
    let mdia_info = find_box(trak_content, b"mdia").expect("mdia not found in trak");
    let mdia_content = box_content(trak_content, &mdia_info);

    let mdhd_info = find_box(mdia_content, b"mdhd").expect("mdhd not found in mdia");
    let mdhd_raw = box_raw(mdia_content, &mdhd_info).to_vec();
    let mdhd_content = box_content(mdia_content, &mdhd_info);
    let (mdhd_version, _, mdhd_data) = fullbox_parse(mdhd_content);
    let timescale = if mdhd_version == 0 {
        read_u32_be(mdhd_data, 8)
    } else {
        read_u32_be(mdhd_data, 16)
    };

    let hdlr_info = find_box(mdia_content, b"hdlr").expect("hdlr not found in mdia");
    let hdlr_raw = box_raw(mdia_content, &hdlr_info).to_vec();
    let hdlr_content = box_content(mdia_content, &hdlr_info);
    let (_, _, hdlr_data) = fullbox_parse(hdlr_content);
    let handler_type: [u8; 4] = hdlr_data[4..8].try_into().unwrap();

    // ---- Extract minf children ----
    let minf_info = find_box(mdia_content, b"minf").expect("minf not found in mdia");
    let minf_content = box_content(mdia_content, &minf_info);

    let media_header_raw = if let Some(vmhd) = find_box(minf_content, b"vmhd") {
        box_raw(minf_content, &vmhd).to_vec()
    } else if let Some(smhd) = find_box(minf_content, b"smhd") {
        box_raw(minf_content, &smhd).to_vec()
    } else if let Some(nmhd) = find_box(minf_content, b"nmhd") {
        box_raw(minf_content, &nmhd).to_vec()
    } else {
        panic!("No media header (vmhd/smhd/nmhd) found in minf");
    };

    let dinf_info = find_box(minf_content, b"dinf").expect("dinf not found in minf");
    let dinf_raw = box_raw(minf_content, &dinf_info).to_vec();

    let stbl_info = find_box(minf_content, b"stbl").expect("stbl not found in minf");
    let stbl_content = box_content(minf_content, &stbl_info);
    let stsd_info = find_box(stbl_content, b"stsd").expect("stsd not found in stbl");
    let stsd_raw = box_raw(stbl_content, &stsd_info).to_vec();

    // ---- Parse trex defaults ----
    let mvex_info = find_box(moov_content, b"mvex").expect("mvex not found");
    let mvex_content = box_content(moov_content, &mvex_info);
    let trex_info = find_box(mvex_content, b"trex").expect("trex not found");
    let trex_content = box_content(mvex_content, &trex_info);
    let (_, _, trex_data) = fullbox_parse(trex_content);

    TrackInfo {
        new_track_id,
        timescale,
        handler_type,
        tkhd_raw,
        mdhd_raw,
        hdlr_raw,
        media_header_raw,
        dinf_raw,
        stsd_raw,
        trex_default_sample_description_index: read_u32_be(trex_data, 4),
        trex_default_sample_duration: read_u32_be(trex_data, 8),
        trex_default_sample_size: read_u32_be(trex_data, 12),
        trex_default_sample_flags: read_u32_be(trex_data, 16),
    }
}

// ===== Chunk Parsing =====

fn parse_tfhd(content: &[u8]) -> TfhdInfo {
    let (_, flags, data) = fullbox_parse(content);
    let track_id = read_u32_be(data, 0);
    let mut pos = 4;

    let base_data_offset = if flags & 0x000001 != 0 {
        let v = read_u64_be(data, pos);
        pos += 8;
        Some(v)
    } else {
        None
    };

    let sample_description_index = if flags & 0x000002 != 0 {
        let v = read_u32_be(data, pos);
        pos += 4;
        Some(v)
    } else {
        None
    };

    let default_sample_duration = if flags & 0x000008 != 0 {
        let v = read_u32_be(data, pos);
        pos += 4;
        Some(v)
    } else {
        None
    };

    let default_sample_size = if flags & 0x000010 != 0 {
        let v = read_u32_be(data, pos);
        pos += 4;
        Some(v)
    } else {
        None
    };

    let default_sample_flags = if flags & 0x000020 != 0 {
        let v = read_u32_be(data, pos);
        Some(v)
    } else {
        None
    };

    TfhdInfo {
        track_id,
        flags,
        base_data_offset,
        sample_description_index,
        default_sample_duration,
        default_sample_size,
        default_sample_flags,
    }
}

fn parse_tfdt(content: &[u8]) -> TfdtInfo {
    let (version, _, data) = fullbox_parse(content);
    let base_media_decode_time = if version == 0 {
        read_u32_be(data, 0) as u64
    } else {
        read_u64_be(data, 0)
    };
    TfdtInfo {
        version,
        base_media_decode_time,
    }
}

fn parse_trun(content: &[u8]) -> TrunInfo {
    let (version, flags, data) = fullbox_parse(content);
    let sample_count = read_u32_be(data, 0);
    let mut pos = 4;

    let data_offset = if flags & 0x000001 != 0 {
        let v = read_i32_be(data, pos);
        pos += 4;
        Some(v)
    } else {
        None
    };

    let first_sample_flags = if flags & 0x000004 != 0 {
        let v = read_u32_be(data, pos);
        pos += 4;
        Some(v)
    } else {
        None
    };

    let has_duration = flags & 0x000100 != 0;
    let has_size = flags & 0x000200 != 0;
    let has_flags = flags & 0x000400 != 0;
    let has_cts = flags & 0x000800 != 0;

    let mut samples = Vec::with_capacity(sample_count as usize);
    for _ in 0..sample_count {
        let duration = if has_duration {
            let v = read_u32_be(data, pos);
            pos += 4;
            Some(v)
        } else {
            None
        };
        let size = if has_size {
            let v = read_u32_be(data, pos);
            pos += 4;
            Some(v)
        } else {
            None
        };
        let sample_flags = if has_flags {
            let v = read_u32_be(data, pos);
            pos += 4;
            Some(v)
        } else {
            None
        };
        let composition_time_offset_raw = if has_cts {
            let v = read_u32_be(data, pos);
            pos += 4;
            Some(v)
        } else {
            None
        };
        samples.push(TrunSample {
            duration,
            size,
            flags: sample_flags,
            composition_time_offset_raw,
        });
    }

    TrunInfo {
        version,
        flags,
        sample_count,
        data_offset,
        first_sample_flags,
        samples,
    }
}

/// Parse a chunk file that may contain multiple moof+mdat fragment pairs
pub fn parse_chunk(data: &[u8]) -> ChunkParseResult {
    let file_size = data.len();
    let top_level_boxes = iter_boxes(data);

    let mut fragments = Vec::new();
    let mut current_moof: Option<BoxInfo> = None;

    for box_info in &top_level_boxes {
        match &box_info.box_type {
            b"moof" => {
                current_moof = Some(box_info.clone());
            }
            b"mdat" => {
                let moof_box_info = current_moof.take().expect("mdat without preceding moof");
                let moof_content = box_content(data, &moof_box_info);

                let traf_info = find_box(moof_content, b"traf").expect("traf not found in moof");
                let traf_content = box_content(moof_content, &traf_info);

                let tfhd_box = find_box(traf_content, b"tfhd").expect("tfhd not found in traf");
                let tfhd = parse_tfhd(box_content(traf_content, &tfhd_box));

                let tfdt = find_box(traf_content, b"tfdt")
                    .map(|info| parse_tfdt(box_content(traf_content, &info)));

                let trun_box = find_box(traf_content, b"trun").expect("trun not found in traf");
                let trun = parse_trun(box_content(traf_content, &trun_box));

                let original_data_offset = trun.data_offset.expect("trun data_offset required");

                fragments.push(FragmentInfo {
                    moof_offset: moof_box_info.offset,
                    original_data_offset,
                    tfhd,
                    tfdt,
                    trun,
                });
            }
            _ => {
                // Skip styp, sidx, etc.
                trace!(
                    box_type = %std::str::from_utf8(&box_info.box_type).unwrap_or("????"),
                    offset = box_info.offset,
                    size = box_info.total_size,
                    "skipping top-level box"
                );
            }
        }
    }

    assert!(
        !fragments.is_empty(),
        "no moof+mdat fragments found in chunk"
    );
    debug!(fragment_count = fragments.len(), file_size, "parsed chunk");

    ChunkParseResult {
        file_size,
        fragments,
    }
}
