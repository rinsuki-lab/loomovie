// Generation of MP4 boxes for Hybrid MP4 output
//
// The Hybrid MP4 wraps original fragmented MP4 source files inside a single mdat,
// with a moov containing full sample tables (stbl) whose chunk offsets skip over
// the fMP4 headers embedded in the source files.

use super::binary::*;
use super::mp4_box::*;
use super::types::*;

// ===== moov children =====

fn generate_mvhd(timescale: u32, duration: u64, next_track_id: u32) -> Vec<u8> {
    // Use version 1 (64-bit times) when duration overflows u32
    let use_v1 = duration > u32::MAX as u64;
    let mut data = Vec::new();
    if use_v1 {
        write_u64_be(&mut data, 0); // creation_time
        write_u64_be(&mut data, 0); // modification_time
        write_u32_be(&mut data, timescale);
        write_u64_be(&mut data, duration);
    } else {
        write_u32_be(&mut data, 0); // creation_time
        write_u32_be(&mut data, 0); // modification_time
        write_u32_be(&mut data, timescale);
        write_u32_be(&mut data, duration as u32);
    }
    write_u32_be(&mut data, 0x00010000); // rate = 1.0
    data.extend_from_slice(&[0x01, 0x00]); // volume = 1.0
    data.extend_from_slice(&[0u8; 10]); // reserved
    // Identity matrix
    for &v in &[0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
        write_u32_be(&mut data, v);
    }
    data.extend_from_slice(&[0u8; 24]); // pre_defined
    write_u32_be(&mut data, next_track_id);
    make_fullbox(b"mvhd", if use_v1 { 1 } else { 0 }, 0, &data)
}

// ===== stbl children =====

fn generate_stts(durations: &[u32]) -> Vec<u8> {
    // Run-length encode consecutive equal durations
    let mut entries: Vec<(u32, u32)> = Vec::new();
    for &d in durations {
        if let Some(last) = entries.last_mut() {
            if last.1 == d {
                last.0 += 1;
                continue;
            }
        }
        entries.push((1, d));
    }

    let mut data = Vec::new();
    write_u32_be(&mut data, entries.len() as u32);
    for &(count, delta) in &entries {
        write_u32_be(&mut data, count);
        write_u32_be(&mut data, delta);
    }
    make_fullbox(b"stts", 0, 0, &data)
}

fn generate_ctts(offsets: &[i32], version: u8) -> Vec<u8> {
    // Run-length encode consecutive equal offsets
    let mut entries: Vec<(u32, i32)> = Vec::new();
    for &o in offsets {
        if let Some(last) = entries.last_mut() {
            if last.1 == o {
                last.0 += 1;
                continue;
            }
        }
        entries.push((1, o));
    }

    let mut data = Vec::new();
    write_u32_be(&mut data, entries.len() as u32);
    for &(count, offset) in &entries {
        write_u32_be(&mut data, count);
        if version == 0 {
            write_u32_be(&mut data, offset as u32);
        } else {
            write_i32_be(&mut data, offset);
        }
    }
    make_fullbox(b"ctts", version, 0, &data)
}

fn generate_stsz(sizes: &[u32]) -> Vec<u8> {
    // If all sizes are equal, use the compact form (sample_size != 0)
    let uniform = if !sizes.is_empty() && sizes.iter().all(|&s| s == sizes[0]) {
        sizes[0]
    } else {
        0
    };

    let mut data = Vec::new();
    write_u32_be(&mut data, uniform);
    write_u32_be(&mut data, sizes.len() as u32);
    if uniform == 0 {
        for &s in sizes {
            write_u32_be(&mut data, s);
        }
    }
    make_fullbox(b"stsz", 0, 0, &data)
}

fn generate_stsc(samples_per_chunk: &[u32]) -> Vec<u8> {
    // Run-length encode: record an entry only when samples_per_chunk changes
    let mut entries: Vec<(u32, u32)> = Vec::new(); // (first_chunk 1-based, samples_per_chunk)
    for (i, &count) in samples_per_chunk.iter().enumerate() {
        let chunk_1based = (i + 1) as u32;
        if let Some(last) = entries.last() {
            if last.1 == count {
                continue; // same as previous run
            }
        }
        entries.push((chunk_1based, count));
    }

    let mut data = Vec::new();
    write_u32_be(&mut data, entries.len() as u32);
    for &(first_chunk, spc) in &entries {
        write_u32_be(&mut data, first_chunk);
        write_u32_be(&mut data, spc);
        write_u32_be(&mut data, 1); // sample_description_index
    }
    make_fullbox(b"stsc", 0, 0, &data)
}

fn generate_co64(offsets: &[u64]) -> Vec<u8> {
    let mut data = Vec::new();
    write_u32_be(&mut data, offsets.len() as u32);
    for &o in offsets {
        write_u64_be(&mut data, o);
    }
    make_fullbox(b"co64", 0, 0, &data)
}

fn generate_stss(sync_samples: &[u32]) -> Vec<u8> {
    let mut data = Vec::new();
    write_u32_be(&mut data, sync_samples.len() as u32);
    for &s in sync_samples {
        write_u32_be(&mut data, s);
    }
    make_fullbox(b"stss", 0, 0, &data)
}

// ===== stbl =====

