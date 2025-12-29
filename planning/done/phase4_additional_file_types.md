# Phase 4: Additional File Types (BMP, TIFF, MP4, RAR, 7z)

Status: Implemented  
Implemented in version: unreleased

## Problem statement
We need to expand carving coverage to common forensic file formats beyond the current set. The requested additions are BMP, TIFF, MP4, RAR, and 7z.

## Scope
- Add signature patterns and default config entries for BMP, TIFF, MP4, RAR, and 7z.
- Implement carve handlers for each type with best-effort validation and size determination.
- Wire handlers into the carve registry.
- Add unit tests that validate size detection/carving for each new type.
- Update README and docs/config to document new types and validators.

## Non-goals
- Deep container parsing (e.g., full MP4/RAW stream extraction).
- Recovery of fragmented/non-contiguous files.
- Metadata extraction from these formats.

## Design notes
- BMP: size from BITMAPFILEHEADER (little-endian u32 at offset 2).
- TIFF: parse IFD chain; use strip/tile offsets + byte counts to estimate end; follow known sub-IFD tags.
- MP4: parse top-level box sizes sequentially; require `ftyp` at start and `moov` present; stop on invalid/truncated.
- RAR: support RAR4 (header chain + end-of-archive) and RAR5 (varint headers; stop at end header).
- 7z: use start header (next header offset/size) to compute total size.

## Expected tests
- Unit tests for each new handler with synthetic minimal samples.
- Validate size computation, `validated` flag, and basic output properties.

## Impact on docs/README
- Update file type lists and examples in `README.md`.
- Extend `docs/config.md` validator list and sample `file_types` entries.
