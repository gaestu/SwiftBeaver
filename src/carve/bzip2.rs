//! BZIP2 carving handler.
//!
//! We scan for the byte-aligned end marker as a best-effort heuristic.

use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PreValidation, output_path,
    write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const BZIP2_MAGIC: [u8; 3] = [0x42, 0x5A, 0x68];
const BZIP2_END: [u8; 6] = [0x17, 0x72, 0x45, 0x38, 0x50, 0x90];
/// Maximum bytes to search for footer before giving up (10MB)
const BZIP2_SEARCH_LIMIT: u64 = 10 * 1024 * 1024;

pub struct Bzip2CarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl Bzip2CarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for Bzip2CarveHandler {
    fn file_type(&self) -> &str {
        "bzip2"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 4];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        if buf[0..3] != BZIP2_MAGIC {
            return Ok(PreValidation::Reject("bzip2 magic mismatch".to_string()));
        }
        if !(b'1'..=b'9').contains(&buf[3]) {
            return Ok(PreValidation::Reject(
                "bzip2 block size invalid".to_string(),
            ));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let header = read_exact_at(ctx, hit.global_offset, 4)
            .ok_or_else(|| CarveError::Invalid("bzip2 header too short".to_string()))?;
        if header[0..3] != BZIP2_MAGIC {
            return Ok(None);
        }
        if !(b'1'..=b'9').contains(&header[3]) {
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
        let mut offset = hit.global_offset + 4;
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
                end_offset = Some(offset);
                break;
            }
            buf.truncate(n);

            let mut search_buf = carry.clone();
            search_buf.extend_from_slice(&buf);
            if let Some(pos) = find_pattern(&search_buf, &BZIP2_END) {
                let absolute = offset
                    .saturating_sub(carry.len() as u64)
                    .saturating_add(pos as u64);
                end_offset = Some(absolute + BZIP2_END.len() as u64);
                validated = true;
                break;
            }

            // Early termination: if we've searched past BZIP2_SEARCH_LIMIT without
            // finding the footer, this is likely a false positive
            let bytes_searched = offset.saturating_sub(hit.global_offset);
            if bytes_searched > BZIP2_SEARCH_LIMIT && !validated {
                // Searched too far without finding footer - likely false positive
                let _ = std::fs::remove_file(&full_path);
                return Ok(None);
            }

            offset = offset.saturating_add(buf.len() as u64);
            if buf.len() >= BZIP2_END.len() - 1 {
                carry = buf[buf.len() - (BZIP2_END.len() - 1)..].to_vec();
            } else {
                carry = buf;
            }
        }

        let end_offset = end_offset.unwrap_or(max_end);
        if self.max_size > 0 && end_offset >= max_end {
            truncated = true;
            errors.push("max_size reached before bzip2 end".to_string());
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
                errors.push("eof before bzip2 end".to_string());
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

#[cfg(test)]
mod tests {
    use super::Bzip2CarveHandler;
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

    #[test]
    fn carves_bzip2_with_end_marker() {
        let mut data = Vec::new();
        data.extend_from_slice(b"BZh9");
        data.extend_from_slice(&[0u8; 10]);
        data.extend_from_slice(&[0x17, 0x72, 0x45, 0x38, 0x50, 0x90]);

        let evidence = SliceEvidence { data: data.clone() };
        let handler = Bzip2CarveHandler::new("bz2".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "bzip2".to_string(),
            pattern_id: "bzip2_header".to_string(),
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
            metadata_only: false,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        let carved = carved.expect("carved");
        assert!(carved.validated);
        assert_eq!(carved.size, data.len() as u64);
    }

    #[test]
    fn rejects_when_footer_not_found_within_limit() {
        // Create bzip2 header followed by data larger than BZIP2_SEARCH_LIMIT (10MB)
        // without a valid footer - this should be rejected as a false positive
        let mut data = Vec::new();
        data.extend_from_slice(b"BZh9");
        // Add 11MB of zeros (exceeds 10MB search limit)
        data.extend(vec![0u8; 11 * 1024 * 1024]);
        // No end marker added - simulates false positive

        let evidence = SliceEvidence { data };
        let handler = Bzip2CarveHandler::new("bz2".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "bzip2".to_string(),
            pattern_id: "bzip2_header".to_string(),
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
            metadata_only: false,
        };

        // Should reject because footer not found within 10MB search limit
        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(
            carved.is_none(),
            "should reject when footer not found within search limit"
        );
    }

    #[test]
    fn accepts_footer_within_limit() {
        // Create bzip2 with footer at ~5MB (within limit)
        let mut data = Vec::new();
        data.extend_from_slice(b"BZh9");
        data.extend(vec![0u8; 5 * 1024 * 1024]); // 5MB of data
        data.extend_from_slice(&[0x17, 0x72, 0x45, 0x38, 0x50, 0x90]); // footer

        let evidence = SliceEvidence { data: data.clone() };
        let handler = Bzip2CarveHandler::new("bz2".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "bzip2".to_string(),
            pattern_id: "bzip2_header".to_string(),
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
            metadata_only: false,
        };

        // Should accept because footer found within limit
        let carved = handler.process_hit(&hit, &ctx).expect("process");
        let carved = carved.expect("should carve file with footer within limit");
        assert!(carved.validated);
    }
}
