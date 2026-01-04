# ZIP Carver

## Overview

The ZIP carver extracts ZIP archives by detecting local file headers and validating the End of Central Directory (EOCD) record. It supports both standard ZIP files and ZIP-based formats (DOCX, XLSX, JAR, APK, etc.).

## Signature Detection

**Primary Pattern**: `PK\x03\x04` (Local File Header)
- Bytes: `50 4B 03 04`
- Marks the start of each file entry in the archive

**End Pattern**: `PK\x05\x06` (End of Central Directory)
- Bytes: `50 4B 05 06`
- Marks the end of the archive and contains directory metadata

## Carving Algorithm

The ZIP carver has two modes based on configuration:

### Mode 1: EOCD Required (`require_eocd: true`)

This is the default and most robust mode:

1. **Find EOCD**: Search forward from hit offset up to max_size for `PK\x05\x06`
2. **Parse EOCD**: Extract metadata from 22-byte EOCD structure:
   ```
   Offset  Size  Description
   0       4     Signature (0x06054b50)
   4       2     Disk number
   6       2     Central directory start disk
   8       2     Entries on this disk
   10      2     Total entries
   12      4     Central directory size
   16      4     Central directory offset
   20      2     Comment length
   22      N     Comment
   ```
3. **Calculate end**: `eocd_offset + 22 + comment_length`
4. **Copy range**: Copy from hit offset to calculated end
5. **Validation**: Marked as validated (EOCD found)

### Mode 2: Stream-based (`require_eocd: false`)

Less reliable but handles damaged archives:

1. **Parse local file headers** sequentially
2. **Follow directory structure** without seeking to EOCD
3. **Search for EOCD** in stream
4. **Mark validated** if EOCD found, truncated otherwise

## Validation

- **Validated**: `true` if EOCD record is found and parsed
- **Truncated**: `true` if:
  - max_size reached after EOCD
  - EOF reached before complete EOCD
- **Invalid**: Removed if:
  - EOCD not found (when required)
  - Kind filtering rejects ZIP type
  - Size below min_size

## Kind Filtering

The ZIP carver supports filtering by embedded file extensions:

```rust
allowed_kinds: Some(vec!["docx", "xlsx", "pptx", "jar", "apk"])
```

- Reads first local file header
- Extracts filename
- Checks extension against allowed list
- Returns `None` if extension not allowed

This prevents carving generic ZIP files when only Office documents are desired.

## Size Constraints

- **Default min_size**: 22 bytes (minimum for EOCD)
- **Default max_size**: 100 MB
- Files below min_size are discarded

## Hash Computation

- **MD5**: Computed incrementally during copy
- **SHA-256**: Computed incrementally during copy
- Covers complete archive from local file header to end of EOCD comment

## Testing

**Test file**: `tests/carver_zip.rs`

### Test Strategy

Comprehensive ZIP variant testing:

1. **Test archives**:
   - Standard ZIP files
   - DOCX (ZIP with XML)
   - XLSX (ZIP with XML)
   - JAR (ZIP with Java classes)
   - APK (ZIP with Android assets)
   - Encrypted ZIPs
   - Multi-part ZIPs
   - ZIP64 (large archives)

2. **Verification**:
   - All ZIPs found at expected offsets
   - Sizes match manifest
   - EOCD validation status correct
   - Files can be opened/extracted

### Example Test

```rust
#[test]
fn test_zip_with_eocd() {
    let config = default_config();
    let (metadata, _) = carver_for_types(&["zip"], &config);
    verify_manifest_match(metadata, "zip");
    assert!(metadata.iter().all(|m| m.validated));
}
```

## Edge Cases Handled

1. **ZIP comment**: Correctly reads variable-length comment after EOCD
2. **ZIP64 format**: Handles extended EOCD (64-bit offsets)
3. **Encrypted entries**: Carves complete archive (doesn't attempt decryption)
4. **Damaged central directory**: Keeps file if local headers are intact
5. **Nested ZIPs**: Carves outer ZIP only (inner ZIPs carved separately)
6. **EOCD at max_size boundary**: Handles truncation correctly

## Performance Characteristics

- **EOCD search**: Linear search through evidence (optimized with buffer)
- **Memory usage**: Constant (64KB buffers for search and copy)
- **I/O pattern**: 
  - Forward search (reading)
  - Sequential copy (writing)
- **No decompression**: Archives copied as-is (compressed)

## Forensic Considerations

- **Deleted files**: ZIPs with missing local files but intact EOCD are preserved
- **Corruption tolerance**: Keeps archives with mismatched central directory
- **Timestamp preservation**: EOCD contains modification time metadata
- **Encryption detection**: File headers indicate if entries are encrypted

## ZIP Structure Overview

```
[Local File Header 1] (PK\x03\x04)
  [File Data 1] (compressed or stored)
[Local File Header 2] (PK\x03\x04)
  [File Data 2]
...
[Central Directory Header 1] (PK\x01\x02)
[Central Directory Header 2]
...
[End of Central Directory] (PK\x05\x06)
  [Comment] (optional, variable length)
```

## ZIP-based Format Examples

### DOCX (Office Open XML)
```
document.xml (content)
_rels/.rels (relationships)
word/ (document parts)
```

### JAR (Java Archive)
```
META-INF/MANIFEST.MF
*.class (Java bytecode)
```

### APK (Android Package)
```
AndroidManifest.xml
classes.dex (Dalvik bytecode)
resources.arsc
```

## Related Carvers

- **RAR**: Alternative archive format (proprietary compression)
- **7Z**: Alternative archive format (LZMA compression)
- **TAR**: Uncompressed archive format
- **GZIP**: Single-file compression (often used with TAR)
- **OLE**: Office documents (older format, DOC/XLS/PPT)
