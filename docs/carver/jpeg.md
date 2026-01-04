# JPEG Carver

## Overview

The JPEG carver extracts JPEG/JFIF images from raw forensic evidence by detecting the JPEG header signature and streaming data until the End of Image (EOI) marker is found.

## Signature Detection

**Header Pattern**: `FF D8` (Start of Image - SOI marker)

The JPEG carver triggers when the scanner identifies a `FF D8` byte sequence. All valid JPEG files begin with this signature.

## Carving Algorithm

The carver uses a streaming approach with a simple state machine:

1. **Signature validation**: Verifies the hit matches `FF D8` exactly
2. **Streaming search**: Reads data chunk-by-chunk (64KB blocks) looking for the EOI marker
3. **EOI detection**: Uses a 2-byte sliding window to detect `FF D9` byte-by-byte
   - Maintains `prev` byte from previous iteration
   - For each byte `cur`:
     - Check if `prev == 0xFF && cur == 0xD9`
     - If match: mark as validated and break
     - Update `prev = cur`
4. **Truncation handling**: 
   - If `max_size` is reached before EOI → marks as truncated, keeps file
   - If EOF is reached before EOI → marks as truncated, keeps file

### State Machine

```
START
  ↓
[Read header FF D8]
  ↓
[Stream data in 64KB chunks]
  ↓
  ├─ Found FF D9? → VALIDATED (break)
  ├─ Reached max_size? → TRUNCATED (break)
  ├─ Reached EOF? → TRUNCATED (break)
  └─ Continue reading
```

## Validation

- **Validated**: `true` if EOI marker (`FF D9`) is found
- **Truncated**: `true` if max_size or EOF reached before EOI
- **Invalid**: Removed if header signature doesn't match `FF D8`

## Size Constraints

- **Default min_size**: 500 bytes (as of v0.2.1)
- **Default max_size**: 100 MB
- Files smaller than `min_size` are discarded
- Files exceeding `max_size` are truncated but kept

## Hash Computation

- **MD5**: Computed incrementally as data streams
- **SHA-256**: Computed incrementally as data streams
- Both hashes cover only the carved data (from SOI to EOI or truncation point)

## Testing

**Test file**: `tests/carver_jpeg.rs`

### Test Strategy

The JPEG carver is tested using the golden image framework:

1. **Golden image**: `tests/golden_image/golden.bin` contains a known JPEG at a specific offset
2. **Manifest**: `tests/golden_image/manifest.json` lists expected JPEGs with:
   - `offset`: Global offset where the JPEG starts
   - `size`: Expected file size in bytes
   - `file_type`: "jpeg"
3. **Test execution**:
   ```rust
   let (metadata, _) = carver_for_types(&["jpeg"], &config);
   ```
4. **Verification**:
   - All expected JPEGs from manifest are found
   - Each carved file has correct size (exact match)
   - Files exist on disk and are readable

### Example Test Output

```
Running: tests/carver_jpeg.rs
  ✓ All expected JPEG files found (12/12)
  ✓ All sizes match manifest
  ✓ All files exist on disk
  ✓ MD5/SHA256 computed for all files
```

## Edge Cases Handled

1. **Embedded JPEG restart markers** (`FF D0` - `FF D7`): Correctly skipped, do not trigger false EOI
2. **FF byte followed by non-D9**: Continues scanning (common in compressed data)
3. **Truncated files**: Kept if they contain substantive data (>500 bytes default)
4. **False positives**: Removed if header doesn't match after streaming starts
5. **Very small fragments**: Discarded if below min_size threshold

## Performance Characteristics

- **Streaming**: Uses `CarveStream` abstraction to handle evidence efficiently
- **Memory usage**: Constant (~64KB buffer regardless of image size)
- **I/O pattern**: Sequential reads from evidence source
- **Hash computation**: Parallel MD5/SHA256 with zero-copy where possible

## Forensic Considerations

- **Evidence integrity**: Never modifies source evidence
- **Reproducibility**: Same input → same output (deterministic hashing)
- **Provenance**: Each carved file includes:
  - `run_id`: Unique run identifier
  - `global_start`: Offset where JPEG was found
  - `global_end`: Offset where carving stopped
  - `validated`: Whether EOI was found
  - `truncated`: Whether file was truncated
  - `errors`: Any errors encountered (if truncated)

## Related Carvers

- **TIFF**: Also uses marker-based structure but different format
- **WEBP**: Can contain JPEG-compressed frames
- **BMP**: Similar image format but no compressed encoding
