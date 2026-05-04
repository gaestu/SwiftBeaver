# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

- No changes yet.

## 0.6.1 (2026-05-04)
- Fix release pipeline

## 0.6.0 (2026-05-04)

### Schema
- Run summary metadata now includes `files_capped` in CSV, JSONL, and Parquet outputs. The Parquet run summary schema adds a non-nullable `Int64` `files_capped` column, so downstream tools that pin the exact run-summary schema should handle this as a schema change.
- Added BitLocker BEK metadata outputs: `metadata/bitlocker_bek.{jsonl,csv}` and `parquet/artefacts_bitlocker_bek.parquet`, each carrying provenance plus key identifier GUID, optional description, key data length, key method, and FILETIME for structurally valid binary `.bek` files.
- Phone artefact metadata is now validated-only. JSONL and Parquet phone artefact outputs include normalized E.164 and country fields when accepted. Parquet output adds `artefacts_phones_summary.parquet`, and run-summary metadata in CSV, JSONL, and Parquet adds deterministic phone candidate, rejection, validation, duplicate, capped-occurrence, distinct, and repeated-value counters.

### Changed
- SQLite carver `validated` flag is now stricter: in addition to the existing magic, page-size, valid-page-ratio, and consecutive-invalid checks, every examined B-tree page (`0x02`, `0x05`, `0x0A`, `0x0D`) must also pass deep structural validation (cell count, cell pointer table, cell content area, freeblock chain). Page-type plausibility alone is no longer sufficient. When deep validation finds failures, an entry is appended to `errors` of the form `deep b-tree validation: N of M pages failed structural checks`. The `validated` field schema is unchanged. Closes #83.

### Fixed
- XZ carving now rejects corrupt or truncated candidates before writing output. The carver requires matching header/footer stream flags plus a CRC-checked, internally consistent XZ Index before marking a stream `validated=true`, and no longer persists `validated=false`, `truncated=true` fallback files when no footer is found. Closes #28.
- EVTX `windows_artefacts` rows are no longer dropped when a chunk header declares an implausible record-number range. The carver now reports `record_count_estimate` as `null` when any chunk's `(last_record - first_record + 1)` exceeds `i64::MAX` or when the running sum would overflow `i64`, and the Parquet sink falls back to `NULL` instead of returning a metadata error for the whole row. Closes #80.
- SQLite carver no longer emits standalone `sqlite` databases for `SQLite format 3\0` magic that occurs inside a SQLite WAL frame payload. `pre_validate` now walks back through possible WAL frame boundaries (bounded by the new `sqlite_suppress_wal_frame_lookback_frames` config knob, default `64`) and rejects candidates that sit inside a valid WAL frame. To preserve evidence, suppression requires the full frame chain from the WAL header through the candidate frame to satisfy the same acceptance rules as the `sqlite_wal` carver — matching salts, non-zero page numbers, and valid rolling frame checksums — so a stale or checksum-invalid WAL header cannot cause a real standalone database to be dropped. The WAL itself is still carved by `sqlite_wal`. Closes #82.
- `sqlite_page` carver no longer emits overlapping nested page fragments. The carve worker pool now feeds a streaming **overlap arbiter** thread: scan workers assign deterministic hit sequences by signature position, carve workers return one completion event per hit, and the arbiter accepts the first sequenced non-overlapping final carve range via a per-type interval check. Rejected jobs have their staging file discarded (`PendingCarve::discard`) before consuming any `max_files` output slot, and `overlap_skipped` is incremented. `max_files` cap drops are now counted separately as `files_capped`. This eliminates two race classes that any pre-claim or in-flight greedy approach is vulnerable to: a false-positive signature byte starving a real concurrent page, and nested candidates slipping past a too-small pre-claim for larger SQLite page sizes (8/16/32/64 KiB). Because arbitration uses a stable evidence-order sequence and checks final carved ranges, the chosen winner is reproducible across runs and worker counts. Closes #84.

