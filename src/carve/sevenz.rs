use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PreValidation, output_path,
    write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const SEVENZ_MAGIC: [u8; 6] = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];
const SEVENZ_HEADER_LEN: usize = 32;

/// Maximum reasonable offset to the next header (1GB)
const MAX_NEXT_HEADER_OFFSET: u64 = 1024 * 1024 * 1024;
/// Maximum reasonable size for header structures (64MB)
const MAX_NEXT_HEADER_SIZE: u64 = 64 * 1024 * 1024;

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

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 6];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        if buf != SEVENZ_MAGIC {
            return Ok(PreValidation::Reject("7z magic mismatch".to_string()));
        }
        Ok(PreValidation::Proceed)
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

        // Validate CRC32 of bytes 12-31 against stored CRC in bytes 8-11
        let stored_crc = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
        let computed_crc = crc32fast::hash(&header[12..32]);
        if stored_crc != computed_crc {
            return Ok(None); // Invalid header CRC - reject
        }

        let next_header_offset = u64::from_le_bytes([
            header[12], header[13], header[14], header[15], header[16], header[17], header[18],
            header[19],
        ]);
        let next_header_size = u64::from_le_bytes([
            header[20], header[21], header[22], header[23], header[24], header[25], header[26],
            header[27],
        ]);

        // Sanity checks on size fields to reject unreasonable values
        if next_header_offset > MAX_NEXT_HEADER_OFFSET {
            return Ok(None);
        }
        if next_header_size > MAX_NEXT_HEADER_SIZE {
            return Ok(None);
        }

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
        let mut md5 = md5::Context::new();
        let mut sha256 = Sha256::new();

        let total_end = hit.global_offset + total_size;
        let (written, eof_truncated) = write_range(
            ctx,
            hit.global_offset,
            total_end,
            &full_path,
            &mut md5,
            &mut sha256,
        )?;
        if eof_truncated {
            truncated = true;
            errors.push("eof before 7z end".to_string());
        }

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

        // Build header bytes 12-31 first (values to be CRCed)
        let next_header_offset: u64 = 0;
        let next_header_size: u64 = 0;
        let next_header_crc: u32 = 0;
        let mut crc_region = Vec::new();
        crc_region.extend_from_slice(&next_header_offset.to_le_bytes());
        crc_region.extend_from_slice(&next_header_size.to_le_bytes());
        crc_region.extend_from_slice(&next_header_crc.to_le_bytes());
        // Compute CRC of bytes 12-31
        let header_crc = crc32fast::hash(&crc_region);

        let mut sevenz = Vec::new();
        sevenz.extend_from_slice(&super::SEVENZ_MAGIC); // bytes 0-5
        sevenz.extend_from_slice(&[0u8, 4u8]); // bytes 6-7 (version)
        sevenz.extend_from_slice(&header_crc.to_le_bytes()); // bytes 8-11 (CRC of 12-31)
        sevenz.extend_from_slice(&crc_region); // bytes 12-31

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &sevenz).expect("write 7z");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
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

    #[test]
    fn rejects_invalid_crc() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let mut sevenz = Vec::new();
        sevenz.extend_from_slice(&super::SEVENZ_MAGIC); // bytes 0-5
        sevenz.extend_from_slice(&[0u8, 4u8]); // bytes 6-7 (version)
        sevenz.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]); // bytes 8-11 (bad CRC)
        sevenz.extend_from_slice(&[0u8; 20]); // bytes 12-31 (zeros)

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &sevenz).expect("write 7z");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
        };
        let handler = SevenZCarveHandler::new("7z".to_string(), 8, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "7z".to_string(),
            pattern_id: "7z_header".to_string(),
        };

        // Should reject due to invalid CRC
        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        assert!(carved.is_none(), "should reject invalid CRC");
    }

    #[test]
    fn rejects_excessive_offset() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        // Create header with excessive next_header_offset (> 1GB)
        let next_header_offset: u64 = 2 * 1024 * 1024 * 1024; // 2GB
        let next_header_size: u64 = 0;
        let next_header_crc: u32 = 0;
        let mut crc_region = Vec::new();
        crc_region.extend_from_slice(&next_header_offset.to_le_bytes());
        crc_region.extend_from_slice(&next_header_size.to_le_bytes());
        crc_region.extend_from_slice(&next_header_crc.to_le_bytes());
        let header_crc = crc32fast::hash(&crc_region);

        let mut sevenz = Vec::new();
        sevenz.extend_from_slice(&super::SEVENZ_MAGIC);
        sevenz.extend_from_slice(&[0u8, 4u8]);
        sevenz.extend_from_slice(&header_crc.to_le_bytes());
        sevenz.extend_from_slice(&crc_region);

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &sevenz).expect("write 7z");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
        };
        let handler = SevenZCarveHandler::new("7z".to_string(), 8, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "7z".to_string(),
            pattern_id: "7z_header".to_string(),
        };

        // Should reject due to excessive offset
        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        assert!(carved.is_none(), "should reject excessive offset");
    }
}
