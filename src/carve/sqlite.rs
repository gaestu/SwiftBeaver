use crate::carve::{
    CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext, PreValidation,
    output_path,
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
}

impl SqliteCarveHandler {
    pub fn new(
        extension: String,
        min_size: u64,
        max_size: u64,
        max_consecutive_invalid_pages: u32,
        min_valid_page_ratio: f64,
    ) -> Self {
        Self {
            extension,
            min_size,
            max_size,
            max_consecutive_invalid_pages,
            min_valid_page_ratio,
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
            let page1_remaining = (page_size as u64).saturating_sub(100);
            if page1_remaining > 0 {
                stream.read_exact(page1_remaining as usize)?;
            }

            // --- Page-by-page validation loop for pages 2..N ---
            let mut valid_pages = 1u64; // page 1 (header) is always valid
            let mut total_examined = 1u64;
            let mut consecutive_invalid = 0u32;
            let mut stopped_early = false;

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
                match stream.read_exact(page_size as usize) {
                    Ok(_) => {}
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
                }
            }

            // Determine validated flag based on valid-page ratio
            if total_examined > 0 {
                let ratio = valid_pages as f64 / total_examined as f64;
                validated = ratio >= self.min_valid_page_ratio && !stopped_early;
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

        let (size, md5_hex, sha256_hex) = stream.finish()?;
        if size < self.min_size {
            let _ = std::fs::remove_file(&full_path);
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

        Ok(Some(CarvedFile {
            run_id: ctx.run_id.to_string(),
            file_type: self.file_type().to_string(),
            path: rel_path,
            extension: self.extension.clone(),
            global_start: hit.global_offset,
            global_end,
            size,
            md5: Some(md5_hex),
            sha256: Some(sha256_hex),
            validated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
        }))
    }
}

fn is_valid_page_size(page_size: u32) -> bool {
    if !(512..=65536).contains(&page_size) {
        return false;
    }
    page_size.is_power_of_two()
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

    /// Build a minimal valid SQLite database image:
    /// - 100-byte header with correct magic, page_size, page_count
    /// - Remaining pages filled with the given page type byte at offset 0 of each page
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
        // Fill page-type byte at the start of each page (skip page 1 = header)
        for i in 1..page_count.max(1) as usize {
            let offset = i * page_size as usize;
            if offset < data.len() {
                data[offset] = page_type_fill;
            }
        }
        data
    }

    /// Build a SQLite image where each page can have a different type byte.
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
        for (i, &page_type) in page_types.iter().enumerate() {
            let offset = (i + 1) * page_size as usize;
            if offset < data.len() {
                data[offset] = page_type;
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
}