### Added
- Added a native Windows EWF release artifact that builds and bundles pinned upstream `libewf` runtime DLLs alongside `swiftbeaver.exe`, while preserving the existing Windows CPU-only ZIP without E01 support. Closes #91.
- Added a `cargo-about` workflow and `scripts/generate-third-party-licenses.sh` to generate `dist/THIRD_PARTY_LICENSES.txt` from the resolved Cargo dependency graph for release artifacts. Closes #3.
- EWF segment cache run-summary observability now reports cache hits, misses, hit rate, and bytes served from cache in the final tracing log line. Closes #55.
- `phone_mode`, `phone_default_region`, and `phone_supported_regions` configuration for local/offline validated phone extraction. `--scan-phones` and `--no-scan-phones` remain compatible aliases for validated/off behavior. Closes #10.
- `sqlite_suppress_wal_frame_lookback_frames` configuration knob to bound the WAL-frame lookback search performed by the SQLite carver. Set to `0` to check only the immediate `wal_start = offset - 56` candidate.
- BitLocker External Key (`.bek`) carver. The carver uses structural BEK/FVE metadata validation rather than filename or extension trust, carves valid BEK files to the output directory, emits BEK-specific metadata, and stays separate from textual BitLocker recovery password detection and `.KPG` key packages. Closes #89.
- BitLocker recovery password detection in string artefact extraction. New `enable_bitlocker_recovery_scan` config flag (default `true`) and matching CLI flags `--scan-bitlocker-recovery` / `--no-scan-bitlocker-recovery`. New `ArtefactKind::BitlockerRecoveryPassword` variant and a new Parquet category file `artefacts_bitlocker_recovery_passwords.parquet` (CSV/JSONL outputs continue to share `string_artefacts.{csv,jsonl}`). Detection accepts hyphen- or whitespace-separated 8 × 6-digit passwords, validates each group is divisible by 11 with quotient ≤ `0xFFFF`, and canonicalises stored content to hyphenated form. Textual recovery passwords only; `.bek` recovery key files and BitLocker volume unlock are out of scope. Closes #88.

## 0.5.1 (2026-04-27)

### Fixed
- Rebuilt release artifacts from the corrected 0.5.x release commit so packaged binaries report the expected version.
- Fixed no-default-features Clippy and rustdoc warning failures in CI.

## 0.5.0 (2026-04-27)

### Added
- Windows Shell Link (`.lnk`) carver: validates Shell Link headers and optional sections, carves exact shortcut extents, and emits `windows_artefacts` rows with target paths, working directories, timestamps, file sizes, volume serials, local base paths, and network paths. See [`docs/carver/lnk.md`](docs/carver/lnk.md). Closes #40.
- Windows Prefetch (`.pf`) carver: supports SCCA versions 17, 23, 26, 30, and 31, plus MAM-compressed Windows 10+ Prefetch files via bounded LZXPRESS Huffman decompression. Emits `windows_artefacts` rows with executable names, Prefetch hashes, run counts, run timestamps, volume paths, truncation state, and version metadata. See [`docs/carver/prefetch.md`](docs/carver/prefetch.md). Closes #41.
- Windows Registry hive carver (`regf`): carves primary hives, transaction logs, and BCD-Template files, emitting `windows_artefacts` rows with `timestamp`, `hive_name`, `hive_type`, and best-effort `root_key_name`. Recognises canonical hive names (`SAM`, `SYSTEM`, `SOFTWARE`, `SECURITY`, `DEFAULT`, `NTUSER.DAT`, `UsrClass.dat`, `BCD`, etc.) and applies a 1 GiB defensive plausibility cap on `hive_bins_data_size`. See [`docs/carver/registry.md`](docs/carver/registry.md). Closes #43.
- Windows Event Log (`.evtx`) carver: validates `ElfFile\0` headers and first-chunk `ElfChnk\0` magic before carving, walks declared chunks defensively, preserves truncated logs when evidence ends early, and emits `windows_artefacts` rows with chunk and record-count metadata. See [`docs/carver/evtx.md`](docs/carver/evtx.md). Closes #42.
- Golden-image and fixture coverage for Windows Prefetch, Registry hive, and EVTX carving scenarios.

### Changed
- Prefetch metadata now records `volume_paths_truncated` when a crafted header claims more volume entries than SwiftBeaver decodes under its defensive cap, so shortened `volume_paths_json` arrays are not ambiguous.
- Prefetch `referenced_files_json` remains nullable with `null` meaning "extraction not implemented"; downstream consumers should not assume an empty array for all versions or runs.
- Updated benchmark coverage for the current carver set.

### Fixed
- JPEG carving now uses a two-phase marker walker that skips length-prefixed metadata segments before `SOS`, preventing embedded Exif/MPF thumbnail `EOI` markers from prematurely ending the outer JPEG carve.
- WebP carving now treats the outer RIFF size as authoritative, validates chunk layout against the declared container extent, rejects implausible RIFF declarations, and marks evidence-boundary short reads as truncated without falling back to `max_size`.

