//! EML carving handler.
//!
//! Validates presence of multiple RFC 822 headers and email-like content.
//! Enhanced validation requires at least 2 header markers and email patterns.

use std::fs::File;

use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, output_path, write_range,
};
use crate::scanner::NormalizedHit;

/// Required header markers for email validation
const HEADER_MARKERS: [&[u8]; 6] = [
    b"From:",
    b"To:",
    b"Subject:",
    b"Date:",
    b"Message-ID:",
    b"MIME-Version:",
];
const MBOX_BOUNDARY: &[u8] = b"\nFrom ";

/// Minimum number of distinct headers required for validation
const MIN_HEADERS_REQUIRED: usize = 2;

/// Check if byte slice contains an @ character (basic email indicator)
fn contains_email_pattern(data: &[u8]) -> bool {
    // Look for patterns like "user@domain" in From: or To: lines
    data.contains(&b'@')
}

/// Check if data has CRLF or LF line endings typical of email
fn has_email_line_endings(data: &[u8]) -> bool {
    // Emails should have line breaks, check for \r\n or \n
    data.windows(2).any(|w| w == b"\r\n") || data.contains(&b'\n')
}

/// Check if this looks like a template string or debug output (not real email)
fn looks_like_template(data: &[u8]) -> bool {
    // Template strings often contain %s, %d, {}, or similar
    let templates = [b"%s" as &[u8], b"%d", b"{}", b"<%s>", b"${"];
    for tmpl in templates {
        if find_pattern(data, tmpl).is_some() {
            return true;
        }
    }
    false
}

pub struct EmlCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl EmlCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for EmlCarveHandler {
    fn file_type(&self) -> &str {
        "eml"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        // Read larger header for better validation
        let head = read_prefix(ctx, hit.global_offset, 2048);
        if head.is_empty() {
            return Ok(None);
        }

        // Count how many distinct header markers are present
        let header_count = HEADER_MARKERS
            .iter()
            .filter(|m| find_pattern(&head, m).is_some())
            .count();

        // Require at least MIN_HEADERS_REQUIRED distinct headers
        if header_count < MIN_HEADERS_REQUIRED {
            return Ok(None);
        }

        // Reject template strings (common false positive)
        if looks_like_template(&head) {
            return Ok(None);
        }

        // Require email-like content (@ symbol in header area)
        if !contains_email_pattern(&head) {
            return Ok(None);
        }

        // Require proper line endings
        if !has_email_line_endings(&head) {
            return Ok(None);
        }

        let max_end = if self.max_size > 0 {
            hit.global_offset.saturating_add(self.max_size)
        } else {
            u64::MAX
        };

        let mut offset = hit.global_offset;
        let mut end_offset = None;
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
            if let Some(pos) = find_pattern(&search_buf, MBOX_BOUNDARY) {
                let boundary = offset
                    .saturating_sub(carry.len() as u64)
                    .saturating_add(pos as u64);
                if boundary > hit.global_offset {
                    end_offset = Some(boundary);
                    break;
                }
            }

            offset = offset.saturating_add(buf.len() as u64);
            if buf.len() >= MBOX_BOUNDARY.len() - 1 {
                carry = buf[buf.len() - (MBOX_BOUNDARY.len() - 1)..].to_vec();
            } else {
                carry = buf;
            }
        }

        let end_offset = end_offset.unwrap_or(max_end);
        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut file = File::create(&full_path)?;
        let mut md5 = md5::Context::new();
        let mut sha256 = Sha256::new();

        let (written, eof_truncated) = write_range(
            ctx,
            hit.global_offset,
            end_offset,
            &mut file,
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
    use super::EmlCarveHandler;
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
    fn carves_valid_eml() {
        // Valid email with multiple headers and @ symbol
        let data = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test Email\r\nDate: Mon, 1 Jan 2024 12:00:00 +0000\r\n\r\nBody content here".to_vec();
        let evidence = SliceEvidence { data: data.clone() };
        let handler = EmlCarveHandler::new("eml".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "eml".to_string(),
            pattern_id: "eml_from".to_string(),
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        let carved = carved.expect("carved");
        assert_eq!(carved.size, data.len() as u64);
    }

    #[test]
    fn rejects_template_string() {
        // Template string that looks like email header (false positive case)
        let data = b"From: %s via WMI auto-mailer\nSubject: %s\n\nBody".to_vec();
        let evidence = SliceEvidence { data };
        let handler = EmlCarveHandler::new("eml".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "eml".to_string(),
            pattern_id: "eml_from".to_string(),
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(carved.is_none(), "template string should be rejected");
    }

    #[test]
    fn rejects_single_header() {
        // Only one header - should be rejected
        let data = b"From: user@example.com\n\nBody only".to_vec();
        let evidence = SliceEvidence { data };
        let handler = EmlCarveHandler::new("eml".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "eml".to_string(),
            pattern_id: "eml_from".to_string(),
        };
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(carved.is_none(), "single header should be rejected");
    }
}
