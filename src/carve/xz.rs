//! XZ stream carving handler.
//!
//! We validate the header magic and scan for a structurally consistent footer and Index.

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PendingCarve, PreValidation,
    create_hashers, finalize_hashers, output_path, write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const XZ_MAGIC: [u8; 6] = [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00];
const XZ_FOOTER_MAGIC: [u8; 2] = [0x59, 0x5A];
const XZ_STREAM_HEADER_LEN: u64 = 12;
const XZ_STREAM_FOOTER_LEN: u64 = 12;
const XZ_CRC32_LEN: usize = 4;
const MAX_XZ_INDEX_SIZE: u64 = 16 * 1024 * 1024;
const MAX_XZ_INDEX_VALIDATION_BYTES: u64 = 64 * 1024 * 1024;
const MAX_XZ_INDEX_RECORDS: u64 = 65_536;

pub struct XzCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl XzCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for XzCarveHandler {
    fn file_type(&self) -> &str {
        "xz"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 6];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        if buf != XZ_MAGIC {
            return Ok(PreValidation::Reject("xz magic mismatch".to_string()));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let Some(header) = read_exact_at(ctx, hit.global_offset, 12)? else {
            return Ok(None);
        };
        if header[0..6] != XZ_MAGIC {
            return Ok(None);
        }
        let header_flags = [header[6], header[7]];
        if !valid_stream_flags(&header_flags) {
            return Ok(None);
        }
        let header_crc = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
        let computed = crc32(&header_flags);
        if header_crc != computed {
            return Ok(None);
        }

        let max_end = if self.max_size > 0 {
            hit.global_offset.saturating_add(self.max_size)
        } else {
            u64::MAX
        };

        let mut end_offset = None;
        let mut offset = hit.global_offset + XZ_STREAM_HEADER_LEN;
        let mut carry: Vec<u8> = Vec::new();
        let buf_size = 64 * 1024;
        let mut search_buf: Vec<u8> = Vec::with_capacity(buf_size + XZ_FOOTER_MAGIC.len() - 1);
        let mut validation_budget = XzValidationBudget::new(MAX_XZ_INDEX_VALIDATION_BYTES);

        while offset < max_end {
            let remaining = (max_end - offset).min(buf_size as u64) as usize;
            let mut buf = vec![0u8; remaining];
            let n = ctx
                .evidence
                .read_at(offset, &mut buf)
                .map_err(|e| CarveError::Evidence(e.to_string()))?;
            if n == 0 {
                break;
            }
            buf.truncate(n);

            search_buf.clear();
            search_buf.extend_from_slice(&carry);
            search_buf.extend_from_slice(&buf);
            let mut search_start = 0usize;
            while let Some(pos) = find_pattern(&search_buf[search_start..], &XZ_FOOTER_MAGIC) {
                let absolute = search_start + pos;
                let footer_end = offset
                    .saturating_sub(carry.len() as u64)
                    .saturating_add(absolute as u64)
                    .saturating_add(2);
                if footer_end < hit.global_offset + XZ_STREAM_HEADER_LEN {
                    search_start = absolute + 1;
                    continue;
                }
                let footer_start = footer_end.saturating_sub(XZ_STREAM_FOOTER_LEN);
                if footer_start <= hit.global_offset {
                    search_start = absolute + 1;
                    continue;
                }
                if !validation_budget.charge_footer_candidate() {
                    return Ok(None);
                }
                if let Some(footer) = read_exact_at(ctx, footer_start, 12)? {
                    if is_valid_xz_footer_and_index(
                        ctx,
                        hit.global_offset,
                        footer_start,
                        &header_flags,
                        &footer,
                        &mut validation_budget,
                    )? {
                        end_offset = Some(footer_end);
                        break;
                    }
                    if validation_budget.exhausted() {
                        return Ok(None);
                    }
                }
                search_start = absolute + 1;
            }
            if end_offset.is_some() {
                break;
            }

            offset = offset.saturating_add(buf.len() as u64);
            if buf.len() >= XZ_FOOTER_MAGIC.len() - 1 {
                carry = buf[buf.len() - (XZ_FOOTER_MAGIC.len() - 1)..].to_vec();
            } else {
                carry = buf;
            }
        }

        let Some(end_offset) = end_offset else {
            return Ok(None);
        };

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let (mut md5, mut sha256) = create_hashers(&ctx.hash_config);

        let mut truncated = false;
        let mut errors: Vec<String> = Vec::new();

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
                errors.push("eof before xz end".to_string());
            }
        }

        if written < self.min_size {
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
                validated: !truncated,
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

fn valid_stream_flags(flags: &[u8; 2]) -> bool {
    flags[0] == 0 && matches!(flags[1], 0x00 | 0x01 | 0x04 | 0x0A)
}

fn stream_check_size(flags: &[u8; 2]) -> Option<u64> {
    match flags[1] {
        0x00 => Some(0),
        0x01 => Some(4),
        0x04 => Some(8),
        0x0A => Some(32),
        _ => None,
    }
}

struct XzValidationBudget {
    remaining_work_bytes: u64,
    exhausted: bool,
}

impl XzValidationBudget {
    fn new(remaining_work_bytes: u64) -> Self {
        Self {
            remaining_work_bytes,
            exhausted: false,
        }
    }

    fn charge_index_read(&mut self, bytes: u64) -> bool {
        self.charge(bytes)
    }

    fn charge_footer_candidate(&mut self) -> bool {
        self.charge(128)
    }

    fn charge_block_record(&mut self, header_size: u64, padding_size: u64) -> bool {
        self.charge(
            256u64
                .saturating_add(header_size)
                .saturating_add(padding_size),
        )
    }

    fn charge(&mut self, bytes: u64) -> bool {
        if bytes > self.remaining_work_bytes {
            self.exhausted = true;
            return false;
        }
        self.remaining_work_bytes -= bytes;
        true
    }

    fn exhausted(&self) -> bool {
        self.exhausted
    }
}

fn is_valid_xz_footer_and_index(
    ctx: &ExtractionContext,
    stream_start: u64,
    footer_start: u64,
    header_flags: &[u8; 2],
    footer: &[u8],
    validation_budget: &mut XzValidationBudget,
) -> Result<bool, CarveError> {
    if footer.len() != XZ_STREAM_FOOTER_LEN as usize || footer[10..12] != XZ_FOOTER_MAGIC {
        return Ok(false);
    }

    let footer_crc = u32::from_le_bytes([footer[0], footer[1], footer[2], footer[3]]);
    if footer_crc != crc32(&footer[4..10]) {
        return Ok(false);
    }

    let footer_flags = [footer[8], footer[9]];
    if &footer_flags != header_flags || !valid_stream_flags(&footer_flags) {
        return Ok(false);
    }

    let Some(check_size) = stream_check_size(header_flags) else {
        return Ok(false);
    };

    let backward_size_stored = u32::from_le_bytes([footer[4], footer[5], footer[6], footer[7]]);
    let index_size = u64::from(backward_size_stored)
        .saturating_add(1)
        .saturating_mul(4);
    if !(8..=MAX_XZ_INDEX_SIZE).contains(&index_size) || index_size % 4 != 0 {
        return Ok(false);
    }

    let header_end = stream_start.saturating_add(XZ_STREAM_HEADER_LEN);
    let Some(index_start) = footer_start.checked_sub(index_size) else {
        return Ok(false);
    };
    if index_start < header_end {
        return Ok(false);
    }

    let block_area_size = index_start - header_end;
    if !block_area_size.is_multiple_of(4) {
        return Ok(false);
    }

    if !validation_budget.charge_index_read(index_size) {
        return Ok(false);
    }

    let Ok(index_len) = usize::try_from(index_size) else {
        return Ok(false);
    };
    let Some(index) = read_exact_at(ctx, index_start, index_len)? else {
        return Ok(false);
    };

    validate_xz_index(
        ctx,
        &index,
        header_end,
        block_area_size,
        check_size,
        validation_budget,
    )
}

fn validate_xz_index(
    ctx: &ExtractionContext,
    index: &[u8],
    blocks_start: u64,
    block_area_size: u64,
    check_size: u64,
    validation_budget: &mut XzValidationBudget,
) -> Result<bool, CarveError> {
    if index.len() < 8 || !index.len().is_multiple_of(4) || index[0] != 0 {
        return Ok(false);
    }

    let crc_start = index.len() - XZ_CRC32_LEN;
    let stored_crc = u32::from_le_bytes([
        index[crc_start],
        index[crc_start + 1],
        index[crc_start + 2],
        index[crc_start + 3],
    ]);
    if stored_crc != crc32(&index[..crc_start]) {
        return Ok(false);
    }

    let Some((record_count, mut pos)) = read_vli(index, 1, crc_start) else {
        return Ok(false);
    };
    if record_count > MAX_XZ_INDEX_RECORDS {
        return Ok(false);
    }

    let mut padded_blocks_size = 0u64;
    let mut block_offset = blocks_start;
    for _ in 0..record_count {
        let Some((unpadded_size, next_pos)) = read_vli(index, pos, crc_start) else {
            return Ok(false);
        };
        if unpadded_size == 0 {
            return Ok(false);
        }
        let Some(padded_size) = round_up_to_4(unpadded_size) else {
            return Ok(false);
        };
        let Some(total) = padded_blocks_size.checked_add(padded_size) else {
            return Ok(false);
        };
        if total > block_area_size {
            return Ok(false);
        }
        padded_blocks_size = total;
        pos = next_pos;

        let Some((uncompressed_size, next_pos)) = read_vli(index, pos, crc_start) else {
            return Ok(false);
        };
        pos = next_pos;

        if !validate_xz_block(
            ctx,
            block_offset,
            unpadded_size,
            padded_size,
            uncompressed_size,
            check_size,
            validation_budget,
        )? {
            return Ok(false);
        }

        let Some(next_block_offset) = block_offset.checked_add(padded_size) else {
            return Ok(false);
        };
        block_offset = next_block_offset;
    }

    if pos > crc_start || index[pos..crc_start].iter().any(|&b| b != 0) {
        return Ok(false);
    }

    Ok(padded_blocks_size == block_area_size)
}

fn validate_xz_block(
    ctx: &ExtractionContext,
    block_offset: u64,
    unpadded_size: u64,
    padded_size: u64,
    uncompressed_size: u64,
    check_size: u64,
    validation_budget: &mut XzValidationBudget,
) -> Result<bool, CarveError> {
    let Some(size_byte) = read_exact_at(ctx, block_offset, 1)? else {
        return Ok(false);
    };
    let header_size = u64::from(size_byte[0]).saturating_add(1).saturating_mul(4);
    if header_size < 8 || header_size > unpadded_size {
        return Ok(false);
    }

    let header_len = header_size as usize;
    let Some(header) = read_exact_at(ctx, block_offset, header_len)? else {
        return Ok(false);
    };
    if header[0] != size_byte[0] {
        return Ok(false);
    }

    let crc_start = header.len() - XZ_CRC32_LEN;
    let stored_crc = u32::from_le_bytes([
        header[crc_start],
        header[crc_start + 1],
        header[crc_start + 2],
        header[crc_start + 3],
    ]);
    if stored_crc != crc32(&header[..crc_start]) {
        return Ok(false);
    }

    let flags = header[1];
    if flags & 0x3C != 0 {
        return Ok(false);
    }

    let filter_count = usize::from((flags & 0x03) + 1);
    let has_compressed_size = flags & 0x40 != 0;
    let has_uncompressed_size = flags & 0x80 != 0;
    let mut pos = 2usize;

    let compressed_size = if has_compressed_size {
        let Some((size, next_pos)) = read_vli(&header, pos, crc_start) else {
            return Ok(false);
        };
        pos = next_pos;
        Some(size)
    } else {
        None
    };

    if has_uncompressed_size {
        let Some((size, next_pos)) = read_vli(&header, pos, crc_start) else {
            return Ok(false);
        };
        if size != uncompressed_size {
            return Ok(false);
        }
        pos = next_pos;
    }

    for _ in 0..filter_count {
        let Some((_filter_id, next_pos)) = read_vli(&header, pos, crc_start) else {
            return Ok(false);
        };
        pos = next_pos;

        let Some((properties_size, next_pos)) = read_vli(&header, pos, crc_start) else {
            return Ok(false);
        };
        pos = next_pos;

        let Ok(properties_len) = usize::try_from(properties_size) else {
            return Ok(false);
        };
        let Some(next_pos) = pos.checked_add(properties_len) else {
            return Ok(false);
        };
        if next_pos > crc_start {
            return Ok(false);
        }
        pos = next_pos;
    }

    if header[pos..crc_start].iter().any(|&b| b != 0) {
        return Ok(false);
    }

    let Some(payload_and_check_size) = unpadded_size.checked_sub(header_size) else {
        return Ok(false);
    };
    let Some(expected_compressed_size) = payload_and_check_size.checked_sub(check_size) else {
        return Ok(false);
    };
    if let Some(size) = compressed_size
        && size != expected_compressed_size
    {
        return Ok(false);
    }

    let padding_len = padded_size - unpadded_size;
    if !validation_budget.charge_block_record(header_size, padding_len) {
        return Ok(false);
    }
    if padding_len > 0 {
        let Some(padding_offset) = block_offset
            .checked_add(header_size)
            .and_then(|offset| offset.checked_add(expected_compressed_size))
        else {
            return Ok(false);
        };
        let Some(padding) = read_exact_at(ctx, padding_offset, padding_len as usize)? else {
            return Ok(false);
        };
        if padding.iter().any(|&b| b != 0) {
            return Ok(false);
        }
    }

    Ok(true)
}

fn read_vli(bytes: &[u8], start: usize, limit: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    let mut pos = start;

    for byte_index in 0..9 {
        if pos >= limit {
            return None;
        }
        let byte = bytes[pos];
        pos += 1;

        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            if byte_index > 0 && byte == 0 {
                return None;
            }
            return Some((value, pos));
        }
        shift += 7;
    }

    None
}

