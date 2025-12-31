# fastcarve

High-speed, forensic-grade file and artefact carver (Phase 2: CPU-only core with SQLite, strings, and expanded file types).

## Quick start

```bash
cargo run -- --input /path/to/image.dd --output ./output
```

E01 input (requires `libewf`, enabled by default):

```bash
cargo run -- --input /path/to/image.E01 --output ./output
```

GPU signature scanning (fallbacks to CPU if GPU is unavailable):

```bash
# OpenCL backend
cargo run --features gpu-opencl -- --input /path/to/image.dd --output ./output --gpu

# CUDA backend (requires NVIDIA CUDA toolkit with NVRTC)
cargo run --features gpu-cuda -- --input /path/to/image.dd --output ./output --gpu
```

GPU string scanning (fallbacks to CPU if GPU is unavailable and requires `--scan-strings`):

```bash
# OpenCL backend
cargo run --features gpu-opencl -- --input /path/to/image.dd --output ./output --gpu --scan-strings

# CUDA backend
cargo run --features gpu-cuda -- --input /path/to/image.dd --output ./output --gpu --scan-strings
```

String scanning (URLs/emails/phones):

```bash
cargo run -- --input /path/to/image.dd --output ./output --scan-strings
```

String scanning (including UTF-16LE/BE runs):

```bash
cargo run -- --input /path/to/image.dd --output ./output --scan-strings --scan-utf16
```

This creates a run directory under `./output/<run_id>/` with:

- `carved/` - carved files per type (jpeg/png/gif/pdf/zip/webp/sqlite/bmp/tiff/mp4/rar/7z). ZIPs are classified into docx/xlsx/pptx when entries match.
- `metadata/` - JSONL records for carved files, string artefacts, and browser history

## Configuration

The default configuration lives in `config/default.yml`. You can override it with:

```bash
cargo run -- --input /path/to/image.dd --output ./output --config-path /path/to/config.yml
```

Key settings:

- `overlap_bytes`: chunk overlap in bytes
- `enable_string_scan`: enable printable string scanning
- `string_min_len`: minimum string length to consider
- `string_max_len`: maximum string length per span
- `file_types`: enabled formats, header patterns, size limits

CLI overrides:

- `--overlap-kib`: overrides `overlap_bytes` when set
- `--scan-strings`: enables ASCII/UTF-8 string scanning
- `--scan-utf16`: enables UTF-16LE/BE string scanning
- `--scan-urls` / `--no-scan-urls`: enable or disable URL extraction
- `--scan-emails` / `--no-scan-emails`: enable or disable email extraction
- `--scan-phones` / `--no-scan-phones`: enable or disable phone extraction
- `--string-min-len`: overrides `string_min_len` when set
- `--scan-entropy`: enable entropy region detection
- `--entropy-window-bytes`: overrides `entropy_window_size` when set
- `--entropy-threshold`: overrides `entropy_threshold` when set
- `--scan-sqlite-pages`: enable SQLite page-level URL recovery for damaged DBs
- `--max-bytes`: stop after scanning this many bytes
- `--max-chunks`: stop after scanning this many chunks
- `--evidence-sha256`: record a known evidence SHA-256
- `--compute-evidence-sha256`: compute evidence SHA-256 before scanning (extra full pass)
- `--metadata-backend csv`: write CSV instead of JSONL
- `--metadata-backend parquet`: write Parquet instead of JSONL
- `--types jpeg,png,sqlite,docx`: limit carving to listed file types (docx/xlsx/pptx are classified from ZIP content)
- `--disable-zip`: disable ZIP carving (skips zip/docx/xlsx/pptx)

See `docs/config.md` for the full schema.

## Output metadata (JSONL)

Carved files are recorded to `metadata/carved_files.jsonl` with run-level provenance.
String artefacts (URLs/emails/phones) are recorded to `metadata/string_artefacts.jsonl`.
Browser history records (from carved SQLite) are recorded to `metadata/browser_history.jsonl`.
Browser cookie records are recorded to `metadata/browser_cookies.jsonl`.
Browser download records are recorded to `metadata/browser_downloads.jsonl`.
Chromium-based browsers (Chrome/Edge/Brave) share a schema and may be labeled `chrome` in browser outputs.
Run summaries are recorded to `metadata/run_summary.jsonl`.
Entropy regions are recorded to `metadata/entropy_regions.jsonl`.

See `docs/metadata_jsonl.md` for the schema.
CSV output is also available with `--metadata-backend csv` (see `docs/metadata_csv.md`).
Parquet output is available with `--metadata-backend parquet` (see `docs/metadata_parquet.md`).

## Architecture

The Phase 2 pipeline:

1. Evidence source (raw file)
2. Chunk scheduler + reader
3. CPU signature scanner
4. Optional CPU string scanner + artefact extraction
5. Carve workers (JPEG/PNG/GIF/PDF/ZIP/WEBP/SQLite/BMP/TIFF/MP4/RAR/7z)
6. SQLite parser for browser history
7. JSONL/CSV metadata sink

See `docs/architecture.md` for details.

## Notes

- E01 support is enabled by default and requires `libewf` installed. Build without EWF via `--no-default-features` (add GPU features explicitly if needed).
- Block device inputs are supported on Linux via read-only access (e.g. `/dev/sdX`).
- GPU signature and string scanning are implemented via OpenCL (`--features gpu-opencl` or `--features gpu` as alias) or CUDA (`--features gpu-cuda`).
- **OpenCL** builds require an ICD loader with `libOpenCL.so` available; install the dev package (`ocl-icd-devel` on Fedora) or provide a symlink if the linker cannot find `-lOpenCL`.
- **CUDA** builds require the full NVIDIA CUDA toolkit including NVRTC (runtime compilation). The build system auto-detects your installed CUDA version. Install via your distro's package manager or from [NVIDIA's CUDA downloads](https://developer.nvidia.com/cuda-downloads). On Fedora:
  ```bash
  dnf config-manager addrepo --from-repofile=https://developer.download.nvidia.com/compute/cuda/repos/fedora39/x86_64/cuda-fedora39.repo
  dnf install cuda-toolkit
  ```

## Testing

Run the test suite:

```bash
cargo test                       # default (includes EWF support)
cargo test --no-default-features # without libewf
cargo test --features gpu-opencl # with OpenCL backend
cargo test --features gpu-cuda   # with CUDA backend
```

CUDA tests skip automatically on machines without a CUDA device. To force CUDA tests to fail on any error (useful for CI on CUDA-capable hosts):

```bash
FASTCARVE_REQUIRE_CUDA=1 cargo test --features gpu-cuda
```

### Golden Image Tests

Comprehensive integration tests can use a golden image that packs all files
under `tests/golden_image/samples/`. See `docs/golden_image.md` for details.

```
tests/golden_image/
├── .goldenignore     # Optional ignore list for non-samples
├── samples/          # Source files organized by type
├── generate.sh       # Packs all samples into an image
├── manifest.json     # Complete offset/hash map
├── golden.raw        # Raw image (gitignored)
└── golden.E01        # EWF image (optional)
```

Generate or regenerate the images:

```bash
cd tests/golden_image
./generate.sh              # Creates raw + E01
./generate.sh --no-e01     # Raw only (faster)
```

Run the golden image tests:

```bash
cargo test golden
cargo test golden --features ewf
```

## License

MIT (see `LICENSE`).
