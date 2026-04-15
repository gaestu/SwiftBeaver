//! WAV (Waveform Audio) file carving handler.
//!
//! WAV files use the RIFF container format with "WAVE" form type.
//! The file size is embedded in the RIFF header (bytes 4-7).

use std::fs::File;

use crate::carve::{
    CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext, output_path, riff,
};
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

/// Validate the fmt chunk data for reasonable audio parameters.
/// Returns true if the format parameters are valid.
fn validate_fmt_chunk(data: &[u8]) -> bool {
    if data.len() < 16 {
        return false;
    }
    let audio_format = u16::from_le_bytes([data[0], data[1]]);
    let num_channels = u16::from_le_bytes([data[2], data[3]]);
    let sample_rate = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let bits_per_sample = u16::from_le_bytes([data[14], data[15]]);

    // Only accept PCM and IEEE float formats for now
    if audio_format != WAV_AUDIO_FORMAT_PCM && audio_format != WAV_AUDIO_FORMAT_IEEE_FLOAT {
        return false;
    }
    if num_channels == 0 || num_channels > WAV_MAX_CHANNELS {
        return false;
    }
    if !(WAV_MIN_SAMPLE_RATE..=WAV_MAX_SAMPLE_RATE).contains(&sample_rate) {
        return false;
    }
    if !matches!(bits_per_sample, 8 | 16 | 24 | 32) {
        return false;
    }
    true
}

/// Search for the "fmt " subchunk within data and validate it.
/// Returns true if a valid fmt chunk is found.
fn find_and_validate_fmt_chunk(data: &[u8]) -> bool {
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
    false
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

            // Read enough data to find and validate the fmt chunk
            let peek_size = WAV_FMT_SEARCH_LIMIT.min((total_size as usize).saturating_sub(12));
            if peek_size > 0 {
                let peek_data = stream.peek_exact(peek_size)?;
                if !find_and_validate_fmt_chunk(&peek_data) {
                    return Err(CarveError::Invalid(
                        "wav fmt chunk missing or invalid".to_string(),
                    ));
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
                    let _ = std::fs::remove_file(&full_path);
                    return Ok(None);
                }
                other => return Err(other),
            }
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
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        let carved = result.expect("carved file");

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
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
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
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        let carved = result.expect("carved file");

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
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
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
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
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
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
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
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        assert!(result.is_none(), "should reject missing fmt chunk");
    }
}
