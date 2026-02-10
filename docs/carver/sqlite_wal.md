# SQLite WAL Carver

## Overview

The SQLite WAL carver extracts SQLite write-ahead log sidecar files as opaque byte artifacts.
It is carve-only: SwiftBeaver does not parse rows from WAL frames in-pipeline.

## Signature Detection

- Header magic at offset 0:
  - `37 7F 06 82`
  - `37 7F 06 83`

## Validation

The handler validates:

- WAL magic and supported version (`3007000`)
- Page size field (`512..65536`, power-of-two, with `1 => 65536`)
- WAL header rolling checksum
- Per-frame sanity while walking:
  - page number is non-zero
  - salts match WAL header salts
  - rolling checksum behavior is bounded by
    `sqlite_wal_max_consecutive_checksum_failures`

## Carving Strategy

1. Parse and validate the 32-byte WAL header.
2. Walk frames (`24-byte header + page_size payload`) sequentially.
3. Stop when frame structure fails, checksum threshold is exceeded, max size is reached, or EOF is hit.
4. Write carved bytes to `carved/sqlite_wal/*.sqlite-wal`.

## Configuration

- File type: `sqlite_wal`
- Extension: `sqlite-wal`
- Config knob: `sqlite_wal_max_consecutive_checksum_failures`

Checksum threshold semantics:

- It controls when traversal stops.
- It does not retroactively remove already traversed mismatching frame bytes.
- Set to `0` for stop-on-first-checksum-mismatch behavior.

## Metadata

WAL outputs are recorded in `metadata/carved_files.*` with `file_type="sqlite_wal"`.
No browser row metadata is emitted from WAL content in carve-only mode.

## Known Limits

- No row extraction from frames.
- No WAL+main-db merge/checkpoint reconstruction.
- No cross-file correlation in the pipeline.
