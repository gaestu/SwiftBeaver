//! RTF carving handler.
//!
//! Uses brace depth tracking with escape and \bin handling to find document end.

use crate::carve::{
    CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext, PendingCarve,
    PreValidation, output_path,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

pub struct RtfCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl RtfCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for RtfCarveHandler {
    fn file_type(&self) -> &str {
        "rtf"
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
        if &buf != b"{\\rtf" {
            return Ok(PreValidation::Reject("rtf header mismatch".to_string()));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut stream = CarveStream::new(ctx, hit.global_offset, self.max_size, full_path.clone());

        let mut validated = false;
        let mut truncated = false;
        let mut errors = Vec::new();

        let result: Result<u64, CarveError> = (|| {
            let header = stream.read_exact(5)?;
            if &header != b"{\\rtf" {
                return Err(CarveError::Invalid("rtf header mismatch".to_string()));
            }

            let mut depth: i32 = 1;
            let mut in_control = false;
            let mut control_buf = Vec::new();
            let mut bin_len: usize = 0;
            let mut pending: Option<u8> = None;

            loop {
                let byte = if let Some(b) = pending.take() {
                    b
                } else {
                    let buf = stream.read_exact(1)?;
                    buf[0]
                };

                if bin_len > 0 {
                    bin_len -= 1;
                    continue;
                }

                if in_control {
                    if control_buf.is_empty() && (byte == b'{' || byte == b'}' || byte == b'\\') {
                        in_control = false;
                        continue;
                    }

                    if byte.is_ascii_alphabetic() {
                        control_buf.push(byte);
                        continue;
                    }

                    if !control_buf.is_empty() && control_buf == b"bin" && byte.is_ascii_digit() {
                        bin_len = bin_len
                            .saturating_mul(10)
                            .saturating_add((byte - b'0') as usize);
                        continue;
                    }

                    if control_buf == b"bin" {
                        bin_len = bin_len.max(0);
                    }

                    in_control = false;
                    control_buf.clear();
                    pending = Some(byte);
                    continue;
                }

                if byte == b'\\' {
                    in_control = true;
                    control_buf.clear();
                    bin_len = 0;
                    continue;
                }

                if byte == b'{' {
                    depth += 1;
                } else if byte == b'}' {
                    depth -= 1;
                    if depth <= 0 {
                        validated = true;
                        break;
                    }
                }
            }

            Ok(stream.bytes_written())
        })();

        if let Err(err) = result {
            match err {
                CarveError::Truncated | CarveError::Eof => {
                    truncated = true;
                    errors.push(err.to_string());
                }
                CarveError::Invalid(_) => {
                    stream.discard();
                    return Ok(None);
                }
                other => return Err(other),
            }
        }

        let (size, md5_hex, sha256_hex, mut writer) = stream.finalize()?;

        if size < self.min_size {
            writer.discard();
            return Ok(None);
        }

        if self.max_size > 0 && size >= self.max_size {
            truncated = true;
            if !errors.iter().any(|e| e.contains("max_size")) {
                errors.push("max_size reached".to_string());
            }
        }

        let global_end = if size == 0 {
            hit.global_offset
        } else {
            hit.global_offset + size - 1
        };

        Ok(Some(PendingCarve::new(
            CarvedFile {
                run_id: ctx.run_id.to_string(),
                file_type: self.file_type().to_string(),
                path: rel_path,
                extension: self.extension.clone(),
                global_start: hit.global_offset,
                global_end,
                size,
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
    use super::RtfCarveHandler;
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
    fn carves_minimal_rtf() {
        let data = b"{\\rtf1 Hello}".to_vec();
        let evidence = SliceEvidence { data: data.clone() };
        let handler = RtfCarveHandler::new("rtf".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "rtf".to_string(),
            pattern_id: "rtf_header".to_string(),
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
            hash_config: crate::hash::HashConfig::default(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        let carved = carved.expect("carved").flush().expect("flush");
        assert!(carved.validated);
        assert_eq!(carved.size, data.len() as u64);
    }
}
