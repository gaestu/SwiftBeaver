# SQLite Carver

## Overview

The SQLite carver extracts SQLite database files by detecting the database header, validating the page size, and calculating the total database size based on the page count metadata.

## Signature Detection

**Header Pattern**: `SQLite format 3\0` (16 bytes)
- Bytes: `53 51 4C 69 74 65 20 66 6F 72 6D 61 74 20 33 00`
- This signature appears at offset 0 of every SQLite3 database file

## Carving Algorithm

The SQLite carver uses metadata-driven size calculation combined with page-by-page validation:

### 1. Header Parsing (100 bytes)

The first 100 bytes of a SQLite database contain critical metadata:

```
Offset  Size  Description
0       16    Magic header ("SQLite format 3\0")
16      2     Page size (big-endian u16)
              Special case: 1 = 65536 bytes
18      1     File format write version
19      1     File format read version
20      1     Reserved space per page
21      1     Max embedded payload fraction
22      1     Min embedded payload fraction
23      1     Leaf payload fraction
24      4     File change counter
28      4     Database size in pages (big-endian u32)
32      4     First freelist trunk page
36      4     Total freelist pages
40      4     Schema cookie
44      4     Schema format number
48      4     Default page cache size
52      4     Largest root b-tree page
56      4     Database text encoding (1=UTF-8, 2=UTF-16le, 3=UTF-16be)
60      4     User version
64      4     Incremental vacuum mode
68      4     Application ID
72      20    Reserved (must be zero)
92      4     Version-valid-for number
96      4     SQLite version number
```

### 2. Page Size Validation

```rust
let page_size_raw = u16::from_be_bytes([header[16], header[17]]);
let page_size = if page_size_raw == 1 {
    65536  // Special case
} else {
    page_size_raw as u32
};
```

Valid page sizes: 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536

### 3. Page-by-Page Validation

After the header page is written, each subsequent page is examined before being written:

1. **Peek** at the first byte of the page (the B-tree page type byte)
2. **Check** if it is a valid SQLite page type:
   - `0x00` — free page / overflow page
   - `0x02` — index B-tree interior page
   - `0x05` — table B-tree interior page
   - `0x0A` — index B-tree leaf page
   - `0x0D` — table B-tree leaf page
3. **Track** consecutive invalid pages. If the count reaches the threshold (`sqlite_max_consecutive_invalid_pages`, default **3**), carving stops early — the database boundary has likely been passed.
4. **Write** the full page regardless (valid or invalid) to preserve evidence up to the termination point.

### 4. Validated Flag

After all pages are processed, the `validated` flag is determined by two criteria:

- The ratio of valid pages to total examined pages must be ≥ `sqlite_min_valid_page_ratio` (default **0.5**)
- Carving must not have been stopped early by the consecutive-invalid threshold

Both conditions must be true for `validated = true`.

## Validation

- **Validated**: `true` if:
  - Header matches "SQLite format 3\0"
  - Page size is valid
  - Valid-page ratio ≥ `sqlite_min_valid_page_ratio` (default 0.5)
  - No early termination from consecutive invalid pages
- **Truncated**: `true` if:
  - EOF reached before all pages read
  - max_size enforced
- **Invalid**: Removed if:
  - Header mismatch
  - Page size invalid

## Size Constraints

- **Default min_size**: 100 bytes (size of SQLite header)
- **Default max_size**: 100 MiB
- Minimum viable SQLite: 512 bytes (single page at min page size)
- Files below min_size are discarded

## Configuration

| Parameter | Default | Description |
|-----------|---------|-------------|
| `sqlite_max_consecutive_invalid_pages` | 3 | Number of consecutive invalid page types before early termination |
| `sqlite_min_valid_page_ratio` | 0.5 | Minimum ratio of valid pages for `validated=true` |
| `sqlite_suppress_wal_frame_lookback_frames` | 64 | Maximum number of preceding WAL frames examined when checking whether a SQLite header candidate sits inside a SQLite WAL frame payload. Set to `0` to check only the immediate `wal_start = offset - 56` candidate. |

## WAL Frame Suppression

SQLite WAL frames embed full database page images, including page 1 with the `SQLite format 3\0` magic. The raw signature scanner therefore produces SQLite header hits inside WAL frame payloads.

