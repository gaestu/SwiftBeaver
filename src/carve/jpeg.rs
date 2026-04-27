use sha2::Digest;

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, DeferredWriter, ExtractionContext, PendingCarve,
    PreValidation, create_hashers, finalize_hashers, output_path,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

fn is_valid_first_marker(marker: u8) -> bool {
    matches!(
        marker,
        0x01 | 0xC0..=0xCF | 0xDA..=0xDF | 0xE0..=0xEF | 0xFE
    )
}

/// Two-phase JPEG marker-walker state.
///
/// Phase 1 (Header*): walk APPn/COM/DQT/DHT/SOF segments by parsing each
/// marker's length field, until we reach SOS (`FF DA`).
/// Phase 2 (Scan*): byte-walk the entropy-coded stream, honouring stuffed
/// bytes (`FF 00`), restart markers (`FF D0..FF D7`), and additional
/// segments inserted between scans of progressive JPEGs.
/// Terminates on real EOI (`FF D9`) only after entering scan phase.
#[derive(Debug, Clone, Copy)]
enum WalkState {
    HeaderExpectFF,
    HeaderExpectMarker,
    HeaderLenHi,
    HeaderLenLo(u8),
    HeaderSkip(u32),
    SosLenHi,
    SosLenLo(u8),
    SosSkip(u32),
    Scan,
    ScanFF,
    ScanLenHi,
    ScanLenLo(u8),
    ScanSkip(u32),
}

