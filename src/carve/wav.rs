//! WAV (Waveform Audio) file carving handler.
//!
//! WAV files use the RIFF container format with "WAVE" form type.
//! The file size is embedded in the RIFF header (bytes 4-7).

use crate::carve::{
    CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext, PendingCarve,
    PreValidation, output_path, riff,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

/// PCM audio format
const WAV_AUDIO_FORMAT_PCM: u16 = 0x0001;
/// IEEE float audio format
const WAV_AUDIO_FORMAT_IEEE_FLOAT: u16 = 0x0003;
/// Minimum reasonable sample rate (8kHz)
const WAV_MIN_SAMPLE_RATE: u32 = 8000;
/// Maximum reasonable sample rate (192kHz)
const WAV_MAX_SAMPLE_RATE: u32 = 192000;
/// Maximum reasonable number of channels
const WAV_MAX_CHANNELS: u16 = 8;
/// Maximum bytes to search for fmt chunk
const WAV_FMT_SEARCH_LIMIT: usize = 65536;
/// Maximum plausible WAV duration in seconds (3 hours)
const WAV_MAX_DURATION_SECS: u64 = 10800;
/// RIFF chunk_size values at or above this are treated as corrupt/sentinel (near u32::MAX)
const WAV_RIFF_SIZE_SUSPECT_THRESHOLD: u32 = 0xFFFF_FF00;

struct WavFmtParams {
    byte_rate: u32,
}

/// Validate the fmt chunk data for reasonable audio parameters.
/// Returns parsed parameters on success.
fn validate_fmt_chunk(data: &[u8]) -> Option<WavFmtParams> {
    if data.len() < 16 {
        return None;
    }
    let audio_format = u16::from_le_bytes([data[0], data[1]]);
    let num_channels = u16::from_le_bytes([data[2], data[3]]);
    let sample_rate = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let byte_rate = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
    let bits_per_sample = u16::from_le_bytes([data[14], data[15]]);

    // Only accept PCM and IEEE float formats for now
    if audio_format != WAV_AUDIO_FORMAT_PCM && audio_format != WAV_AUDIO_FORMAT_IEEE_FLOAT {
        return None;
    }
    if num_channels == 0 || num_channels > WAV_MAX_CHANNELS {
        return None;
    }
    if !(WAV_MIN_SAMPLE_RATE..=WAV_MAX_SAMPLE_RATE).contains(&sample_rate) {
        return None;
    }
    if !matches!(bits_per_sample, 8 | 16 | 24 | 32) {
        return None;
    }
    if byte_rate == 0 {
        return None;
    }
    Some(WavFmtParams { byte_rate })
}

/// Find the "data" subchunk and return (offset_of_data_marker, data_chunk_size).
fn find_data_chunk(data: &[u8]) -> Option<(usize, u32)> {
    const DATA_MARKER: &[u8; 4] = b"data";
    for i in 0..data.len().saturating_sub(8) {
        if &data[i..i + 4] == DATA_MARKER {
            let chunk_size =
                u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]);
            return Some((i, chunk_size));
        }
    }
    None
}

/// Search for the "fmt " subchunk within data and validate it.
/// Returns parsed parameters if a valid fmt chunk is found.
fn find_and_validate_fmt_chunk(data: &[u8]) -> Option<WavFmtParams> {
    // fmt chunk starts with "fmt " followed by 4-byte size
    const FMT_MARKER: &[u8; 4] = b"fmt ";

    for i in 0..data.len().saturating_sub(8) {
        if &data[i..i + 4] == FMT_MARKER {
            let chunk_size =
                u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]) as usize;
            let chunk_data_start = i + 8;
            if chunk_data_start + 16 <= data.len() && chunk_size >= 16 {
                return validate_fmt_chunk(&data[chunk_data_start..]);
            }
        }
    }
    None
}

