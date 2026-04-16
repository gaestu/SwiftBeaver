//! EML carving handler.
//!
//! Multi-strategy email end detection and content validation.
//! Uses MIME boundary detection, mbox boundary detection, and binary content
//! transition detection to avoid oversized output from forensic disk images.

use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, output_path, write_range,
};
use crate::scanner::NormalizedHit;

/// RFC 822 header markers used for email validation.
const HEADER_MARKERS: [&[u8]; 7] = [
    b"From:",
    b"To:",
    b"Subject:",
    b"Date:",
    b"Message-ID:",
    b"MIME-Version:",
    b"Received:",
];

/// Mbox boundary marker.
const MBOX_BOUNDARY: &[u8] = b"\nFrom ";

/// Minimum number of distinct RFC 822 headers required for validation.
const MIN_HEADERS_REQUIRED: usize = 3;

/// Size of the sliding window (in bytes) for binary content transition detection.
const BINARY_WINDOW_SIZE: usize = 512;

/// If more than this fraction of bytes in a window are binary indicators,
/// the content is considered binary and the carve is terminated.
const BINARY_INDICATOR_THRESHOLD: f64 = 0.30;

/// For post-carve validation: if more than this fraction of total scanned bytes
/// are binary indicators, reject the carved file.
const MAX_BINARY_RATIO: f64 = 0.30;

/// Minimum number of bytes to scan before applying binary transition detection.
/// Skips the header area which is always textual.
const MIN_SCAN_BEFORE_BINARY_CHECK: u64 = 512;

/// Check if a byte is a strong indicator of binary (non-text) content.
/// These byte values almost never appear in legitimate email text, including UTF-8.
fn is_binary_indicator(b: u8) -> bool {
    matches!(b, 0x00..=0x08 | 0x0E..=0x1F | 0x7F)
}

/// Check if byte slice contains an @ character (basic email indicator).
fn contains_email_pattern(data: &[u8]) -> bool {
    data.contains(&b'@')
}

/// Check if data has CRLF or LF line endings typical of email.
fn has_email_line_endings(data: &[u8]) -> bool {
    data.windows(2).any(|w| w == b"\r\n") || data.contains(&b'\n')
}

/// Check if this looks like a template string or debug output (not real email).
/// Only checks the header area (before the first blank line) to avoid false
/// positives on legitimate emails containing code, JSON, or template syntax.
fn looks_like_template(data: &[u8]) -> bool {
    // Find end of headers (first blank line)
    let header_end = data
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .or_else(|| data.windows(2).position(|w| w == b"\n\n"))
        .unwrap_or(data.len());
    let header_area = &data[..header_end];
    let templates = [b"%s" as &[u8], b"%d", b"{}", b"<%s>", b"${"];
    for tmpl in templates {
        if find_pattern(header_area, tmpl).is_some() {
            return true;
        }
    }
    false
}

/// Extract MIME boundary from Content-Type header.
/// Returns the final boundary marker `--<boundary>--` if found.
fn extract_mime_boundary(head: &[u8]) -> Option<Vec<u8>> {
    let lower: Vec<u8> = head.iter().map(|b| b.to_ascii_lowercase()).collect();
    let marker = b"boundary=";
    let pos = find_pattern(&lower, marker)?;
    let start = pos + marker.len();
    if start >= head.len() {
        return None;
    }
    let after = &head[start..];

    let boundary_value = if after.first() == Some(&b'"') {
        let content = &after[1..];
        let end = content.iter().position(|&b| b == b'"')?;
        &content[..end]
    } else {
        let end = after
            .iter()
            .position(|&b| matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b';'))
            .unwrap_or(after.len());
        if end == 0 {
            return None;
        }
        &after[..end]
    };

    if boundary_value.is_empty() || boundary_value.len() > 200 {
        return None;
    }

    let mut final_boundary = Vec::with_capacity(boundary_value.len() + 4);
    final_boundary.extend_from_slice(b"--");
    final_boundary.extend_from_slice(boundary_value);
    final_boundary.extend_from_slice(b"--");
    Some(final_boundary)
}

