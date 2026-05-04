use std::collections::HashSet;

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PendingCarve, PreValidation,
    create_hashers, finalize_hashers, output_path, write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const SQLITE_TABLE_LEAF_PAGE: u8 = 0x0D;
const SQLITE_INDEX_LEAF_PAGE: u8 = 0x0A;
const SQLITE_TABLE_INTERIOR_PAGE: u8 = 0x05;
const SQLITE_INDEX_INTERIOR_PAGE: u8 = 0x02;
const MAX_FRAGMENTED_FREE_BYTES: u8 = 60;
const PAGE_SIZE_ORDER: [u32; 8] = [4096, 1024, 2048, 8192, 16384, 32768, 65536, 512];

#[derive(Debug, Clone, Copy)]
struct PageHeader {
    page_type: u8,
    first_freeblock: u16,
    cell_count: u16,
    cell_content_area: u16,
    fragmented_free_bytes: u8,
}

pub struct SqlitePageCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl SqlitePageCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for SqlitePageCarveHandler {
    fn file_type(&self) -> &str {
        "sqlite_page"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 1];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < 1 {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        if !matches!(buf[0], 2 | 5 | 10 | 13) {
            return Ok(PreValidation::Reject(
                "sqlite page type invalid".to_string(),
            ));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let page_size = match detect_page_size(ctx, hit.global_offset)? {
            Some(page_size) => page_size,
            None => return Ok(None),
        };

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let (mut md5, mut sha256) = create_hashers(&ctx.hash_config);

        let mut target_size = page_size as u64;
        let mut truncated = false;
        let mut errors = Vec::new();
        if self.max_size > 0 && target_size > self.max_size {
            target_size = self.max_size;
            truncated = true;
            errors.push("max_size reached before sqlite page end".to_string());
        }

        let end = hit.global_offset.saturating_add(target_size);
        let (written, eof_truncated, mut writer) = write_range(
            ctx,
            hit.global_offset,
            end,
            &full_path,
            md5.as_mut(),
            sha256.as_mut(),
        )?;
        if eof_truncated {
            truncated = true;
            errors.push("eof before sqlite page end".to_string());
        }

        if written < self.min_size {
            writer.discard();
            return Ok(None);
        }

        let global_end = if written == 0 {
            hit.global_offset
        } else {
            hit.global_offset + written - 1
        };
        let (md5_hex, sha256_hex) = finalize_hashers(md5, sha256);

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

fn detect_page_size(ctx: &ExtractionContext, start: u64) -> Result<Option<u32>, CarveError> {
    let header_bytes = match read_exact_at(ctx, start, 8)? {
        Some(bytes) => bytes,
        None => return Ok(None),
    };
    let header = match parse_header(&header_bytes) {
        Some(header) => header,
        None => return Ok(None),
    };

    if header.cell_count == 0 {
        return Ok(None);
    }
    if header.fragmented_free_bytes > MAX_FRAGMENTED_FREE_BYTES {
        return Ok(None);
    }

    let evidence_len = ctx.evidence.len();
    for page_size in PAGE_SIZE_ORDER {
        let page_size_usize = page_size as usize;
        if start.saturating_add(page_size as u64) > evidence_len {
            continue;
        }
        if !quick_validate_header(header, page_size_usize) {
            continue;
        }

        let page = match read_exact_at(ctx, start, page_size_usize)? {
            Some(page) => page,
            None => continue,
        };
        if validate_page_structure(&page) {
            return Ok(Some(page_size));
        }
    }

    Ok(None)
}

fn parse_header(page: &[u8]) -> Option<PageHeader> {
    if page.len() < 8 {
        return None;
    }
    let page_type = page[0];
    if page_type != SQLITE_TABLE_LEAF_PAGE && page_type != SQLITE_INDEX_LEAF_PAGE {
        return None;
    }
    Some(PageHeader {
        page_type,
        first_freeblock: u16::from_be_bytes([page[1], page[2]]),
        cell_count: u16::from_be_bytes([page[3], page[4]]),
        cell_content_area: u16::from_be_bytes([page[5], page[6]]),
        fragmented_free_bytes: page[7],
    })
}

fn page_header_size(page_type: u8) -> usize {
    match page_type {
        SQLITE_TABLE_LEAF_PAGE | SQLITE_INDEX_LEAF_PAGE => 8,
        _ => 0,
    }
}

/// Header size for any b-tree page type, including interior pages
/// (which carry an additional 4-byte right-most child pointer).
fn btree_page_header_size(page_type: u8) -> usize {
    match page_type {
        SQLITE_TABLE_LEAF_PAGE | SQLITE_INDEX_LEAF_PAGE => 8,
        SQLITE_TABLE_INTERIOR_PAGE | SQLITE_INDEX_INTERIOR_PAGE => 12,
        _ => 0,
    }
}

fn cell_content_start(cell_content_area: u16, page_size: usize) -> Option<usize> {
    if cell_content_area == 0 {
        if page_size == 65536 {
            Some(65536)
        } else {
            None
        }
    } else {
        let value = cell_content_area as usize;
        if value <= page_size {
            Some(value)
        } else {
            None
        }
    }
}

fn validate_pointer_table_fits(
    header_size: usize,
    cell_count: u16,
    cell_content: usize,
    page_size: usize,
) -> bool {
    let pointer_bytes = match (cell_count as usize).checked_mul(2) {
        Some(value) => value,
        None => return false,
    };
    let pointer_table_end = match header_size.checked_add(pointer_bytes) {
        Some(value) => value,
        None => return false,
    };
    pointer_table_end <= page_size && pointer_table_end <= cell_content
}

fn validate_cell_pointers(
    page: &[u8],
    header_size: usize,
    cell_count: u16,
    cell_content: usize,
) -> bool {
    let page_size = page.len();
    let mut pointer_set = HashSet::new();
    for idx in 0..cell_count as usize {
        let off = header_size + idx * 2;
        if off + 2 > page_size {
            return false;
        }
        let ptr = u16::from_be_bytes([page[off], page[off + 1]]) as usize;
        if ptr < cell_content || ptr >= page_size {
            return false;
        }
        if !pointer_set.insert(ptr) {
            return false;
        }
    }
    true
}

fn quick_validate_header(header: PageHeader, page_size: usize) -> bool {
    let header_size = page_header_size(header.page_type);
    if header_size == 0 {
        return false;
    }
    let cell_content = match cell_content_start(header.cell_content_area, page_size) {
        Some(value) => value,
        None => return false,
    };
    if cell_content < header_size || cell_content > page_size {
        return false;
    }

    if !validate_pointer_table_fits(header_size, header.cell_count, cell_content, page_size) {
        return false;
    }

    if header.first_freeblock != 0 {
        let free = header.first_freeblock as usize;
        if free < cell_content || free.saturating_add(4) > page_size {
            return false;
        }
    }

    true
}

fn validate_page_structure(page: &[u8]) -> bool {
    let header = match parse_header(page) {
        Some(header) => header,
        None => return false,
    };
    if header.cell_count == 0 || header.fragmented_free_bytes > MAX_FRAGMENTED_FREE_BYTES {
        return false;
    }

    let page_size = page.len();
    if !quick_validate_header(header, page_size) {
        return false;
    }

    let header_size = page_header_size(header.page_type);
    let cell_content = match cell_content_start(header.cell_content_area, page_size) {
        Some(value) => value,
        None => return false,
    };

    if !validate_cell_pointers(page, header_size, header.cell_count, cell_content) {
        return false;
    }

    validate_freeblock_chain(page, header.first_freeblock as usize, cell_content)
}

fn validate_freeblock_chain(page: &[u8], first_freeblock: usize, cell_content: usize) -> bool {
    if first_freeblock == 0 {
        return true;
    }

    let page_size = page.len();
    let mut current = first_freeblock;
    let mut visited = HashSet::new();
    let max_steps = (page_size / 4).max(1);
    let mut steps = 0usize;

    while current != 0 {
        if current < cell_content || current.saturating_add(4) > page_size {
            return false;
        }
        if !visited.insert(current) {
            return false;
        }

        let next = u16::from_be_bytes([page[current], page[current + 1]]) as usize;
        let size = u16::from_be_bytes([page[current + 2], page[current + 3]]) as usize;
        if size < 4 || current.saturating_add(size) > page_size {
            return false;
        }

        if next != 0
            && (next < cell_content || next.saturating_add(4) > page_size || next <= current)
        {
            return false;
        }

        current = next;
        steps = steps.saturating_add(1);
        if steps > max_steps {
            return false;
        }
    }

    true
}

fn read_exact_at(
    ctx: &ExtractionContext,
    offset: u64,
    len: usize,
) -> Result<Option<Vec<u8>>, CarveError> {
    let mut buf = vec![0u8; len];
    let n = ctx
        .evidence
        .read_at(offset, &mut buf)
        .map_err(|e| CarveError::Evidence(e.to_string()))?;
    if n < len {
        return Ok(None);
    }
    Ok(Some(buf))
}

/// Deep structural validation of a SQLite b-tree page (table/index, leaf or interior).
///
/// Reuses the same building blocks as `validate_page_structure` (cell pointer
/// table, freeblock chain) so that the two carvers remain in lockstep.
///
/// Validates:
/// - the page-type byte is a recognised b-tree type (`0x02`, `0x05`, `0x0A`, `0x0D`),
/// - `fragmented_free_bytes` is within SQLite's documented bound,
/// - `cell_content_area` is within page bounds and after the page header,
/// - the cell pointer table fits before the cell content area,
/// - every cell pointer points into the cell content region and is unique,
/// - the freeblock chain (if any) is bounded, monotonically increasing, and contained in the page.
///
/// Empty b-tree pages (`cell_count == 0`) are accepted: SQLite uses `cell_count == 0`
/// for freshly created or emptied table/index root pages, and they are
/// structurally indistinguishable from a valid empty page. This is the only
/// behavioural difference vs. `validate_page_structure`, which is intended for
/// the standalone `sqlite_page` carver where empty pages are not useful.
///
/// Returns `false` for non-b-tree page bytes (e.g. `0x00` overflow / freelist pages); callers
/// must filter those out before invoking this function.
pub(crate) fn deep_validate_btree_page(page: &[u8]) -> bool {
    deep_validate_btree_page_with_header_offset(page, 0)
}

/// Deep structural validation of a SQLite b-tree page whose b-tree header
/// begins at `header_offset` within `page`.
///
/// This is used for page 1 in full SQLite databases, where the first 100
/// bytes are the database header and the b-tree page header starts at byte 100.
/// Cell pointers, cell content offsets, and freeblock offsets remain absolute
/// offsets relative to the beginning of the SQLite page.
pub(crate) fn deep_validate_btree_page_with_header_offset(
    page: &[u8],
    header_offset: usize,
) -> bool {
    let page_size = page.len();
    if page_size < 12 || header_offset >= page_size {
        return false;
    }

    let page_type = page[header_offset];
    let header_size = btree_page_header_size(page_type);
    if header_size == 0 {
        return false;
    }
    if header_offset.saturating_add(header_size) > page_size {
        return false;
    }

    let first_freeblock = u16::from_be_bytes([page[header_offset + 1], page[header_offset + 2]]);
    let cell_count = u16::from_be_bytes([page[header_offset + 3], page[header_offset + 4]]);
    let cell_content_area = u16::from_be_bytes([page[header_offset + 5], page[header_offset + 6]]);
    let fragmented_free_bytes = page[header_offset + 7];

    if fragmented_free_bytes > MAX_FRAGMENTED_FREE_BYTES {
        return false;
    }

    let cell_content = match cell_content_start(cell_content_area, page_size) {
        Some(v) => v,
        None => return false,
    };
    if cell_content < header_size {
        return false;
    }

    if !validate_pointer_table_fits(
        header_offset.saturating_add(header_size),
        cell_count,
        cell_content,
        page_size,
    ) {
        return false;
    }

    if !validate_cell_pointers(
        page,
        header_offset.saturating_add(header_size),
        cell_count,
        cell_content,
    ) {
        return false;
    }

    validate_freeblock_chain(page, first_freeblock as usize, cell_content)
}

#[cfg(test)]
mod tests {
    use super::validate_page_structure;

    fn build_valid_leaf_page(page_size: usize) -> Vec<u8> {
        let mut page = vec![0u8; page_size];
        page[0] = 0x0D; // table leaf
        page[1..3].copy_from_slice(&0u16.to_be_bytes()); // first freeblock
        page[3..5].copy_from_slice(&1u16.to_be_bytes()); // cell count
        let cell_start = (page_size - 16) as u16;
        page[5..7].copy_from_slice(&cell_start.to_be_bytes());
        page[7] = 0; // fragmented free bytes
        page[8..10].copy_from_slice(&cell_start.to_be_bytes()); // pointer table
        page[cell_start as usize] = 0x01;
        page
    }

    #[test]
    fn accepts_valid_leaf_page_structure() {
        let page = build_valid_leaf_page(4096);
        assert!(validate_page_structure(&page));
    }

    #[test]
    fn rejects_zero_cell_count() {
        let mut page = build_valid_leaf_page(4096);
        page[3..5].copy_from_slice(&0u16.to_be_bytes());
        assert!(!validate_page_structure(&page));
    }

    #[test]
    fn rejects_out_of_bounds_pointer() {
        let mut page = build_valid_leaf_page(4096);
        page[8..10].copy_from_slice(&10u16.to_be_bytes());
        assert!(!validate_page_structure(&page));
    }

    #[test]
    fn rejects_freeblock_loop() {
        let mut page = build_valid_leaf_page(4096);
        page[1..3].copy_from_slice(&4080u16.to_be_bytes());
        page[4080..4082].copy_from_slice(&4080u16.to_be_bytes()); // next loops to itself
        page[4082..4084].copy_from_slice(&8u16.to_be_bytes());
        assert!(!validate_page_structure(&page));
    }
}