fn generate_stbl(stsd_raw: &[u8], st: &TrackSampleTable) -> Vec<u8> {
    let mut content = Vec::new();
    content.extend_from_slice(stsd_raw);
    content.extend_from_slice(&generate_stts(&st.sample_durations));
    if st.has_cts {
        content.extend_from_slice(&generate_ctts(&st.cts_offsets, st.cts_version));
    }
    content.extend_from_slice(&generate_stsc(&st.samples_per_chunk));
    content.extend_from_slice(&generate_stsz(&st.sample_sizes));
    content.extend_from_slice(&generate_co64(&st.chunk_offsets));

    // stss only needed when not every sample is a sync sample
    if !st.sync_samples.is_empty() && st.sync_samples.len() < st.sample_sizes.len() {
        content.extend_from_slice(&generate_stss(&st.sync_samples));
    }

    make_box(b"stbl", &content)
}

// ===== trak (with full stbl) =====

/// Patch the duration field in a raw tkhd box.
/// tkhd duration is in movie timescale.
fn patch_tkhd_duration(tkhd: &mut [u8], duration: u64) {
    let version = tkhd[8]; // first byte after box header
    if version == 0 {
        // v0: header(8) + ver_flags(4) + creation(4) + modification(4) + track_id(4) + reserved(4) + duration(4)
        let off = 8 + 4 + 4 + 4 + 4 + 4;
        tkhd[off..off + 4].copy_from_slice(&(duration as u32).to_be_bytes());
    } else {
        // v1: header(8) + ver_flags(4) + creation(8) + modification(8) + track_id(4) + reserved(4) + duration(8)
        let off = 8 + 4 + 8 + 8 + 4 + 4;
        tkhd[off..off + 8].copy_from_slice(&duration.to_be_bytes());
    }
}

/// Patch the duration field in a raw mdhd box.
/// mdhd duration is in the track's own timescale.
fn patch_mdhd_duration(mdhd: &mut [u8], duration: u64) {
    let version = mdhd[8];
    if version == 0 {
        // v0: header(8) + ver_flags(4) + creation(4) + modification(4) + timescale(4) + duration(4)
        let off = 8 + 4 + 4 + 4 + 4;
        mdhd[off..off + 4].copy_from_slice(&(duration as u32).to_be_bytes());
    } else {
        // v1: header(8) + ver_flags(4) + creation(8) + modification(8) + timescale(4) + duration(8)
        let off = 8 + 4 + 8 + 8 + 4;
        mdhd[off..off + 8].copy_from_slice(&duration.to_be_bytes());
    }
}

/// Build a complete trak box for Hybrid MP4 output.
pub fn generate_hybrid_trak(
    track: &TrackInfo,
    st: &TrackSampleTable,
    movie_timescale: u32,
) -> Vec<u8> {
    // Patch tkhd: set duration in movie timescale
    let mut tkhd = track.tkhd_raw.clone();
    let tkhd_duration = if track.timescale == movie_timescale {
        st.total_duration
    } else {
        st.total_duration * movie_timescale as u64 / track.timescale as u64
    };
    patch_tkhd_duration(&mut tkhd, tkhd_duration);

    // Patch mdhd: set duration in track timescale
    let mut mdhd = track.mdhd_raw.clone();
    patch_mdhd_duration(&mut mdhd, st.total_duration);

    // Build stbl
    let stbl = generate_stbl(&track.stsd_raw, st);

    // minf = media_header + dinf + stbl
    let mut minf_content = Vec::new();
    minf_content.extend_from_slice(&track.media_header_raw);
    minf_content.extend_from_slice(&track.dinf_raw);
    minf_content.extend_from_slice(&stbl);
    let minf = make_box(b"minf", &minf_content);

    // mdia = mdhd + hdlr + minf
    let mut mdia_content = Vec::new();
    mdia_content.extend_from_slice(&mdhd);
    mdia_content.extend_from_slice(&track.hdlr_raw);
    mdia_content.extend_from_slice(&minf);
    let mdia = make_box(b"mdia", &mdia_content);

    // trak = tkhd + mdia
    let mut trak_content = Vec::new();
    trak_content.extend_from_slice(&tkhd);
    trak_content.extend_from_slice(&mdia);
    make_box(b"trak", &trak_content)
}

// ===== Generate Hybrid moov =====

/// Generate a complete moov box for Hybrid MP4.
pub fn generate_hybrid_moov(
    tracks: &[TrackInfo],
    sample_tables: &[TrackSampleTable],
) -> Vec<u8> {
    let movie_timescale = tracks[0].timescale;
    let max_track_id = tracks.iter().map(|t| t.new_track_id).max().unwrap_or(0);

    // Movie duration = max across tracks (converted to movie timescale)
    let movie_duration = tracks
        .iter()
        .zip(sample_tables.iter())
        .map(|(t, st)| {
            if t.timescale == movie_timescale {
                st.total_duration
            } else {
                st.total_duration * movie_timescale as u64 / t.timescale as u64
            }
        })
        .max()
        .unwrap_or(0);

    let mvhd = generate_mvhd(movie_timescale, movie_duration, max_track_id + 1);

    let mut moov_content = mvhd;
    for (track, st) in tracks.iter().zip(sample_tables.iter()) {
        moov_content.extend_from_slice(&generate_hybrid_trak(track, st, movie_timescale));
    }

    make_box(b"moov", &moov_content)
}

// ===== Generate ftyp =====

pub fn generate_ftyp() -> Vec<u8> {
    let mut content = Vec::new();
    content.extend_from_slice(b"isom"); // major_brand
    write_u32_be(&mut content, 0x200); // minor_version
    content.extend_from_slice(b"isom");
    content.extend_from_slice(b"iso6");
    content.extend_from_slice(b"mp41");
    make_box(b"ftyp", &content)
}
