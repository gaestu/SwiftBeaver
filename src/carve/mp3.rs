//! MP3 (MPEG Audio Layer III) file carving handler.
//!
//! MP3 files can start with:
//! - ID3v2 tag header: "ID3" (0x49 0x44 0x33)
//! - MPEG audio frame sync: 0xFF 0xFB, 0xFF 0xFA, 0xFF 0xF3, 0xFF 0xF2
//!
//! Size detection walks MPEG audio frames until end of stream.

use std::fs::File;

use crate::carve::{
    CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext, output_path,
};
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

/// Sample rate table [version][index] in Hz
const SAMPLE_RATES: [[u32; 4]; 4] = [
    [11025, 12000, 8000, 0],  // MPEG 2.5
    [0, 0, 0, 0],             // Reserved
    [22050, 24000, 16000, 0], // MPEG 2
    [44100, 48000, 32000, 0], // MPEG 1
];

/// Samples per frame [version][layer]
const SAMPLES_PER_FRAME: [[u32; 4]; 4] = [
    [0, 576, 1152, 384],  // MPEG 2.5
    [0, 0, 0, 0],         // Reserved
    [0, 576, 1152, 384],  // MPEG 2
    [0, 1152, 1152, 384], // MPEG 1
];

/// Minimum number of consecutive valid frames required for sync-word based detection.
/// This helps reduce false positives from random 0xFFFB/0xFFFA bytes.
const MIN_FRAMES_FOR_SYNC_VALIDATION: u32 = 3;

pub struct Mp3CarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
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

    // Syncsafe integer: each byte's MSB is 0, so only 7 bits per byte
    let size = ((header[6] as u64 & 0x7F) << 21)
        | ((header[7] as u64 & 0x7F) << 14)
        | ((header[8] as u64 & 0x7F) << 7)
        | (header[9] as u64 & 0x7F);

    // Total = 10-byte header + tag data
    Some(10 + size)
}

/// Parse MPEG audio frame header and return frame size in bytes.
/// Frame header is 4 bytes with sync word 0xFFE or 0xFFF.
fn parse_frame_header(header: &[u8]) -> Option<u32> {
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

    Some(frame_size)
}

/// Check for ID3v1 tag at the given data (128 bytes starting with "TAG").
fn is_id3v1_tag(data: &[u8]) -> bool {
    data.len() >= 3 && &data[0..3] == b"TAG"
}

fn read_exact_at(ctx: &ExtractionContext, offset: u64, len: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let n = ctx.evidence.read_at(offset, &mut buf).ok()?;
    if n < len {
        return None;
    }
    Some(buf)
}