enum StepResult {
    Continue,
    Validated,
    Malformed(&'static str),
}

fn step(state: &mut WalkState, b: u8) -> StepResult {
    match *state {
        WalkState::HeaderExpectFF => {
            if b != 0xFF {
                return StepResult::Malformed("expected FF in header");
            }
            *state = WalkState::HeaderExpectMarker;
        }
        WalkState::HeaderExpectMarker => match b {
            0xFF => {
                // Fill byte; marker byte still pending.
            }
            0x00 => return StepResult::Malformed("FF 00 in header"),
            0x01 | 0xD0..=0xD8 => {
                // Standalone marker (TEM, RST0..7, SOI). No payload.
                *state = WalkState::HeaderExpectFF;
            }
            0xD9 => return StepResult::Malformed("EOI before SOS"),
            0xDA => *state = WalkState::SosLenHi,
            _ => *state = WalkState::HeaderLenHi,
        },
        WalkState::HeaderLenHi => *state = WalkState::HeaderLenLo(b),
        WalkState::HeaderLenLo(hi) => {
            let length = ((hi as u32) << 8) | (b as u32);
            if length < 2 {
                return StepResult::Malformed("segment length < 2");
            }
            let payload = length - 2;
            *state = if payload == 0 {
                WalkState::HeaderExpectFF
            } else {
                WalkState::HeaderSkip(payload)
            };
        }
        WalkState::HeaderSkip(remaining) => {
            let next = remaining - 1;
            *state = if next == 0 {
                WalkState::HeaderExpectFF
            } else {
                WalkState::HeaderSkip(next)
            };
        }
        WalkState::SosLenHi => *state = WalkState::SosLenLo(b),
        WalkState::SosLenLo(hi) => {
            let length = ((hi as u32) << 8) | (b as u32);
            if length < 2 {
                return StepResult::Malformed("SOS length < 2");
            }
            let payload = length - 2;
            *state = if payload == 0 {
                WalkState::Scan
            } else {
                WalkState::SosSkip(payload)
            };
        }
        WalkState::SosSkip(remaining) => {
            let next = remaining - 1;
            *state = if next == 0 {
                WalkState::Scan
            } else {
                WalkState::SosSkip(next)
            };
        }
        WalkState::Scan => {
            if b == 0xFF {
                *state = WalkState::ScanFF;
            }
        }
        WalkState::ScanFF => match b {
            0x00 => *state = WalkState::Scan,        // stuffed literal 0xFF
            0xFF => {}                               // fill byte; keep waiting for the marker byte
            0xD0..=0xD7 => *state = WalkState::Scan, // restart marker
            0xD8 => *state = WalkState::Scan,        // stray SOI; tolerate
            0xD9 => return StepResult::Validated,    // true EOI
            0xDA => *state = WalkState::SosLenHi,    // additional SOS (progressive)
            0x01 => *state = WalkState::Scan,
            _ => *state = WalkState::ScanLenHi,
        },
        WalkState::ScanLenHi => *state = WalkState::ScanLenLo(b),
        WalkState::ScanLenLo(hi) => {
            let length = ((hi as u32) << 8) | (b as u32);
            if length < 2 {
                return StepResult::Malformed("inter-scan segment length < 2");
            }
            let payload = length - 2;
            *state = if payload == 0 {
                WalkState::Scan
            } else {
                WalkState::ScanSkip(payload)
            };
        }
        WalkState::ScanSkip(remaining) => {
            let next = remaining - 1;
            *state = if next == 0 {
                WalkState::Scan
            } else {
                WalkState::ScanSkip(next)
            };
        }
    }
    StepResult::Continue
}

pub struct JpegCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl JpegCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for JpegCarveHandler {
    fn file_type(&self) -> &str {
        "jpeg"
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
        if buf[0] != 0xFF || buf[1] != 0xD8 || buf[2] != 0xFF || !is_valid_first_marker(buf[3]) {
            return Ok(PreValidation::Reject("jpeg signature mismatch".to_string()));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let mut sig = [0u8; 4];
        let read = ctx
            .evidence
            .read_at(hit.global_offset, &mut sig)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if read < sig.len()
            || sig[0] != 0xFF
            || sig[1] != 0xD8
            || sig[2] != 0xFF
            || !is_valid_first_marker(sig[3])
        {
            return Ok(None);
        }

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut writer = DeferredWriter::new(
            full_path.clone(),
            ctx.deferred_buffer_bytes,
            ctx.metadata_only,
        );
        let (mut md5, mut sha256) = create_hashers(&ctx.hash_config);

        let mut offset = hit.global_offset;
        let mut bytes_written = 0u64;
        let mut validated = false;
        let mut truncated = false;
        let mut errors = Vec::new();
        let mut state = WalkState::HeaderExpectFF;
        let buf_size = 64 * 1024;

        loop {
            if self.max_size > 0 && bytes_written >= self.max_size {
                truncated = true;
                errors.push("max_size reached before EOI".to_string());
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
                errors.push("eof before EOI".to_string());
                break;
            }
            buf.truncate(n);

            let mut consumed = 0usize;
            let mut malformed: Option<&'static str> = None;
            for (i, &b) in buf.iter().enumerate() {
                consumed = i + 1;
                match step(&mut state, b) {
                    StepResult::Continue => {}
                    StepResult::Validated => {
                        validated = true;
                        break;
                    }
                    StepResult::Malformed(reason) => {
                        malformed = Some(reason);
                        break;
                    }
                }
            }

            let slice = &buf[..consumed];
            writer.write_all(slice)?;
            if let Some(ref mut m) = md5 {
                m.consume(slice);
            }
            if let Some(ref mut s) = sha256 {
                s.update(slice);
            }
            bytes_written = bytes_written.saturating_add(consumed as u64);
            offset = offset.saturating_add(consumed as u64);

            if let Some(_reason) = malformed {
                writer.discard();
                return Ok(None);
            }
            if validated {
                break;
            }
        }

        if bytes_written < self.min_size {
            writer.discard();
            return Ok(None);
        }

        let (md5_hex, sha256_hex) = finalize_hashers(md5, sha256);
        let global_end = if bytes_written == 0 {
            hit.global_offset
        } else {
            hit.global_offset + bytes_written - 1
        };

        Ok(Some(PendingCarve::new(
            CarvedFile {
                run_id: ctx.run_id.to_string(),
                file_type: self.file_type().to_string(),
                path: rel_path,
                extension: self.extension.clone(),
                global_start: hit.global_offset,
                global_end,
                size: bytes_written,
                md5: md5_hex,
                sha256: sha256_hex,
                validated,
                truncated,
                errors,
                pattern_id: Some(hit.pattern_id.clone()),
                is_duplicate: false,
                duplicate_of_offset: None,
            },
            writer,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::JpegCarveHandler;
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::RawFileSource;
    use crate::scanner::NormalizedHit;
    use tempfile::tempdir;

    fn make_test_jpeg(first_marker: u8) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0xFF, 0xD8, 0xFF, first_marker]);
        out.extend_from_slice(&[0x00, 0x10]); // segment length = 16 (incl. these 2 bytes)
        out.extend_from_slice(b"SwiftBeaver\0\0\0"); // 14 bytes payload
        // Add a minimal SOS + entropy + EOI so the structured walker reaches scan phase.
        out.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]); // SOS, length=2 (no payload)
        out.extend_from_slice(&[0x00, 0x11, 0x22]); // entropy
        out.extend_from_slice(&[0xFF, 0xD9]); // EOI
        out
    }

    fn run_handler(
        data: Vec<u8>,
        min_size: u64,
    ) -> (tempfile::TempDir, Option<crate::carve::CarvedFile>) {
        let temp_dir = tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("mkdir");
        let input_path = temp_dir.path().join("input.bin");
        std::fs::write(&input_path, data).expect("write");

        let evidence = RawFileSource::open(&input_path).expect("open evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };
        let handler = JpegCarveHandler::new("jpg".to_string(), min_size, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "jpeg".to_string(),
            pattern_id: "jpeg_soi".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let pending = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = pending.map(|p| p.flush().expect("flush"));
        (temp_dir, carved)
    }

    #[test]
    fn accepts_jpeg_with_valid_first_marker() {
        let data = make_test_jpeg(0xE0);
        let (_t, carved) = run_handler(data, 10);
        let carved = carved.expect("expected jpeg");
        assert!(carved.validated);
        assert!(carved.size >= 10);
    }

    #[test]
    fn rejects_jpeg_with_invalid_first_marker() {
        let data = make_test_jpeg(0x83);
        let (_t, carved) = run_handler(data, 10);
        assert!(carved.is_none(), "expected invalid marker to be rejected");
    }

    /// Embedded thumbnail JPEG inside an APP1 segment must not terminate the outer
    /// carve at the thumbnail's `FF D9`. Regression for issue #77.
    #[test]
    fn does_not_stop_at_embedded_thumbnail_eoi() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0xFF, 0xD8]); // SOI

