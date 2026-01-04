# SQLite Carver

## Overview

The SQLite carver extracts SQLite database files by detecting the database header, validating the page size, and calculating the total database size based on the page count metadata.

## Signature Detection

**Header Pattern**: `SQLite format 3\0` (16 bytes)
- Bytes: `53 51 4C 69 74 65 20 66 6F 72 6D 61 74 20 33 00`
- This signature appears at offset 0 of every SQLite3 database file

## Carving Algorithm

The SQLite carver uses metadata-driven size calculation:

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

### 3. Database Size Calculation

```rust
let page_count = u32::from_be_bytes([header[28], header[29], header[30], header[31]]);
let total_size = if page_count == 0 {
    page_size as u64  // Single-page database
} else {
    page_size as u64 * page_count as u64
};
```

### 4. Carving

```rust
let target_size = total_size.min(max_size);
stream.read_exact((target_size - 100) as usize)?;
```

Reads exactly the calculated size (or max_size, whichever is smaller).

## Validation

- **Validated**: `true` if:
  - Header matches "SQLite format 3\0"
  - Page size is valid
  - Complete database size is read
- **Truncated**: `true` if:
  - EOF reached before complete size
  - max_size enforced
- **Invalid**: Removed if:
  - Header mismatch
  - Page size invalid
  - Total size < 100 bytes

## Size Constraints

- **Default min_size**: 100 bytes (size of SQLite header)
- **Default max_size**: 1 GB
- Minimum viable SQLite: 512 bytes (single page at min page size)
- Files below min_size are discarded

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
- **Corruption**: Carves database even if corrupted (integrity check not performed)
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
2. **No integrity check**: Does not validate b-tree structure or checksums
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
