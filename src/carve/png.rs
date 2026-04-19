use crate::carve::{
    CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext, PendingCarve,
    PreValidation, output_path,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Check that all 4 bytes of a PNG chunk type are ASCII letters (a-z, A-Z).
fn is_valid_png_chunk_type(bytes: &[u8]) -> bool {
    bytes.len() == 4 && bytes.iter().all(|&b| b.is_ascii_alphabetic())
}

/// Validate bit_depth against color_type per the PNG specification.
fn is_valid_bit_depth(color_type: u8, bit_depth: u8) -> bool {
    match color_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        2 => matches!(bit_depth, 8 | 16),
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),
        4 => matches!(bit_depth, 8 | 16),
        6 => matches!(bit_depth, 8 | 16),
        _ => false,
    }
}

pub struct PngCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl PngCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for PngCarveHandler {
    fn file_type(&self) -> &str {
        "png"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        // Read 26 bytes: 8 (signature) + 4 (IHDR length) + 4 (IHDR tag)
        //   + 4 (width) + 4 (height) + 1 (bit_depth) + 1 (color_type)
        let mut buf = [0u8; 26];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }

        // Validate PNG signature
        if buf[..8] != PNG_SIGNATURE {
            return Ok(PreValidation::Reject("png signature mismatch".to_string()));
        }

        // IHDR chunk length must be exactly 13
        let ihdr_len = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
        if ihdr_len != 13 {
            return Ok(PreValidation::Reject(format!(
                "IHDR length {} != 13",
                ihdr_len
            )));
        }

        // First chunk must be IHDR
        if &buf[12..16] != b"IHDR" {
            return Ok(PreValidation::Reject("first chunk is not IHDR".to_string()));
        }

        // Width and height must be in [1, 2^31 - 1] per PNG spec
        let width = u32::from_be_bytes([buf[16], buf[17], buf[18], buf[19]]);
        let height = u32::from_be_bytes([buf[20], buf[21], buf[22], buf[23]]);
        if width == 0 || width > 0x7FFF_FFFF {
            return Ok(PreValidation::Reject(format!("invalid width {}", width)));
        }
        if height == 0 || height > 0x7FFF_FFFF {
            return Ok(PreValidation::Reject(format!("invalid height {}", height)));
        }

        // Validate bit depth and color type
        let bit_depth = buf[24];
        let color_type = buf[25];
        if !matches!(color_type, 0 | 2 | 3 | 4 | 6) {
            return Ok(PreValidation::Reject(format!(
                "invalid color_type {}",
                color_type
            )));
        }
        if !is_valid_bit_depth(color_type, bit_depth) {
            return Ok(PreValidation::Reject(format!(
                "invalid bit_depth {} for color_type {}",
                bit_depth, color_type
            )));
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

        let result: Result<(), CarveError> = (|| {
            let sig = stream.read_exact(PNG_SIGNATURE.len())?;
            if sig != PNG_SIGNATURE {
                return Err(CarveError::Invalid("png signature mismatch".to_string()));
            }

            let mut total_bytes: u64 = PNG_SIGNATURE.len() as u64;

            loop {
                let len_bytes = stream.read_exact(4)?;
                let len =
                    u32::from_be_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]);
                let typ_bytes = stream.read_exact(4)?;

                if !is_valid_png_chunk_type(&typ_bytes) {
                    return Err(CarveError::Truncated);
                }

                // 4 (length) + 4 (type) + data + 4 (CRC)
                let chunk_total = 4u64 + 4 + (len as u64) + 4;
                total_bytes = total_bytes.saturating_add(chunk_total);

                if self.max_size > 0 && (len as u64) > self.max_size {
                    return Err(CarveError::Truncated);
                }
                // Defense-in-depth: also enforced by CarveStream internally
                if self.max_size > 0 && total_bytes > self.max_size {
                    return Err(CarveError::Truncated);
                }

                if len > 0 {
                    stream.read_exact(len as usize)?;
                }
                stream.read_exact(4)?; // CRC

                if &typ_bytes == b"IEND" {
                    validated = true;
                    break;
                }
            }

            Ok(())
        })();

        if let Err(err) = result {
            match err {
                CarveError::Truncated | CarveError::Eof => {
                    truncated = true;
                    errors.push(err.to_string());
                }
                CarveError::Invalid(_msg) => {
                    stream.discard();
                    return Ok(None);
                }
                other => return Err(other),
            }
        }

        let (size, md5_hex, sha256_hex, mut writer) = stream.finalize()?;
        if size < self.min_size {
            writer.discard();
            return Ok(None);
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
