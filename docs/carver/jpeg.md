# JPEG Carver

## Overview

The JPEG carver extracts JPEG/JFIF images from raw forensic evidence by detecting the JPEG header signature and streaming data until the End of Image (EOI) marker is found.

## Signature Detection

**Header Pattern**: `FF D8` (Start of Image - SOI marker)

The JPEG carver triggers when the scanner identifies a `FF D8` byte sequence. All valid JPEG files begin with this signature.

## Carving Algorithm

The carver uses a streaming, two-phase JPEG marker walker. A naive
"first `FF D9` wins" search is incorrect: `FF D9` legitimately appears
inside `APP1` Exif thumbnails, `APP2` MPF embedded JPEGs, and other
length-prefixed segments, which would cause the file to be truncated at
the embedded thumbnail's EOI rather than the real one.

### Phase 1 — Header walk (until SOS)

The walker parses each marker by length and skips its payload exactly:

- Skip fill bytes (`FF FF ...`).
- Treat standalone markers (`FF 01`, `FF D0..FF D8`) as zero-length.
- For all other markers (APPn, COM, DQT, DHT, SOFn, ...), read the
  big-endian `u16` length field and skip exactly that many bytes
  (the length includes the two length bytes themselves).
- Reject the carve if a segment length is `< 2` (malformed).
- On `FF DA` (SOS), consume the SOS segment header (also length-prefixed)
  and transition to phase 2.

Because the walker honours segment lengths, embedded JPEGs inside
`APPn` payloads are skipped over as opaque bytes — their internal
`FF D9` markers can never be mistaken for the outer EOI.

### Phase 2 — Scan walk (entropy-coded stream)

Once inside the entropy-coded stream:

- `FF 00` is a stuffed literal `0xFF` (continue).
- `FF D0..FF D7` are restart markers (continue).
- `FF D9` is the true End-Of-Image — stop, mark `validated = true`.
- `FF DA` is another SOS segment (progressive multi-scan JPEGs); the
  walker re-enters the SOS-header path, then resumes scanning.
- Any other `FF xx` mid-scan is treated as a length-bearing marker
  (e.g. a `DHT` between scans of a progressive image): the length is
  read and the segment is consumed before scanning resumes.

### State machine (summary)

```text
HeaderExpectFF → HeaderExpectMarker → HeaderLen{Hi,Lo} → HeaderSkip(N)
                                  ↘ (FF DA) → SosLen{Hi,Lo} → SosSkip(N) → Scan
HeaderExpectMarker on standalone markers (01, D0..D8) → HeaderExpectFF

Scan → ScanFF → {
    FF 00          → Scan        (stuffed byte)
    FF D0..FF D7   → Scan        (restart marker)
    FF D9          → DONE         (validated)
    FF DA          → SosLenHi    (progressive multi-scan)
    FF other       → ScanLen{Hi,Lo} → ScanSkip(N) → Scan
}
```

### Truncation and rejection

- `max_size` reached before EOI → `truncated = true`, file kept.
- EOF reached before EOI → `truncated = true`, file kept.
- Malformed structure (segment length `< 2` in any phase) → carve dropped
  entirely; `process_hit` returns `Ok(None)` and no file is written.

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

1. **Embedded thumbnail JPEGs** (e.g. inside `APP1` Exif or `APP2` MPF segments):
   the inner thumbnail's `FF D9` is contained within a length-prefixed segment
   and is skipped over as opaque payload — the outer carve runs through to the
   real EOI rather than truncating at the thumbnail. (Fix for issue #77.)
2. **Progressive JPEGs** with multiple `FF DA` (SOS) segments: each scan is
   carried through; additional inter-scan length-prefixed segments
   (e.g. `DHT`/`DQT`) are consumed correctly.
3. **Stuffed bytes** (`FF 00`) inside the entropy stream: treated as literal
   `0xFF` and never mistaken for a marker.
4. **Restart markers** (`FF D0` – `FF D7`): scan continues across them.
5. **Malformed segment length** (`length < 2`): the carve is rejected
   (`process_hit` returns `Ok(None)`) — no partial file is written.
6. **Truncated files** (EOF before EOI): kept with `truncated = true` and
   `validated = false` if they exceed `min_size`.
7. **False positives**: rejected if the header signature does not match
   `FF D8 FF <valid first marker>`.
8. **Very small fragments**: discarded if below the `min_size` threshold.

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
