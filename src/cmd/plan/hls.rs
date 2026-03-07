/// HLS playlist generation (RFC 8216 compliant).

/// Per-stream info needed for master playlist generation.
pub struct StreamInfo<'a> {
    pub codecs: &'a [String],
    pub bandwidth: u64,
    pub is_video: bool,
    pub is_audio: bool,
}

/// Compute peak segment bit rate (BANDWIDTH) for a stream.
///
/// RFC 8216: "the largest bit rate of any contiguous set of segments whose total
/// duration is between 0.5 and 1.5 times the target duration"
pub fn compute_peak_bandwidth(chunk_sizes: &[u64], chunk_durations: &[f64]) -> u64 {
    let target_dur = chunk_durations.iter().cloned().fold(0.0f64, f64::max).ceil();
    let mut peak: f64 = 0.0;
    for start in 0..chunk_durations.len() {
        let mut total_size: u64 = 0;
        let mut total_dur: f64 = 0.0;
        for end in start..chunk_durations.len() {
            total_size += chunk_sizes[end];
            total_dur += chunk_durations[end];
            if total_dur >= 0.5 * target_dur {
                if total_dur > 1.5 * target_dur {
                    break;
                }
                let bitrate = (total_size as f64 * 8.0) / total_dur;
                if bitrate > peak {
                    peak = bitrate;
                }
            }
        }
    }
    peak.ceil() as u64
}

/// Generate the master playlist (generated.m3u8).
pub fn generate_master_playlist(streams: &[StreamInfo]) -> Vec<u8> {
    let num_streams = streams.len();

    // Find audio-only stream index to pair with video-only streams
    let audio_only_idx: Option<usize> = {
        let has_video = streams.iter().any(|s| s.is_video);
        let audio_idx = streams.iter().position(|s| s.is_audio);
        if has_video { audio_idx } else { None }
    };

    let mut m3u8 = String::new();
    m3u8.push_str("#EXTM3U\n");
    m3u8.push_str("#EXT-X-VERSION:6\n");
    m3u8.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");

    // If we have separate audio stream, declare it as a media group
    if let Some(audio_idx) = audio_only_idx {
        let group_name = format!("streams.{}", audio_idx);
        m3u8.push_str(&format!(
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"{}\",NAME=\"{}\",DEFAULT=YES,AUTOSELECT=YES,URI=\"streams.{}/generated.m3u8\"\n",
            group_name, group_name, audio_idx,
        ));
    }

    for stream_idx in 0..num_streams {
        // Skip audio-only streams from STREAM-INF (they're referenced via AUDIO group)
        if audio_only_idx == Some(stream_idx) {
            continue;
        }

        let stream = &streams[stream_idx];
        let mut bandwidth = stream.bandwidth;
        let mut codecs: Vec<&str> = stream.codecs.iter().map(|s| s.as_str()).collect();
        if let Some(audio_idx) = audio_only_idx {
            if stream.is_video {
                // BANDWIDTH must cover all playable renditions combined
                bandwidth += streams[audio_idx].bandwidth;
                // CODECS must include every format in all renditions
                codecs.extend(streams[audio_idx].codecs.iter().map(|s| s.as_str()));
                let group_name = format!("streams.{}", audio_idx);
                m3u8.push_str(&format!(
                    "#EXT-X-STREAM-INF:BANDWIDTH={},CODECS=\"{}\",AUDIO=\"{}\"\n",
                    bandwidth,
                    codecs.join(","),
                    group_name,
                ));
            } else {
                m3u8.push_str(&format!(
                    "#EXT-X-STREAM-INF:BANDWIDTH={},CODECS=\"{}\"\n",
                    bandwidth,
                    codecs.join(","),
                ));
            }
        } else {
            m3u8.push_str(&format!(
                "#EXT-X-STREAM-INF:BANDWIDTH={},CODECS=\"{}\"\n",
                bandwidth,
                codecs.join(","),
            ));
        }
        m3u8.push_str(&format!("streams.{}/generated.m3u8\n", stream_idx));
    }

    m3u8.into_bytes()
}

/// Generate a per-stream media playlist (streams.N/generated.m3u8).
pub fn generate_media_playlist(chunk_durations: &[f64]) -> Vec<u8> {
    let target_duration = chunk_durations.iter().cloned().fold(0.0f64, f64::max).ceil() as u64;

    let mut m3u8 = String::new();
    m3u8.push_str("#EXTM3U\n");
    m3u8.push_str(&format!("#EXT-X-TARGETDURATION:{}\n", target_duration));
    m3u8.push_str("#EXT-X-VERSION:6\n");
    m3u8.push_str("#EXT-X-MAP:URI=\"init.m4s\"\n");
    m3u8.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");
    m3u8.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
    for (chunk_idx, dur) in chunk_durations.iter().enumerate() {
        m3u8.push_str(&format!("#EXTINF:{:.6},\n", dur));
        m3u8.push_str(&format!("chunks/chunk.{:06}.m4s\n", chunk_idx));
    }
    m3u8.push_str("#EXT-X-ENDLIST\n");

    m3u8.into_bytes()
}
