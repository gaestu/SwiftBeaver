use std::fs::File;

use crate::carve::{
    output_path, CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext,
};
use crate::scanner::NormalizedHit;

const RIFF: &[u8; 4] = b"RIFF";
const WEBP: &[u8; 4] = b"WEBP";

pub struct WebpCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl WebpCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for WebpCarveHandler {
    fn file_type(&self) -> &str {
        "webp"
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
            let header = stream.read_exact(12)?;
            if &header[0..4] != RIFF || &header[8..12] != WEBP {
                return Err(CarveError::Invalid("webp header mismatch".to_string()));
            }
            let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as u64;
            let total_size = size.saturating_add(8);
            if total_size < 12 {
                return Err(CarveError::Invalid("webp size invalid".to_string()));
            }
            let max_size = if self.max_size > 0 {
                self.max_size
            } else {
                total_size
            };
            let target_size = total_size.min(max_size);
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
                CarveError::Invalid(_msg) => {
                    let _ = std::fs::remove_file(&full_path);
                    return Ok(None);
                }
                other => return Err(other),
            }
        }

        let (size, md5_hex, sha256_hex) = stream.finish()?;
        if size < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

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
