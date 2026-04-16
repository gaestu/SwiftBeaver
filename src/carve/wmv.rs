//! WMV/ASF carving handler.
//!
//! Uses ASF header and file properties to determine file size.

use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PreValidation, output_path,
    write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const ASF_HEADER_GUID: [u8; 16] = [
    0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C,
];
const ASF_FILE_PROP_GUID: [u8; 16] = [
    0xA1, 0xDC, 0xAB, 0x8C, 0x47, 0xA9, 0xCF, 0x11, 0x8E, 0xE4, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65,
];

pub struct WmvCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl WmvCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for WmvCarveHandler {
    fn file_type(&self) -> &str {
        "wmv"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 16];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        if buf != ASF_HEADER_GUID {
            return Ok(PreValidation::Reject(
                "asf header GUID mismatch".to_string(),
            ));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let evidence_len = ctx.evidence.len();
        if hit.global_offset >= evidence_len {
            return Ok(None);
        }
        let remaining = evidence_len.saturating_sub(hit.global_offset);
        if remaining < 30 {
            return Ok(None);
        }

        let header = read_exact_at(ctx, hit.global_offset, 30)
            .ok_or_else(|| CarveError::Invalid("asf header too short".to_string()))?;
        if header[0..16] != ASF_HEADER_GUID {
            return Ok(None);
        }

        // Reserved bytes are fixed in ASF header object and useful to reject random GUID matches.
        if header[28] != 0x01 || header[29] != 0x02 {
            return Ok(None);
        }

        let header_size = u64::from_le_bytes([
            header[16], header[17], header[18], header[19], header[20], header[21], header[22],
            header[23],
        ]);
        if header_size < 30 || header_size > remaining {
            return Ok(None);
        }
        if self.max_size > 0 && header_size > self.max_size {
            return Ok(None);
        }

        let object_count = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
        if object_count == 0 || object_count > 4096 {
            return Ok(None);
        }

        let mut file_size = None;
        let mut offset = hit.global_offset + 30;
        let header_end = hit.global_offset + header_size;
        let mut seen = 0u32;
        while seen < object_count && offset + 24 <= header_end {
            let obj = read_exact_at(ctx, offset, 24)
                .ok_or_else(|| CarveError::Invalid("asf object truncated".to_string()))?;
            let guid = &obj[0..16];
            let obj_size = u64::from_le_bytes([
                obj[16], obj[17], obj[18], obj[19], obj[20], obj[21], obj[22], obj[23],
            ]);
            if obj_size < 24 {
                return Ok(None);
            }
            let next = match offset.checked_add(obj_size) {
                Some(v) => v,
                None => return Ok(None),
            };
            if next > header_end {
                return Ok(None);
            }
            if guid == ASF_FILE_PROP_GUID {
                if obj_size < 104 {
                    return Ok(None);
                }
                if let Some(bytes) = read_exact_at(ctx, offset + 40, 8) {
                    file_size = Some(u64::from_le_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
                        bytes[7],
                    ]));
                    break;
                }
            }
            offset = next;
            seen = seen.saturating_add(1);
        }

        let mut total_end = header_end;
        if let Some(size) = file_size {
            if size < header_size || size > remaining {
                return Ok(None);
            }
            total_end = hit.global_offset + size;
        }

        if self.max_size > 0 {
            let max_end = hit.global_offset.saturating_add(self.max_size);
            if total_end > max_end {
                total_end = max_end;
            }
        }

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut md5 = md5::Context::new();
        let mut sha256 = Sha256::new();

        let (written, eof_truncated) = write_range(
            ctx,
            hit.global_offset,
            total_end,
            &full_path,
            &mut md5,
            &mut sha256,
        )?;

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
            validated: !eof_truncated,
            truncated: eof_truncated,
            errors: Vec::new(),
            pattern_id: Some(hit.pattern_id.clone()),
        }))
    }
}

fn read_exact_at(ctx: &ExtractionContext, offset: u64, len: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let n = ctx.evidence.read_at(offset, &mut buf).ok()?;
    if n < len {
        return None;
    }
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::{ASF_FILE_PROP_GUID, ASF_HEADER_GUID, WmvCarveHandler};
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::RawFileSource;
    use crate::scanner::NormalizedHit;
    use tempfile::tempdir;

    fn minimal_asf() -> Vec<u8> {
        let mut data = Vec::new();
        let header_size = 30u64 + 104u64;
        data.extend_from_slice(&ASF_HEADER_GUID);
        data.extend_from_slice(&header_size.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.push(1);
        data.push(2);

        data.extend_from_slice(&ASF_FILE_PROP_GUID);
        data.extend_from_slice(&104u64.to_le_bytes());
        data.extend_from_slice(&[0u8; 16]);
        data.extend_from_slice(&(header_size).to_le_bytes());
        data.extend_from_slice(&[0u8; 104 - 24 - 16 - 8]);

        data
    }

    #[test]
    fn carves_minimal_wmv() {
        let temp_dir = tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let data = minimal_asf();
        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &data).expect("write asf");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
        };
        let handler = WmvCarveHandler::new("wmv".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "wmv".to_string(),
            pattern_id: "wmv_asf".to_string(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = carved.expect("carved");
        assert_eq!(carved.size, data.len() as u64);
    }

    #[test]
    fn rejects_unreasonable_header_size() {
        let temp_dir = tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let mut data = vec![0u8; 256];
        data[0..16].copy_from_slice(&ASF_HEADER_GUID);
        data[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        data[24..28].copy_from_slice(&1u32.to_le_bytes());
        data[28] = 1;
        data[29] = 2;

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &data).expect("write asf");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
        };
        let handler = WmvCarveHandler::new("wmv".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "wmv".to_string(),
            pattern_id: "wmv_asf".to_string(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        assert!(carved.is_none());
    }
}
