//! AVI (Audio Video Interleave) file carving handler.
//!
//! AVI files use the RIFF container format with "AVI " form type.
//! The file size is embedded in the RIFF header (bytes 4-7).

use std::fs::File;

use crate::carve::{
    output_path, riff, CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext,
};
use crate::scanner::NormalizedHit;

pub struct AviCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl AviCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for AviCarveHandler {
    fn file_type(&self) -> &str {
        "avi"
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

            // Verify this is an AVI file
            if &form_type != riff::AVI_FORM {
                return Err(CarveError::Invalid(format!(
                    "avi form type mismatch: expected AVI, got {:?}",
                    String::from_utf8_lossy(&form_type)
                )));
            }

            // Sanity check on size
            if total_size < 12 {
                return Err(CarveError::Invalid("avi size too small".to_string()));
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

    fn create_minimal_avi() -> Vec<u8> {
        let mut avi = Vec::new();

        // RIFF header
        avi.extend_from_slice(b"RIFF");
        // We'll set the size after building the content
        let size_pos = avi.len();
        avi.extend_from_slice(&0u32.to_le_bytes()); // Placeholder
        avi.extend_from_slice(b"AVI ");

        // hdrl LIST chunk (header list)
        avi.extend_from_slice(b"LIST");
        let hdrl_size_pos = avi.len();
        avi.extend_from_slice(&0u32.to_le_bytes()); // Placeholder
        let hdrl_start = avi.len();
        avi.extend_from_slice(b"hdrl");

        // avih chunk (main AVI header)
        avi.extend_from_slice(b"avih");
        avi.extend_from_slice(&56u32.to_le_bytes()); // Size of avih data

        // AVI main header structure (56 bytes)
        avi.extend_from_slice(&33333u32.to_le_bytes()); // dwMicroSecPerFrame
        avi.extend_from_slice(&0u32.to_le_bytes()); // dwMaxBytesPerSec
        avi.extend_from_slice(&0u32.to_le_bytes()); // dwPaddingGranularity
        avi.extend_from_slice(&0u32.to_le_bytes()); // dwFlags
        avi.extend_from_slice(&1u32.to_le_bytes()); // dwTotalFrames
        avi.extend_from_slice(&0u32.to_le_bytes()); // dwInitialFrames
        avi.extend_from_slice(&1u32.to_le_bytes()); // dwStreams
        avi.extend_from_slice(&0u32.to_le_bytes()); // dwSuggestedBufferSize
        avi.extend_from_slice(&320u32.to_le_bytes()); // dwWidth
        avi.extend_from_slice(&240u32.to_le_bytes()); // dwHeight
        avi.extend_from_slice(&[0u8; 16]); // dwReserved[4]

        let hdrl_size = (avi.len() - hdrl_start) as u32;

        // movi LIST chunk (movie data)
        avi.extend_from_slice(b"LIST");
        avi.extend_from_slice(&4u32.to_le_bytes()); // Size (just the type)
        avi.extend_from_slice(b"movi");

        // Update sizes
        let total_size = (avi.len() - 8) as u32;
        avi[size_pos..size_pos + 4].copy_from_slice(&total_size.to_le_bytes());
        avi[hdrl_size_pos..hdrl_size_pos + 4].copy_from_slice(&hdrl_size.to_le_bytes());

        avi
    }

    #[test]
    fn carves_minimal_avi() {
        let avi_data = create_minimal_avi();
        let evidence = SliceEvidence {
            data: avi_data.clone(),
        };
        let handler = AviCarveHandler::new("avi".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "avi".to_string(),
            pattern_id: "avi_riff".to_string(),
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        let carved = result.expect("carved file");

        assert_eq!(carved.file_type, "avi");
        assert_eq!(carved.size, avi_data.len() as u64);
        assert!(carved.validated);
        assert!(!carved.truncated);

        // Verify file contents
        let mut file = File::open(dir.path().join(&carved.path)).expect("open");
        let mut contents = Vec::new();
        file.read_to_end(&mut contents).expect("read");
        assert_eq!(contents, avi_data);
    }

    #[test]
    fn rejects_non_avi_riff() {
        // Create a RIFF file with different form type (like WAVE)
        let mut data = Vec::new();
        data.extend_from_slice(b"RIFF");
        data.extend_from_slice(&100u32.to_le_bytes());
        data.extend_from_slice(b"WAVE"); // Not AVI
        data.extend_from_slice(&vec![0u8; 100]);

        let evidence = SliceEvidence { data };
        let handler = AviCarveHandler::new("avi".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "avi".to_string(),
            pattern_id: "avi_riff".to_string(),
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        assert!(result.is_none(), "should reject non-AVI RIFF");
    }

    #[test]
    fn respects_max_size() {
        let avi_data = create_minimal_avi();
        let evidence = SliceEvidence {
            data: avi_data.clone(),
        };
        let handler = AviCarveHandler::new("avi".to_string(), 0, 20); // Max 20 bytes
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "avi".to_string(),
            pattern_id: "avi_riff".to_string(),
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
        let avi_data = create_minimal_avi();
        let evidence = SliceEvidence {
            data: avi_data.clone(),
        };
        let handler = AviCarveHandler::new("avi".to_string(), 10000, 0); // Min 10000 bytes
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "avi".to_string(),
            pattern_id: "avi_riff".to_string(),
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
}
