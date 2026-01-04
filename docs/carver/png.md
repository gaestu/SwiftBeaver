# PNG Carver

## Overview

The PNG carver extracts Portable Network Graphics files by parsing the chunk-based structure and validating against the PNG specification.

## Signature Detection

**Header Pattern**: `89 50 4E 47 0D 0A 1A 0A` (PNG magic bytes)

This 8-byte signature is mandatory for all PNG files and includes:
- `89`: High bit set (not valid ASCII)
- `PNG`: ASCII characters
- `0D 0A`: DOS line ending
- `1A`: EOF character (DOS)
- `0A`: Unix line ending

## Carving Algorithm

The PNG carver uses chunk-based validation:

1. **Signature validation**: Verifies exact 8-byte header match
2. **Chunk iteration**: Reads chunks sequentially
   - Each chunk: `[4-byte length][4-byte type][data][4-byte CRC]`
3. **Chunk parsing**:
   ```
   Read 4 bytes → chunk length (big-endian u32)
   Read 4 bytes → chunk type (ASCII, e.g. "IHDR", "IDAT", "IEND")
   Read length bytes → chunk data
   Read 4 bytes → CRC-32 checksum
   ```
4. **Termination**: Loop until "IEND" chunk type is found
5. **Validation**: File is validated only if IEND is reached

### Chunk Types

Common PNG chunks encountered:
- **IHDR**: Image header (must be first chunk)
- **IDAT**: Image data (can be multiple)
- **IEND**: End of image (must be last chunk)
- **PLTE**: Palette (optional)
- **tRNS**: Transparency (optional)
- Many others (text, metadata, etc.)

## Validation

- **Validated**: `true` if IEND chunk is successfully parsed
- **Truncated**: `true` if max_size or EOF reached before IEND
- **Invalid**: Removed if:
  - Header signature doesn't match
  - Chunk type is not valid UTF-8
  - Chunk length exceeds max_size

## Size Constraints

- **Default min_size**: 100 bytes (as of v0.2.1)
- **Default max_size**: 100 MB
- Files smaller than `min_size` are discarded
- Files exceeding `max_size` are truncated

## Hash Computation

- **MD5**: Computed incrementally via `CarveStream`
- **SHA-256**: Computed incrementally via `CarveStream`
- Hashes cover entire carved data (signature + all chunks up to IEND or truncation)

## Testing

**Test file**: `tests/carver_png.rs`

### Test Strategy

Uses golden image framework:

1. **Manifest-based testing**: `manifest.json` lists expected PNGs
2. **Execution**: `carver_for_types(&["png"], &config)`
3. **Verification**:
   - Count matches expected
   - Sizes match exactly
   - All files exist and are valid
4. **Coverage**: Tests multiple PNG variants:
   - Grayscale
   - Indexed color (with palette)
   - RGB
   - RGBA (with transparency)
   - Interlaced
   - Non-interlaced

### Example Manifest Entry

```json
{
  "offset": 1048576,
  "size": 54328,
  "file_type": "png",
  "validated": true
}
```

## Edge Cases Handled

1. **Large chunk lengths**: Validates chunk length doesn't exceed max_size before reading
2. **Invalid chunk types**: Returns error if chunk type contains non-UTF-8 bytes
3. **Missing IEND**: File is truncated but kept if substantive data exists
4. **Zero-length chunks**: Handles chunks with length=0 (e.g., IEND)
5. **Multiple IDAT chunks**: Correctly reads through all data chunks
6. **Malformed CRC**: Reads CRC but doesn't validate (forensic mode - keep corrupted files)

## Performance Characteristics

- **Streaming**: Uses `CarveStream` for efficient evidence reading
- **Memory usage**: Constant (small buffer for chunk headers)
- **I/O pattern**: Sequential reads of exact chunk sizes
- **Chunk skipping**: Efficient for large IDAT chunks (direct stream advancement)

## Forensic Considerations

- **Corruption tolerance**: Keeps files with CRC errors (doesn't validate checksums)
- **Partial files**: Preserves truncated PNGs for potential analysis
- **Metadata preservation**: Carves entire file including text/metadata chunks
- **Provenance tracking**: Records validation status and any truncation errors

## Related Carvers

- **WEBP**: Similar chunk-based structure but different signature
- **TIFF**: Alternative image format with tag-based structure
- **GIF**: Alternative image format with block-based structure