/// Find the byte offset within `data` where content transitions from text to binary.
/// Uses a sliding window approach. Returns None if content remains textual throughout.
fn find_binary_transition(data: &[u8]) -> Option<usize> {
    if data.len() < BINARY_WINDOW_SIZE {
        return None;
    }
    let step = BINARY_WINDOW_SIZE / 2;
    let mut pos = 0;
    while pos + BINARY_WINDOW_SIZE <= data.len() {
        let window = &data[pos..pos + BINARY_WINDOW_SIZE];
        let binary_count = window.iter().filter(|&&b| is_binary_indicator(b)).count();
        if binary_count as f64 / BINARY_WINDOW_SIZE as f64 > BINARY_INDICATOR_THRESHOLD {
            return Some(pos);
        }
        pos += step;
    }
    None
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
        let head = read_prefix(ctx, hit.global_offset, 2048);
        if head.is_empty() {
            return Ok(None);
        }

        // Count distinct RFC 822 headers present
        let header_count = HEADER_MARKERS
            .iter()
            .filter(|m| find_pattern(&head, m).is_some())
            .count();
        if header_count < MIN_HEADERS_REQUIRED {
            return Ok(None);
        }

        // Reject template strings (common false positive in binaries)
        if looks_like_template(&head) {
            return Ok(None);
        }

        // Require @ symbol (email address indicator)
        if !contains_email_pattern(&head) {
            return Ok(None);
        }

        // Require proper line endings
        if !has_email_line_endings(&head) {
            return Ok(None);
        }

        // Extract MIME boundary for multipart emails
        let mime_final_boundary = extract_mime_boundary(&head);

        let max_end = if self.max_size > 0 {
            hit.global_offset.saturating_add(self.max_size)
        } else {
            u64::MAX
        };

        // Compute carry buffer size based on longest pattern to match
        let mbox_carry = MBOX_BOUNDARY.len().saturating_sub(1);
        let mime_carry = mime_final_boundary
            .as_ref()
            .map_or(0, |b| b.len().saturating_sub(1));
        let carry_size = mbox_carry.max(mime_carry);

        let mut offset = hit.global_offset;
        let mut end_offset = None;
        let mut found_boundary = false;
        let mut carry: Vec<u8> = Vec::new();
        let buf_size = 64 * 1024;
        let mut total_bytes_scanned: u64 = 0;
        let mut total_binary_indicators: u64 = 0;

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

            // Track binary indicator count for post-carve validation
            let binary_in_chunk = buf.iter().filter(|&&b| is_binary_indicator(b)).count() as u64;
            total_binary_indicators += binary_in_chunk;
            total_bytes_scanned += n as u64;

            // Build combined search buffer with carry from previous chunk
            let mut search_buf = carry.clone();
            search_buf.extend_from_slice(&buf);

            // Strategy 1: MIME final boundary detection (highest confidence)
            if let Some(ref boundary) = mime_final_boundary
                && let Some(pos) = find_pattern(&search_buf, boundary)
            {
                let boundary_abs = offset
                    .saturating_sub(carry.len() as u64)
                    .saturating_add(pos as u64)
                    .saturating_add(boundary.len() as u64);
                if boundary_abs > hit.global_offset {
                    end_offset = Some(boundary_abs);
                    found_boundary = true;
                    break;
                }
            }

            // Strategy 2: Mbox boundary detection
            if let Some(pos) = find_pattern(&search_buf, MBOX_BOUNDARY) {
                let boundary_abs = offset
                    .saturating_sub(carry.len() as u64)
                    .saturating_add(pos as u64);
                if boundary_abs > hit.global_offset {
                    end_offset = Some(boundary_abs);
                    found_boundary = true;
                    break;
                }
            }

            // Strategy 3: Binary content transition detection
            // Only check after scanning past the header area
            if total_bytes_scanned > MIN_SCAN_BEFORE_BINARY_CHECK {
                let bytes_from_carve_start = offset.saturating_sub(hit.global_offset);
                let skip = if bytes_from_carve_start < MIN_SCAN_BEFORE_BINARY_CHECK {
                    (MIN_SCAN_BEFORE_BINARY_CHECK - bytes_from_carve_start) as usize
                } else {
                    0
                };
                if skip < buf.len()
                    && let Some(transition_pos) = find_binary_transition(&buf[skip..])
                {
                    let transition_abs = offset.saturating_add(skip as u64 + transition_pos as u64);
                    end_offset = Some(transition_abs);
                    found_boundary = true;
                    break;
                }
            }

            offset = offset.saturating_add(buf.len() as u64);
            if carry_size > 0 && buf.len() >= carry_size {
                carry = buf[buf.len() - carry_size..].to_vec();
            } else {
                carry = buf;
            }
        }

        let end_offset = end_offset.unwrap_or(max_end);

        // Post-carve validation: if no structural boundary was found,
        // check that content is predominantly text (not binary garbage)
        if !found_boundary && total_bytes_scanned > MIN_SCAN_BEFORE_BINARY_CHECK {
            let binary_ratio = total_binary_indicators as f64 / total_bytes_scanned as f64;
            if binary_ratio > MAX_BINARY_RATIO {
                return Ok(None);
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
            end_offset,
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
    use super::*;
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

    fn make_hit() -> NormalizedHit {
        NormalizedHit {
            global_offset: 0,
            file_type_id: "eml".to_string(),
            pattern_id: "eml_from".to_string(),
            chunk_data: None,
            chunk_start: 0,
        }
    }

    fn make_handler(max_size: u64) -> EmlCarveHandler {
        EmlCarveHandler::new("eml".to_string(), 0, max_size)
    }

    #[test]
    fn carves_valid_eml() {
        let data = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test Email\r\nDate: Mon, 1 Jan 2024 12:00:00 +0000\r\n\r\nBody content here".to_vec();
        let evidence = SliceEvidence { data: data.clone() };
        let handler = make_handler(0);
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

        let carved = handler.process_hit(&make_hit(), &ctx).expect("process");
        let carved = carved.expect("carved");
        assert_eq!(carved.size, data.len() as u64);
    }

    #[test]
    fn rejects_template_string() {
        // Three headers so MIN_HEADERS_REQUIRED is met; rejection is due to templates
        let data = b"From: %s via WMI auto-mailer\nSubject: %s\nDate: Mon, 1 Jan 2024 12:00:00 +0000\n\nBody".to_vec();
        let evidence = SliceEvidence { data };
        let handler = make_handler(0);
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

        let carved = handler.process_hit(&make_hit(), &ctx).expect("process");
        assert!(carved.is_none(), "template string should be rejected");
    }

    #[test]
    fn rejects_single_header() {
        let data = b"From: user@example.com\n\nBody only".to_vec();
        let evidence = SliceEvidence { data };
        let handler = make_handler(0);
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

        let carved = handler.process_hit(&make_hit(), &ctx).expect("process");
        assert!(carved.is_none(), "single header should be rejected");
    }

    #[test]
    fn rejects_two_headers() {
        let data = b"From: user@example.com\nSubject: Hello\n\nBody text".to_vec();
        let evidence = SliceEvidence { data };
        let handler = make_handler(0);
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

        let carved = handler.process_hit(&make_hit(), &ctx).expect("process");
        assert!(carved.is_none(), "two headers should be rejected (need 3)");
    }

    #[test]
    fn detects_binary_transition() {
        let mut data = Vec::new();
        data.extend_from_slice(
            b"From: sender@example.com\r\nTo: recipient@example.com\r\n\
              Subject: Test\r\nDate: Mon, 1 Jan 2024 12:00:00 +0000\r\n\r\n",
        );
        // Add text padding to get well past MIN_SCAN_BEFORE_BINARY_CHECK
        for _ in 0..60 {
            data.extend_from_slice(b"Normal email text line content here.\r\n");
        }
        let text_end = data.len();
        // Add 2KB of binary data (NULL bytes)
        data.extend(vec![0x00u8; 2048]);

        let evidence = SliceEvidence { data: data.clone() };
        let handler = make_handler(10 * 1024 * 1024);
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

        let carved = handler.process_hit(&make_hit(), &ctx).expect("process");
        let carved = carved.expect("should be carved");
        // Should stop near binary transition, not include all binary data
        assert!(
            carved.size < data.len() as u64,
            "should not include all binary data: carved {} vs total {}",
            carved.size,
            data.len()
        );
        assert!(
            carved.size <= (text_end + BINARY_WINDOW_SIZE) as u64,
            "should stop near text end: carved {} vs text_end {}",
            carved.size,
            text_end
        );
    }

    #[test]
    fn detects_mime_boundary() {
        let data = b"From: sender@example.com\r\nTo: recipient@example.com\r\n\
            Subject: Test\r\nDate: Mon, 1 Jan 2024 12:00:00 +0000\r\n\
            MIME-Version: 1.0\r\n\
            Content-Type: multipart/mixed; boundary=\"BOUNDARY123\"\r\n\r\n\
            --BOUNDARY123\r\nContent-Type: text/plain\r\n\r\n\
            Hello World\r\n\
            --BOUNDARY123--\r\n\
            Trailing garbage that should not be included"
            .to_vec();

        let evidence = SliceEvidence { data: data.clone() };
        let handler = make_handler(10 * 1024 * 1024);
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

        let carved = handler.process_hit(&make_hit(), &ctx).expect("process");
        let carved = carved.expect("should be carved");
        assert!(
            carved.size < data.len() as u64,
            "MIME boundary should trim trailing data"
        );
        let carved_path = dir.path().join(&carved.path);
        let content = std::fs::read(&carved_path).expect("read carved file");
        let content_str = String::from_utf8_lossy(&content);
        assert!(
            content_str.contains("--BOUNDARY123--"),
            "carved file should include final MIME boundary"
        );
        assert!(
            !content_str.contains("Trailing garbage"),
            "carved file should not include content after MIME boundary"
        );
    }

    #[test]
    fn preserves_base64_attachment() {
        let data = b"From: sender@example.com\r\nTo: recipient@example.com\r\n\
            Subject: With Attachment\r\nDate: Mon, 1 Jan 2024 12:00:00 +0000\r\n\
            MIME-Version: 1.0\r\n\
            Content-Type: multipart/mixed; boundary=\"B\"\r\n\r\n\
            --B\r\nContent-Type: text/plain\r\n\r\nHello\r\n\
            --B\r\nContent-Transfer-Encoding: base64\r\n\r\n\
            VGhpcyBpcyBhIHRlc3QgYXR0YWNobWVudC4gSXQgY29udGFpbnMgc29tZSBi\r\n\
            YXNlNjQgZW5jb2RlZCBkYXRhIHRoYXQgc2hvdWxkIG5vdCB0cmlnZ2VyIGJp\r\n\
            bmFyeSBkZXRlY3Rpb24u\r\n\
            --B--"
            .to_vec();

        let evidence = SliceEvidence { data: data.clone() };
        let handler = make_handler(10 * 1024 * 1024);
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

        let carved = handler.process_hit(&make_hit(), &ctx).expect("process");
        let carved = carved.expect("should be carved");
        let carved_path = dir.path().join(&carved.path);
        let content = std::fs::read(&carved_path).expect("read");
        let content_str = String::from_utf8_lossy(&content);
        assert!(
            content_str.contains("--B--"),
            "base64 attachment email should be fully preserved"
        );
    }

    #[test]
    fn stops_at_mbox_boundary() {
        let data = b"From: sender@example.com\r\nTo: recipient@example.com\r\n\
            Subject: First\r\nDate: Mon, 1 Jan 2024 12:00:00 +0000\r\n\r\n\
            First email body\r\n\
            \nFrom second@example.com Mon Jan 01 12:00:00 2024\r\n\
            Another email starts here"
            .to_vec();

        let evidence = SliceEvidence { data: data.clone() };
        let handler = make_handler(10 * 1024 * 1024);
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

        let carved = handler.process_hit(&make_hit(), &ctx).expect("process");
        let carved = carved.expect("should be carved");
        let carved_path = dir.path().join(&carved.path);
        let content = std::fs::read(&carved_path).expect("read");
        let content_str = String::from_utf8_lossy(&content);
        assert!(content_str.contains("First email body"));
        assert!(
            !content_str.contains("Another email starts here"),
            "should stop at mbox boundary"
        );
    }

    #[test]
    fn regression_binary_data_after_header() {
        // Simulates the old behavior: email header followed by binary disk data.
        // Before fix: would carve up to max_size (50 MiB).
        // After fix: stops at binary transition.
        let mut data = Vec::new();
        data.extend_from_slice(
            b"From: sender@example.com\r\nTo: recipient@example.com\r\n\
              Subject: Test\r\nDate: Mon, 1 Jan 2024 12:00:00 +0000\r\n\r\n\
              Brief body\r\n",
        );
        // Pad with text to get past header protection zone
        for _ in 0..15 {
            data.extend_from_slice(b"More email body text padding.\r\n");
        }
        // Simulate 100KB of binary filesystem data after the email
        data.extend(vec![0x00u8; 100 * 1024]);

        let evidence = SliceEvidence { data };
        let handler = make_handler(50 * 1024 * 1024); // Old 50 MiB max
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

        let carved = handler.process_hit(&make_hit(), &ctx).expect("process");
        let carved = carved.expect("should carve the email portion");
        assert!(
            carved.size < 5000,
            "should stop near email content, not at max_size: {} bytes",
            carved.size
        );
    }

    #[test]
    fn extract_mime_boundary_quoted() {
        let head = b"Content-Type: multipart/mixed; boundary=\"abc123\"";
        let boundary = extract_mime_boundary(head).expect("should extract");
        assert_eq!(boundary, b"--abc123--");
    }

    #[test]
    fn extract_mime_boundary_unquoted() {
        let head = b"Content-Type: multipart/mixed; boundary=abc123\r\n";
        let boundary = extract_mime_boundary(head).expect("should extract");
        assert_eq!(boundary, b"--abc123--");
    }

    #[test]
    fn extract_mime_boundary_case_insensitive() {
        let head = b"Content-Type: multipart/mixed; BOUNDARY=\"Test456\"";
        let boundary = extract_mime_boundary(head).expect("should extract");
        assert_eq!(boundary, b"--Test456--");
    }

    #[test]
    fn find_binary_transition_in_mixed_data() {
        let mut data = vec![b'A'; 1024]; // 1KB of text
        data.extend(vec![0x00u8; 1024]); // 1KB of binary
        let pos = find_binary_transition(&data).expect("should detect transition");
        // Transition should be detected near the 1024 boundary
        assert!(
            (512..=1280).contains(&pos),
            "transition at unexpected pos: {}",
            pos
        );
    }

    #[test]
    fn no_binary_transition_in_text() {
        let data = vec![b'A'; 2048];
        assert!(find_binary_transition(&data).is_none());
    }
}