pub struct WavCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl WavCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for WavCarveHandler {
    fn file_type(&self) -> &str {
        "wav"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 12];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        if &buf[0..4] != b"RIFF" {
            return Ok(PreValidation::Reject("wav RIFF magic mismatch".to_string()));
        }
        if &buf[8..12] != b"WAVE" {
            return Ok(PreValidation::Reject(
                "wav WAVE marker mismatch".to_string(),
            ));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
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
            // Read RIFF header (12 bytes)
            let header = stream.read_exact(12)?;

            // Parse and validate RIFF structure
            let (form_type, total_size) = riff::parse_riff_header(&header)?;

            // Verify this is a WAVE file
            if &form_type != riff::WAVE_FORM {
                return Err(CarveError::Invalid(format!(
                    "wav form type mismatch: expected WAVE, got {:?}",
                    String::from_utf8_lossy(&form_type)
                )));
            }

            // Sanity check on size
            if total_size < 12 {
                return Err(CarveError::Invalid("wav size too small".to_string()));
            }

            // Reject near-max RIFF sizes (sentinel/placeholder values)
            let riff_chunk_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
            if riff_chunk_size >= WAV_RIFF_SIZE_SUSPECT_THRESHOLD {
                return Err(CarveError::Invalid(
                    "wav riff size near u32::MAX (likely corrupt)".to_string(),
                ));
            }

            // Read enough data to find and validate the fmt chunk
            let peek_size = WAV_FMT_SEARCH_LIMIT.min((total_size as usize).saturating_sub(12));
            if peek_size > 0 {
                let peek_data = stream.peek_exact(peek_size)?;
                let fmt_params = match find_and_validate_fmt_chunk(&peek_data) {
                    Some(params) => params,
                    None => {
                        return Err(CarveError::Invalid(
                            "wav fmt chunk missing or invalid".to_string(),
                        ));
                    }
                };

                // Check data subchunk consistency
                if let Some((_data_offset, data_chunk_size)) = find_data_chunk(&peek_data) {
                    if (data_chunk_size as u64) > total_size {
                        return Err(CarveError::Invalid(
                            "wav data subchunk size exceeds riff container".to_string(),
                        ));
                    }

                    // Duration plausibility
                    if fmt_params.byte_rate > 0 {
                        let duration_secs = data_chunk_size as u64 / fmt_params.byte_rate as u64;
                        if duration_secs > WAV_MAX_DURATION_SECS {
                            return Err(CarveError::Invalid(format!(
                                "wav implied duration {}s exceeds maximum {}s",
                                duration_secs, WAV_MAX_DURATION_SECS,
                            )));
                        }
                    }
                }
            } else {
                return Err(CarveError::Invalid(
                    "wav too small for fmt chunk".to_string(),
                ));
            }

            // Apply max_size limit
            let max_size = if self.max_size > 0 {
                self.max_size
            } else {
                total_size
            };
            let target_size = total_size.min(max_size);

            // Read remaining data
            let remaining = target_size.saturating_sub(12);
            if remaining > 0 {
                stream.read_exact(remaining as usize)?;
            }

            validated = true;
            Ok(target_size)
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
    use std::fs::File;
    use std::io::Read;
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

    fn create_minimal_wav() -> Vec<u8> {
        let mut wav = Vec::new();

        // RIFF header
        wav.extend_from_slice(b"RIFF");
        // Chunk size: 36 bytes (total 44 bytes - 8 for RIFF header)
        wav.extend_from_slice(&36u32.to_le_bytes());
        wav.extend_from_slice(b"WAVE");

        // fmt subchunk
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // Subchunk size
        wav.extend_from_slice(&1u16.to_le_bytes()); // Audio format (PCM)
        wav.extend_from_slice(&1u16.to_le_bytes()); // Num channels
        wav.extend_from_slice(&44100u32.to_le_bytes()); // Sample rate
        wav.extend_from_slice(&88200u32.to_le_bytes()); // Byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // Block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // Bits per sample

        // data subchunk (empty)
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&0u32.to_le_bytes()); // Data size

        wav
    }

