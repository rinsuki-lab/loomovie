// Data types used throughout the application

use serde::{Deserialize, Serialize};

// ===== Parsed MP4 Structures =====

#[derive(Debug, Clone)]
pub struct TfhdInfo {
    #[allow(dead_code)]
    pub track_id: u32,
    pub flags: u32,
    #[allow(dead_code)]
    pub base_data_offset: Option<u64>,
    pub sample_description_index: Option<u32>,
    pub default_sample_duration: Option<u32>,
    pub default_sample_size: Option<u32>,
    pub default_sample_flags: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct TfdtInfo {
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
    #[allow(dead_code)]
    pub handler_type: [u8; 4],
    pub trak_raw: Vec<u8>,
    pub trex_default_sample_description_index: u32,
    pub trex_default_sample_duration: u32,
    #[allow(dead_code)]
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

// ===== Sidx =====

pub struct SidxReference {
    pub referenced_size: u32,
    pub subsegment_duration: u32,
    pub starts_with_sap: bool,
    pub sap_type: u8,
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

#[derive(Serialize)]
pub struct SourceDest {
    pub r#type: String,
    pub offset: u64,
    pub length: u64,
}

#[derive(Serialize)]
pub struct SourceFile {
    pub source: String,
    pub sha256: String,
    pub dest: SourceDest,
}

#[derive(Serialize)]
pub struct SourcesOutput {
    pub files: Vec<SourceFile>,
}
