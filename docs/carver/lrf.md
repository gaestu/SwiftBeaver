# LRF Carver

## Overview

The LRF (BroadBand eBook / BBeB) carver extracts Sony eBook files used primarily by the Sony Reader line of devices. The format was developed by Sony around 2004 and largely superseded by EPUB after 2010. LRF files contain structured document content including text, images, and metadata.

Due to the short 4-byte magic signature (`LRF\0`), this carver relies on extensive structural header validation to reject false positives.

## Signature Detection

**Header Pattern**: `4C 52 46 00` (ASCII: `LRF\0`)

This 4-byte magic number is at offset 0 of every LRF file.

## Header Structure

The carver reads and validates a 32-byte header:

```
Offset  Size  Field                   Validation
0       4     Magic ("LRF\0")         Must match exactly
4       2     Version (LE u16)        Must be > 0 and ≤ 10,000
6       2     Pseudo-DRM key (u16)    Not validated
8       4     File size (LE u32)      Must be > 0 and ≤ max_size
12      4     (reserved)              Not validated
16      4     Root object ID (LE u32) Must be non-zero
20      4     Number of objects (u32) Must be > 0 and ≤ 100,000
24      8     Object index offset     Must be > 0 and < file size
```

## Carving Algorithm

1. **Magic check**: Verify the 4-byte `LRF\0` signature
2. **Version validation**: Reject if version is 0 or exceeds 10,000
3. **Root object ID**: Reject if zero (every LRF must have a root object)
4. **Object count**: Reject if zero or exceeds 100,000
5. **Object index offset**: Reject if zero; if declared file size is known, reject if offset ≥ file size
6. **Declared size check**: Reject if declared size is 0 or exceeds `max_size` — the carver does **not** fall back to `max_size` for unknown sizes, as a missing or garbage size field is a strong false-positive indicator
7. **Extraction**: Write `declared_size` bytes starting from the header offset
8. **Min-size filter**: Discard if written bytes < `min_size`

## Validation

- **Validated**: `true` if all structural checks pass and the full declared size was written without truncation
- **Truncated**: `true` if EOF was reached before the declared size
- **Rejected** (returns `None`): Any structural check failure, zero or excessive declared size, or size below `min_size`

## False Positive Mitigation

The 4-byte magic `LRF\0` (`4C 52 46 00`) is short enough to match random binary data frequently. The previous implementation accepted hits with garbage size fields by falling back to `max_size`, which resulted in large (100+ MB) false-positive outputs.

Current mitigations:
1. **Six structural field checks** beyond the magic signature
2. **Strict size rejection** — zero or oversized declared values are rejected rather than clamped
3. **Reduced max_size** (20 MiB) — real LRF eBooks are typically 0.5–5 MB

## Size Constraints

- **Default min_size**: 64 bytes
- **Default max_size**: 20 MiB
- Configurable via `config/default.yml` under the `lrf` file type entry

## Hash Computation

- **MD5**: Computed incrementally during extraction
- **SHA-256**: Computed incrementally during extraction
- Hashes cover the entire carved output from header to end (or truncation point)

## Known Limitations

- **Obsolete format**: LRF was discontinued by Sony in favour of EPUB; encounters in modern forensic images are rare
- **Limited public specification**: The format was never formally standardised; field semantics are based on reverse-engineering efforts
- **No footer detection**: LRF files have no reliable footer marker; carving relies entirely on the declared size field
- **DRM files**: DRM-encrypted LRF files are carved as-is without decryption

## Forensic Considerations

- **Evidence integrity**: Input is never modified; extraction is read-only
- **Provenance**: Every carved file record includes `run_id`, `tool_version`, `config_hash`, and `evidence_path`
- **Reproducibility**: Deterministic output for the same input and configuration
