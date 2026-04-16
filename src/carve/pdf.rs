use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, DeferredWriter, ExtractionContext, PreValidation,
    output_path,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const PDF_HEADER: &[u8] = b"%PDF-";
const PDF_EOF: &[u8] = b"%%EOF";

pub struct PdfCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl PdfCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for PdfCarveHandler {
    fn file_type(&self) -> &str {
        "pdf"
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
        if &buf[..] != PDF_HEADER {
            return Ok(PreValidation::Reject("pdf header mismatch".to_string()));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut writer = DeferredWriter::new(full_path.clone(), ctx.deferred_buffer_bytes);
        let mut md5 = md5::Context::new();
        let mut sha256 = Sha256::new();

        let mut offset = hit.global_offset;
        let mut bytes_written = 0u64;
        let mut validated = false;
        let mut truncated = false;
        let mut errors = Vec::new();

        let mut carry: Vec<u8> = Vec::new();
        let buf_size = 64 * 1024;

        loop {
            if self.max_size > 0 && bytes_written >= self.max_size {
                truncated = true;
                errors.push("max_size reached before EOF".to_string());
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
                errors.push("eof before %%EOF".to_string());
                break;
            }
            buf.truncate(n);

            if bytes_written == 0
                && buf.len() >= PDF_HEADER.len()
                && &buf[..PDF_HEADER.len()] != PDF_HEADER
            {
                writer.discard();
                return Ok(None);
            }

            let mut search_buf = carry.clone();
            search_buf.extend_from_slice(&buf);
            if let Some(pos) = find_pattern(&search_buf, PDF_EOF) {
                let write_len = if pos < carry.len() {
                    pos + PDF_EOF.len() - carry.len()
                } else {
                    pos - carry.len() + PDF_EOF.len()
                };

                if write_len > 0 {
                    let slice = &buf[..write_len.min(buf.len())];
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

            carry = if buf.len() >= PDF_EOF.len() - 1 {
                buf[buf.len() - (PDF_EOF.len() - 1)..].to_vec()
            } else {
                buf.clone()
            };
        }

        if validated
            && let Some(next) = read_byte(ctx, hit.global_offset + bytes_written)
            && (next == b'\n' || next == b'\r')
        {
            writer.write_all(&[next])?;
            md5.consume([next]);
            sha256.update([next]);
            bytes_written = bytes_written.saturating_add(1);
            if next == b'\r'
                && let Some(next2) = read_byte(ctx, hit.global_offset + bytes_written)
                && next2 == b'\n'
            {
                writer.write_all(&[next2])?;
                md5.consume([next2]);
                sha256.update([next2]);
                bytes_written = bytes_written.saturating_add(1);
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

fn read_byte(ctx: &ExtractionContext, offset: u64) -> Option<u8> {
    let mut buf = [0u8; 1];
    let n = ctx.evidence.read_at(offset, &mut buf).ok()?;
    if n == 1 { Some(buf[0]) } else { None }
}
