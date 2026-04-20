# SQLite Page Fragment Carver

## Overview

The SQLite page carver recovers individual SQLite leaf-page fragments as standalone files.
It is carve-only: no row-level interpretation is performed in the pipeline.

## Signature Detection

Candidate markers:

- `0D` (table leaf page)
- `0A` (index leaf page)

Because these are single-byte markers, the handler applies strict structure validation before carving.

## Validation

The handler infers page size by trying:

`[4096, 1024, 2048, 8192, 16384, 32768, 65536, 512]`

A candidate is accepted only when checks pass, including:

- valid page header type and bounds
- non-zero cell count
- sane pointer-table bounds
- cell pointers within page bounds and non-duplicate
- freeblock chain bounds and loop prevention
- fragmented free bytes sanity

## Carving Strategy

- Carves one validated page per accepted hit.
- Output path: `carved/sqlite_page/*.sqlite-page`.
- File type id: `sqlite_page`.

## False-Positive Controls

- Structural validation-first policy.
- Per-chunk hit cap via `sqlite_page_max_hits_per_chunk`.
- Scanner-side drop logging when cap is exceeded.

## Metadata

Page fragment outputs are recorded in `metadata/carved_files.*` with `file_type="sqlite_page"`.
No browser row metadata is emitted from page fragments in carve-only mode.

## Known Limits

- No multi-page logical reconstruction.
- No schema reconstruction.
- No cross-page record reassembly.