During `pre_validate`, after the magic and page-size checks succeed, the carver walks back through possible WAL frame boundaries (`offset - 56 - n * (24 + page_size)` for `n in 0..=sqlite_suppress_wal_frame_lookback_frames`). For each candidate offset it attempts a strict WAL-header parse (magic, version, page size, header checksum) and, when the WAL header's `page_size` matches the SQLite hit's `page_size`, walks the frame chain from frame 0 through frame `n` applying the same acceptance rules as the `sqlite_wal` carver: each frame header must have salts matching the WAL header salts, a non-zero page number, and a rolling frame checksum that matches the value stored in the frame header (computed across the frame's first 8 header bytes followed by the page payload, seeded with the previous frame's or header's checksum). Only when the full chain validates is the SQLite hit rejected with reason `sqlite hit inside sqlite_wal frame payload`. The reject is counted in `files_prevalidation_rejected`.

This chain check guarantees suppression is at least as strict as the WAL carver's own acceptance rules: a stale, unrelated, or checksum-invalid WAL header lying earlier in the image cannot cause a legitimate standalone SQLite database to be dropped.

The WAL itself is unaffected — only standalone `sqlite` candidates that demonstrably lie inside a valid WAL frame are suppressed. The WAL is still carved by the `sqlite_wal` carver and recorded in `metadata/carved_files.*`.

## Hash Computation

- **MD5**: Computed via `CarveStream` as pages are read
- **SHA-256**: Computed via `CarveStream` as pages are read
- Covers complete database from byte 0 to calculated end

## Testing

**Test file**: `tests/carver_sqlite.rs`

### Test Strategy

Golden image framework with various database types:

1. **Test databases**:
   - Empty database (page_count=0)
   - Single-table database
   - Multi-table database
   - Database with indices
   - Database with BLOB data
   - Various page sizes (512, 1024, 4096, 8192, 16384)
   - Large databases (>10MB)

2. **Verification**:
   - All databases found at expected offsets
   - Sizes match exactly (page_count * page_size)
   - All marked as validated
   - Can be opened with `sqlite3` command-line tool
   - Schema and data can be queried

### Example Test

```rust
#[test]
fn test_sqlite_carver() {
    let config = default_config();
    let (metadata, output_dir) = carver_for_types(&["sqlite"], &config);
    verify_manifest_match(metadata, "sqlite");
    
    // Verify databases are valid
    for entry in metadata {
        let db_path = output_dir.join(&entry.path);
        assert!(verify_sqlite_integrity(&db_path));
    }
}
```

## Edge Cases Handled

1. **Empty database** (page_count=0): Carves single page
2. **Page size = 1**: Correctly interprets as 65536 bytes
3. **Huge page counts**: Respects max_size limit
4. **Truncated database**: Keeps partial database if > min_size
5. **WAL files**: Carves main database only (WAL files carved separately if present)
6. **Journal files**: Ignored (separate journal files not carved with main DB)

## Performance Characteristics

- **Metadata-driven**: No searching required (size known from header)
- **Memory usage**: Constant (streaming read of calculated size)
- **I/O pattern**: Single sequential read (very efficient)
- **No parsing**: Treats database as opaque byte blob

## Forensic Considerations

- **Deleted records**: Database may contain deleted data in free pages
- **WAL mode**: If database was in WAL mode, -wal and -shm files may exist separately
- **Corruption**: Page-by-page validation detects non-SQLite data; early termination prevents carving past the real database boundary
- **Timestamps**: Database header contains no timestamps (check file metadata)
- **Encryption**: Cannot detect if database is encrypted (SQLCipher uses same header)

## SQLite Page Structure Overview

```
Page 1 (Database Header Page):
  [100-byte header]
  [Page data...]

Page 2-N (B-tree Pages):
  [Page type]
  [Freeblock pointers]
  [Cell pointers]
  [Cell content]
  [Unallocated space]
```

## Page Types

- **Table B-tree interior page**: Index nodes for tables
- **Table B-tree leaf page**: Actual row data
- **Index B-tree interior page**: Index nodes for indices
- **Index B-tree leaf page**: Index entries
- **Freelist pages**: Available for reuse

## Known Limitations

1. **WAL files not included**: Write-Ahead Log files must be carved separately
2. **No deep integrity check**: Validates page-type bytes but does not verify full b-tree structure, cell pointers, or checksums
3. **Assumes contiguous**: Does not handle fragmented databases
4. **Page count trusted**: Relies on header metadata (could be incorrect in corrupted DB)

## Related Carvers

- **None directly** - SQLite is unique format
- Databases often found in:
  - Browser artifacts (cookies, history, etc.)
  - Mobile applications
  - Application data stores

## Recovery Techniques

For deeper analysis of carved SQLite databases:

1. **Integrity check**: `sqlite3 db.sqlite "PRAGMA integrity_check;"`
2. **Unallocated space**: Use specialized tools (e.g., SQLite Deleted Records Parser)
3. **WAL recovery**: If -wal file found, apply with `PRAGMA wal_checkpoint;`
4. **Schema extraction**: `sqlite3 db.sqlite ".schema"`
