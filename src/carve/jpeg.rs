use sha2::Digest;

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, DeferredWriter, ExtractionContext, PreValidation,
    create_hashers, finalize_hashers, output_path,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

fn is_valid_first_marker(marker: u8) -> bool {
    matches!(
        marker,
        0x01 | 0xC0..=0xCF | 0xDA..=0xDF | 0xE0..=0xEF | 0xFE
    )
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
    ) -> Result<Option<CarvedFile>, CarveError> {
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

        let mut prev: Option<u8> = None;
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

            let mut write_len = n;
            for (i, b) in buf.iter().enumerate() {
                if prev == Some(0xFF) && *b == 0xD9 {
                    write_len = i + 1;
                    validated = true;
                    break;
                }
                prev = Some(*b);
            }

            let slice = &buf[..write_len];
            writer.write_all(slice)?;
            if let Some(ref mut m) = md5 {
                m.consume(slice);
            }
            if let Some(ref mut s) = sha256 {
                s.update(slice);
            }

            bytes_written = bytes_written.saturating_add(write_len as u64);
            offset = offset.saturating_add(write_len as u64);

            if validated {
                break;
            }

            if write_len < n {
                break;
            }
        }

        writer.flush_to_disk()?;

        if bytes_written < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

        let (md5_hex, sha256_hex) = finalize_hashers(md5, sha256);
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
            md5: md5_hex,
            sha256: sha256_hex,
            validated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
            is_duplicate: false,
            duplicate_of_offset: None,
        }))
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
        out.extend_from_slice(&[0x00, 0x10]); // segment length
        out.extend_from_slice(b"SwiftBeaver");
        out.extend_from_slice(&[0xFF, 0xD9]);
        out
    }

    #[test]
    fn accepts_jpeg_with_valid_first_marker() {
        let temp_dir = tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("mkdir");

        let data = make_test_jpeg(0xE0);
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
        let handler = JpegCarveHandler::new("jpg".to_string(), 10, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "jpeg".to_string(),
            pattern_id: "jpeg_soi".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = carved.expect("expected jpeg");
        assert!(carved.validated);
        assert!(carved.size >= 10);
    }

    #[test]
    fn rejects_jpeg_with_invalid_first_marker() {
        let temp_dir = tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("mkdir");

        let data = make_test_jpeg(0x83);
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
        let handler = JpegCarveHandler::new("jpg".to_string(), 10, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "jpeg".to_string(),
            pattern_id: "jpeg_soi".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        assert!(carved.is_none(), "expected invalid marker to be rejected");
    }
}
