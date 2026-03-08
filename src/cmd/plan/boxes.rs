// Generation of MP4 boxes for Hybrid MP4 output
//
// The Hybrid MP4 wraps original fragmented MP4 source files inside a single mdat,
// with a moov containing full sample tables (stbl) whose chunk offsets skip over
// the fMP4 headers embedded in the source files.

use bytes::{BufMut, BytesMut};

use super::mp4_box::*;
use super::types::*;

// ===== moov children =====

fn generate_mvhd(buf: &mut BytesMut, timescale: u32, duration: u64, next_track_id: u32) {
    // Use version 1 (64-bit times) when duration overflows u32
    let use_v1 = duration > u32::MAX as u64;
    let mut buf = start_fullbox(buf, b"mvhd", if use_v1 { 1 } else { 0 }, 0);
    if use_v1 {
        buf.put_u64(0); // creation_time
        buf.put_u64(0); // modification_time
        buf.put_u32(timescale);
        buf.put_u64(duration);
    } else {
        buf.put_u32(0); // creation_time
        buf.put_u32(0); // modification_time
        buf.put_u32(timescale);
        buf.put_u32(duration as u32);
    }
    buf.put_u32(0x00010000); // rate = 1.0
    buf.put_slice(&[0x01, 0x00]); // volume = 1.0
    buf.put_slice(&[0u8; 10]); // reserved
    // Identity matrix
    for &v in &[0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
        buf.put_u32(v);
    }
    buf.put_slice(&[0u8; 24]); // pre_defined
    buf.put_u32(next_track_id);
    buf.finish();
}

// ===== stbl children =====

fn generate_stts(buf: &mut BytesMut, durations: &[u32]) {
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

    let mut buf = start_fullbox(buf, b"stts", 0, 0);
    buf.put_u32(entries.len() as u32);
    for &(count, delta) in &entries {
        buf.put_u32(count);
        buf.put_u32(delta);
    }
    buf.finish();
}

fn generate_ctts(buf: &mut BytesMut, offsets: &[i32], version: u8) {
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

    let mut buf = start_fullbox(buf, b"ctts", version, 0);
    buf.put_u32(entries.len() as u32);
    for &(count, offset) in &entries {
        buf.put_u32(count);
        if version == 0 {
            buf.put_u32(offset as u32);
        } else {
            buf.put_i32(offset);
        }
    }
    buf.finish();
}

fn generate_stsz(buf: &mut BytesMut, sizes: &[u32]) {
    // If all sizes are equal, use the compact form (sample_size != 0)
    let uniform = if !sizes.is_empty() && sizes.iter().all(|&s| s == sizes[0]) {
        sizes[0]
    } else {
        0
    };

    let mut buf = start_fullbox(buf, b"stsz", 0, 0);
    buf.put_u32(uniform);
    buf.put_u32(sizes.len() as u32);
    if uniform == 0 {
        for &s in sizes {
            buf.put_u32(s);
        }
    }
    buf.finish();
}

fn generate_stsc(buf: &mut BytesMut, samples_per_chunk: &[u32]) {
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

    let mut buf = start_fullbox(buf, b"stsc", 0, 0);
    buf.put_u32(entries.len() as u32);
    for &(first_chunk, spc) in &entries {
        buf.put_u32(first_chunk);
        buf.put_u32(spc);
        buf.put_u32(1); // sample_description_index
    }
    buf.finish();
}

fn generate_co64(buf: &mut BytesMut, offsets: &[u64]) {
    let mut buf = start_fullbox(buf, b"co64", 0, 0);
    buf.put_u32(offsets.len() as u32);
    for &o in offsets {
        buf.put_u64(o);
    }
    buf.finish();
}

fn generate_stss(buf: &mut BytesMut, sync_samples: &[u32]) {
    let mut buf = start_fullbox(buf, b"stss", 0, 0);
    buf.put_u32(sync_samples.len() as u32);
    for &s in sync_samples {
        buf.put_u32(s);
    }
    buf.finish();
}

// ===== stbl =====