impl CarveHandler for Mp3CarveHandler {
    fn file_type(&self) -> &str {
        "mp3"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let file = File::create(&full_path)?;
        let mut stream = CarveStream::new(ctx.evidence, hit.global_offset, self.max_size, file);

        let mut validated = false;
        let mut truncated = false;
        let mut errors = Vec::new();
        // Track if we started with ID3 tag (more reliable) vs sync word (needs more validation)
        let mut started_with_id3 = false;

        let result: Result<u64, CarveError> = (|| {
            // Read initial header to check for ID3v2
            let header = stream.read_exact(10)?;

            let mut audio_start = 0u64;

            // Check for ID3v2 tag
            if let Some(id3_size) = parse_id3v2_size(&header) {
                // Skip rest of ID3v2 tag
                let remaining_id3 = id3_size.saturating_sub(10);
                if remaining_id3 > 0 {
                    stream.read_exact(remaining_id3 as usize)?;
                }
                audio_start = id3_size;
                started_with_id3 = true;
            } else {
                // No ID3v2, check if this starts with audio frame
                if header[0] != 0xFF || (header[1] & 0xE0) != 0xE0 {
                    return Err(CarveError::Invalid(
                        "mp3: no ID3v2 tag and no sync word".to_string(),
                    ));
                }
                // Re-parse as frame header (we already read 10 bytes, need to account for that)
            }

            // Now walk audio frames
            let mut total_size = audio_start.max(10); // We've already read at least 10 bytes
            let mut frame_count = 0u32;
            let max_frames = 100000; // Reasonable limit
            let max_size = if self.max_size > 0 {
                self.max_size
            } else {
                500 * 1024 * 1024
            };

            // If we didn't have ID3v2, we need to parse the first frame from what we read
            if audio_start == 0 {
                if let Some(frame_size) = parse_frame_header(&header[0..4]) {
                    // Read rest of first frame (we read 10 bytes, frame needs frame_size)
                    let remaining = frame_size.saturating_sub(10) as usize;
                    if remaining > 0 {
                        stream.read_exact(remaining)?;
                    }
                    total_size = frame_size as u64;
                    frame_count = 1;
                } else {
                    return Err(CarveError::Invalid(
                        "mp3: invalid first frame header".to_string(),
                    ));
                }
            }

            // Walk remaining frames (peek before writing to avoid trailing garbage)
            while frame_count < max_frames && total_size < max_size {
                let next_offset = hit.global_offset.saturating_add(total_size);
                let frame_header = match read_exact_at(ctx, next_offset, 4) {
                    Some(h) => h,
                    None => break,
                };

                // Check for ID3v1 tag at end
                if is_id3v1_tag(&frame_header) {
                    stream.read_exact(128)?;
                    total_size += 128;
                    break;
                }

                // Parse frame header
                if let Some(frame_size) = parse_frame_header(&frame_header) {
                    stream.read_exact(frame_size as usize)?;
                    total_size += frame_size as u64;
                    frame_count += 1;
                } else {
                    // Invalid frame header - stop without writing it
                    break;
                }
            }

            // Validation requirements:
            // - ID3v2 tag: valid even with 0 frames (metadata-only is rare but possible)
            // - Sync word only: require MIN_FRAMES_FOR_SYNC_VALIDATION consecutive valid frames
            //   to avoid false positives from random 0xFFFB/0xFFFA occurrences
            if started_with_id3 {
                validated = true;
            } else if frame_count >= MIN_FRAMES_FOR_SYNC_VALIDATION {
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
                    let _ = std::fs::remove_file(&full_path);
                    return Ok(None);
                }
                other => return Err(other),
            }
        }

        // If not validated (e.g., sync word with fewer than MIN_FRAMES), reject
        if !validated && !truncated {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

        let (size, md5_hex, sha256_hex) = stream.finish()?;

        // Check minimum size
        if size < self.min_size {
            let _ = std::fs::remove_file(&full_path);
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

        Ok(Some(CarvedFile {
            run_id: ctx.run_id.to_string(),
            file_type: self.file_type().to_string(),
            path: rel_path,
            extension: self.extension.clone(),
            global_start: hit.global_offset,
            global_end,
            size,
            md5: Some(md5_hex),
            sha256: Some(sha256_hex),
            validated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
        }))
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
        // MPEG1 Layer III frame header
        let mut header = vec![
            0xFF,
            0xFB, // Sync + MPEG1 Layer III
            (bitrate_idx << 4) | (sample_rate_idx << 2) | if padding { 2 } else { 0 },
            0x00, // Private, channel mode, etc.
        ];

        // Calculate frame size and add padding data
        if let Some(frame_size) = parse_frame_header(&header) {
            header.resize(frame_size as usize, 0x00);
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
        let size = parse_frame_header(&header).unwrap();
        assert_eq!(size, 417); // 144 * 128000 / 44100 = 417
    }

    #[test]
    fn carves_mp3_with_id3v2() {
        let mut mp3_data = create_id3v2_header(100);
        mp3_data.resize(110, 0x00); // ID3 tag data

        // Add a few frames
        for _ in 0..3 {
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
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        let carved = result.expect("carved file");

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
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        let carved = result.expect("carved file");

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
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
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
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

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
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        assert!(
            result.is_some(),
            "Should accept sync-word hit with {} valid frames",
            MIN_FRAMES_FOR_SYNC_VALIDATION
        );
    }
}
