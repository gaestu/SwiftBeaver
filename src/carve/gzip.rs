//! GZIP carving handler.
//!
//! GZIP streams do not include a reliable end marker without parsing deflate,
//! so we use a best-effort scan for the next gzip header or EOF.

use std::fs::File;
use std::io::{self, Seek};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PendingCarve, PreValidation,
    create_hashers, finalize_hashers, output_path, write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;
use flate2::read::GzDecoder;

const GZIP_MAGIC: [u8; 3] = [0x1F, 0x8B, 0x08];

pub struct GzipCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl GzipCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for GzipCarveHandler {
    fn file_type(&self) -> &str {
        "gzip"
    }

    fn is_fast(&self) -> bool {
        true
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 3];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        if buf != GZIP_MAGIC {
            return Ok(PreValidation::Reject("gzip magic mismatch".to_string()));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let header_len = match parse_gzip_header(ctx, hit.global_offset) {
            Ok(len) => len,
            Err(CarveError::Invalid(_)) => return Ok(None),
            Err(err) => return Err(err),
        };

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let (mut md5, mut sha256) = create_hashers(&ctx.hash_config);

        let mut truncated = false;
        let mut errors = Vec::new();

        let max_end = if self.max_size > 0 {
            hit.global_offset.saturating_add(self.max_size)
        } else {
            u64::MAX
        };

        let mut end_offset = None;
        let mut offset = hit.global_offset.saturating_add(header_len);
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
            let mut search_start = 0usize;
            while let Some(pos) = find_pattern(&search_buf[search_start..], &GZIP_MAGIC) {
                let absolute = search_start + pos;
                let gzip_offset = offset
                    .saturating_sub(carry.len() as u64)
                    .saturating_add(absolute as u64);
                if gzip_offset > hit.global_offset {
                    end_offset = Some(gzip_offset);
                    break;
                }
                search_start = absolute + 1;
            }
            if end_offset.is_some() {
                break;
            }

            offset = offset.saturating_add(buf.len() as u64);
            if buf.len() >= GZIP_MAGIC.len() - 1 {
                carry = buf[buf.len() - (GZIP_MAGIC.len() - 1)..].to_vec();
            } else {
                carry = buf;
            }
        }

        let end_offset = end_offset.unwrap_or(max_end);
        if self.max_size > 0 && end_offset >= max_end {
            truncated = true;
            errors.push("max_size reached before gzip end".to_string());
        }

        let (written, eof_truncated, mut writer) = write_range(
            ctx,
            hit.global_offset,
            end_offset,
            &full_path,
            md5.as_mut(),
            sha256.as_mut(),
        )?;
        if eof_truncated {
            truncated = true;
            if !errors.iter().any(|e| e.contains("eof")) {
                errors.push("eof before gzip end".to_string());
            }
        }

        if written < self.min_size {
            writer.discard();
            return Ok(None);
        }

        writer.flush_to_disk()?;

        if !validate_gzip_member(&full_path, written)? {
            writer.discard();
            return Ok(None);
        }

        let (md5_hex, sha256_hex) = finalize_hashers(md5, sha256);
        let global_end = if written == 0 {
            hit.global_offset
        } else {
            hit.global_offset + written - 1
        };

        Ok(Some(PendingCarve::new(
            CarvedFile {
                run_id: ctx.run_id.to_string(),
                file_type: self.file_type().to_string(),
                path: rel_path,
                extension: self.extension.clone(),
                global_start: hit.global_offset,
                global_end,
                size: written,
                md5: md5_hex,
                sha256: sha256_hex,
                validated: true,
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

fn parse_gzip_header(ctx: &ExtractionContext, offset: u64) -> Result<u64, CarveError> {
    let fixed = read_exact_at(ctx, offset, 10)
        .ok_or_else(|| CarveError::Invalid("gzip header too short".to_string()))?;
    if fixed[0..3] != GZIP_MAGIC {
        return Err(CarveError::Invalid("gzip magic mismatch".to_string()));
    }
    let method = fixed[2];
    if method != 8 {
        return Err(CarveError::Invalid("gzip method unsupported".to_string()));
    }
    let flags = fixed[3];
    let mut cursor = offset + 10;

    if flags & 0x04 != 0 {
        let extra_len = read_exact_at(ctx, cursor, 2)
            .ok_or_else(|| CarveError::Invalid("gzip extra len missing".to_string()))?;
        let xlen = u16::from_le_bytes([extra_len[0], extra_len[1]]) as u64;
        cursor = cursor.saturating_add(2 + xlen);
    }

    if flags & 0x08 != 0 {
        cursor = skip_cstring(ctx, cursor)?;
    }

    if flags & 0x10 != 0 {
        cursor = skip_cstring(ctx, cursor)?;
    }

    if flags & 0x02 != 0 {
        cursor = cursor.saturating_add(2);
    }

    Ok(cursor.saturating_sub(offset))
}

fn skip_cstring(ctx: &ExtractionContext, mut offset: u64) -> Result<u64, CarveError> {
    let limit = ctx.evidence.len().min(offset.saturating_add(1024 * 1024));
    while offset < limit {
        let byte = read_exact_at(ctx, offset, 1)
            .ok_or_else(|| CarveError::Invalid("gzip string truncated".to_string()))?;
        offset = offset.saturating_add(1);
        if byte[0] == 0 {
            return Ok(offset);
        }
    }
    Err(CarveError::Invalid("gzip string too long".to_string()))
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

fn validate_gzip_member(path: &std::path::Path, expected_size: u64) -> Result<bool, CarveError> {
    let file = File::open(path)?;
    let mut decoder = GzDecoder::new(file);
    if io::copy(&mut decoder, &mut io::sink()).is_err() {
        return Ok(false);
    }

    let mut file = decoder.into_inner();
    let consumed = file.stream_position()?;
    Ok(consumed == expected_size)
}

#[cfg(test)]
mod tests {
    use super::GzipCarveHandler;
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::{EvidenceError, EvidenceSource};
    use crate::scanner::NormalizedHit;
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;
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

    fn minimal_gzip_payload() -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"DATA").expect("write payload");
        encoder.finish().expect("finish gzip")
    }

    #[test]
    fn carves_until_next_gzip_header() {
        let mut data = minimal_gzip_payload();
        let second = minimal_gzip_payload();
        data.extend_from_slice(&second);

        let evidence = SliceEvidence { data: data.clone() };
        let handler = GzipCarveHandler::new("gz".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "gzip".to_string(),
            pattern_id: "gzip_header".to_string(),
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
        assert_eq!(carved.size as usize, minimal_gzip_payload().len());
    }

    #[test]
    fn rejects_invalid_gzip_stream() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0x1F, 0x8B, 0x08, 0x00]);
        data.extend_from_slice(&[0x00; 6]);
        data.extend_from_slice(b"NOT_A_VALID_STREAM");

        let evidence = SliceEvidence { data };
        let handler = GzipCarveHandler::new("gz".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "gzip".to_string(),
            pattern_id: "gzip_header".to_string(),
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
        assert!(carved.is_none());
    }
}
