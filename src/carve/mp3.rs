//! MP3 (MPEG Audio Layer III) file carving handler.
//!
//! MP3 files can start with:
//! - ID3v2 tag header: "ID3" (0x49 0x44 0x33)
//! - MPEG audio frame sync: 0xFF 0xFB, 0xFF 0xFA, 0xFF 0xF3, 0xFF 0xF2
//!
//! Size detection walks MPEG audio frames until end of stream.

use crate::carve::{
    CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext, PendingCarve,
    PreValidation, output_path,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

/// MPEG audio version IDs
const _MPEG_VERSION_25: u8 = 0;
const _MPEG_VERSION_2: u8 = 2;
const MPEG_VERSION_1: u8 = 3;

/// MPEG audio layer IDs
const LAYER_III: u8 = 1;
const _LAYER_II: u8 = 2;
const LAYER_I: u8 = 3;

/// Bitrate table for MPEG1 Layer III (kbps)
const BITRATES_V1_L3: [u16; 16] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
];

/// Bitrate table for MPEG2/2.5 Layer III (kbps)
const BITRATES_V2_L3: [u16; 16] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
];

/// Sample rate table `[version][index]` in Hz
const SAMPLE_RATES: [[u32; 4]; 4] = [
    [11025, 12000, 8000, 0],  // MPEG 2.5
    [0, 0, 0, 0],             // Reserved
    [22050, 24000, 16000, 0], // MPEG 2
    [44100, 48000, 32000, 0], // MPEG 1
];

/// Samples per frame `[version][layer]`
const SAMPLES_PER_FRAME: [[u32; 4]; 4] = [
    [0, 576, 1152, 384],  // MPEG 2.5
    [0, 0, 0, 0],         // Reserved
    [0, 576, 1152, 384],  // MPEG 2
    [0, 1152, 1152, 384], // MPEG 1
];

/// Minimum number of consecutive valid frames required for sync-word based detection.
/// This helps reduce false positives from random 0xFFFB/0xFFFA bytes.
/// Increased from 3 to 5 for better false positive rejection.
const MIN_FRAMES_FOR_SYNC_VALIDATION: u32 = 5;

/// ID3-backed candidates already have a validated metadata header, so they only need a
/// short run of consistent audio frames to prove the stream is real.
const MIN_FRAMES_FOR_ID3_VALIDATION: u32 = 2;

/// Maximum duration in seconds (1 hour) - used to reject implausibly long files
const MAX_DURATION_SECONDS: u64 = 60 * 60;

/// Maximum ID3v2 tag size (32 MB) — real tags rarely exceed this even with embedded art
const MAX_ID3V2_TAG_SIZE: u64 = 32 * 1024 * 1024;

pub struct Mp3CarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mp3CandidateKind {
    Id3Backed,
    SyncOnly,
}

