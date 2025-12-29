use std::fs::File;
use std::io::Write;

use sha2::{Digest, Sha256};

use crate::carve::{output_path, write_range, CarveError, CarveHandler, CarvedFile, ExtractionContext};
use crate::scanner::NormalizedHit;

const BMP_HEADER_LEN: usize = 14;
const BMP_MAGIC: [u8; 2] = [0x42, 0x4D];

pub struct BmpCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl BmpCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for BmpCarveHandler {
    fn file_type(&self) -> &str {
        "bmp"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let mut header = [0u8; BMP_HEADER_LEN];
        let n = ctx
            .evidence
            .read_at(hit.global_offset, &mut header)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < BMP_HEADER_LEN {
            return Ok(None);
        }
        if header[0..2] != BMP_MAGIC {
            return Ok(None);
        }

        let file_size = u32::from_le_bytes([header[2], header[3], header[4], header[5]]) as u64;
        let pixel_offset = u32::from_le_bytes([header[10], header[11], header[12], header[13]]) as u64;

        if file_size < BMP_HEADER_LEN as u64 || pixel_offset < BMP_HEADER_LEN as u64 || pixel_offset > file_size
        {
            return Ok(None);
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

        let mut total_end = hit.global_offset + file_size;
        let mut truncated = false;
        let mut errors = Vec::new();

        if self.max_size > 0 && file_size > self.max_size {
            total_end = hit.global_offset + self.max_size;
            truncated = true;
            errors.push("max_size reached before BMP end".to_string());
        }

        let (written, eof_truncated) =
            write_range(ctx, hit.global_offset, total_end, &mut file, &mut md5, &mut sha256)?;
        if eof_truncated {
            truncated = true;
            errors.push("eof before BMP end".to_string());
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
    use super::BmpCarveHandler;
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::scanner::NormalizedHit;
    use crate::evidence::RawFileSource;

    #[test]
    fn carves_minimal_bmp() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let mut bmp = Vec::new();
        let file_size = 58u32;
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&file_size.to_le_bytes());
        bmp.extend_from_slice(&0u16.to_le_bytes());
        bmp.extend_from_slice(&0u16.to_le_bytes());
        bmp.extend_from_slice(&(54u32).to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes()); // DIB header size
        bmp.extend_from_slice(&[0u8; 36]);
        bmp.extend_from_slice(&[0u8; 4]);

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &bmp).expect("write bmp");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
        };
        let handler = BmpCarveHandler::new("bmp".to_string(), 10, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "bmp".to_string(),
            pattern_id: "bmp_header".to_string(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = carved.expect("carved");
        assert!(carved.validated);
        assert_eq!(carved.size, file_size as u64);
    }
}
