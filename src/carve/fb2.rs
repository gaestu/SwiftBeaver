//! FB2 (FictionBook) carving handler.
//!
//! Enhanced validation requires FictionBook tag or namespace within first 4KB.
//! Rejects generic XML files that don't contain FictionBook markers.

use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, DeferredWriter, ExtractionContext, PreValidation,
    output_path,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const FB2_HEADER: &[u8] = b"<?xml";
const FB2_TAG_LOWER: &[u8] = b"<fictionbook";
const FB2_NAMESPACE: &[u8] = b"fictionbook";
const FB2_END: &[u8] = b"</FictionBook>";
const FB2_END_LOWER: &[u8] = b"</fictionbook>";

/// Maximum bytes to scan for FictionBook tag at start of file
const MAX_TAG_SEARCH_BYTES: usize = 4096;

/// Check if data contains FictionBook tag or namespace (case-insensitive)
fn contains_fictionbook_marker(data: &[u8]) -> bool {
    // Convert to lowercase for case-insensitive search
    let lower: Vec<u8> = data.iter().map(|b| b.to_ascii_lowercase()).collect();

    // Look for <fictionbook tag or fictionbook namespace reference
    find_pattern(&lower, FB2_TAG_LOWER).is_some() || find_pattern(&lower, FB2_NAMESPACE).is_some()
}

pub struct Fb2CarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl Fb2CarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for Fb2CarveHandler {
    fn file_type(&self) -> &str {
        "fb2"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 5];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        if &buf != b"<?xml" {
            return Ok(PreValidation::Reject(
                "fb2 xml declaration mismatch".to_string(),
            ));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        // Early validation: check for FictionBook marker in first 4KB BEFORE creating file
        let preview = read_prefix(ctx, hit.global_offset, MAX_TAG_SEARCH_BYTES);
        if preview.is_empty() {
            return Ok(None);
        }

        // Must start with <?xml
        if preview.len() < FB2_HEADER.len() || &preview[..FB2_HEADER.len()] != FB2_HEADER {
            return Ok(None);
        }

        // Must contain FictionBook tag or namespace in header area
        if !contains_fictionbook_marker(&preview) {
            return Ok(None);
        }

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut writer = DeferredWriter::new(full_path.clone(), ctx.deferred_buffer_bytes);
        let mut md5 = md5::Context::new();
        let mut sha256 = Sha256::new();

        let mut validated = false;
        let mut truncated = false;
        let mut errors = Vec::new();

        let mut offset = hit.global_offset;
        let buf_size = 64 * 1024;
        let mut bytes_written = 0u64;
        let mut carry: Vec<u8> = Vec::new();

        loop {
            if self.max_size > 0 && bytes_written >= self.max_size {
                truncated = true;
                errors.push("max_size reached before fb2 end".to_string());
                break;
            }

            let remaining = if self.max_size > 0 {
                (self.max_size - bytes_written).min(buf_size as u64)
            } else {
                buf_size as u64
            };

            let mut buf = vec![0u8; remaining as usize];
            let n = ctx
                .evidence
                .read_at(offset, &mut buf)
                .map_err(|e| CarveError::Evidence(e.to_string()))?;
            if n == 0 {
                truncated = true;
                errors.push("eof before fb2 end".to_string());
                break;
            }
            buf.truncate(n);

            // Search for end tag (case-insensitive)
            let mut search_buf = carry.clone();
            search_buf.extend_from_slice(&buf);
            let lower_search: Vec<u8> = search_buf.iter().map(|b| b.to_ascii_lowercase()).collect();

            let end_pos = find_pattern(&lower_search, FB2_END_LOWER);
            if let Some(pos) = end_pos {
                let write_len = if pos < carry.len() {
                    pos + FB2_END.len() - carry.len()
                } else {
                    pos - carry.len() + FB2_END.len()
                };
                let write_len = write_len.min(buf.len());
                if write_len > 0 {
                    let slice = &buf[..write_len];
                    writer.write_all(slice)?;
                    md5.consume(slice);
                    sha256.update(slice);
                    bytes_written = bytes_written.saturating_add(slice.len() as u64);
                }
                validated = true;
                break;
            }

            writer.write_all(&buf)?;
            md5.consume(&buf);
            sha256.update(&buf);
            bytes_written = bytes_written.saturating_add(buf.len() as u64);
            offset = offset.saturating_add(buf.len() as u64);

            if FB2_END.len() > 1 {
                let keep = FB2_END.len() - 1;
                if buf.len() >= keep {
                    carry = buf[buf.len() - keep..].to_vec();
                } else {
                    carry = buf.clone();
                }
            } else {
                carry.clear();
            }
        }

        writer.flush_to_disk()?;

        if bytes_written < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

        let md5_hex = format!("{:x}", md5.compute());
        let sha256_hex = hex::encode(sha256.finalize());
        let global_end = if bytes_written == 0 {
            hit.global_offset
        } else {
            hit.global_offset + bytes_written - 1
        };

        Ok(Some(CarvedFile {
            run_id: ctx.run_id.to_string(),
            file_type: self.file_type().to_string(),
            path: rel_path,
            extension: self.extension.clone(),
            global_start: hit.global_offset,
            global_end,
            size: bytes_written,
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

fn read_prefix(ctx: &ExtractionContext, offset: u64, len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    let n = ctx.evidence.read_at(offset, &mut buf).ok().unwrap_or(0);
    buf.truncate(n);
    buf
}

#[cfg(test)]
mod tests {
    use super::Fb2CarveHandler;
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
    fn carves_fb2_with_footer() {
        let data = b"<?xml version='1.0'?><FictionBook>hi</FictionBook>".to_vec();
        let evidence = SliceEvidence { data: data.clone() };
        let handler = Fb2CarveHandler::new("fb2".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "fb2".to_string(),
            pattern_id: "fb2_xml".to_string(),
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        let carved = carved.expect("carved");
        assert!(carved.validated);
        assert_eq!(carved.size, data.len() as u64);
    }

    #[test]
    fn rejects_generic_xml() {
        // Generic XML without FictionBook marker should be rejected
        let data = b"<?xml version='1.0'?><scpd xmlns='urn:schemas-upnp-org:service-1-0'><specVersion><major>1</major></specVersion></scpd>".to_vec();
        let evidence = SliceEvidence { data };
        let handler = Fb2CarveHandler::new("fb2".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "fb2".to_string(),
            pattern_id: "fb2_xml".to_string(),
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(carved.is_none(), "generic XML should be rejected");
    }

    #[test]
    fn accepts_fictionbook_namespace() {
        // FB2 with namespace reference
        let data = b"<?xml version='1.0'?><FictionBook xmlns='http://www.gribuser.ru/xml/fictionbook/2.0'>content</FictionBook>".to_vec();
        let evidence = SliceEvidence { data: data.clone() };
        let handler = Fb2CarveHandler::new("fb2".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "fb2".to_string(),
            pattern_id: "fb2_xml".to_string(),
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(
            carved.is_some(),
            "FictionBook with namespace should be accepted"
        );
    }
}
