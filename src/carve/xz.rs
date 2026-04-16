//! XZ stream carving handler.
//!
//! We validate the header magic and scan for a footer with a valid CRC32.

use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PreValidation, output_path,
    write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const XZ_MAGIC: [u8; 6] = [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00];
const XZ_FOOTER_MAGIC: [u8; 2] = [0x59, 0x5A];

pub struct XzCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl XzCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for XzCarveHandler {
    fn file_type(&self) -> &str {
        "xz"
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
        if buf != XZ_MAGIC {
            return Ok(PreValidation::Reject("xz magic mismatch".to_string()));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let header = read_exact_at(ctx, hit.global_offset, 12)
            .ok_or_else(|| CarveError::Invalid("xz header too short".to_string()))?;
        if header[0..6] != XZ_MAGIC {
            return Ok(None);
        }
        let header_crc = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
        let computed = crc32(&header[6..8]);
        if header_crc != computed {
            return Ok(None);
        }

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut md5 = md5::Context::new();
        let mut sha256 = Sha256::new();

        let mut validated = false;
        let mut truncated = false;
        let mut errors = Vec::new();

        let max_end = if self.max_size > 0 {
            hit.global_offset.saturating_add(self.max_size)
        } else {
            u64::MAX
        };

        let mut end_offset = None;
        let mut offset = hit.global_offset + 12;
        let mut carry: Vec<u8> = Vec::new();
        let buf_size = 64 * 1024;

        while offset < max_end {
            let remaining = (max_end - offset).min(buf_size as u64) as usize;
            let mut buf = vec![0u8; remaining];
            let n = ctx
                .evidence
                .read_at(offset, &mut buf)
                .map_err(|e| CarveError::Evidence(e.to_string()))?;
            if n == 0 {
                break;
            }
            buf.truncate(n);

            let mut search_buf = carry.clone();
            search_buf.extend_from_slice(&buf);
            let mut search_start = 0usize;
            while let Some(pos) = find_pattern(&search_buf[search_start..], &XZ_FOOTER_MAGIC) {
                let absolute = search_start + pos;
                let footer_end = offset
                    .saturating_sub(carry.len() as u64)
                    .saturating_add(absolute as u64)
                    .saturating_add(2);
                if footer_end < hit.global_offset + 12 {
                    search_start = absolute + 1;
                    continue;
                }
                let footer_start = footer_end.saturating_sub(12);
                if footer_start <= hit.global_offset {
                    search_start = absolute + 1;
                    continue;
                }
                if let Some(footer) = read_exact_at(ctx, footer_start, 12)
                    && footer[10..12] == XZ_FOOTER_MAGIC
                {
                    let footer_crc =
                        u32::from_le_bytes([footer[0], footer[1], footer[2], footer[3]]);
                    let computed = crc32(&footer[4..10]);
                    if footer_crc == computed {
                        end_offset = Some(footer_end);
                        validated = true;
                        break;
                    }
                }
                search_start = absolute + 1;
            }
            if end_offset.is_some() {
                break;
            }

            offset = offset.saturating_add(buf.len() as u64);
            if buf.len() >= XZ_FOOTER_MAGIC.len() - 1 {
                carry = buf[buf.len() - (XZ_FOOTER_MAGIC.len() - 1)..].to_vec();
            } else {
                carry = buf;
            }
        }

        let end_offset = end_offset.unwrap_or(max_end);
        if self.max_size > 0 && end_offset >= max_end {
            truncated = true;
            errors.push("max_size reached before xz end".to_string());
        }

        let (written, eof_truncated) = write_range(
            ctx,
            hit.global_offset,
            end_offset,
            &full_path,
            &mut md5,
            &mut sha256,
        )?;
        if eof_truncated {
            truncated = true;
            if !errors.iter().any(|e| e.contains("eof")) {
                errors.push("eof before xz end".to_string());
            }
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
            validated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
        }))
    }
}

fn find_pattern(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let first = needle[0];
    let mut i = 0usize;
    while i + needle.len() <= haystack.len() {
        if haystack[i] == first && &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn read_exact_at(ctx: &ExtractionContext, offset: u64, len: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let n = ctx.evidence.read_at(offset, &mut buf).ok()?;
    if n < len {
        return None;
    }
    Some(buf)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320u32 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::{XzCarveHandler, crc32};
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::{EvidenceError, EvidenceSource};
    use crate::scanner::NormalizedHit;
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

    fn minimal_xz() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]);
        let flags = [0x00, 0x00];
        out.extend_from_slice(&flags);
        out.extend_from_slice(&crc32(&flags).to_le_bytes());
        out.extend_from_slice(&[0u8; 4]); // dummy index bytes

        let backward_size = [0x00, 0x00, 0x00, 0x00];
        let stream_flags = [0x00, 0x00];
        let mut footer = Vec::new();
        let footer_crc = crc32(&[
            backward_size[0],
            backward_size[1],
            backward_size[2],
            backward_size[3],
            stream_flags[0],
            stream_flags[1],
        ]);
        footer.extend_from_slice(&footer_crc.to_le_bytes());
        footer.extend_from_slice(&backward_size);
        footer.extend_from_slice(&stream_flags);
        footer.extend_from_slice(&[0x59, 0x5A]);
        out.extend_from_slice(&footer);
        out
    }

    #[test]
    fn carves_minimal_xz_with_footer() {
        let data = minimal_xz();
        let evidence = SliceEvidence { data: data.clone() };
        let handler = XzCarveHandler::new("xz".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "xz".to_string(),
            pattern_id: "xz_header".to_string(),
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
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        let carved = carved.expect("carved");
        assert!(carved.validated);
        assert_eq!(carved.size, data.len() as u64);
    }
}
