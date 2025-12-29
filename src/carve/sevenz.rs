use std::fs::File;
use std::io::Write;

use sha2::{Digest, Sha256};

use crate::carve::{output_path, write_range, CarveError, CarveHandler, CarvedFile, ExtractionContext};
use crate::scanner::NormalizedHit;

const SEVENZ_MAGIC: [u8; 6] = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];
const SEVENZ_HEADER_LEN: usize = 32;

pub struct SevenZCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl SevenZCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for SevenZCarveHandler {
    fn file_type(&self) -> &str {
        "7z"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let mut header = [0u8; SEVENZ_HEADER_LEN];
        let n = ctx
            .evidence
            .read_at(hit.global_offset, &mut header)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < SEVENZ_HEADER_LEN {
            return Ok(None);
        }
        if header[..SEVENZ_MAGIC.len()] != SEVENZ_MAGIC {
            return Ok(None);
        }

        let next_header_offset = u64::from_le_bytes([
            header[12], header[13], header[14], header[15], header[16], header[17], header[18],
            header[19],
        ]);
        let next_header_size = u64::from_le_bytes([
            header[20], header[21], header[22], header[23], header[24], header[25], header[26],
            header[27],
        ]);

        let mut total_size = (SEVENZ_HEADER_LEN as u64)
            .saturating_add(next_header_offset)
            .saturating_add(next_header_size);
        if total_size < SEVENZ_HEADER_LEN as u64 {
            return Ok(None);
        }

        let mut truncated = false;
        let mut errors = Vec::new();
        if self.max_size > 0 && total_size > self.max_size {
            total_size = self.max_size;
            truncated = true;
            errors.push("max_size reached before 7z end".to_string());
        }

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut file = File::create(&full_path)?;
        let mut md5 = md5::Context::new();
        let mut sha256 = Sha256::new();

        let total_end = hit.global_offset + total_size;
        let (written, eof_truncated) =
            write_range(ctx, hit.global_offset, total_end, &mut file, &mut md5, &mut sha256)?;
        if eof_truncated {
            truncated = true;
            errors.push("eof before 7z end".to_string());
        }
        file.flush()?;

        if written < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

        let md5_hex = format!("{:x}", md5.compute());
        let sha256_hex = hex::encode(sha256.finalize());
        let global_end = if written == 0 {
            hit.global_offset
        } else {
            hit.global_offset + written - 1
        };

        Ok(Some(CarvedFile {
            run_id: ctx.run_id.to_string(),
            file_type: self.file_type().to_string(),
            path: rel_path,
            extension: self.extension.clone(),
            global_start: hit.global_offset,
            global_end,
            size: written,
            md5: Some(md5_hex),
            sha256: Some(sha256_hex),
            validated: !truncated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::SevenZCarveHandler;
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::RawFileSource;
    use crate::scanner::NormalizedHit;

    #[test]
    fn carves_minimal_7z() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let mut sevenz = Vec::new();
        sevenz.extend_from_slice(&super::SEVENZ_MAGIC);
        sevenz.extend_from_slice(&[0u8, 4u8]);
        sevenz.extend_from_slice(&[0u8; 4]);
        sevenz.extend_from_slice(&0u64.to_le_bytes());
        sevenz.extend_from_slice(&0u64.to_le_bytes());
        sevenz.extend_from_slice(&[0u8; 4]);

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &sevenz).expect("write 7z");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
        };
        let handler = SevenZCarveHandler::new("7z".to_string(), 8, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "7z".to_string(),
            pattern_id: "7z_header".to_string(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = carved.expect("carved");
        assert!(carved.validated);
        assert_eq!(carved.size, sevenz.len() as u64);
    }
}
