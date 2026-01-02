# Contributing

Thanks for contributing to fastcarve. This guide covers local setup, tests, and style.

## Requirements

- Rust toolchain (stable)
- libewf development headers for E01 support
  - Debian/Ubuntu: `sudo apt install libewf-dev`
  - Fedora/RHEL: `sudo dnf install libewf-devel`

Optional GPU dependencies:
- OpenCL: install an ICD loader with `libOpenCL.so`
- CUDA: install the NVIDIA CUDA toolkit with NVRTC

## Setup

```bash
git clone <repo>
cd SwiftBeaver
cargo build
```

## Tests

```bash
cargo test
cargo test --no-default-features
```

Golden image tests:

```bash
cd tests/golden_image
./generate.sh
cd ../../
cargo test golden
cargo test golden --features ewf
```

Stress tests (ignored by default):

```bash
cargo test stress_ -- --ignored
FASTCARVE_STRESS_BYTES=1073741824 cargo test stress_large_image_scan -- --ignored
FASTCARVE_STRESS_HITS=5000 FASTCARVE_STRESS_MAX_FILES=1000 cargo test stress_high_hit_density -- --ignored
```

GPU build verification (no GPU required):

```bash
cargo build --features gpu-opencl
cargo build --features gpu-cuda
```

## Benchmarks

```bash
cargo bench
```

## Formatting and linting

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

## Code style

- Keep changes focused and minimal.
- Add tests for behavior changes.
- Avoid panics in library code for expected errors.
- Keep documentation and README in sync with user-facing changes.
