use crate::carve::sqlite_page::{
    deep_validate_btree_page, deep_validate_btree_page_with_header_offset,
};
use crate::carve::sqlite_wal::{
    WAL_FRAME_HEADER_LEN, WAL_HEADER_LEN, WAL_MAGIC_1, WAL_MAGIC_2, parse_wal_header,
};
use crate::carve::{
    CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext, PendingCarve,
    PreValidation, output_path,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// Valid SQLite B-tree page type bytes.
/// 0x02 = index interior, 0x05 = table interior,
/// 0x0A = index leaf, 0x0D = table leaf,
/// 0x00 = free page / overflow page.
const VALID_PAGE_TYPES: [u8; 5] = [0x00, 0x02, 0x05, 0x0A, 0x0D];

pub struct SqliteCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
    max_consecutive_invalid_pages: u32,
    min_valid_page_ratio: f64,
    suppress_wal_frame_lookback_frames: u32,
}

impl SqliteCarveHandler {
    pub fn new(
        extension: String,
        min_size: u64,
        max_size: u64,
        max_consecutive_invalid_pages: u32,
        min_valid_page_ratio: f64,
        suppress_wal_frame_lookback_frames: u32,
    ) -> Self {
        Self {
            extension,
            min_size,
            max_size,
            max_consecutive_invalid_pages,
            min_valid_page_ratio,
            suppress_wal_frame_lookback_frames,
        }
    }
}