    #[test]
    fn carves_minimal_wav() {
        let wav_data = create_minimal_wav();
        let evidence = SliceEvidence {
            data: wav_data.clone(),
        };
        let handler = WavCarveHandler::new("wav".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "wav".to_string(),
            pattern_id: "wav_riff".to_string(),
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

        assert_eq!(carved.file_type, "wav");
        assert_eq!(carved.size, wav_data.len() as u64);
        assert!(carved.validated);
        assert!(!carved.truncated);

        // Verify file contents
        let mut file = File::open(dir.path().join(&carved.path)).expect("open");
        let mut contents = Vec::new();
        file.read_to_end(&mut contents).expect("read");
        assert_eq!(contents, wav_data);
    }

    #[test]
    fn rejects_non_wav_riff() {
        // Create a RIFF file with different form type (like AVI)
        let mut data = Vec::new();
        data.extend_from_slice(b"RIFF");
        data.extend_from_slice(&100u32.to_le_bytes());
        data.extend_from_slice(b"AVI "); // Not WAVE
        data.extend_from_slice(&[0u8; 100]);

        let evidence = SliceEvidence { data };
        let handler = WavCarveHandler::new("wav".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "wav".to_string(),
            pattern_id: "wav_riff".to_string(),
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
        assert!(result.is_none(), "should reject non-WAV RIFF");
    }

    #[test]
    fn respects_max_size() {
        let wav_data = create_minimal_wav();
        let evidence = SliceEvidence {
            data: wav_data.clone(),
        };
        let handler = WavCarveHandler::new("wav".to_string(), 0, 20); // Max 20 bytes
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "wav".to_string(),
            pattern_id: "wav_riff".to_string(),
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

        assert!(carved.truncated);
        assert!(carved.size <= 20);
    }

    #[test]
    fn respects_min_size() {
        let wav_data = create_minimal_wav();
        let evidence = SliceEvidence {
            data: wav_data.clone(),
        };
        let handler = WavCarveHandler::new("wav".to_string(), 1000, 0); // Min 1000 bytes
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "wav".to_string(),
            pattern_id: "wav_riff".to_string(),
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
        assert!(result.is_none(), "should reject file below min_size");
    }

    #[test]
    fn rejects_invalid_sample_rate() {
        // Create WAV with invalid sample rate (too low)
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&36u32.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // 1 channel
        wav.extend_from_slice(&100u32.to_le_bytes()); // Invalid sample rate (too low, < 8000)
        wav.extend_from_slice(&200u32.to_le_bytes()); // Byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // Block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // Bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&0u32.to_le_bytes());

        let evidence = SliceEvidence { data: wav };
        let handler = WavCarveHandler::new("wav".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "wav".to_string(),
            pattern_id: "wav_riff".to_string(),
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
        assert!(result.is_none(), "should reject invalid sample rate");
    }

    #[test]
    fn rejects_invalid_channels() {
        // Create WAV with too many channels
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&36u32.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&100u16.to_le_bytes()); // Invalid: 100 channels (> 8)
        wav.extend_from_slice(&44100u32.to_le_bytes()); // Sample rate
        wav.extend_from_slice(&88200u32.to_le_bytes()); // Byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // Block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // Bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&0u32.to_le_bytes());

        let evidence = SliceEvidence { data: wav };
        let handler = WavCarveHandler::new("wav".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "wav".to_string(),
            pattern_id: "wav_riff".to_string(),
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
        assert!(result.is_none(), "should reject too many channels");
    }

    #[test]
    fn rejects_missing_fmt_chunk() {
        // Create WAV without fmt chunk
        // RIFF size = total file size - 8
        // We have: "RIFF"(4) + size(4) + "WAVE"(4) + "data"(4) + size(4) + data(20) = 40 bytes
        // So RIFF size = 40 - 8 = 32
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&32u32.to_le_bytes()); // Correct size field
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"data"); // Missing fmt chunk
        wav.extend_from_slice(&20u32.to_le_bytes());
        wav.extend_from_slice(&[0u8; 20]);

        let evidence = SliceEvidence { data: wav };
        let handler = WavCarveHandler::new("wav".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "wav".to_string(),
            pattern_id: "wav_riff".to_string(),
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
        assert!(result.is_none(), "should reject missing fmt chunk");
    }

    #[test]
    fn rejects_near_max_riff_size() {
        // RIFF with chunk_size = 0xFFFFFFF8 (near u32::MAX)
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&0xFFFFFFF8u32.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        // Valid fmt chunk
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&2u16.to_le_bytes()); // Stereo
        wav.extend_from_slice(&44100u32.to_le_bytes());
        wav.extend_from_slice(&176400u32.to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&0xFFFFFFD4u32.to_le_bytes());
        wav.extend_from_slice(&[0u8; 1024]);

        let evidence = SliceEvidence { data: wav };
        let handler = WavCarveHandler::new("wav".to_string(), 0, 524_288_000);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "wav".to_string(),
            pattern_id: "wav_riff".to_string(),
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
        assert!(result.is_none(), "should reject near-max RIFF size");
    }

    #[test]
    fn rejects_inconsistent_data_chunk() {
        // WAV where data subchunk size exceeds RIFF container size
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&100u32.to_le_bytes()); // Small RIFF size
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // Mono
        wav.extend_from_slice(&44100u32.to_le_bytes());
        wav.extend_from_slice(&88200u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&500000u32.to_le_bytes()); // data claims 500KB > 108 bytes total
        wav.extend_from_slice(&[0u8; 200]);

        let evidence = SliceEvidence { data: wav };
        let handler = WavCarveHandler::new("wav".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "wav".to_string(),
            pattern_id: "wav_riff".to_string(),
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
            result.is_none(),
            "should reject data chunk larger than RIFF container"
        );
    }