fn generate_stbl(buf: &mut BytesMut, stsd_raw: &[u8], st: &TrackSampleTable) {
    let mut buf = start_box(buf, b"stbl");
    buf.put_slice(stsd_raw);
    generate_stts(&mut buf, &st.sample_durations);
    if st.has_cts {
        generate_ctts(&mut buf, &st.cts_offsets, st.cts_version);
    }
    generate_stsc(&mut buf, &st.samples_per_chunk);
    generate_stsz(&mut buf, &st.sample_sizes);
    generate_co64(&mut buf, &st.chunk_offsets);

    // stss only needed when not every sample is a sync sample
    if !st.sync_samples.is_empty() && st.sync_samples.len() < st.sample_sizes.len() {
        generate_stss(&mut buf, &st.sync_samples);
    }

    buf.finish();
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

/// Write an edts box with an elst entry for initial media time offset.
///
/// If media_start_time is 0, writes nothing.
fn generate_edts(
    buf: &mut BytesMut,
    media_start_time: u64,
    media_duration: u64,
    track_timescale: u32,
    movie_timescale: u32,
) {
    if media_start_time == 0 {
        return;
    }

    // Convert media_start_time from track timescale to movie timescale for the empty edit
    let empty_duration_movie =
        media_start_time * movie_timescale as u64 / track_timescale as u64;

    // The media edit segment duration (in movie timescale)
    let media_segment_duration_movie =
        media_duration * movie_timescale as u64 / track_timescale as u64;

    // Use version 1 (64-bit) if any value overflows u32
    let use_v1 = empty_duration_movie > u32::MAX as u64
        || media_segment_duration_movie > u32::MAX as u64
        || media_start_time > u32::MAX as u64;

    let mut edts = start_box(buf, b"edts");
    let mut elst = start_fullbox(&mut edts, b"elst", if use_v1 { 1 } else { 0 }, 0);

    // Two entries: empty edit + media edit
    elst.put_u32(2);

    // Entry 1: empty edit (segment_duration, media_time=-1)
    if use_v1 {
        elst.put_u64(empty_duration_movie);
        elst.put_i64(-1); // media_time = -1 means empty
    } else {
        elst.put_u32(empty_duration_movie as u32);
        elst.put_i32(-1);
    }
    elst.put_u16(1); // media_rate_integer
    elst.put_u16(0); // media_rate_fraction

    // Entry 2: media edit (play all media from time 0)
    if use_v1 {
        elst.put_u64(media_segment_duration_movie);
        elst.put_i64(0); // media_time = 0 (media starts at sample 0)
    } else {
        elst.put_u32(media_segment_duration_movie as u32);
        elst.put_i32(0);
    }
    elst.put_u16(1); // media_rate_integer
    elst.put_u16(0); // media_rate_fraction

    elst.finish();
    edts.finish();
}

/// Build a complete trak box for Hybrid MP4 output, writing directly to `buf`.
fn generate_hybrid_trak(
    buf: &mut BytesMut,
    track: &TrackInfo,
    st: &TrackSampleTable,
    movie_timescale: u32,
) {
    let mut trak = start_box(buf, b"trak");

    // Patch tkhd: set duration in movie timescale (includes initial empty edit)
    let mut tkhd = track.tkhd_raw.clone();
    let media_duration_movie = if track.timescale == movie_timescale {
        st.total_duration
    } else {
        st.total_duration * movie_timescale as u64 / track.timescale as u64
    };
    let empty_duration_movie = if st.media_start_time > 0 {
        st.media_start_time * movie_timescale as u64 / track.timescale as u64
    } else {
        0
    };
    let tkhd_duration = empty_duration_movie + media_duration_movie;
    patch_tkhd_duration(&mut tkhd, tkhd_duration);
    trak.put_slice(&tkhd);

    // edts (optional, writes nothing if media_start_time == 0)
    generate_edts(
        &mut trak,
        st.media_start_time,
        st.total_duration,
        track.timescale,
        movie_timescale,
    );

    // Patch mdhd: set duration in track timescale
    let mut mdhd = track.mdhd_raw.clone();
    patch_mdhd_duration(&mut mdhd, st.total_duration);

    // mdia = mdhd + hdlr + minf
    let mut mdia = start_box(&mut trak, b"mdia");
    mdia.put_slice(&mdhd);
    mdia.put_slice(&track.hdlr_raw);

    // minf = media_header + dinf + stbl
    let mut minf = start_box(&mut mdia, b"minf");
    minf.put_slice(&track.media_header_raw);
    minf.put_slice(&track.dinf_raw);
    generate_stbl(&mut minf, &track.stsd_raw, st);
    minf.finish();

    mdia.finish();
    trak.finish();
}

// ===== Generate Hybrid moov =====

/// Generate a complete moov box for Hybrid MP4.
pub fn generate_hybrid_moov(
    tracks: &[TrackInfo],
    sample_tables: &[TrackSampleTable],
) -> Vec<u8> {
    let movie_timescale = tracks[0].timescale;
    let max_track_id = tracks.iter().map(|t| t.new_track_id).max().unwrap_or(0);

    // Movie duration = max across tracks (converted to movie timescale, including initial offset)
    let movie_duration = tracks
        .iter()
        .zip(sample_tables.iter())
        .map(|(t, st)| {
            let media_dur = if t.timescale == movie_timescale {
                st.total_duration
            } else {
                st.total_duration * movie_timescale as u64 / t.timescale as u64
            };
            let empty_dur = if st.media_start_time > 0 {
                st.media_start_time * movie_timescale as u64 / t.timescale as u64
            } else {
                0
            };
            empty_dur + media_dur
        })
        .max()
        .unwrap_or(0);

    let mut buf = BytesMut::new();
    let mut moov = start_box(&mut buf, b"moov");
    generate_mvhd(&mut moov, movie_timescale, movie_duration, max_track_id + 1);
    for (track, st) in tracks.iter().zip(sample_tables.iter()) {
        generate_hybrid_trak(&mut moov, track, st, movie_timescale);
    }
    moov.finish();
    buf.into()
}

// ===== Generate ftyp =====

pub fn generate_ftyp() -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(28); // 8 (box header) + 20 (content)
    let mut ftyp = start_box(&mut buf, b"ftyp");
    ftyp.put_slice(b"isom"); // major_brand
    ftyp.put_u32(0x200); // minor_version
    ftyp.put_slice(b"isom");
    ftyp.put_slice(b"iso6");
    ftyp.put_slice(b"mp41");
    ftyp.finish();
    buf.into()
}
