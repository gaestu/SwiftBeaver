# GPU Signature Scanner

Status: Implemented  
Implemented in version: 0.1.0

## Problem statement

Signature scanning on multi-terabyte images is CPU-bound in Phase 2. We need a GPU-accelerated scanner that can search header patterns faster when data is resident in RAM or on fast storage.

## Scope

- Provide a GPU-backed `SignatureScanner` behind feature flag `gpu-opencl` (alias `gpu`).
- Select GPU scanner when `--gpu` is set.
- Fall back to CPU when GPU is unavailable or the backend is not implemented.

## Non-goals

- GPU string scanning (tracked separately).
- CUDA backend (future work).

## Design notes

- `scanner::opencl::OpenClScanner` implements OpenCL scanning with CPU fallback.
- `scanner::build_signature_scanner(cfg, use_gpu)` selects OpenCL when available.
- Logging warns when GPU is requested but unavailable.

## Expected tests

- Unit test that `build_signature_scanner(..., true)` returns a scanner (fallback when `gpu` feature is disabled).

## Impact on docs and README

- Document the `--gpu` flag and `--features gpu` build.
- Note the OpenCL backend and CPU fallback behavior.