    #[test]
    fn rejects_implausible_duration() {
        // WAV with valid params but data claims > 3 hours of audio
        // 44100 Hz * 2 channels * 2 bytes = 176400 bytes/sec
        // 3 hours = 10800 sec → 10800 * 176400 = 1,905,120,000 bytes
        // Use a data size just over the limit
        let data_size: u32 = 1_910_000_000;
        let riff_size: u32 = data_size + 36; // 36 bytes for fmt + data headers
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&riff_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&2u16.to_le_bytes()); // Stereo
        wav.extend_from_slice(&44100u32.to_le_bytes());
        wav.extend_from_slice(&176400u32.to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.extend_from_slice(&[0u8; 65536]);

        let evidence = SliceEvidence { data: wav };
        let handler = WavCarveHandler::new("wav".to_string(), 0, 2_000_000_000);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "wav".to_string(),
            pattern_id: "wav_riff".to_string(),
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
        assert!(result.is_none(), "should reject implausible duration (>3h)");
    }

    #[test]
    fn accepts_valid_long_wav_under_limit() {
        // WAV with ~2 hours of audio (under 3h limit)
        // 8000 Hz * 1 channel * 1 byte = 8000 bytes/sec (8-bit mono, smallest byte rate)
        // 2 hours = 7200 sec → 7200 * 8000 = 57,600,000 bytes (~55 MB)
        let data_size: u32 = 57_600_000;
        let riff_size: u32 = data_size + 36;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&riff_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // Mono
        wav.extend_from_slice(&8000u32.to_le_bytes()); // 8kHz
        wav.extend_from_slice(&8000u32.to_le_bytes()); // Byte rate
        wav.extend_from_slice(&1u16.to_le_bytes()); // Block align
        wav.extend_from_slice(&8u16.to_le_bytes()); // 8-bit
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        // We only need enough data for the peek; the rest will cause Eof/Truncated which is fine
        wav.extend_from_slice(&[0x80u8; 256]);

        let evidence = SliceEvidence { data: wav };
        // Use a large max_size so it doesn't interfere
        let handler = WavCarveHandler::new("wav".to_string(), 0, 100_000_000);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "wav".to_string(),
            pattern_id: "wav_riff".to_string(),
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
        // Should be accepted (truncated because evidence is smaller than claimed, but not rejected)
        let carved = result
            .expect("should accept valid 2-hour WAV")
            .flush()
            .expect("flush");
        assert!(carved.truncated, "should be truncated (evidence too small)");
    }
}
