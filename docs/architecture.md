# Architecture

SwiftBeaver currently includes SQLite carving, string scanning,
PDF/ZIP/WEBP carving (with ZIP classification for docx/xlsx/pptx/odt/ods/odp/epub),
BMP/TIFF/MP4/MOV/RAR/7z carving, WAV/AVI/OGG audio-video carving,
MP3 audio carving with ID3v2 support, TAR/GZIP/BZIP2/XZ archive carving,
RTF document carving, ICO/ELF/EML/MOBI/FB2/LRF carving, OLE compound document carving (DOC/XLS/PPT),
and optional GPU-accelerated signature and string scanning via OpenCL (`gpu-opencl` / `gpu` alias)
or CUDA (`gpu-cuda`). GPU signature scanning uses a single multi-pattern kernel, and GPU string
scanning emits ASCII spans with URL/email/phone hints.

## GPU Backends

### OpenCL
- Feature: `--features gpu-opencl` (or `--features gpu`)
- Requirements: OpenCL ICD loader (`libOpenCL.so`, install `ocl-icd-devel` on Fedora)
- Supports any OpenCL-capable GPU (NVIDIA, AMD, Intel)

### CUDA
- Feature: `--features gpu-cuda`
- Requirements: NVIDIA CUDA toolkit with NVRTC (runtime compilation)
- Auto-detects installed CUDA version at build time
- Only supports NVIDIA GPUs

Both backends compile kernels at scanner initialization and fall back to CPU if initialization fails.

## Pipeline

1. **EvidenceSource** reads a raw file (or E01 with default EWF support enabled, requires `libewf`) into a linear byte space.
2. **Chunk scheduler** streams the image into overlapping chunks.
3. **CPU signature scanner** searches for file headers within each chunk.
4. **CPU string scanner** (optional) extracts printable spans and artefacts.
5. **Carve workers** validate and extract files from the evidence source.
6. **Metadata sink** writes JSONL, CSV, or Parquet records.

## Concurrency model

- Reader thread: reads chunks and feeds scan jobs.
- Scan workers: perform signature scanning and emit normalized hits.
- Carve workers: validate/extract files and emit metadata.
- Metadata writer: serializes JSONL/CSV/Parquet records.

## Modules

- `src/evidence.rs` - raw file evidence source
- `src/chunk.rs` - chunk scheduling
- `src/pipeline/limiter.rs` - strict `max_files` limiter
- `src/scanner/` - CPU signature scanner
- `src/carve/` - file-type handlers
- `src/strings/` - printable string scanning and artefact extraction
- `src/parsers/sqlite_db.rs` - browser history parsing
- `src/metadata/` - JSONL, CSV, and Parquet sinks