impl CarveHandler for SqliteCarveHandler {
    fn file_type(&self) -> &str {
        "sqlite"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 18];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        if &buf[..16] != SQLITE_HEADER.as_slice() {
            return Ok(PreValidation::Reject("sqlite header mismatch".to_string()));
        }
        let page_size_raw = u16::from_be_bytes([buf[16], buf[17]]);
        let page_size = if page_size_raw == 1 {
            65536
        } else {
            page_size_raw as u32
        };
        if !is_valid_page_size(page_size) {
            return Ok(PreValidation::Reject(
                "sqlite page size invalid".to_string(),
            ));
        }
        if let Some(reason) = wal_frame_payload_reason(
            evidence,
            offset,
            page_size,
            self.suppress_wal_frame_lookback_frames,
        )? {
            return Ok(PreValidation::Reject(reason));
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

        let result: Result<(), CarveError> = (|| {
            // --- Read and validate the 100-byte header ---
            let header = stream.read_exact(100)?;
            if &header[..SQLITE_HEADER.len()] != SQLITE_HEADER {
                return Err(CarveError::Invalid("sqlite header mismatch".to_string()));
            }

            let page_size_raw = u16::from_be_bytes([header[16], header[17]]);
            let page_size = if page_size_raw == 1 {
                65536
            } else {
                page_size_raw as u32
            };
            if !is_valid_page_size(page_size) {
                return Err(CarveError::Invalid("sqlite page size invalid".to_string()));
            }

            let page_count = u32::from_be_bytes([header[28], header[29], header[30], header[31]]);
            let total_pages = if page_count == 0 {
                1u64
            } else {
                page_count as u64
            };

            let max_size = if self.max_size > 0 {
                self.max_size
            } else {
                total_pages * page_size as u64
            };
            let max_pages = max_size / page_size as u64;
            let target_pages = total_pages.min(max_pages);

            if target_pages < total_pages {
                truncated = true;
                errors.push("page count limited by max_size".to_string());
            }

            // --- Write remainder of page 1 (already read 100 bytes of header) ---
            let mut page1 = header;
            let page1_remaining = (page_size as u64).saturating_sub(100);
            if page1_remaining > 0 {
                let mut remaining = stream.read_exact(page1_remaining as usize)?;
                page1.append(&mut remaining);
            }

            // --- Page-by-page validation loop for pages 2..N ---
            let mut valid_pages = 1u64; // page 1 (header) is always valid
            let mut total_examined = 1u64;
            let mut consecutive_invalid = 0u32;
            let mut stopped_early = false;
            // Deep b-tree structural validation accounting.
            let mut btree_pages_examined = 0u64;
            let mut btree_pages_failed_struct = 0u64;

            // SQLite page 1 contains the 100-byte database header followed by
            // a normal b-tree page header at byte 100. Validate it so
            // single-page DBs and malformed schema root pages are covered.
            let page1_type = page1.get(100).copied().unwrap_or(0);
            if matches!(page1_type, 0x02 | 0x05 | 0x0A | 0x0D) {
                btree_pages_examined += 1;
                if !deep_validate_btree_page_with_header_offset(&page1, 100) {
                    btree_pages_failed_struct += 1;
                }
            } else {
                btree_pages_examined += 1;
                btree_pages_failed_struct += 1;
            }

            for _page_idx in 1..target_pages {
                // Peek at the first byte of the next page to check page type
                let type_byte = match stream.peek_exact(1) {
                    Ok(buf) => buf[0],
                    Err(CarveError::Eof) => {
                        truncated = true;
                        errors.push("eof during page validation".to_string());
                        break;
                    }
                    Err(CarveError::Truncated) => {
                        truncated = true;
                        errors.push("max_size reached during page validation".to_string());
                        break;
                    }
                    Err(other) => return Err(other),
                };

                let is_valid_type = VALID_PAGE_TYPES.contains(&type_byte);

                if is_valid_type {
                    consecutive_invalid = 0;
                    valid_pages += 1;
                } else {
                    consecutive_invalid += 1;
                }

                total_examined += 1;

                // Check consecutive-failure threshold
                if consecutive_invalid >= self.max_consecutive_invalid_pages {
                    stopped_early = true;
                    errors.push(format!(
                        "stopped at page {}: {} consecutive invalid page types",
                        total_examined, consecutive_invalid
                    ));
                    break;
                }

                // Write the full page (valid or not — preserve evidence up to threshold)
                let page_bytes = match stream.read_exact(page_size as usize) {
                    Ok(buf) => buf,
                    Err(CarveError::Eof) => {
                        truncated = true;
                        errors.push("eof during page read".to_string());
                        break;
                    }
                    Err(CarveError::Truncated) => {
                        truncated = true;
                        errors.push("max_size reached".to_string());
                        break;
                    }
                    Err(other) => return Err(other),
                };

                // Deep b-tree page structural validation. Overflow / freelist
                // pages (type 0x00) have no b-tree layout, so they are
                // accepted by type alone (matches SQLite's own treatment).
                if matches!(type_byte, 0x02 | 0x05 | 0x0A | 0x0D) {
                    btree_pages_examined += 1;
                    if !deep_validate_btree_page(&page_bytes) {
                        btree_pages_failed_struct += 1;
                    }
                }
            }

            // Determine validated flag. Tightened semantics (issue #83):
            //   - magic + page-size header checks must pass (already enforced),
            //   - carving must not have stopped early on the consecutive-invalid threshold,
            //   - valid-page-type ratio must meet the configured minimum,
            //   - every examined b-tree page must pass deep structural validation.
            // Page-type plausibility alone is not sufficient.
            if total_examined > 0 {
                let ratio = valid_pages as f64 / total_examined as f64;
                let ratio_ok = ratio >= self.min_valid_page_ratio;
                let structure_ok = btree_pages_failed_struct == 0;
                validated = ratio_ok && !stopped_early && structure_ok;
                if !structure_ok {
                    errors.push(format!(
                        "deep b-tree validation: {} of {} pages failed structural checks",
                        btree_pages_failed_struct, btree_pages_examined
                    ));
                }
            }

            Ok(())
        })();

        if let Err(err) = result {
            match err {
                CarveError::Truncated | CarveError::Eof => {
                    truncated = true;
                    errors.push(err.to_string());
                }
                CarveError::Invalid(_msg) => {
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

fn is_valid_page_size(page_size: u32) -> bool {
    if !(512..=65536).contains(&page_size) {
        return false;
    }
    page_size.is_power_of_two()
}

/// Determine whether a SQLite header candidate at `offset` actually lies inside
/// a SQLite WAL frame payload. WAL frames embed full database page images,
/// including page 1 (which carries the `SQLite format 3\0` magic), so the raw
/// signature scanner can mistake them for standalone databases.
///
/// Walks back through possible frame boundaries `n = 0..=max_lookback_frames`,
/// computing `candidate_wal_start = offset - 56 - n * (24 + page_size)`. For
/// each candidate the function performs a strict structural check matching the
/// rules used by the `sqlite_wal` carver itself (see `walk_frames` in
/// `src/carve/sqlite_wal.rs`):
///
/// 1. The 32-byte WAL header at `candidate_wal_start` must parse (magic,
///    version, page size, header checksum) and its declared `page_size` must
///    match the SQLite candidate's `page_size`.
/// 2. Every frame from frame 0 through frame `n` must have salts that match
///    the WAL header salts, a non-zero page number, AND a valid rolling
///    frame checksum (8 bytes of frame header + page payload, seeded with the
///    previous frame's or header's checksum).
///
/// Only when all conditions hold is the SQLite carve suppressed. This avoids
/// dropping a legitimate standalone SQLite database simply because unrelated
/// or stale bytes earlier in the image happen to parse as a WAL header at the
/// computed distance — suppression must be at least as strict as the WAL
/// carver's acceptance rules.
///
/// Returns `Ok(Some(reason))` to reject, `Ok(None)` to allow.
fn wal_frame_payload_reason(
    evidence: &dyn EvidenceSource,
    offset: u64,
    page_size: u32,
    max_lookback_frames: u32,
) -> Result<Option<String>, CarveError> {
    use crate::carve::sqlite_wal::wal_checksum_bytes;

    let frame_size = WAL_FRAME_HEADER_LEN.saturating_add(page_size as u64);
    let first_payload_offset = WAL_HEADER_LEN.saturating_add(WAL_FRAME_HEADER_LEN);
    let mut header_buf = [0u8; WAL_HEADER_LEN as usize];
    let mut frame_buf = [0u8; WAL_FRAME_HEADER_LEN as usize];
    let mut page_buf = vec![0u8; page_size as usize];
    for n in 0..=max_lookback_frames {
        let back_bytes =
            match first_payload_offset.checked_add((n as u64).saturating_mul(frame_size)) {
                Some(v) => v,
                None => break,
            };
        let candidate_start = match offset.checked_sub(back_bytes) {
            Some(v) => v,
            None => break,
        };
        // Cheap pre-filter: only read the full header if the magic is plausible.
        let mut magic_buf = [0u8; 4];
        let magic_n = evidence
            .read_at(candidate_start, &mut magic_buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if magic_n < magic_buf.len() {
            continue;
        }
        let magic = u32::from_be_bytes(magic_buf);
        if magic != WAL_MAGIC_1 && magic != WAL_MAGIC_2 {
            continue;
        }
        let header_n = evidence
            .read_at(candidate_start, &mut header_buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if header_n < header_buf.len() {
            continue;
        }
        let header = match parse_wal_header(&header_buf) {
            Some(h) => h,
            None => continue,
        };
        if header.page_size != page_size {
            continue;
        }
        // Walk and checksum-validate every frame 0..=n. Suppression must be at
        // least as strict as `sqlite_wal::walk_frames`: salts match, page_no
        // non-zero, and rolling frame checksum agrees with the values stored
        // in each frame header. A WAL header alone — even with structurally
        // plausible frame headers — is not sufficient to drop a standalone DB.
        let mut frame_offset = candidate_start.saturating_add(WAL_HEADER_LEN);
        let mut rolling = header.frame_checksum;
        let mut chain_ok = true;
        for _ in 0..=n {
            let read_n = evidence
                .read_at(frame_offset, &mut frame_buf)
                .map_err(|e| CarveError::Evidence(e.to_string()))?;
            if read_n < frame_buf.len() {
                chain_ok = false;
                break;
            }
            let page_no =
                u32::from_be_bytes([frame_buf[0], frame_buf[1], frame_buf[2], frame_buf[3]]);
            let salt_1 =
                u32::from_be_bytes([frame_buf[8], frame_buf[9], frame_buf[10], frame_buf[11]]);
            let salt_2 =
                u32::from_be_bytes([frame_buf[12], frame_buf[13], frame_buf[14], frame_buf[15]]);
            if page_no == 0 || salt_1 != header.salt_1 || salt_2 != header.salt_2 {
                chain_ok = false;
                break;
            }
            let payload_offset = frame_offset.saturating_add(WAL_FRAME_HEADER_LEN);
            let payload_n = evidence
                .read_at(payload_offset, &mut page_buf)
                .map_err(|e| CarveError::Evidence(e.to_string()))?;
            if payload_n < page_buf.len() {
                chain_ok = false;
                break;
            }
            let mut next =
                match wal_checksum_bytes(header.checksum_byte_order, &frame_buf[..8], rolling) {
                    Some(c) => c,
                    None => {
                        chain_ok = false;
                        break;
                    }
                };
            next = match wal_checksum_bytes(header.checksum_byte_order, &page_buf, next) {
                Some(c) => c,
                None => {
                    chain_ok = false;
                    break;
                }
            };
            let expected_1 =
                u32::from_be_bytes([frame_buf[16], frame_buf[17], frame_buf[18], frame_buf[19]]);
            let expected_2 =
                u32::from_be_bytes([frame_buf[20], frame_buf[21], frame_buf[22], frame_buf[23]]);
            if next[0] != expected_1 || next[1] != expected_2 {
                chain_ok = false;
                break;
            }
            rolling = next;
            frame_offset = frame_offset.saturating_add(frame_size);
        }
        if !chain_ok {
            continue;
        }
        return Ok(Some(format!(
            "sqlite hit inside sqlite_wal frame payload (wal_start=0x{candidate_start:X}, frame_index={n})"
        )));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carve::{CarveHandler, ExtractionContext, PreValidation};
    use crate::evidence::EvidenceSource;
    use tempfile::tempdir;

    /// Test evidence source backed by an in-memory buffer.
    struct MemEvidence {
        data: Vec<u8>,
    }

    impl MemEvidence {
        fn new(data: Vec<u8>) -> Self {
            Self { data }
        }
    }

    impl EvidenceSource for MemEvidence {
        fn read_at(
            &self,
            offset: u64,
            buf: &mut [u8],
        ) -> Result<usize, crate::evidence::EvidenceError> {
            let start = offset as usize;
            if start >= self.data.len() {
                return Ok(0);
            }
            let end = (start + buf.len()).min(self.data.len());
            let n = end - start;
            buf[..n].copy_from_slice(&self.data[start..end]);
            Ok(n)
        }
        fn len(&self) -> u64 {
            self.data.len() as u64
        }
    }

    /// Write a structurally-valid minimal b-tree page header (cell_count=1,
    /// one cell pointer near the end of the page) into `data[offset..offset+page_size]`.
    /// Used by the test page builders so that pages tagged with a b-tree type
    /// also pass deep structural validation.
    fn write_minimal_btree_page(data: &mut [u8], offset: usize, page_size: u32, page_type: u8) {
        let header_size: usize = match page_type {
            0x0A | 0x0D => 8,
            0x02 | 0x05 => 12,
            _ => return, // not a b-tree page; leave bytes as-is
        };
        let ps = page_size as usize;
        if offset + ps > data.len() || ps < header_size + 4 {
            return;
        }
        let page = &mut data[offset..offset + ps];
        // Wipe the page header region we are about to write.
        for b in page.iter_mut().take(header_size + 2) {
            *b = 0;
        }
        page[0] = page_type;
        // first_freeblock = 0
        page[1..3].copy_from_slice(&0u16.to_be_bytes());
        // cell_count = 1
        page[3..5].copy_from_slice(&1u16.to_be_bytes());
        let cell_start = (ps as u16).saturating_sub(16);
        // cell_content_area
        page[5..7].copy_from_slice(&cell_start.to_be_bytes());
        // fragmented_free_bytes
        page[7] = 0;
        if header_size == 12 {
            // right-most child pointer (interior pages only); any non-zero value is fine
            page[8..12].copy_from_slice(&1u32.to_be_bytes());
        }
        // single cell pointer entry
        page[header_size..header_size + 2].copy_from_slice(&cell_start.to_be_bytes());
        // a non-zero byte in the cell content area
        page[cell_start as usize] = 0x01;
    }

    /// Write a structurally-valid minimal page-1 b-tree header.
    ///
    /// In SQLite page 1, the first 100 bytes are the database header and the
    /// b-tree page header starts at byte 100. Offsets stored in the b-tree
    /// header remain absolute offsets relative to the beginning of page 1.
    fn write_minimal_page1_btree(data: &mut [u8], page_size: u32, page_type: u8) {
        let header_size: usize = match page_type {
            0x0A | 0x0D => 8,
            0x02 | 0x05 => 12,
            _ => return,
        };
        let ps = page_size as usize;
        if data.len() < ps || ps < 100 + header_size + 4 {
            return;
        }
        let page = &mut data[..ps];
        let header_offset = 100usize;
        for b in page.iter_mut().skip(header_offset).take(header_size + 2) {
            *b = 0;
        }
        page[header_offset] = page_type;
        // first_freeblock = 0
        page[header_offset + 1..header_offset + 3].copy_from_slice(&0u16.to_be_bytes());
        // cell_count = 1
        page[header_offset + 3..header_offset + 5].copy_from_slice(&1u16.to_be_bytes());
        let cell_start = (ps as u16).saturating_sub(16);
        // cell_content_area (absolute from page start)
        page[header_offset + 5..header_offset + 7].copy_from_slice(&cell_start.to_be_bytes());
        // fragmented_free_bytes
        page[header_offset + 7] = 0;
        if header_size == 12 {
            page[header_offset + 8..header_offset + 12].copy_from_slice(&1u32.to_be_bytes());
        }
        // single cell pointer entry
        page[header_offset + header_size..header_offset + header_size + 2]
            .copy_from_slice(&cell_start.to_be_bytes());
        page[cell_start as usize] = 0x01;
    }

    /// Build a minimal valid SQLite database image:
    /// - 100-byte header with correct magic, page_size, page_count
    /// - Remaining pages filled with the given page type byte at offset 0 of each page.
    ///   B-tree page types additionally get a structurally valid minimal page header.
    fn build_sqlite_image(page_size: u32, page_count: u32, page_type_fill: u8) -> Vec<u8> {
        let total = page_size as usize * page_count.max(1) as usize;
        let mut data = vec![0u8; total];
        // Magic header
        data[..16].copy_from_slice(b"SQLite format 3\0");
        // Page size
        let ps_raw = if page_size == 65536 {
            1u16
        } else {
            page_size as u16
        };
        data[16..18].copy_from_slice(&ps_raw.to_be_bytes());
        // Page count
        data[28..32].copy_from_slice(&page_count.to_be_bytes());
        // Page 1 is always a b-tree page after the 100-byte DB header.
        write_minimal_page1_btree(&mut data, page_size, 0x0D);
        // Fill page-type byte at the start of each page (skip page 1 = header)
        for i in 1..page_count.max(1) as usize {
            let offset = i * page_size as usize;
            if offset < data.len() {
                data[offset] = page_type_fill;
                write_minimal_btree_page(&mut data, offset, page_size, page_type_fill);
            }
        }
        data
    }

    /// Build a SQLite image where each page can have a different type byte.
    /// B-tree page types additionally get a structurally valid minimal page header.
    fn build_sqlite_image_with_types(page_size: u32, page_types: &[u8]) -> Vec<u8> {
        let page_count = (page_types.len() + 1) as u32; // +1 for header page
        let total = page_size as usize * page_count as usize;
        let mut data = vec![0u8; total];
        data[..16].copy_from_slice(b"SQLite format 3\0");
        let ps_raw = if page_size == 65536 {
            1u16
        } else {
            page_size as u16
        };
        data[16..18].copy_from_slice(&ps_raw.to_be_bytes());
        data[28..32].copy_from_slice(&page_count.to_be_bytes());
        // Page 1 is always a b-tree page after the 100-byte DB header.
        write_minimal_page1_btree(&mut data, page_size, 0x0D);
        for (i, &page_type) in page_types.iter().enumerate() {
            let offset = (i + 1) * page_size as usize;
            if offset < data.len() {
                data[offset] = page_type;
                write_minimal_btree_page(&mut data, offset, page_size, page_type);
            }
        }
        data
    }

    fn make_handler(
        max_consecutive_invalid_pages: u32,
        min_valid_page_ratio: f64,
    ) -> SqliteCarveHandler {
        SqliteCarveHandler::new(
            "sqlite".to_string(),
            100,
            0, // no max_size limit
            max_consecutive_invalid_pages,
            min_valid_page_ratio,
            64,
        )
    }

    fn carve_and_check(
        handler: &SqliteCarveHandler,
        evidence: &MemEvidence,
    ) -> Option<crate::carve::CarvedFile> {
        let tmp = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test_run",
            output_root: tmp.path(),
            evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };
        let hit = crate::scanner::NormalizedHit {
            global_offset: 0,
            pattern_id: "sqlite_header".to_string(),
            file_type_id: "sqlite".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };
        handler
            .process_hit(&hit, &ctx)
            .expect("process_hit should not error")
            .map(|p| p.flush().expect("flush"))
    }

    #[test]
    fn sqlite_page_sizes() {
        assert!(is_valid_page_size(512));
        assert!(is_valid_page_size(4096));
        assert!(is_valid_page_size(65536));
        assert!(!is_valid_page_size(1000));
        assert!(!is_valid_page_size(128));
    }

    #[test]
    fn sqlite_valid_db_carved_fully() {
        // 10-page database, all pages are table leaf (0x0D)
        let data = build_sqlite_image(4096, 10, 0x0D);
        let evidence = MemEvidence::new(data.clone());
        let handler = make_handler(3, 0.5);
        let result = carve_and_check(&handler, &evidence);
        let carved = result.expect("should produce a carved file");
        assert_eq!(carved.size, data.len() as u64);
        assert!(carved.validated, "fully valid DB should be validated");
        assert!(!carved.truncated);
        assert!(carved.errors.is_empty());
    }

    #[test]
    fn sqlite_garbage_after_header_stops_early() {
        // Header page + 20 pages of garbage (type byte 0xFF)
        let data = build_sqlite_image(4096, 21, 0xFF);
        let evidence = MemEvidence::new(data);
        let handler = make_handler(3, 0.5);
        let result = carve_and_check(&handler, &evidence);
        let carved = result.expect("should produce a carved file");
        // Threshold=3: pages written are header + 2 invalid (3rd triggers break before write)
        assert_eq!(carved.size, 4096 * 3);
        assert!(!carved.validated, "garbage should not be validated");
        assert!(
            carved
                .errors
                .iter()
                .any(|e| e.contains("consecutive invalid"))
        );
    }

    #[test]
    fn sqlite_interleaved_invalid_pages_continue() {
        // Pattern: valid, invalid, valid, valid, invalid, valid, valid, valid, valid
        // Consecutive invalid never reaches 3
        let page_types = vec![0x0D, 0xFF, 0x0D, 0x0D, 0xFF, 0x0D, 0x0D, 0x0D, 0x0D];
        let data = build_sqlite_image_with_types(4096, &page_types);
        let evidence = MemEvidence::new(data.clone());
        let handler = make_handler(3, 0.5);
        let result = carve_and_check(&handler, &evidence);
        let carved = result.expect("should produce a carved file");
        assert_eq!(carved.size, data.len() as u64, "should carve all pages");
        // 8 valid out of 10 total (header + 9 data pages, 7 valid data pages)
        assert!(carved.validated, "high valid ratio should validate");
    }

    #[test]
    fn sqlite_all_overflow_pages_valid() {
        // All pages are overflow/free (type 0x00) — valid SQLite page type
        let data = build_sqlite_image(4096, 10, 0x00);
        let evidence = MemEvidence::new(data.clone());
        let handler = make_handler(3, 0.5);
        let result = carve_and_check(&handler, &evidence);
        let carved = result.expect("should produce a carved file");
        assert_eq!(carved.size, data.len() as u64);
        assert!(carved.validated, "overflow pages are valid page types");
    }

    #[test]
    fn sqlite_consecutive_failure_threshold_exact() {
        // 2 invalid then valid — should NOT trigger with threshold=3
        let page_types = vec![0xFF, 0xFF, 0x0D, 0x0D, 0x0D];
        let data = build_sqlite_image_with_types(4096, &page_types);
        let evidence = MemEvidence::new(data.clone());
        let handler = make_handler(3, 0.5);
        let result = carve_and_check(&handler, &evidence);
        let carved = result.expect("should produce a carved file");
        assert_eq!(carved.size, data.len() as u64, "should carve all pages");

        // 3 invalid in a row — should trigger with threshold=3
        let page_types2 = vec![0xFF, 0xFF, 0xFF, 0x0D, 0x0D];
        let data2 = build_sqlite_image_with_types(4096, &page_types2);
        let evidence2 = MemEvidence::new(data2);
        let handler2 = make_handler(3, 0.5);
        let result2 = carve_and_check(&handler2, &evidence2);
        let carved2 = result2.expect("should produce a carved file");
        // Threshold=3: header + 2 invalid pages written (3rd triggers break before write)
        assert_eq!(carved2.size, 4096 * 3);
        assert!(!carved2.validated);
    }

    #[test]
    fn sqlite_validated_flag_with_low_ratio() {
        // 4 valid pages + 5 invalid (but never 3 consecutive)
        // Pattern: valid, invalid, valid, invalid, valid, invalid, valid, invalid, invalid
        // Wait — that last pair is 2 consecutive, under threshold of 3
        let page_types = vec![0x0D, 0xFF, 0x0D, 0xFF, 0x0D, 0xFF, 0x0D, 0xFF, 0xFF];
        let data = build_sqlite_image_with_types(4096, &page_types);
        let evidence = MemEvidence::new(data.clone());
        let handler = make_handler(3, 0.5);
        let result = carve_and_check(&handler, &evidence);
        let carved = result.expect("should produce a carved file");
        assert_eq!(carved.size, data.len() as u64, "should carve all pages");
        // Total examined: 10 (header + 9 data pages)
        // Valid: header(1) + 4 valid data = 5
        // Ratio: 5/10 = 0.5 — exactly at threshold
        assert!(
            carved.validated,
            "ratio exactly at threshold should validate"
        );
    }

    #[test]
    fn sqlite_validated_flag_below_ratio() {
        // lots of invalid pages (never 3 consecutive) but ratio < 0.5
        // Pattern: invalid, invalid, valid, invalid, invalid, valid, invalid, invalid
        let page_types = vec![0xFF, 0xFF, 0x0D, 0xFF, 0xFF, 0x0D, 0xFF, 0xFF];
        let data = build_sqlite_image_with_types(4096, &page_types);
        let evidence = MemEvidence::new(data.clone());
        let handler = make_handler(3, 0.5);
        let result = carve_and_check(&handler, &evidence);
        let carved = result.expect("should produce a carved file");
        // Total: 9 pages (header + 8 data)
        // Valid: header(1) + 2 = 3
        // Ratio: 3/9 = 0.33 < 0.5
        assert!(!carved.validated, "low ratio should not validate");
    }

    #[test]
    fn sqlite_max_size_still_respected() {
        // 100-page database, but max_size = 4096 * 5 = 20480 → only 5 pages
        let data = build_sqlite_image(4096, 100, 0x0D);
        let evidence = MemEvidence::new(data);
        let handler = SqliteCarveHandler::new(
            "sqlite".to_string(),
            100,
            4096 * 5, // max 5 pages
            3,
            0.5,
            64,
        );
        let result = carve_and_check(&handler, &evidence);
        let carved = result.expect("should produce a carved file");
        assert!(carved.size <= 4096 * 5, "should respect max_size");
        assert!(carved.truncated, "should be marked truncated");
    }

    #[test]
    fn sqlite_pre_validate_rejects_bad_magic() {
        let mut data = vec![0u8; 100];
        data[..4].copy_from_slice(b"NOPE");
        data[16..18].copy_from_slice(&4096u16.to_be_bytes());
        let evidence = MemEvidence::new(data);
        let handler = make_handler(3, 0.5);
        let result = handler
            .pre_validate(&evidence, 0)
            .expect("pre_validate should not error");
        assert!(matches!(result, PreValidation::Reject(_)));
    }

    #[test]
    fn sqlite_single_page_db() {
        // page_count = 0 means single-page database
        let data = build_sqlite_image(4096, 0, 0x00);
        let evidence = MemEvidence::new(data.clone());
        let handler = make_handler(3, 0.5);
        let result = carve_and_check(&handler, &evidence);
        let carved = result.expect("should produce a carved file");
        assert_eq!(carved.size, 4096);
        assert!(carved.validated);
    }

    /// Construct a minimal valid WAL header (32 bytes) for the given page size.
    /// Uses big-endian checksum byte order (magic 0x377F_0683) so the bytes
    /// match the parser without computing a checksum manually.
    fn build_test_wal_header(page_size: u32) -> [u8; 32] {
        use crate::carve::sqlite_wal::{
            ChecksumByteOrder, WAL_MAGIC_2, WAL_VERSION, wal_checksum_bytes,
        };
        let mut h = [0u8; 32];
        h[0..4].copy_from_slice(&WAL_MAGIC_2.to_be_bytes());
        h[4..8].copy_from_slice(&WAL_VERSION.to_be_bytes());
        h[8..12].copy_from_slice(&page_size.to_be_bytes());
        h[12..16].copy_from_slice(&0u32.to_be_bytes());
        h[16..20].copy_from_slice(&TEST_SALT_1.to_be_bytes());
        h[20..24].copy_from_slice(&TEST_SALT_2.to_be_bytes());
        let cks = wal_checksum_bytes(ChecksumByteOrder::BigEndian, &h[..24], [0, 0])
            .expect("wal header checksum");
        h[24..28].copy_from_slice(&cks[0].to_be_bytes());
        h[28..32].copy_from_slice(&cks[1].to_be_bytes());
        h
    }

    const TEST_SALT_1: u32 = 0xAABB_CCDD;
    const TEST_SALT_2: u32 = 0x1122_3344;

    /// Append a checksum-valid WAL frame (24-byte header + payload) to `out`,
    /// updating `rolling` to the new running checksum. `payload` must be
    /// exactly `page_size` bytes.
    fn append_valid_frame(out: &mut Vec<u8>, rolling: &mut [u32; 2], page_no: u32, payload: &[u8]) {
        use crate::carve::sqlite_wal::{ChecksumByteOrder, wal_checksum_bytes};
        let mut frame = [0u8; 24];
        frame[0..4].copy_from_slice(&page_no.to_be_bytes());
        frame[8..12].copy_from_slice(&TEST_SALT_1.to_be_bytes());
        frame[12..16].copy_from_slice(&TEST_SALT_2.to_be_bytes());
        let mut next =
            wal_checksum_bytes(ChecksumByteOrder::BigEndian, &frame[..8], *rolling).unwrap();
        next = wal_checksum_bytes(ChecksumByteOrder::BigEndian, payload, next).unwrap();
        frame[16..20].copy_from_slice(&next[0].to_be_bytes());
        frame[20..24].copy_from_slice(&next[1].to_be_bytes());
        *rolling = next;
        out.extend_from_slice(&frame);
        out.extend_from_slice(payload);
    }

    /// Build a payload that is a SQLite page-1 image of the given page_size.
    fn sqlite_page1_payload(page_size: u32) -> Vec<u8> {
        let mut page = vec![0u8; page_size as usize];
        page[..16].copy_from_slice(SQLITE_HEADER);
        let ps_raw = if page_size == 65536 {
            1u16
        } else {
            page_size as u16
        };
        page[16..18].copy_from_slice(&ps_raw.to_be_bytes());
        page[28..32].copy_from_slice(&1u32.to_be_bytes());
        page
    }

    /// Build a buffer that places a SQLite page-1 image inside a WAL frame
    /// payload. Layout:
    ///   [WAL header (32B)] [WAL frame header (24B)] [SQLite page1 (page_size B)]
    /// The SQLite header at offset 56 is what would otherwise be carved as a
    /// standalone DB. Frame checksums match the WAL header (rolling).
    fn build_wal_with_page1(page_size: u32) -> Vec<u8> {
        let header = build_test_wal_header(page_size);
        let mut out = Vec::with_capacity(32 + 24 + page_size as usize);
        out.extend_from_slice(&header);
        let header_cks = [
            u32::from_be_bytes([header[24], header[25], header[26], header[27]]),
            u32::from_be_bytes([header[28], header[29], header[30], header[31]]),
        ];
        let mut rolling = header_cks;
        let payload = sqlite_page1_payload(page_size);
        append_valid_frame(&mut out, &mut rolling, 1, &payload);
        out
    }

    #[test]
    fn sqlite_pre_validate_suppresses_wal_page1_frame() {
        let page_size = 4096u32;
        let data = build_wal_with_page1(page_size);
        let evidence = MemEvidence::new(data);
        let handler = make_handler(3, 0.5);
        // SQLite header is at WAL header (32) + frame header (24) = 56.
        let result = handler
            .pre_validate(&evidence, 56)
            .expect("pre_validate must not error");
        match result {
            PreValidation::Reject(reason) => {
                assert!(
                    reason.contains("sqlite_wal frame payload"),
                    "unexpected reason: {reason}"
                );
            }
            PreValidation::Proceed => panic!("expected suppression of WAL frame payload"),
        }
    }

    #[test]
    fn sqlite_pre_validate_suppresses_deeper_wal_frame() {
        let page_size = 4096u32;
        let header = build_test_wal_header(page_size);
        let frame_size = 24 + page_size as usize;
        let mut data = Vec::with_capacity(32 + 3 * frame_size);
        data.extend_from_slice(&header);
        let mut rolling = [
            u32::from_be_bytes([header[24], header[25], header[26], header[27]]),
            u32::from_be_bytes([header[28], header[29], header[30], header[31]]),
        ];
        // Three checksum-valid frames; the third carries a SQLite page-1
        // image at offset 32 + 2*(24+P) + 24.
        let dummy_payload = vec![0u8; page_size as usize];
        append_valid_frame(&mut data, &mut rolling, 1, &dummy_payload);
        append_valid_frame(&mut data, &mut rolling, 2, &dummy_payload);
        let page1 = sqlite_page1_payload(page_size);
        append_valid_frame(&mut data, &mut rolling, 3, &page1);
        let sqlite_offset = (32 + 2 * frame_size + 24) as u64;
        let evidence = MemEvidence::new(data);
        let handler = make_handler(3, 0.5);
        let result = handler
            .pre_validate(&evidence, sqlite_offset)
            .expect("pre_validate must not error");
        assert!(
            matches!(result, PreValidation::Reject(_)),
            "expected suppression for deeper WAL frame"
        );
    }

    #[test]
    fn sqlite_pre_validate_allows_standalone_db_after_random_bytes() {
        // No WAL header in the lookback range; SQLite hit must be allowed.
        let page_size = 4096u32;
        let mut data = vec![0u8; 8192];
        // Fill with non-WAL bytes (zeros are not WAL magic).
        let sqlite_off = 4096usize;
        data[sqlite_off..sqlite_off + 16].copy_from_slice(SQLITE_HEADER);
        data[sqlite_off + 16..sqlite_off + 18].copy_from_slice(&(page_size as u16).to_be_bytes());
        data[sqlite_off + 28..sqlite_off + 32].copy_from_slice(&1u32.to_be_bytes());
        let evidence = MemEvidence::new(data);
        let handler = make_handler(3, 0.5);
        let result = handler
            .pre_validate(&evidence, sqlite_off as u64)
            .expect("pre_validate must not error");
        assert!(
            matches!(result, PreValidation::Proceed),
            "standalone SQLite header must not be suppressed"
        );
    }

    #[test]
    fn sqlite_pre_validate_lookback_disabled_allows_wal_payload() {
        // With max_lookback_frames = 0 disabled? Actually 0 still checks n=0.
        // Use the dedicated knob value of 0 to mean "only check immediate wal_start"
        // which is still the most common case. To verify the knob actually limits
        // search depth we set it to 0 and place the SQLite hit deeper than n=0.
        let page_size = 4096u32;
        let header = build_test_wal_header(page_size);
        let frame_size = 24 + page_size as usize;
        let mut data = Vec::with_capacity(32 + 2 * frame_size);
        data.extend_from_slice(&header);
        let mut rolling = [
            u32::from_be_bytes([header[24], header[25], header[26], header[27]]),
            u32::from_be_bytes([header[28], header[29], header[30], header[31]]),
        ];
        let dummy = vec![0u8; page_size as usize];
        append_valid_frame(&mut data, &mut rolling, 1, &dummy);
        let page1 = sqlite_page1_payload(page_size);
        append_valid_frame(&mut data, &mut rolling, 2, &page1);
        let sqlite_offset = (32 + frame_size + 24) as u64;
        let evidence = MemEvidence::new(data);
        // Lookback = 0 means only n=0 (wal_start = offset - 56) is examined,
        // which does NOT match the actual wal_start for this deeper hit, so the
        // hit is allowed through (knob honored).
        let handler = SqliteCarveHandler::new("sqlite".to_string(), 100, 0, 3, 0.5, 0);
        let result = handler
            .pre_validate(&evidence, sqlite_offset)
            .expect("pre_validate must not error");
        assert!(
            matches!(result, PreValidation::Proceed),
            "lookback knob must bound search depth"
        );
    }

    /// Regression for forensic-safety review: a stale or unrelated WAL header
    /// that lands at the computed lookback distance, followed by garbage rather
    /// than a real WAL frame chain, must NOT cause suppression of a legitimate
    /// standalone SQLite database whose header happens to land at the same
    /// offset. Suppression requires the frame chain from the WAL start to the
    /// candidate frame to be structurally valid.
    #[test]
    fn sqlite_pre_validate_does_not_suppress_real_db_after_stale_wal_header() {
        let page_size = 4096u32;
        let frame_size = 24 + page_size as usize;
        let total = 32 + frame_size; // wal header + space for one fake frame
        let mut data = vec![0u8; total];
        // Place a structurally valid WAL header at offset 0.
        let wal_header = build_test_wal_header(page_size);
        data[..32].copy_from_slice(&wal_header);
        // Bytes 32..56 (the supposed frame-0 header) are left as zeros, so
        // salts do NOT match the WAL header salts and the chain check fails.
        // Place a real standalone SQLite page-1 header at offset 56.
        let sqlite_off = 56usize;
        data[sqlite_off..sqlite_off + 16].copy_from_slice(SQLITE_HEADER);
        data[sqlite_off + 16..sqlite_off + 18].copy_from_slice(&(page_size as u16).to_be_bytes());
        data[sqlite_off + 28..sqlite_off + 32].copy_from_slice(&1u32.to_be_bytes());
        let evidence = MemEvidence::new(data);
        let handler = make_handler(3, 0.5);
        let result = handler
            .pre_validate(&evidence, sqlite_off as u64)
            .expect("pre_validate must not error");
        assert!(
            matches!(result, PreValidation::Proceed),
            "real standalone DB must not be suppressed when frame chain is invalid"
        );
    }

    /// Regression for forensic-safety review: even when the salts and page
    /// numbers in the frame header look correct, a frame whose stored
    /// rolling checksum does not match the WAL carver's computation must NOT
    /// cause suppression. The WAL carver itself rejects such frames in
    /// `walk_frames`, so suppression must apply the same standard.
    #[test]
    fn sqlite_pre_validate_does_not_suppress_when_frame_checksum_invalid() {
        let page_size = 4096u32;
        let header = build_test_wal_header(page_size);
        let mut data = Vec::with_capacity(32 + 24 + page_size as usize);
        data.extend_from_slice(&header);
        // Hand-build a frame header with matching salts and a non-zero page
        // number, but leave the stored checksum bytes (offsets 16..24) at
        // zero so they will not match the rolling checksum.
        let mut frame = [0u8; 24];
        frame[0..4].copy_from_slice(&1u32.to_be_bytes());
        frame[8..12].copy_from_slice(&TEST_SALT_1.to_be_bytes());
        frame[12..16].copy_from_slice(&TEST_SALT_2.to_be_bytes());
        // checksum bytes deliberately left zero
        data.extend_from_slice(&frame);
        let payload = sqlite_page1_payload(page_size);
        data.extend_from_slice(&payload);
        let evidence = MemEvidence::new(data);
        let handler = make_handler(3, 0.5);
        let result = handler
            .pre_validate(&evidence, 56)
            .expect("pre_validate must not error");
        assert!(
            matches!(result, PreValidation::Proceed),
            "frame with invalid rolling checksum must not suppress real DB"
        );
    }

    /// Regression for issue #83: a database whose pages all carry a
    /// recognised b-tree page type byte (`0x0D` = table leaf) but malformed
    /// b-tree headers (out-of-bounds `cell_content_area` and bogus cell
    /// pointers) must NOT be reported as `validated = true`. Page-type
    /// plausibility alone is insufficient.
    #[test]
    fn sqlite_validated_requires_deep_btree_structure() {
        let page_size: u32 = 4096;
        let page_count: u32 = 10;
        let total = page_size as usize * page_count as usize;
        let mut data = vec![0u8; total];
        // Page 1: SQLite header.
        data[..16].copy_from_slice(SQLITE_HEADER);
        data[16..18].copy_from_slice(&(page_size as u16).to_be_bytes());
        data[28..32].copy_from_slice(&page_count.to_be_bytes());
        // Pages 2..N: page-type byte 0x0D (table leaf), with a non-zero
        // cell_count and a cell_content_area that points outside the page.
        // This mimics the page-type-plausible but corrupt bytes that
        // produced false `validated = true` in the bug report.
        for i in 1..page_count as usize {
            let off = i * page_size as usize;
            data[off] = 0x0D;
            // cell_count = 4
            data[off + 3..off + 5].copy_from_slice(&4u16.to_be_bytes());
            // cell_content_area = page_size + 1 (out of bounds)
            data[off + 5..off + 7]
                .copy_from_slice(&(page_size as u16).saturating_add(1).to_be_bytes());
        }
        let evidence = MemEvidence::new(data.clone());
        let handler = make_handler(3, 0.5);
        let carved = carve_and_check(&handler, &evidence)
            .expect("should produce a carved file (page bytes are written)");
        assert_eq!(carved.size, data.len() as u64);
        assert!(
            !carved.validated,
            "page-type-plausible but malformed pages must not validate"
        );
        assert!(
            carved
                .errors
                .iter()
                .any(|e| e.contains("deep b-tree validation")),
            "expected error note from deep validation; got {:?}",
            carved.errors
        );
    }

    /// Regression: a database with one empty-table root page (cell_count=0,
    /// otherwise a structurally valid b-tree page) must remain
    /// `validated = true`. Empty pages are legitimate in SQLite.
    #[test]
    fn sqlite_validated_allows_empty_btree_root_pages() {
        // 5 pages: header + 1 empty (cell_count=0) + 3 normal table-leaf pages.
        let mut data = build_sqlite_image_with_types(4096, &[0x0D, 0x0D, 0x0D, 0x0D]);
        // Overwrite page 2 (offset 4096) so cell_count = 0 and
        // cell_content_area = page_size. This is what SQLite emits for an
        // empty table's root page on non-65536-byte pages.
        let off = 4096usize;
        for b in data[off..off + 8].iter_mut() {
            *b = 0;
        }
        data[off] = 0x0D;
        data[off + 5..off + 7].copy_from_slice(&4096u16.to_be_bytes());
        let evidence = MemEvidence::new(data.clone());
        let handler = make_handler(3, 0.5);
        let carved = carve_and_check(&handler, &evidence).expect("should produce a carved file");
        assert_eq!(carved.size, data.len() as u64);
        assert!(
            carved.validated,
            "empty b-tree root pages must not break validation; errors={:?}",
            carved.errors
        );
    }

    /// Regression: deep validation must also catch pages whose b-tree
    /// header would point past the end of the page (e.g. a corrupted
    /// `cell_content_area`).
    #[test]
    fn sqlite_validated_rejects_out_of_bounds_cell_content_area() {
        let page_size: u32 = 4096;
        // Build a single data page with a structurally valid header,
        // then corrupt cell_content_area to point past the page.
        let mut data = build_sqlite_image(page_size, 2, 0x0D);
        let off = page_size as usize;
        // cell_content_area = page_size + 1 (out of bounds)
        let bad = (page_size as u16).saturating_add(1).to_be_bytes();
        data[off + 5..off + 7].copy_from_slice(&bad);
        let evidence = MemEvidence::new(data);
        let handler = make_handler(3, 0.5);
        let carved = carve_and_check(&handler, &evidence).expect("should produce a carved file");
        assert!(
            !carved.validated,
            "out-of-bounds header must fail deep check"
        );
        assert!(
            carved
                .errors
                .iter()
                .any(|e| e.contains("deep b-tree validation"))
        );
    }

    #[test]
    fn sqlite_validated_rejects_zero_cell_content_area_on_non_64k_page() {
        let mut data = build_sqlite_image(4096, 2, 0x0D);
        let off = 4096usize;
        // Set cell_count = 0 and cell_content_area = 0 on a 4096-byte page.
        // SQLite zero encoding for cell_content_area is only valid for 65536.
        data[off + 3..off + 5].copy_from_slice(&0u16.to_be_bytes());
        data[off + 5..off + 7].copy_from_slice(&0u16.to_be_bytes());

        let evidence = MemEvidence::new(data);
        let handler = make_handler(3, 0.5);
        let carved = carve_and_check(&handler, &evidence).expect("should produce a carved file");
        assert!(!carved.validated);
        assert!(
            carved
                .errors
                .iter()
                .any(|e| e.contains("deep b-tree validation"))
        );
    }

    #[test]
    fn sqlite_single_page_db_with_invalid_page1_btree_not_validated() {
        let mut data = build_sqlite_image(4096, 1, 0x00);
        // Page 1 b-tree header starts at byte 100. Set an invalid page type.
        data[100] = 0xFF;

        let evidence = MemEvidence::new(data);
        let handler = make_handler(3, 0.5);
        let carved = carve_and_check(&handler, &evidence).expect("should produce a carved file");
        assert!(
            !carved.validated,
            "single-page DB with invalid page-1 b-tree must not validate"
        );
        assert!(
            carved
                .errors
                .iter()
                .any(|e| e.contains("deep b-tree validation"))
        );
    }
}
