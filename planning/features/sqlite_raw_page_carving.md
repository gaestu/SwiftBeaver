# SQLite Raw Page Carving

## Problem Statement

When a SQLite database is deleted and partially overwritten, the database header (first 100 bytes) may be destroyed while individual data pages remain intact elsewhere on disk. The current SQLite carver requires a valid header to calculate database size and cannot recover data from these orphaned pages.

Browser databases (history, cookies, downloads) are high-value forensic targets and are frequently deleted. Recovering URLs, titles, and timestamps from surviving pages would significantly improve evidence recovery.

## Current State

SwiftBeaver has two SQLite-related capabilities:
1. **Full database carving** (`src/carve/sqlite.rs`): Requires intact header, carves complete database
2. **Page-level parsing** (`src/parsers/sqlite_pages.rs`): Parses leaf pages from carved databases, extracts browser history

The page-level parser (`extract_history_from_pages`) already knows how to:
- Identify leaf table pages (0x0D marker)
- Parse cell payloads and varints
- Extract text fields and timestamps
- Follow overflow page chains (within an intact file)

However, this only works on **already-carved** SQLite files. It doesn't scan raw disk for orphaned pages.

## Scope

### In Scope
- Add signature pattern for SQLite leaf table page headers
- Carve individual SQLite pages from raw disk
- Extract browser-related data (URLs, titles, timestamps) from carved pages
- Handle common page sizes (512, 1024, 2048, 4096, 8192, 16384, 32768)
- Emit recovered data as `BrowserHistoryRecord` entries
- Gate feature behind config flag

### Out of Scope
- Full database reconstruction from scattered pages
- Schema reconstruction
- Index page recovery
- Freelist page recovery
- Transaction journal / WAL recovery
- Overflow page linking across non-contiguous pages

## Design Notes

### 1. Signature Pattern

SQLite leaf table pages start with `0x0D` at position 0. However, this single-byte signature would produce excessive false positives. 

A better approach is to validate page structure heuristically after finding `0x0D`:

```
Offset  Size  Description
0       1     Page type (0x0D = leaf table)
1       2     First freeblock offset (big-endian u16)
3       2     Cell count (big-endian u16)
5       2     Cell content area start (big-endian u16)
7       1     Fragmented free bytes count
```

Validation criteria:
- `page_type == 0x0D`
- `cell_count > 0 && cell_count < page_size / 4` (reasonable cell count)
- `cell_content_start > 8 && cell_content_start < page_size`
- Cell pointers are within page bounds

### 2. Page Size Detection

Since we don't have the database header, we must infer page size:
- Try common sizes: 4096 (most common), 1024, 2048, 8192, 16384
- Validate cell pointers fall within the guessed page boundary
- Use the smallest size that passes validation

### 3. Carving Strategy

Option A: **Carve whole page, parse inline**
- Carve the detected page to a file
- Immediately parse and extract browser data
- Mark `validated: true` only if records extracted

Option B: **Direct extraction without carving**
- Don't save individual pages as files (creates noise)
- Extract browser data directly to metadata sink
- Add `source_type: sqlite_page` to distinguish from full database recovery

**Recommendation**: Option B - direct extraction is cleaner and avoids cluttering output with thousands of small page files.

### 4. Implementation Approach

Create a new "page scanner" that runs after signature scanning:

```rust
// src/parsers/sqlite_page_scanner.rs

pub struct SqlitePageScanner {
    enabled: bool,
    page_sizes: Vec<usize>,
}

impl SqlitePageScanner {
    pub fn scan_chunk(
        &self,
        chunk_data: &[u8],
        chunk_offset: u64,
    ) -> Vec<RawPageRecord> {
        // Scan for 0x0D bytes
        // For each, validate as leaf page header
        // Extract records using existing parse_record_fields logic
    }
}
```

### 5. Integration Points

- **Scanner stage**: Add 0x0D pattern or run page scanner as separate pass
- **Carve stage**: Either skip (Option B) or add minimal PageCarveHandler
- **Metadata sink**: Emit `BrowserHistoryRecord` with `browser: sqlite_raw_page`
- **Config**: Add `sqlite_page_carving.enabled` and related settings

### 6. False Positive Mitigation

To reduce false positives:
1. Require at least one valid URL extracted from the page
2. Require cell structure to be internally consistent
3. Optionally require page to contain plausible timestamps
4. Track "page confidence" score based on validation checks passed

## Configuration

```yaml
sqlite_page_carving:
  enabled: false  # Off by default
  page_sizes: [4096, 1024, 2048, 8192]
  min_urls_per_page: 1
  require_timestamp: false
```

## Expected Tests

1. Create SQLite database with browser history
2. Zero out the first 512 bytes (destroy header)
3. Run scan with `sqlite_page_carving.enabled: true`
4. Verify URLs recovered from intact pages
5. Verify `browser: sqlite_raw_page` marking

## Impact on Docs and README

- Add documentation for raw page carving feature
- Document performance implications (additional scanning)
- Note that recovered data has lower confidence than full database recovery
- Update `docs/carver/sqlite.md` with raw page carving section

## Performance Considerations

- Single-byte 0x0D signature is extremely common → need efficient rejection
- Consider running as post-processing on chunks rather than main signature scan
- Page validation adds CPU overhead but dramatically reduces false positives
- Memory impact is minimal (parsing in-place)

## Open Questions

1. Should we carve pages to disk or only extract metadata?
2. Should this run on all chunks or only chunks where no SQLite header was found?
3. How to handle potential duplicate records from multiple recovered pages?
4. Should we attempt to correlate pages that might be from the same database?

## References

- SQLite file format: https://www.sqlite.org/fileformat.html
- SQLite Deleted Records Parser: External tool for similar recovery
- Existing page parser: `src/parsers/sqlite_pages.rs`
