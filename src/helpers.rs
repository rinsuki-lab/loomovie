// Helper functions for sample analysis

use crate::types::*;

/// Check if the first sample is a SAP (Stream Access Point / keyframe)
pub fn is_first_sample_sap(trun: &TrunInfo, tfhd: &TfhdInfo, trex_default_flags: u32) -> bool {
    let first_flags = if let Some(fsf) = trun.first_sample_flags {
        fsf
    } else if let Some(ref sample) = trun.samples.first() {
        if let Some(f) = sample.flags {
            f
        } else {
            tfhd.default_sample_flags.unwrap_or(trex_default_flags)
        }
    } else {
        tfhd.default_sample_flags.unwrap_or(trex_default_flags)
    };

    let sample_depends_on = (first_flags >> 24) & 0x3;
    let sample_is_non_sync = (first_flags >> 16) & 0x1;
    sample_depends_on == 2 || sample_is_non_sync == 0
}

/// Calculate total sample duration for a single fragment's trun
pub fn total_sample_duration(trun: &TrunInfo, tfhd: &TfhdInfo, trex_default_duration: u32) -> u64 {
    let default_duration = tfhd.default_sample_duration.unwrap_or(trex_default_duration);
    trun.samples
        .iter()
        .map(|s| s.duration.unwrap_or(default_duration) as u64)
        .sum()
}

/// Calculate total duration across all fragments in a chunk for a given track
pub fn total_chunk_duration(
    fragments: &[FragmentInfo],
    trex_default_duration: u32,
) -> u64 {
    fragments
        .iter()
        .map(|frag| total_sample_duration(&frag.trun, &frag.tfhd, trex_default_duration))
        .sum()
}