### Documentation
- Added or expanded carver documentation for AVI, BZIP2, ELF, EML, FB2, GZIP, ICO, MOBI, MOV, OGG, OLE/CFB, RTF, TAR, WEBM, WMV/ASF, and XZ.

## 0.4.0 (2026-04-20)

### Changed
- Added a `pre_validate()` pipeline for hot carvers to reject bad hits before file creation, with `files_prevalidation_rejected` surfaced in run summary metadata.
- Added `DeferredWriter` and the `deferred_buffer_kb` config knob to avoid create-write-delete I/O for candidates rejected during structural validation.
- Added a sharded `CachedEwfSource` LRU with `ewf_cache_segments` to reduce repeated EWF decompression on hot segments.
- Reused per-worker I/O buffers in the extraction pipeline and shared scan chunk bytes through `NormalizedHit.chunk_data` to reduce allocation churn.
- Added a dedicated prefetch reader thread to decouple evidence reads from scan dispatch and improve throughput on slower storage.
- Added per-worker overlap tracking with the `overlap_skipped` metric to suppress duplicate carve attempts from overlapping chunk hits.
- Added `--metadata-only` mode to run detection and metadata export without writing carved files to disk.
- Tightened SQLite carving with page-by-page validation, early termination on invalid runs, WAL checksum-stop controls, and page-fragment / WAL sidecar outputs.
- Improved OGG validation with CRC32, codec identification, serial consistency checks, and stricter page-count / page-size limits.
- Expanded QuickTime handling with configurable MOV vs MP4 classification and additional AVI/RIFF validation.
- Reduced default `max_size` for false-positive-prone formats:
  - BZIP2: 1GB → 100MB
  - OGG: 1GB → 500MB
  - WAV: 1GB → 500MB

### Added
- New carvers for HEIC/HEIF, SQLite WAL, SQLite page fragments, TIFF, LRF, WAV, OGG, EML, MOBI, FB2, WMV, and additional RIFF-based formats.
- Run summary metadata now includes `files_prevalidation_rejected` for early signature rejects.
- Documentation for BZIP2 and OGG carvers (`docs/carver/bzip2.md`, `docs/carver/ogg.md`).

### Fixed
- Reduced false positive rates across multiple carvers with stronger structural validation and saner carve limits:
  - 7z: CRC32 header validation and sanity limits (max offset: 1GB, max header size: 64MB)
  - WAV: fmt subchunk validation, channel / sample-rate bounds, and implausible-duration rejection
  - OGG: lower page cap, per-page data size checks, CRC32 validation, and codec filtering
  - BZIP2: 10MB search limit when footer is not found quickly
  - MP3: interior-frame rejection, stronger frame consistency checks, and longer minimum-frame requirements
  - OLE: FAT sector cap to prevent excessive size estimates
- Added PNG IHDR pre-validation and a larger minimum size to reject truncated or malformed headers earlier.
- Improved TIFF plausibility checks to reject header-only or structurally implausible matches.
- Hardened LRF, OLE, WAV, and AVI handling to avoid large garbage outputs from weak signatures or corrupt container fields.
- Improved EML end detection so multipart messages and mbox boundaries are handled more reliably.

## 0.3.0

- Added new file type carvers: AVI, WAV, WebP, ICO, BMP, OLE (MS Office documents)
- Added shared RIFF module for AVI/WAV/WebP carving
- Enhanced phone number validation with entropy filtering (requires 4+ unique digits)
- Improved GPU string scanner with overflow fallback to CPU
- Increased minimum file sizes for image carvers to reduce false positives (JPEG: 500B, GIF/PNG: 100B, BMP: 200B)
- Enhanced validators for BMP and ICO formats with stricter validation rules
- Upgraded to Rust edition 2024
- Expanded documentation for carvers and formats

## 0.2.1

- Fixed code formatting to pass `cargo fmt --check` in CI.

## 0.2.0

- Added progress reporting, JSON logging, and error counters.
- Added checkpoint/resume support, graceful shutdown, and output limits.
- Added resource limits (max memory, max open files).
- Added malformed input, boundary, stress tests, and benchmarks.
- Added CI coverage + release workflows.
- Expanded documentation (config, architecture, contributing, metadata examples).

## 0.1.0

- Initial release of SwiftBeaver.