fn round_up_to_4(value: u64) -> Option<u64> {
    value.checked_add(3).map(|v| v & !3)
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

fn read_exact_at(
    ctx: &ExtractionContext,
    offset: u64,
    len: usize,
) -> Result<Option<Vec<u8>>, CarveError> {
    let mut buf = vec![0u8; len];
    let n = ctx.evidence.read_at(offset, &mut buf).map_err(|e| {
        CarveError::Evidence(format!(
            "failed to read xz bytes at offset {offset} length {len}: {e}"
        ))
    })?;
    if n < len {
        return Ok(None);
    }
    Ok(Some(buf))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320u32 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::{XzCarveHandler, crc32};
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

    fn minimal_xz() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]);
        let flags = [0x00, 0x00];
        out.extend_from_slice(&flags);
        out.extend_from_slice(&crc32(&flags).to_le_bytes());

        let index_prefix = [0x00, 0x00, 0x00, 0x00];
        out.extend_from_slice(&index_prefix);
        out.extend_from_slice(&crc32(&index_prefix).to_le_bytes());

        let backward_size = [0x01, 0x00, 0x00, 0x00];
        let stream_flags = [0x00, 0x00];
        let mut footer = Vec::new();
        let footer_crc = crc32(&[
            backward_size[0],
            backward_size[1],
            backward_size[2],
            backward_size[3],
            stream_flags[0],
            stream_flags[1],
        ]);
        footer.extend_from_slice(&footer_crc.to_le_bytes());
        footer.extend_from_slice(&backward_size);
        footer.extend_from_slice(&stream_flags);
        footer.extend_from_slice(&[0x59, 0x5A]);
        out.extend_from_slice(&footer);
        out
    }

    fn footer(backward_size: [u8; 4], stream_flags: [u8; 2]) -> Vec<u8> {
        let mut footer = Vec::new();
        let footer_crc = crc32(&[
            backward_size[0],
            backward_size[1],
            backward_size[2],
            backward_size[3],
            stream_flags[0],
            stream_flags[1],
        ]);
        footer.extend_from_slice(&footer_crc.to_le_bytes());
        footer.extend_from_slice(&backward_size);
        footer.extend_from_slice(&stream_flags);
        footer.extend_from_slice(&[0x59, 0x5A]);
        footer
    }

    fn index_with_one_record(unpadded_size: u8, uncompressed_size: u8) -> Vec<u8> {
        let mut index = vec![0x00, 0x01, unpadded_size, uncompressed_size];
        while !index.len().is_multiple_of(4) {
            index.push(0);
        }
        let crc = crc32(&index);
        index.extend_from_slice(&crc.to_le_bytes());
        index
    }

    fn hit() -> NormalizedHit {
        NormalizedHit {
            global_offset: 0,
            file_type_id: "xz".to_string(),
            pattern_id: "xz_header".to_string(),
            chunk_data: None,
            chunk_start: 0,
        }
    }

    fn ctx<'a>(
        evidence: &'a SliceEvidence,
        output_root: &'a std::path::Path,
    ) -> ExtractionContext<'a> {
        ExtractionContext {
            run_id: "test",
            output_root,
            evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        }
    }

    #[test]
    fn carves_minimal_xz_with_footer() {
        let data = minimal_xz();
        let evidence = SliceEvidence { data: data.clone() };
        let handler = XzCarveHandler::new("xz".to_string(), 0, 0);
        let dir = tempdir().expect("tempdir");
        let ctx = ctx(&evidence, dir.path());

        let carved = handler.process_hit(&hit(), &ctx).expect("process");
        let carved = carved.expect("carved").flush().expect("flush");
        assert!(carved.validated);
        assert_eq!(carved.size, data.len() as u64);
    }

    #[test]
    fn carves_real_xz_fixture_with_index_record() {
        let data =
            include_bytes!("../../tests/golden_image/samples/archives/xz/test.txt.xz").to_vec();
        let evidence = SliceEvidence { data: data.clone() };
        let handler = XzCarveHandler::new("xz".to_string(), 0, 0);
        let dir = tempdir().expect("tempdir");
        let ctx = ctx(&evidence, dir.path());

        let carved = handler.process_hit(&hit(), &ctx).expect("process");
        let carved = carved.expect("carved").flush().expect("flush");
        assert!(carved.validated);
        assert_eq!(carved.size, data.len() as u64);
    }

    #[test]
    fn rejects_valid_header_without_footer() {
        let mut data = minimal_xz();
        data.truncate(12);
        data.extend_from_slice(&[0xAA; 64]);
        let evidence = SliceEvidence { data };
        let handler = XzCarveHandler::new("xz".to_string(), 0, 0);
        let dir = tempdir().expect("tempdir");
        let ctx = ctx(&evidence, dir.path());

        let carved = handler.process_hit(&hit(), &ctx).expect("process");
        assert!(carved.is_none());
        assert!(!dir.path().join("xz").exists());
    }

    #[test]
    fn rejects_max_size_without_persisting_fallback() {
        let mut data = minimal_xz();
        data.truncate(12);
        data.extend_from_slice(&[0xAA; 128]);
        let evidence = SliceEvidence { data };
        let handler = XzCarveHandler::new("xz".to_string(), 0, 32);
        let dir = tempdir().expect("tempdir");
        let ctx = ctx(&evidence, dir.path());

        let carved = handler.process_hit(&hit(), &ctx).expect("process");
        assert!(carved.is_none());
        assert!(!dir.path().join("xz").exists());
    }

    #[test]
    fn rejects_footer_with_mismatched_stream_flags() {
        let mut data = minimal_xz();
        data.truncate(data.len() - 12);
        data.extend_from_slice(&footer([0x01, 0x00, 0x00, 0x00], [0x00, 0x04]));
        let evidence = SliceEvidence { data };
        let handler = XzCarveHandler::new("xz".to_string(), 0, 0);
        let dir = tempdir().expect("tempdir");
        let ctx = ctx(&evidence, dir.path());

        let carved = handler.process_hit(&hit(), &ctx).expect("process");
        assert!(carved.is_none());
        assert!(!dir.path().join("xz").exists());
    }

    #[test]
    fn rejects_footer_crc_match_with_invalid_index() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]);
        let flags = [0x00, 0x00];
        data.extend_from_slice(&flags);
        data.extend_from_slice(&crc32(&flags).to_le_bytes());
        data.extend_from_slice(&[0u8; 4]);
        data.extend_from_slice(&footer([0x00, 0x00, 0x00, 0x00], flags));
        let evidence = SliceEvidence { data };
        let handler = XzCarveHandler::new("xz".to_string(), 0, 0);
        let dir = tempdir().expect("tempdir");
        let ctx = ctx(&evidence, dir.path());

        let carved = handler.process_hit(&hit(), &ctx).expect("process");
        assert!(carved.is_none());
        assert!(!dir.path().join("xz").exists());
    }

    #[test]
    fn rejects_index_consistent_candidate_with_invalid_block_header() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]);
        let flags = [0x00, 0x00];
        data.extend_from_slice(&flags);
        data.extend_from_slice(&crc32(&flags).to_le_bytes());
        data.extend_from_slice(&[0u8; 4]);
        data.extend_from_slice(&index_with_one_record(4, 0));
        data.extend_from_slice(&footer([0x01, 0x00, 0x00, 0x00], flags));
        let evidence = SliceEvidence { data };
        let handler = XzCarveHandler::new("xz".to_string(), 0, 0);
        let dir = tempdir().expect("tempdir");
        let ctx = ctx(&evidence, dir.path());

        let carved = handler.process_hit(&hit(), &ctx).expect("process");
        assert!(carved.is_none());
        assert!(!dir.path().join("xz").exists());
    }
}
