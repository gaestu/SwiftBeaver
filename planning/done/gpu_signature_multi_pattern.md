Status: Implemented
Implemented in version: 0.1.0

# GPU Signature Multi-Pattern Kernel

## Problem statement
GPU signature scanning launches one kernel per pattern, creating overhead and limiting throughput on large pattern sets.

## Scope
- Build one GPU kernel (OpenCL/CUDA) that scans all patterns in a single pass.
- Preload pattern bytes/offsets/lengths into device buffers.
- Emit hit offsets with pattern indices; map back to pattern metadata on CPU.

## Non-goals
- Aho-Corasick or other advanced multi-pattern algorithms.
- GPU-side validation of file formats.
- GPU support for footer scanning (still CPU-driven).

## Design notes
- Flatten pattern bytes into one buffer plus offsets/lengths arrays.
- Use a per-chunk max hits cap to bound output size.
- Fallback to CPU on buffer creation/launch failure.

## Expected tests
- Keep existing GPU scanner initialization tests working (skip if no device).
- Add a minimal scan test with two patterns (when GPU available).

## Impact on docs and README
- Document GPU multi-pattern behavior briefly in `docs/architecture.md` or README.
