# Architecture (Phase 1)

Phase 2 adds SQLite carving, string scanning, browser history extraction, and PDF/ZIP/WEBP carving.
Phase 3 adds optional GPU-accelerated signature and string scanning (feature `gpu-opencl` or alias `gpu`), with CPU fallback when OpenCL is unavailable. A CUDA backend is planned.

## Pipeline

1. **EvidenceSource** reads a raw file (or E01 when built with `--features ewf`) into a linear byte space.
2. **Chunk scheduler** splits the image into overlapping chunks.
3. **CPU signature scanner** searches for file headers within each chunk.
4. **CPU string scanner** (optional) extracts printable spans and artefacts.
5. **Carve workers** validate and extract files from the evidence source.
6. **SQLite parser** extracts browser history from carved SQLite databases.
7. **Metadata sink** writes JSONL, CSV, or Parquet records.

## Concurrency model

- Reader thread: reads chunks and feeds scan jobs.
- Scan workers: perform signature scanning and emit normalized hits.
- Carve workers: validate/extract files and emit metadata.
- Metadata writer: serializes JSONL/CSV/Parquet records.

## Modules

- `src/evidence.rs` - raw file evidence source
- `src/chunk.rs` - chunk scheduling
- `src/scanner/` - CPU signature scanner
- `src/carve/` - file-type handlers
- `src/strings/` - printable string scanning and artefact extraction
- `src/parsers/sqlite_db.rs` - browser history parsing
- `src/metadata/` - JSONL, CSV, and Parquet sinks
