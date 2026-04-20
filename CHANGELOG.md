# Changelog

All notable changes to this project will be documented in this file.

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
