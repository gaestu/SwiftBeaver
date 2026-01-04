# RAR Carver

## Overview

The RAR carver extracts RAR archive files by detecting the format version (RAR 4.x or RAR 5.x), parsing the block/header structure, and finding the archive end marker.

## Signature Detection

**RAR 4.x Pattern**: `Rar!\x1A\x07\x00` (7 bytes)
- Bytes: 0x52 0x61 0x72 0x21 0x1A 0x07 0x00

**RAR 5.x Pattern**: `Rar!\x1A\x07\x01\x00` (8 bytes)
- Bytes: 0x52 0x61 0x72 0x21 0x1A 0x07 0x01 0x00

## Carving Algorithm

The RAR carver handles two distinct formats:

### RAR 4.x Format

Block-based structure with fixed-size headers:

```rust
loop {
    // Read block header (7 bytes minimum)
    let header = read_bytes(ctx, offset, 7)?;
    let block_crc = u16::from_le_bytes([header[0], header[1]]);
    let block_type = header[2];
    let block_flags = u16::from_le_bytes([header[3], header[4]]);
    let block_size = u16::from_le_bytes([header[5], header[6]]);
    
    // Check for terminator
    if block_type == RAR4_HEAD_END (0x7B) {
        offset += block_size;
        break;  // Found end marker
    }
    
    // Skip this block
    offset += block_size;
}
```

**RAR 4.x Block Types**:
- `0x73`: Marker block
- `0x74`: Archive header
- `0x75`: File header
- `0x76`: Comment header
- `0x77`: Old-style authentication
- `0x78`: Old-style subblock
- `0x79`: Recovery record
- `0x7A`: Authenticity information
- `0x7B`: Subblock (also used as terminator)

### RAR 5.x Format

Variable-length header structure with vint encoding:

```rust
loop {
    // Read header (variable length)
    let header_crc = read_u32_le(ctx, offset)?;
    let header_size_vint = read_vint(ctx, offset + 4)?;
    let header_type_vint = read_vint(ctx, offset + 4 + header_size_vint.len)?;
    
    // Check for terminator
    if header_type == RAR5_HEAD_END (5) {
        offset += total_header_size;
        break;  // Found end marker
    }
    
    // Skip header + data
    offset += header_size + data_size;
}
```

**RAR 5.x Header Types**:
- `1`: Main archive header
- `2`: File header
- `3`: Service header
- `4`: Archive encryption header
- `5`: End of archive header

## Validation

- **Validated**: `true` if:
  - Format signature matches (RAR 4.x or RAR 5.x)
  - End marker found (block type 0x7B or header type 5)
- **Truncated**: `true` if:
  - max_size reached before end marker
  - EOF reached before end marker
- **Invalid**: Removed if:
  - Signature mismatch
  - Block/header structure invalid

## Size Constraints

- **Default min_size**: 100 bytes
- **Default max_size**: 500 MB
- Minimum viable RAR: ~20 bytes (signature + minimal headers + end marker)
- Files below min_size are discarded

## Hash Computation

- **MD5**: Computed via `write_range` during copy
- **SHA-256**: Computed via `write_range` during copy
- Covers complete archive from signature to end marker

## Testing

**Test file**: `tests/carver_rar.rs`

### Test Strategy

Golden image framework with both RAR versions:

1. **Test archives**:
   - RAR 4.x archives
   - RAR 5.x archives
   - Encrypted RAR files
   - Solid archives (multiple files compressed together)
   - Multi-volume RAR (part001, part002, etc.)
   - Recovery records
   - Archives with comments

2. **Verification**:
   - All RARs found at expected offsets
   - Sizes match manifest
   - Version detection correct (4.x vs 5.x)
   - Archives can be extracted with `unrar`

## Edge Cases Handled

1. **RAR version detection**: Automatically detects RAR 4.x vs 5.x
2. **Variable-length integers**: RAR 5.x uses vint encoding for sizes
3. **Encrypted archives**: Carves complete archive (decryption not performed)
4. **Recovery records**: Includes optional recovery data
5. **Multi-volume archives**: Carves individual volume (not reassembled)
6. **Large headers**: RAR 5.x headers can be >1MB (safety limit applied)

## Performance Characteristics

- **Block iteration**: Many small reads (block/header structures)
- **Memory usage**: Constant (only header data loaded)
- **I/O pattern**: Sequential reads with variable-length skips
- **No decompression**: Archive copied as-is (compressed)

## Forensic Considerations

- **Encryption**: Cannot extract if archive is password-protected
- **Solid archives**: Files compressed together (cannot extract individual files without full archive)
- **Timestamps**: File headers contain modification times
- **Compression history**: RAR 5.x has better compression than RAR 4.x
- **Proprietary format**: RAR is proprietary (UNRAR source available but restricted license)

## RAR 4.x Structure Example

```
[RAR Signature: 7 bytes]
  "Rar!\x1A\x07\x00"

[Marker Block]
  CRC: 0x6152
  Type: 0x72
  Flags: 0x1A21
  Size: 7

[Archive Header Block]
  CRC: 0x1234
  Type: 0x73
  Flags: 0x0000
  Size: 13
  [Archive flags, etc.]

[File Header Block]
  CRC: 0x5678
  Type: 0x74
  Flags: 0x8000 (has data)
  Size: 45
  [Compressed size, uncompressed size, filename, etc.]
  [Compressed file data]

[File Header Block]
  ...

[End Block]
  CRC: 0xC43D
  Type: 0x7B
  Flags: 0x4000
  Size: 7
```

## RAR 5.x Structure Example

```
[RAR Signature: 8 bytes]
  "Rar!\x1A\x07\x01\x00"

[Main Archive Header]
  CRC32: 0x12345678
  Header size: vint(0x0A)
  Header type: vint(1)
  Flags: vint(0)
  [Archive flags, volume number, etc.]

[File Header]
  CRC32: 0x9ABCDEF0
  Header size: vint(0x45)
  Header type: vint(2)
  Flags: vint(0x03)
  [File size vint, attributes vint, mtime, filename, ...]

[Compressed Data]
  [File data - size determined from header]

[End of Archive Header]
  CRC32: 0xAABBCCDD
  Header size: vint(0x07)
  Header type: vint(5)
  Flags: vint(0)
```

## Variable Integer (vint) Encoding

RAR 5.x uses variable-length integers:

```
0x00-0x7F: 1 byte (value = byte)
0x80-0xBFFF: 2 bytes (value = ((byte1 & 0x3F) << 8) | byte2)
0xC0-0xDFFFFF: 3 bytes
... up to 10 bytes for largest values
```

## Known Limitations

1. **Multi-volume not reassembled**: Each volume carved separately
2. **No decryption**: Encrypted archives not decrypted
3. **No decompression validation**: Compressed data not validated
4. **RAR 1.x/2.x/3.x not supported**: Only RAR 4.x and RAR 5.x

## Related Carvers

- **ZIP**: Alternative archive format (open standard)
- **7Z**: Alternative archive format (LZMA compression)
- **TAR**: Uncompressed archive format
- **GZIP**: Single-file compression
