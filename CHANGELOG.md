# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

### Fixed
- **Reduced false positive rates for 6 carvers** (Issue #6):
  - **7z**: Added CRC32 header validation and sanity limits (max offset: 1GB, max header size: 64MB)
  - **WAV**: Added fmt subchunk validation (audio format, channels, sample rate, bits per sample)
  - **OGG**: Reduced page limit from 1M to 100K, added per-page data size validation
  - **BZIP2**: Added 10MB search limit to reject false positives when footer not found quickly
  - **MP3**: Increased minimum frames from 3 to 5, added sample rate consistency check, added 3-hour max duration
  - **OLE**: Added 1000 FAT sector cap to prevent excessive size estimates

### Changed
- Reduced default max_size for false-positive-prone formats:
  - BZIP2: 1GB → 100MB
  - OGG: 1GB → 500MB
  - WAV: 1GB → 500MB

### Added
- Documentation for BZIP2 and OGG carvers (`docs/carver/bzip2.md`, `docs/carver/ogg.md`)

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
