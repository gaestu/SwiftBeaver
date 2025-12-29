Status: Implemented
Implemented in version: 0.1.0

# GPU String Spans + Hint Flags

## Problem statement
GPU string scanning only marks printable bytes; span detection and URL/email/phone hinting are done on CPU, limiting the benefit of GPU acceleration.

## Scope
- Add GPU kernels (OpenCL/CUDA) to emit ASCII string spans directly.
- Compute URL/email/phone hint flags inside the GPU span kernel.
- Keep UTF-8/UTF-16 span detection on CPU (existing behavior).
- Add a config limit for maximum GPU string spans per chunk.

## Non-goals
- Full GPU regex validation for URLs/emails/phones.
- GPU span detection for UTF-8/UTF-16 runs.
- Eliminating all CPU post-processing (small splitting logic may remain).

## Design notes
- Use one thread per byte, emit spans only at run starts (previous byte not printable).
- Bound span length by `string_max_len` to cap per-thread work; CPU may split long runs.
- GPU writes span start/length/flags into fixed-size buffers with atomic count.
- Flags must match existing bitmask values in `strings::flags`.

## Expected tests
- Update existing GPU string scanner tests to ensure kernel compiles/initializes.
- Add unit tests for CPU fallback and span splitting behavior where possible.

## Impact on docs and README
- Document `gpu_max_string_spans_per_chunk` in `docs/config.md`.
- Mention GPU string span/hint offload in README and/or docs/architecture.
