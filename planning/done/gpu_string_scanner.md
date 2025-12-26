# GPU String Scanner

Status: Implemented  
Implemented in version: 0.1.0

## Problem statement

Printable string scanning across large images is CPU-heavy. We need a GPU-accelerated string scanner to quickly identify candidate spans for URL/email/phone extraction.

## Scope

- Provide a GPU-backed `StringScanner` behind feature flag `gpu-opencl` (alias `gpu`).
- Select GPU scanner when `--gpu` is set and string scanning is enabled.
- Fall back to CPU when GPU is unavailable or the backend is not implemented.

## Non-goals

- CUDA backend (future work).
- UTF-16 string scanning (future work).

## Design notes

- `strings::opencl::OpenClStringScanner` uses OpenCL to build a printable mask and derives spans on CPU.
- `strings::build_string_scanner(cfg, use_gpu)` selects OpenCL when available.
- Logging warns when GPU is requested but unavailable.

## Expected tests

- Unit test that `build_string_scanner(..., true)` returns a scanner (fallback when `gpu` feature is disabled).

## Impact on docs and README

- Document the `--gpu` flag and `--features gpu` build for string scanning.
- Note the OpenCL backend and CPU fallback behavior.
