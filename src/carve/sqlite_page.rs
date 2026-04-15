use std::collections::HashSet;
use std::fs::File;
use std::io::Write;

use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, output_path, write_range,
};
use crate::scanner::NormalizedHit;

const SQLITE_TABLE_LEAF_PAGE: u8 = 0x0D;
const SQLITE_INDEX_LEAF_PAGE: u8 = 0x0A;
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

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
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
        let mut file = File::create(&full_path)?;
        let mut md5 = md5::Context::new();
        let mut sha256 = Sha256::new();

        let mut target_size = page_size as u64;
        let mut truncated = false;
        let mut errors = Vec::new();
        if self.max_size > 0 && target_size > self.max_size {
            target_size = self.max_size;
            truncated = true;
            errors.push("max_size reached before sqlite page end".to_string());
        }

        let end = hit.global_offset.saturating_add(target_size);
        let (written, eof_truncated) = write_range(
            ctx,
            hit.global_offset,
            end,
            &mut file,
            &mut md5,
            &mut sha256,
        )?;
        if eof_truncated {
            truncated = true;
            errors.push("eof before sqlite page end".to_string());
        }
        file.flush()?;

        if written < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

        let global_end = if written == 0 {
            hit.global_offset
        } else {
            hit.global_offset + written - 1
        };
        let md5_hex = format!("{:x}", md5.compute());
        let sha256_hex = hex::encode(sha256.finalize());

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
            validated: !truncated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
        }))
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

fn cell_content_start(cell_content_area: u16, page_size: usize) -> Option<usize> {
    if cell_content_area == 0 {
        Some(page_size)
    } else {
        let value = cell_content_area as usize;
        if value <= page_size {
            Some(value)
        } else {
            None
        }
    }
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

    let pointer_bytes = match (header.cell_count as usize).checked_mul(2) {
        Some(value) => value,
        None => return false,
    };
    let pointer_table_end = match header_size.checked_add(pointer_bytes) {
        Some(value) => value,
        None => return false,
    };
    if pointer_table_end > page_size || pointer_table_end > cell_content {
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

    let mut pointer_set = HashSet::new();
    for idx in 0..header.cell_count as usize {
        let off = header_size + idx * 2;
        let ptr = u16::from_be_bytes([page[off], page[off + 1]]) as usize;
        if ptr < cell_content || ptr >= page_size {
            return false;
        }
        if !pointer_set.insert(ptr) {
            return false;
        }
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