        // Build an inner JPEG that itself has SOI + dummy APP0 + SOS + entropy + EOI.
        // The bytes 0xFF 0xD9 will appear inside the APP1 payload.
        let mut inner = Vec::new();
        inner.extend_from_slice(&[0xFF, 0xD8]);
        inner.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x06, b'J', b'F', b'I', b'F']);
        inner.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        inner.extend_from_slice(&[0x00, 0x11, 0x22]);
        inner.extend_from_slice(&[0xFF, 0xD9]);

        // APP1 segment carrying the inner JPEG (plus a couple of pre/post bytes).
        let app1_payload_len = inner.len() + 4; // 2 bytes pre + inner + 2 bytes post
        let app1_seg_len = app1_payload_len + 2; // length field includes its own 2 bytes
        assert!(app1_seg_len <= 0xFFFF);
        data.extend_from_slice(&[0xFF, 0xE1]);
        data.extend_from_slice(&[(app1_seg_len >> 8) as u8, (app1_seg_len & 0xFF) as u8]);
        data.extend_from_slice(&[0xAA, 0xBB]);
        data.extend_from_slice(&inner);
        data.extend_from_slice(&[0xCC, 0xDD]);

        // Outer SOS + entropy + EOI.
        data.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        data.extend_from_slice(&[0x10, 0x20, 0x30, 0x40, 0x50]);
        data.extend_from_slice(&[0xFF, 0xD9]);

        let expected_size = data.len() as u64;
        let (_t, carved) = run_handler(data, 10);
        let carved = carved.expect("carved file");
        assert!(carved.validated, "outer EOI must be detected");
        assert_eq!(
            carved.size, expected_size,
            "must carve through to outer EOI, not stop at thumbnail"
        );
    }

    /// Progressive JPEG with multiple SOS segments: every scan must be carved.
    #[test]
    fn handles_progressive_multiple_sos() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0xFF, 0xD8]); // SOI
        data.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x06, b'J', b'F', b'I', b'F']); // APP0
        // First SOS + entropy
        data.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x11, 0x22, 0x33]);
        // Inter-scan DHT-like length-bearing marker
        data.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x05, 0xAA, 0xBB, 0xCC]);
        // Second SOS + entropy (with a stuffed FF 00 for good measure)
        data.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x44, 0xFF, 0x00, 0x55]);
        data.extend_from_slice(&[0xFF, 0xD9]); // EOI

        let expected_size = data.len() as u64;
        let (_t, carved) = run_handler(data, 10);
        let carved = carved.expect("carved file");
        assert!(carved.validated);
        assert_eq!(carved.size, expected_size);
    }

    /// Stuffed `FF 00` bytes inside the entropy stream must not be misread as EOI.
    #[test]
    fn handles_stuffed_ff_00_in_scan() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0xFF, 0xD8]);
        data.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x06, b'J', b'F', b'I', b'F']);
        data.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        // Entropy stream containing several stuffed FF 00 sequences.
        for _ in 0..8 {
            data.extend_from_slice(&[0x12, 0xFF, 0x00, 0x34, 0xFF, 0x00]);
        }
        data.extend_from_slice(&[0xFF, 0xD9]);

        let expected_size = data.len() as u64;
        let (_t, carved) = run_handler(data, 10);
        let carved = carved.expect("carved file");
        assert!(carved.validated);
        assert_eq!(carved.size, expected_size);
    }

    /// Truncated input (no EOI before EOF) must produce a non-validated, truncated carve.
    #[test]
    fn marks_truncated_when_eoi_missing() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0xFF, 0xD8]);
        data.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x06, b'J', b'F', b'I', b'F']);
        data.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        // Lots of entropy but no EOI.
        data.extend(std::iter::repeat_n(0x42u8, 200));

        let (_t, carved) = run_handler(data, 10);
        let carved = carved.expect("carved file");
        assert!(!carved.validated, "must not be validated");
        assert!(carved.truncated, "must be marked truncated");
        assert!(!carved.errors.is_empty(), "must record an error");
    }

    /// Malformed segment length (length < 2) must cause the carve to be rejected.
    #[test]
    fn rejects_malformed_segment_length() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]);
        // Length = 1 is invalid (must be >= 2).
        data.extend_from_slice(&[0x00, 0x01, 0x00]);
        // Pad so file is large enough that min_size wouldn't be the reason for rejection.
        data.extend(std::iter::repeat_n(0x00u8, 200));

        let (_t, carved) = run_handler(data, 10);
        assert!(
            carved.is_none(),
            "malformed segment length must reject the carve"
        );
    }
}
