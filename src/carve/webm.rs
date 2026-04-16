//! WEBM/Matroska carving handler.
//!
//! Uses EBML headers to validate and reads the Segment element size when known.

use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PreValidation, output_path,
    write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const EBML_ID: u64 = 0x1A45DFA3;
const SEGMENT_ID: u64 = 0x18538067;
const DOCTYPE_ID: u64 = 0x4282;

pub struct WebmCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl WebmCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for WebmCarveHandler {
    fn file_type(&self) -> &str {
        "webm"
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
        if buf != [0x1A, 0x45, 0xDF, 0xA3] {
            return Ok(PreValidation::Reject("ebml header ID mismatch".to_string()));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let (ebml_id, ebml_id_len) = read_vint_id(ctx, hit.global_offset)
            .ok_or_else(|| CarveError::Invalid("ebml id missing".to_string()))?;
        if ebml_id != EBML_ID {
            return Ok(None);
        }
        let (ebml_size, ebml_size_len, _) =
            read_vint_size(ctx, hit.global_offset + ebml_id_len as u64)
                .ok_or_else(|| CarveError::Invalid("ebml size missing".to_string()))?;
        let ebml_header_start = hit.global_offset + ebml_id_len as u64 + ebml_size_len as u64;
        let ebml_header = read_exact_at(ctx, ebml_header_start, ebml_size as usize)
            .ok_or_else(|| CarveError::Invalid("ebml header truncated".to_string()))?;

        let doc_type = parse_doctype(&ebml_header).unwrap_or_default();
        if doc_type != "webm" && doc_type != "matroska" {
            return Ok(None);
        }

        let mut offset = ebml_header_start.saturating_add(ebml_size);
        let mut segment_size = None;
        let mut segment_start = None;

        let scan_limit = offset.saturating_add(1024 * 1024);
        while offset < scan_limit {
            let (id, id_len) = match read_vint_id(ctx, offset) {
                Some(v) => v,
                None => break,
            };
            let (size, size_len, unknown) = match read_vint_size(ctx, offset + id_len as u64) {
                Some(v) => v,
                None => break,
            };

            let data_start = offset + id_len as u64 + size_len as u64;
            if id == SEGMENT_ID {
                segment_start = Some(data_start);
                if !unknown {
                    segment_size = Some(size);
                }
                break;
            }

            offset = data_start.saturating_add(size);
        }

        let segment_start =
            segment_start.ok_or_else(|| CarveError::Invalid("segment missing".to_string()))?;
        let total_end = if let Some(size) = segment_size {
            segment_start.saturating_add(size)
        } else if self.max_size > 0 {
            hit.global_offset.saturating_add(self.max_size)
        } else {
            ctx.evidence.len()
        };

        let mut total_end = total_end;
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

        let mut truncated = eof_truncated;
        if self.max_size > 0 && total_end >= hit.global_offset + self.max_size {
            truncated = true;
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
            validated: !truncated && segment_size.is_some(),
            truncated,
            errors: Vec::new(),
            pattern_id: Some(hit.pattern_id.clone()),
        }))
    }
}

fn parse_doctype(buf: &[u8]) -> Option<String> {
    let mut idx = 0usize;
    while idx < buf.len() {
        let (id, id_len) = read_vint_id_from(buf, idx)?;
        let (size, size_len, _) = read_vint_size_from(buf, idx + id_len)?;
        let data_start = idx + id_len + size_len;
        if data_start + size > buf.len() {
            return None;
        }
        if id == DOCTYPE_ID {
            let value = &buf[data_start..data_start + size];
            return Some(String::from_utf8_lossy(value).to_ascii_lowercase());
        }
        idx = data_start + size;
    }
    None
}

fn read_vint_id(ctx: &ExtractionContext, offset: u64) -> Option<(u64, usize)> {
    let first = read_exact_at(ctx, offset, 1)?[0];
    let len = 1 + first.leading_zeros() as usize;
    if len == 0 || len > 8 {
        return None;
    }
    let bytes = read_exact_at(ctx, offset, len)?;
    let mut value = 0u64;
    for b in bytes {
        value = (value << 8) | b as u64;
    }
    Some((value, len))
}

fn read_vint_size(ctx: &ExtractionContext, offset: u64) -> Option<(u64, usize, bool)> {
    let first = read_exact_at(ctx, offset, 1)?[0];
    let len = 1 + first.leading_zeros() as usize;
    if len == 0 || len > 8 {
        return None;
    }
    let mask = 1u8 << (8 - len);
    let mut value = (first & (mask - 1)) as u64;
    let bytes = read_exact_at(ctx, offset + 1, len - 1)?;
    for b in bytes {
        value = (value << 8) | b as u64;
    }
    let unknown = value == (1u64 << (7 * len)) - 1;
    Some((value, len, unknown))
}

fn read_vint_id_from(buf: &[u8], offset: usize) -> Option<(u64, usize)> {
    if offset >= buf.len() {
        return None;
    }
    let first = buf[offset];
    let len = 1 + first.leading_zeros() as usize;
    if len == 0 || len > 8 || offset + len > buf.len() {
        return None;
    }
    let mut value = 0u64;
    for b in &buf[offset..offset + len] {
        value = (value << 8) | *b as u64;
    }
    Some((value, len))
}

fn read_vint_size_from(buf: &[u8], offset: usize) -> Option<(usize, usize, bool)> {
    if offset >= buf.len() {
        return None;
    }
    let first = buf[offset];
    let len = 1 + first.leading_zeros() as usize;
    if len == 0 || len > 8 || offset + len > buf.len() {
        return None;
    }
    let mask = 1u8 << (8 - len);
    let mut value = (first & (mask - 1)) as usize;
    for b in &buf[offset + 1..offset + len] {
        value = (value << 8) | *b as usize;
    }
    let unknown = value == (1usize << (7 * len)) - 1;
    Some((value, len, unknown))
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
    use super::WebmCarveHandler;
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::RawFileSource;
    use crate::scanner::NormalizedHit;
    use tempfile::tempdir;

    fn minimal_webm() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&[0x1A, 0x45, 0xDF, 0xA3]);
        data.push(0x87); // header size 7
        data.extend_from_slice(&[0x42, 0x82]);
        data.push(0x84);
        data.extend_from_slice(b"webm");
        data.extend_from_slice(&[0x18, 0x53, 0x80, 0x67]);
        data.push(0x80); // segment size 0
        data
    }

    #[test]
    fn carves_minimal_webm() {
        let temp_dir = tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let data = minimal_webm();
        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &data).expect("write webm");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
        };
        let handler = WebmCarveHandler::new("webm".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "webm".to_string(),
            pattern_id: "webm_ebml".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = carved.expect("carved");
        assert_eq!(carved.size, data.len() as u64);
        assert!(carved.validated);
    }
}