impl Mp3CandidateKind {
    fn min_required_frames(self) -> u32 {
        match self {
            Self::Id3Backed => MIN_FRAMES_FOR_ID3_VALIDATION,
            Self::SyncOnly => MIN_FRAMES_FOR_SYNC_VALIDATION,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Id3Backed => "ID3-backed",
            Self::SyncOnly => "sync-word",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mp3CandidateStart {
    kind: Mp3CandidateKind,
    audio_offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mp3FrameParams {
    frame_size: u32,
    version_id: u8,
    layer_id: u8,
    sample_rate: u32,
    samples_per_frame: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mp3StreamProfile {
    version_id: u8,
    layer_id: u8,
    sample_rate: u32,
}

impl Mp3StreamProfile {
    fn from_frame(frame: Mp3FrameParams) -> Self {
        Self {
            version_id: frame.version_id,
            layer_id: frame.layer_id,
            sample_rate: frame.sample_rate,
        }
    }

    fn matches(self, frame: Mp3FrameParams) -> bool {
        self.version_id == frame.version_id
            && self.layer_id == frame.layer_id
            && self.sample_rate == frame.sample_rate
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mp3ProbeResult {
    candidate: Mp3CandidateStart,
    frame_count: u32,
}

impl Mp3ProbeResult {
    fn is_valid(self) -> bool {
        self.frame_count >= self.candidate.kind.min_required_frames()
    }
}

impl Mp3CarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

/// Parse ID3v2 tag header and return total tag size including header (10 bytes + tag data).
/// ID3v2 header format:
/// - Bytes 0-2: "ID3"
/// - Byte 3: Version major
/// - Byte 4: Version minor  
/// - Byte 5: Flags
/// - Bytes 6-9: Size (syncsafe integer, 28 bits)
fn parse_id3v2_size(header: &[u8]) -> Option<u64> {
    if header.len() < 10 {
        return None;
    }
    if &header[0..3] != b"ID3" {
        return None;
    }

    // Major version must be 2, 3, or 4
    let major = header[3];
    if major != 2 && major != 3 && major != 4 {
        return None;
    }

    // Syncsafe bytes: bits 7 must be 0 per spec
    if header[6] & 0x80 != 0
        || header[7] & 0x80 != 0
        || header[8] & 0x80 != 0
        || header[9] & 0x80 != 0
    {
        return None;
    }

    // Syncsafe integer: each byte's MSB is 0, so only 7 bits per byte.
    // Masks retained as defense-in-depth even though MSB=0 is enforced above.
    let size = ((header[6] as u64 & 0x7F) << 21)
        | ((header[7] as u64 & 0x7F) << 14)
        | ((header[8] as u64 & 0x7F) << 7)
        | (header[9] as u64 & 0x7F);

    let total = 10 + size;

    // Cap at maximum ID3v2 tag size
    if total > MAX_ID3V2_TAG_SIZE {
        return None;
    }

    Some(total)
}

/// Parse MPEG audio frame header and return (frame_size, sample_rate, samples_per_frame).
/// Frame header is 4 bytes with sync word 0xFFE or 0xFFF.
#[cfg_attr(not(test), allow(dead_code))]
fn parse_frame_header_with_rate(header: &[u8]) -> Option<(u32, u32, u32)> {
    let frame = extract_frame_params(header)?;
    Some((frame.frame_size, frame.sample_rate, frame.samples_per_frame))
}

/// Extract the frame metadata needed for probing and in-carve consistency checks.
fn extract_frame_params(header: &[u8]) -> Option<Mp3FrameParams> {
    if header.len() < 4 {
        return None;
    }

    // Check frame sync (11 bits: 0xFF + upper 3 bits of second byte)
    if header[0] != 0xFF || (header[1] & 0xE0) != 0xE0 {
        return None;
    }

    let version_id = (header[1] >> 3) & 0x03;
    let layer_id = (header[1] >> 1) & 0x03;
    let bitrate_idx = (header[2] >> 4) & 0x0F;
    let sample_rate_idx = (header[2] >> 2) & 0x03;
    let padding = (header[2] >> 1) & 0x01;

    // Invalid values
    if version_id == 1
        || layer_id == 0
        || bitrate_idx == 0
        || bitrate_idx == 15
        || sample_rate_idx == 3
    {
        return None;
    }

    let sample_rate = SAMPLE_RATES[version_id as usize][sample_rate_idx as usize];
    if sample_rate == 0 {
        return None;
    }

    let bitrate = if version_id == MPEG_VERSION_1 {
        match layer_id {
            LAYER_III => BITRATES_V1_L3[bitrate_idx as usize],
            // For simplicity, use Layer III table for others too
            _ => BITRATES_V1_L3[bitrate_idx as usize],
        }
    } else {
        BITRATES_V2_L3[bitrate_idx as usize]
    } as u32;

    if bitrate == 0 {
        return None;
    }

    let samples = SAMPLES_PER_FRAME[version_id as usize][layer_id as usize];
    if samples == 0 {
        return None;
    }

    // Frame size calculation
    // For Layer I: frame_size = (12 * bitrate * 1000 / sample_rate + padding) * 4
    // For Layer II/III: frame_size = 144 * bitrate * 1000 / sample_rate + padding
    let frame_size = if layer_id == LAYER_I {
        (12 * bitrate * 1000 / sample_rate + padding as u32) * 4
    } else {
        let slot_size = if version_id == MPEG_VERSION_1 {
            144
        } else {
            72
        };
        slot_size * bitrate * 1000 / sample_rate + padding as u32
    };

    if frame_size < 4 {
        return None;
    }

    Some(Mp3FrameParams {
        frame_size,
        version_id,
        layer_id,
        sample_rate,
        samples_per_frame: samples,
    })
}

/// Check for ID3v1 tag at the given data (128 bytes starting with "TAG").
fn is_id3v1_tag(data: &[u8]) -> bool {
    data.len() >= 3 && &data[0..3] == b"TAG"
}

fn read_exact_at(
    evidence: &dyn EvidenceSource,
    offset: u64,
    len: usize,
) -> Result<Option<Vec<u8>>, CarveError> {
    let mut buf = vec![0u8; len];

    let mut read = 0usize;
    let mut current_offset = offset;
    while read < len {
        let n = evidence
            .read_at(current_offset, &mut buf[read..])
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n == 0 {
            return Ok(None);
        }
        read += n;
        current_offset = current_offset.saturating_add(n as u64);
    }

    Ok(Some(buf))
}

fn detect_mp3_candidate(
    evidence: &dyn EvidenceSource,
    offset: u64,
) -> Result<Mp3CandidateStart, CarveError> {
    let prefix = read_exact_at(evidence, offset, 4)?
        .ok_or_else(|| CarveError::Invalid("mp3: truncated header".to_string()))?;

    if &prefix[0..3] == b"ID3" {
        let id3_header = read_exact_at(evidence, offset, 10)?
            .ok_or_else(|| CarveError::Invalid("mp3: truncated ID3v2 header".to_string()))?;
        let audio_offset = parse_id3v2_size(&id3_header)
            .ok_or_else(|| CarveError::Invalid("mp3: invalid ID3v2 header".to_string()))?;
        return Ok(Mp3CandidateStart {
            kind: Mp3CandidateKind::Id3Backed,
            audio_offset,
        });
    }

    if extract_frame_params(&prefix).is_some() {
        return Ok(Mp3CandidateStart {
            kind: Mp3CandidateKind::SyncOnly,
            audio_offset: 0,
        });
    }

    Err(CarveError::Invalid("mp3: signature mismatch".to_string()))
}

/// Maximum MPEG frame size (320kbps, 32kHz, MPEG1 Layer III with padding):
/// 144 * 320000 / 32000 + 1 = 1441 bytes
const MAX_FRAME_SIZE: u64 = 1441;

/// Check whether the hit at `offset` is an interior frame within an existing MP3 stream.
///
/// Reads up to MAX_FRAME_SIZE bytes before the hit offset and looks for a valid MPEG
/// audio frame header whose computed frame size lands exactly at `offset`.
/// If found, this hit is interior (not the start of a stream) and should be rejected.
fn is_interior_frame(evidence: &dyn EvidenceSource, offset: u64) -> Result<bool, CarveError> {
    if offset < 4 {
        return Ok(false);
    }

    let lookback = offset.min(MAX_FRAME_SIZE);
    let start = offset - lookback;

    let buf = match read_exact_at(evidence, start, lookback as usize)? {
        Some(buf) => buf,
        None => return Ok(false),
    };

    // Scan for sync words in the buffer
    for i in 0..buf.len().saturating_sub(3) {
        if buf[i] == 0xFF
            && (buf[i + 1] & 0xE0) == 0xE0
            && let Some(frame) = extract_frame_params(&buf[i..i + 4])
        {
            let candidate_start = start + i as u64;
            if candidate_start + frame.frame_size as u64 == offset {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

fn probe_mp3_candidate(
    evidence: &dyn EvidenceSource,
    offset: u64,
) -> Result<Mp3ProbeResult, CarveError> {
    let candidate = detect_mp3_candidate(evidence, offset)?;
    let mut current_offset = offset.saturating_add(candidate.audio_offset);
    let mut frame_count = 0u32;
    let mut total_samples = 0u64;
    let mut expected_profile: Option<Mp3StreamProfile> = None;

    while frame_count < candidate.kind.min_required_frames() {
        let frame_header = match read_exact_at(evidence, current_offset, 4)? {
            Some(header) => header,
            None => break,
        };

        if frame_count > 0 && is_id3v1_tag(&frame_header) {
            break;
        }

        let frame = match extract_frame_params(&frame_header) {
            Some(frame) => frame,
            None => break,
        };

        if let Some(profile) = expected_profile {
            if !profile.matches(frame) {
                break;
            }
        } else {
            expected_profile = Some(Mp3StreamProfile::from_frame(frame));
        }

        total_samples += frame.samples_per_frame as u64;
        if let Some(profile) = expected_profile
            && total_samples / profile.sample_rate as u64 > MAX_DURATION_SECONDS
        {
            break;
        }

        frame_count += 1;
        current_offset = current_offset.saturating_add(frame.frame_size as u64);
    }

    Ok(Mp3ProbeResult {
        candidate,
        frame_count,
    })
}

fn probe_rejection_reason(probe: Mp3ProbeResult) -> String {
    format!(
        "mp3 {} candidate had {} consistent frame(s); requires {}",
        probe.candidate.kind.label(),
        probe.frame_count,
        probe.candidate.kind.min_required_frames()
    )
}

impl CarveHandler for Mp3CarveHandler {
    fn file_type(&self) -> &str {
        "mp3"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        match probe_mp3_candidate(evidence, offset) {
            Ok(probe) if probe.is_valid() => {
                // For sync-word candidates, check if this is an interior frame
                // within an existing MP3 stream (not the start of a new file).
                if probe.candidate.kind == Mp3CandidateKind::SyncOnly
                    && is_interior_frame(evidence, offset)?
                {
                    return Ok(PreValidation::Reject(
                        "mp3: interior frame within existing stream".to_string(),
                    ));
                }
                Ok(PreValidation::Proceed)
            }
            Ok(probe) => Ok(PreValidation::Reject(probe_rejection_reason(probe))),
            Err(CarveError::Invalid(reason)) => Ok(PreValidation::Reject(reason)),
            Err(other) => Err(other),
        }
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let probe = match probe_mp3_candidate(ctx.evidence, hit.global_offset) {
            Ok(probe) if probe.is_valid() => probe,
            Ok(_) | Err(CarveError::Invalid(_)) => return Ok(None),
            Err(other) => return Err(other),
        };

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut stream = CarveStream::new(ctx, hit.global_offset, self.max_size, full_path.clone());

        let mut validated = false;
        let mut truncated = false;
        let mut errors = Vec::new();

        let result: Result<u64, CarveError> = (|| {
            if probe.candidate.audio_offset > 0 {
                stream.read_exact(probe.candidate.audio_offset as usize)?;
            }

            let mut total_size = probe.candidate.audio_offset;
            let mut frame_count = 0u32;
            let max_frames = 100_000u32; // Reasonable limit
            let max_size = if self.max_size > 0 {
                self.max_size
            } else {
                500 * 1024 * 1024
            };

            let mut expected_profile: Option<Mp3StreamProfile> = None;
            let mut total_samples: u64 = 0;

            // Walk remaining frames (peek before writing to avoid trailing garbage)
            while frame_count < max_frames && total_size < max_size {
                let next_offset = hit.global_offset.saturating_add(total_size);
                let frame_header = match read_exact_at(ctx.evidence, next_offset, 4)? {
                    Some(header) => header,
                    None => break,
                };

                // Check for ID3v1 tag at end
                if frame_count > 0 && is_id3v1_tag(&frame_header) {
                    stream.read_exact(128)?;
                    total_size += 128;
                    break;
                }

                if let Some(frame) = extract_frame_params(&frame_header) {
                    if let Some(profile) = expected_profile {
                        if !profile.matches(frame) {
                            break;
                        }
                    } else {
                        expected_profile = Some(Mp3StreamProfile::from_frame(frame));
                    }

                    stream.read_exact(frame.frame_size as usize)?;
                    total_size += frame.frame_size as u64;
                    frame_count += 1;
                    total_samples += frame.samples_per_frame as u64;

                    // Check duration limit
                    if let Some(profile) = expected_profile
                        && total_samples / profile.sample_rate as u64 > MAX_DURATION_SECONDS
                    {
                        // Implausibly long - stop here
                        break;
                    }
                } else {
                    // Invalid frame header - stop without writing it
                    break;
                }
            }

            if frame_count >= probe.candidate.kind.min_required_frames() {
                validated = true;
            }

            Ok(total_size)
        })();

        if let Err(err) = result {
            match err {
                CarveError::Truncated | CarveError::Eof => {
                    truncated = true;
                    errors.push(err.to_string());
                }
                CarveError::Invalid(_) => {
                    stream.discard();
                    return Ok(None);
                }
                other => return Err(other),
            }
        }

        // If not validated (e.g., sync word with fewer than MIN_FRAMES), reject
        if !validated && !truncated {
            stream.discard();
            return Ok(None);
        }

        let (size, md5_hex, sha256_hex, mut writer) = stream.finalize()?;

        // Check minimum size
        if size < self.min_size {
            writer.discard();
            return Ok(None);
        }

        // Check if we hit max_size
        if self.max_size > 0 && size >= self.max_size {
            truncated = true;
            if !errors.iter().any(|e| e.contains("max_size")) {
                errors.push("max_size reached".to_string());
            }
        }

        let global_end = if size == 0 {
            hit.global_offset
        } else {
            hit.global_offset + size - 1
        };

        Ok(Some(PendingCarve::new(
            CarvedFile {
                run_id: ctx.run_id.to_string(),
                file_type: self.file_type().to_string(),
                path: rel_path,
                extension: self.extension.clone(),
                global_start: hit.global_offset,
                global_end,
                size,
                md5: md5_hex,
                sha256: sha256_hex,
                validated,
                truncated,
                errors,
                pattern_id: Some(hit.pattern_id.clone()),
                is_duplicate: false,
                duplicate_of_offset: None,
            },
            writer,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::{EvidenceError, EvidenceSource};
    use tempfile::tempdir;

    struct SliceEvidence {
        data: Vec<u8>,
    }

    impl EvidenceSource for SliceEvidence {
        fn len(&self) -> u64 {
            self.data.len() as u64
        }

        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, EvidenceError> {
            if offset as usize >= self.data.len() {
                return Ok(0);
            }
            let max = self.data.len() - offset as usize;
            let to_copy = buf.len().min(max);
            buf[..to_copy].copy_from_slice(&self.data[offset as usize..offset as usize + to_copy]);
            Ok(to_copy)
        }
    }

    fn create_id3v2_header(tag_size: u32) -> Vec<u8> {
        let mut header = Vec::new();
        header.extend_from_slice(b"ID3");
        header.push(4); // Version major
        header.push(0); // Version minor
        header.push(0); // Flags

        // Syncsafe integer encoding
        header.push(((tag_size >> 21) & 0x7F) as u8);
        header.push(((tag_size >> 14) & 0x7F) as u8);
        header.push(((tag_size >> 7) & 0x7F) as u8);
        header.push((tag_size & 0x7F) as u8);

        header
    }

    fn create_mp3_frame(bitrate_idx: u8, sample_rate_idx: u8, padding: bool) -> Vec<u8> {
        create_audio_frame(0xFB, bitrate_idx, sample_rate_idx, padding)
    }

    fn create_audio_frame(
        second_byte: u8,
        bitrate_idx: u8,
        sample_rate_idx: u8,
        padding: bool,
    ) -> Vec<u8> {
        let mut header = vec![
            0xFF,
            second_byte,
            (bitrate_idx << 4) | (sample_rate_idx << 2) | if padding { 2 } else { 0 },
            0x00, // Private, channel mode, etc.
        ];

        // Calculate frame size and add padding data
        if let Some(frame) = extract_frame_params(&header) {
            header.resize(frame.frame_size as usize, 0x00);
        }

        header
    }

    #[test]
    fn parse_id3v2_size_basic() {
        let header = create_id3v2_header(1000);
        let size = parse_id3v2_size(&header).unwrap();
        assert_eq!(size, 1010); // 10 + 1000
    }

    #[test]
    fn parse_frame_header_basic() {
        // MPEG1 Layer III, 128kbps, 44100Hz, no padding
        let header = [0xFF, 0xFB, 0x90, 0x00];
        let (size, rate, samples) = parse_frame_header_with_rate(&header).unwrap();
        assert_eq!(size, 417); // 144 * 128000 / 44100 = 417
        assert_eq!(rate, 44100);
        assert_eq!(samples, 1152); // MPEG1 Layer III
    }

    #[test]
    fn parse_frame_header_mpeg2_samples() {
        // MPEG2 Layer III uses 576 samples per frame, not 1152
        // 0xFF 0xF3 = sync + MPEG2 (10) + Layer III (01) + no CRC (1)
        // 0x90 = bitrate_idx=9 (80kbps for MPEG2 L3), sample_rate_idx=0 (22050Hz)
        let header = [0xFF, 0xF3, 0x90, 0x00];
        let (size, rate, samples) = parse_frame_header_with_rate(&header).unwrap();
        assert_eq!(rate, 22050);
        assert_eq!(samples, 576); // MPEG2 Layer III uses 576 samples
        assert_eq!(size, 72 * 80000 / 22050); // 72 slot size for MPEG2, 80kbps
    }

    #[test]
    fn carves_mp3_with_id3v2() {
        let mut mp3_data = create_id3v2_header(100);
        mp3_data.resize(110, 0x00); // ID3 tag data

        // Add enough frames to pass validation (MIN_FRAMES_FOR_SYNC_VALIDATION)
        for _ in 0..5 {
            mp3_data.extend_from_slice(&create_mp3_frame(9, 0, false)); // 128kbps, 44100Hz
        }

        let evidence = SliceEvidence {
            data: mp3_data.clone(),
        };
        let handler = Mp3CarveHandler::new("mp3".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "mp3".to_string(),
            pattern_id: "mp3_id3v2".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        let carved = result.expect("carved file").flush().expect("flush");

        assert_eq!(carved.file_type, "mp3");
        assert!(carved.validated);
    }

    #[test]
    fn carves_mp3_without_id3() {
        let mut mp3_data = Vec::new();

        // Just frames, no ID3
        for _ in 0..5 {
            mp3_data.extend_from_slice(&create_mp3_frame(9, 0, false));
        }

        let evidence = SliceEvidence {
            data: mp3_data.clone(),
        };
        let handler = Mp3CarveHandler::new("mp3".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "mp3".to_string(),
            pattern_id: "mp3_sync".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        let carved = result.expect("carved file").flush().expect("flush");

        assert_eq!(carved.file_type, "mp3");
        assert!(carved.validated);
    }

    #[test]
    fn rejects_invalid_data() {
        let data = vec![0x00; 100]; // Not an MP3

        let evidence = SliceEvidence { data };
        let handler = Mp3CarveHandler::new("mp3".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "mp3".to_string(),
            pattern_id: "mp3_id3v2".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        assert!(result.is_none());
    }

    #[test]
    fn rejects_sync_word_with_too_few_frames() {
        // Create only 1-2 valid frames - should be rejected for sync-word based detection
        let mut mp3_data = Vec::new();
        mp3_data.extend_from_slice(&create_mp3_frame(9, 0, false)); // Just 1 frame
        mp3_data.extend_from_slice(&[0x00; 100]); // Garbage after

        let evidence = SliceEvidence {
            data: mp3_data.clone(),
        };
        let handler = Mp3CarveHandler::new("mp3".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "mp3".to_string(),
            pattern_id: "mp3_sync".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        assert!(matches!(
            handler.pre_validate(&evidence, 0),
            Ok(PreValidation::Reject(_))
        ));

        let result = handler.process_hit(&hit, &ctx).expect("process");
        assert!(
            result.is_none(),
            "Should reject sync-word hit with only 1 valid frame"
        );
    }

    #[test]
    fn accepts_sync_word_with_enough_frames() {
        // Create exactly MIN_FRAMES_FOR_SYNC_VALIDATION frames
        let mut mp3_data = Vec::new();
        for _ in 0..MIN_FRAMES_FOR_SYNC_VALIDATION {
            mp3_data.extend_from_slice(&create_mp3_frame(9, 0, false));
        }

        let evidence = SliceEvidence {
            data: mp3_data.clone(),
        };
        let handler = Mp3CarveHandler::new("mp3".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "mp3".to_string(),
            pattern_id: "mp3_sync".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        assert!(
            result.is_some(),
            "Should accept sync-word hit with {} valid frames",
            MIN_FRAMES_FOR_SYNC_VALIDATION
        );
    }

    #[test]
    fn rejects_invalid_id3v2_version() {
        let mut data = Vec::new();
        data.extend_from_slice(b"ID3");
        data.push(0x30); // Invalid major version
        data.push(0x00);
        data.push(0x00);
        data.extend_from_slice(&[0x00, 0x00, 0x01, 0x00]); // syncsafe size
        data.resize(200, 0x00);

        let evidence = SliceEvidence { data };
        let handler = Mp3CarveHandler::new("mp3".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "mp3".to_string(),
            pattern_id: "mp3_id3v2".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        assert!(result.is_none(), "Should reject invalid ID3v2 version");
    }

    #[test]
    fn rejects_id3v2_without_audio_frames() {
        // Valid ID3v2.3 header with reasonable size, but no valid frames after
        let mut data = create_id3v2_header(200);
        data.resize(210, 0x00); // ID3 tag data (zeros, no valid frames)
        data.resize(500, 0x00); // Extra space with no valid frames

        let evidence = SliceEvidence { data };
        let handler = Mp3CarveHandler::new("mp3".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "mp3".to_string(),
            pattern_id: "mp3_id3v2".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        assert!(matches!(
            handler.pre_validate(&evidence, 0),
            Ok(PreValidation::Reject(_))
        ));

        let result = handler.process_hit(&hit, &ctx).expect("process");
        assert!(
            result.is_none(),
            "Should reject ID3v2 tag with no audio frames"
        );
    }

    #[test]
    fn probe_rejects_sync_word_with_inconsistent_version() {
        let mut data = Vec::new();
        for _ in 0..(MIN_FRAMES_FOR_SYNC_VALIDATION - 1) {
            data.extend_from_slice(&create_mp3_frame(9, 0, false));
        }
        data.extend_from_slice(&create_audio_frame(0xF3, 9, 0, false));

        let evidence = SliceEvidence { data };
        let probe = probe_mp3_candidate(&evidence, 0).expect("probe");

        assert_eq!(probe.frame_count, MIN_FRAMES_FOR_SYNC_VALIDATION - 1);
        assert!(!probe.is_valid());
    }

    #[test]
    fn probe_rejects_sync_word_with_inconsistent_layer() {
        let mut data = Vec::new();
        for _ in 0..(MIN_FRAMES_FOR_SYNC_VALIDATION - 1) {
            data.extend_from_slice(&create_mp3_frame(9, 0, false));
        }
        data.extend_from_slice(&create_audio_frame(0xFD, 9, 0, false));

        let evidence = SliceEvidence { data };
        let probe = probe_mp3_candidate(&evidence, 0).expect("probe");

        assert_eq!(probe.frame_count, MIN_FRAMES_FOR_SYNC_VALIDATION - 1);
        assert!(!probe.is_valid());
    }

    #[test]
    fn accepts_id3v2_vbr_with_two_consistent_frames() {
        let mut mp3_data = create_id3v2_header(32);
        mp3_data.resize(42, 0x00);
        mp3_data.extend_from_slice(&create_mp3_frame(9, 0, false));
        mp3_data.extend_from_slice(&create_mp3_frame(11, 0, false));

        let evidence = SliceEvidence {
            data: mp3_data.clone(),
        };
        let handler = Mp3CarveHandler::new("mp3".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "mp3".to_string(),
            pattern_id: "mp3_id3v2".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        assert!(matches!(
            handler.pre_validate(&evidence, 0),
            Ok(PreValidation::Proceed)
        ));

        let result = handler.process_hit(&hit, &ctx).expect("process");
        let carved = result.expect("carved file").flush().expect("flush");

        assert!(carved.validated);
        assert_eq!(
            carved.size,
            42 + create_mp3_frame(9, 0, false).len() as u64
                + create_mp3_frame(11, 0, false).len() as u64
        );
    }

    #[test]
    fn rejects_interior_frame_at_second_offset() {
        // Create a multi-frame MP3 (no ID3)
        let mut mp3_data = Vec::new();
        for _ in 0..10 {
            mp3_data.extend_from_slice(&create_mp3_frame(9, 0, false)); // 128kbps, 44100Hz, size=417
        }

        let evidence = SliceEvidence { data: mp3_data };
        let handler = Mp3CarveHandler::new("mp3".to_string(), 0, 0);

        // First frame (offset 0) should be accepted
        assert!(matches!(
            handler.pre_validate(&evidence, 0),
            Ok(PreValidation::Proceed)
        ));

        // Second frame (offset 417) should be rejected as interior frame
        assert!(matches!(
            handler.pre_validate(&evidence, 417),
            Ok(PreValidation::Reject(_))
        ));

        // Third frame (offset 834) should also be rejected
        assert!(matches!(
            handler.pre_validate(&evidence, 834),
            Ok(PreValidation::Reject(_))
        ));
    }

    #[test]
    fn accepts_first_frame_of_stream() {
        let mut mp3_data = Vec::new();
        // Put some non-MP3 data first
        mp3_data.extend_from_slice(&[0x00; 500]);
        // Then valid MP3 frames
        for _ in 0..5 {
            mp3_data.extend_from_slice(&create_mp3_frame(9, 0, false));
        }

        let evidence = SliceEvidence { data: mp3_data };
        let handler = Mp3CarveHandler::new("mp3".to_string(), 0, 0);

        // Frame at offset 500 should be accepted (no preceding frame points to it)
        assert!(matches!(
            handler.pre_validate(&evidence, 500),
            Ok(PreValidation::Proceed)
        ));
    }

    #[test]
    fn id3_backed_not_affected_by_backward_check() {
        // Create ID3-backed MP3 with valid frames
        let mut mp3_data = create_id3v2_header(100);
        mp3_data.resize(110, 0x00); // ID3 tag data
        for _ in 0..5 {
            mp3_data.extend_from_slice(&create_mp3_frame(9, 0, false));
        }

        let evidence = SliceEvidence { data: mp3_data };
        let handler = Mp3CarveHandler::new("mp3".to_string(), 0, 0);

        // ID3-backed hit at offset 0 should always be accepted
        assert!(matches!(
            handler.pre_validate(&evidence, 0),
            Ok(PreValidation::Proceed)
        ));
    }

    #[test]
    fn accepts_frame_after_gap() {
        let mut data = Vec::new();
        // First: some valid MP3 frames
        for _ in 0..5 {
            data.extend_from_slice(&create_mp3_frame(9, 0, false));
        }
        // Gap: random non-frame data (not aligned to any frame boundary)
        data.extend_from_slice(&[0x42; 200]);
        // Second stream: more valid MP3 frames
        let second_stream_offset = data.len();
        for _ in 0..5 {
            data.extend_from_slice(&create_mp3_frame(9, 0, false));
        }

        let evidence = SliceEvidence { data };
        let handler = Mp3CarveHandler::new("mp3".to_string(), 0, 0);

        // Frame at second_stream_offset should be accepted (gap breaks the chain)
        assert!(matches!(
            handler.pre_validate(&evidence, second_stream_offset as u64),
            Ok(PreValidation::Proceed)
        ));
    }
}
