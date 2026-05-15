// Data types used throughout the application

use bytes::Bytes;
use serde::Deserialize;

// ===== Parsed MP4 Structures =====

#[derive(Debug, Clone)]
pub struct TfhdInfo {
    #[allow(dead_code)]
    pub track_id: u32,
    #[allow(dead_code)]
    pub flags: u32,
    #[allow(dead_code)]
    pub base_data_offset: Option<u64>,
    #[allow(dead_code)]
    pub sample_description_index: Option<u32>,
    pub default_sample_duration: Option<u32>,
    pub default_sample_size: Option<u32>,
    pub default_sample_flags: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct TfdtInfo {
    #[allow(dead_code)]
    pub version: u8,
    pub base_media_decode_time: u64,
}

#[derive(Debug, Clone)]
pub struct TrunSample {
    pub duration: Option<u32>,
    pub size: Option<u32>,
    pub flags: Option<u32>,
    pub composition_time_offset_raw: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct TrunInfo {
    pub version: u8,
    #[allow(dead_code)]
    pub flags: u32,
    pub sample_count: u32,
    #[allow(dead_code)]
    pub data_offset: Option<i32>,
    pub first_sample_flags: Option<u32>,
    pub samples: Vec<TrunSample>,
}

#[derive(Debug)]
pub struct TrackInfo {
    pub new_track_id: u32,
    pub timescale: u32,
    pub handler_type: [u8; 4],
    /// Raw tkhd box (with track_id already patched)
    pub tkhd_raw: Vec<u8>,
    /// Raw mdhd box
    pub mdhd_raw: Vec<u8>,
    /// Raw hdlr box
    pub hdlr_raw: Bytes,
    /// Raw media header box (vmhd, smhd, or nmhd)
    pub media_header_raw: Bytes,
    /// Raw dinf box
    pub dinf_raw: Bytes,
    /// Raw stsd box
    pub stsd_raw: Bytes,
    #[allow(dead_code)]
    pub trex_default_sample_description_index: u32,
    pub trex_default_sample_duration: u32,
    pub trex_default_sample_size: u32,
    pub trex_default_sample_flags: u32,
}

/// A single moof+mdat fragment within a chunk file
#[derive(Debug)]
pub struct FragmentInfo {
    /// Offset of the moof box within the original chunk file
    pub moof_offset: usize,
    /// Data offset from original trun (offset from moof start to first sample)
    pub original_data_offset: i32,
    /// Parsed tfhd
    pub tfhd: TfhdInfo,
    /// Parsed tfdt
    pub tfdt: Option<TfdtInfo>,
    /// Parsed trun
    pub trun: TrunInfo,
}

/// Result of parsing a chunk file — may contain multiple fragments
#[derive(Debug)]
pub struct ChunkParseResult {
    /// Total file size
    pub file_size: usize,
    /// All moof+mdat fragment pairs in this chunk file
    pub fragments: Vec<FragmentInfo>,
}

/// Per-track sample table data collected from all chunks (for Hybrid MP4 moov)
pub struct TrackSampleTable {
    /// base_media_decode_time of first fragment (from tfdt), in track timescale.
    /// Non-zero when media doesn't start at time 0 (e.g. audio with initial offset).
    pub media_start_time: u64,
    /// Total media duration in track timescale
    pub total_duration: u64,
    /// Size of each sample in bytes
    pub sample_sizes: Vec<u32>,
    /// Duration of each sample in track timescale
    pub sample_durations: Vec<u32>,
    /// Whether any sample has composition time offsets
    pub has_cts: bool,
    /// CTS version (0=unsigned, 1=signed)
    pub cts_version: u8,
    /// Composition time offset for each sample (empty if !has_cts)
    pub cts_offsets: Vec<i32>,
    /// 1-based sample indices of sync (key) samples
    pub sync_samples: Vec<u32>,
    /// Number of samples in each stbl "chunk".
    ///
    /// Each fragment is split into one or more *sub-chunks* whose maximum
    /// playback duration is bounded (see plan::SUBCHUNK_TARGET_DENOMINATOR) so
    /// that constrained players don't drop large trailing chunks. The length
    /// of this Vec equals the number of stbl chunks (= sub-chunks), not the
    /// number of source fragments.
    pub samples_per_chunk: Vec<u32>,
    /// Absolute file offset of first sample in each stbl "chunk" (sub-chunk).
    /// Same length as `samples_per_chunk`.
    pub chunk_offsets: Vec<u64>,
    /// Per-sub-chunk metadata used to fill in `chunk_offsets` once the moov
    /// size (and thus mdat start) is known. Same length as `chunk_offsets`.
    pub sub_chunks: Vec<SubChunkInfo>,
}

/// Locates a sub-chunk within the original input layout so its absolute byte
/// offset can be computed once the mdat position is finalized.
#[derive(Debug, Clone)]
pub struct SubChunkInfo {
    /// 0-based index into `loaded_chunks[stream_idx][..]` (the source m4s file).
    pub chunk_file_idx: u32,
    /// 0-based index of the fragment within that m4s file (typically 0).
    pub fragment_idx: u16,
    /// Byte offset from the start of the fragment's sample data to this
    /// sub-chunk's first sample (i.e. sum of `stsz` of preceding samples
    /// within the same fragment).
    pub byte_offset_in_fragment: u32,
}

// ===== JSON Types =====

#[derive(Deserialize)]
pub struct StreamConfig {
    #[allow(dead_code)]
    pub format: String,
    #[allow(dead_code)]
    pub codecs: Vec<String>,
    pub init: String,
    pub chunks: Vec<String>,
}

#[derive(Deserialize)]
pub struct InputConfig {
    pub streams: Vec<StreamConfig>,
}
