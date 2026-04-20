# SQLite Carve-Only Handoff Workflow

This workflow describes how to hand off carved SQLite artifacts to external analysis tools.
It is intentionally tool-agnostic.

## Scope

SwiftBeaver recovers SQLite-related files as carved artifacts:

- `sqlite` (main database files)
- `sqlite_wal` (WAL sidecar files)
- `sqlite_page` (single-page fragments)

SwiftBeaver does not parse browser rows from these artifacts in-pipeline.

## 1. Carve Target Types

Use type filters so output is focused and repeatable.

Example:

```bash
swiftbeaver \
  --input <evidence> \
  --output <out_dir> \
  --enable-types sqlite,sqlite_wal,sqlite_page
```

Optional hardening knobs:

- `sqlite_page_max_hits_per_chunk`
- `sqlite_wal_max_consecutive_checksum_failures`

## 2. Collect Provenance

Use `metadata/carved_files.*` as the source of truth for:

- `path`
- `file_type`
- `global_start` / `global_end`
- `size`
- `sha256` / `md5`
- `validated`, `truncated`, `errors`

Keep these fields with downstream analysis artifacts so findings remain traceable to evidence offsets.

## 3. Triage by Confidence

Suggested triage order:

1. `validated=true` and `truncated=false`
2. `validated=true` with warnings
3. `truncated=true`
4. explicit `errors` entries

For WAL files, checksum-threshold behavior matters:

- traversal may include mismatching frames before stop threshold is reached
- use stricter threshold (`0`) when you need stop-on-first-mismatch behavior

## 4. External Parsing Pass

Run your preferred SQLite-capable tooling over:

- `carved/sqlite/`
- `carved/sqlite_wal/`
- `carved/sqlite_page/`

Recommended analysis approach:

1. Analyze intact DB files first.
2. Apply WAL-aware analysis for recovered WAL files.
3. Treat `sqlite_page` files as fragment evidence; parse with lower confidence and explicit provenance tags.

## 5. Correlation and Deduplication

When merging external results:

- deduplicate by content hash + record key
- retain the originating carved file path
- retain original evidence offset range
- annotate confidence by source (`sqlite` > `sqlite_wal` > `sqlite_page`)

## 6. Reporting Guidance

In reports, separate:

- directly queryable full databases
- WAL-derived findings
- fragment-derived findings

Include carve metadata references for every finding so results are reproducible.
