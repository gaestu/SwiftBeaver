use std::fs::File;

use crate::carve::{
    output_path, CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext,
};
use crate::scanner::NormalizedHit;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

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

        let result: Result<(), CarveError> = (|| {
            let sig = stream.read_exact(PNG_SIGNATURE.len())?;
            if sig != PNG_SIGNATURE {
                return Err(CarveError::Invalid("png signature mismatch".to_string()));
            }

            loop {
                let len_bytes = stream.read_exact(4)?;
                let len =
                    u32::from_be_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]);
                let typ_bytes = stream.read_exact(4)?;
                let chunk_type = std::str::from_utf8(&typ_bytes)
                    .map_err(|_| CarveError::Invalid("png chunk type invalid".to_string()))?
                    .to_string();
                if len > (self.max_size as u32) && self.max_size > 0 {
                    return Err(CarveError::Truncated);
                }
                if len > 0 {
                    stream.read_exact(len as usize)?;
                }
                stream.read_exact(4)?; // CRC

                if chunk_type == "IEND" {
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
