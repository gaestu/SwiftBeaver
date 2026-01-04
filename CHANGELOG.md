# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

- TBD

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
