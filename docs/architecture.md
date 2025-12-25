# Architecture (Phase 1)

Phase 1 focuses on CPU-only carving of JPEG/PNG/GIF from raw disk images.

## Pipeline

1. **EvidenceSource** reads a raw file (or E01 when built with `--features ewf`) into a linear byte space.
2. **Chunk scheduler** splits the image into overlapping chunks.
3. **CPU signature scanner** searches for file headers within each chunk.
4. **Carve workers** validate and extract files from the evidence source.
5. **Metadata sink** writes JSONL records for each carved file.

## Concurrency model

- Reader thread: reads chunks and feeds scan jobs.
- Scan workers: perform signature scanning and emit normalized hits.
- Carve workers: validate/extract files and emit metadata.
- Metadata writer: serializes JSONL records.

## Modules

- `src/evidence.rs` - raw file evidence source
- `src/chunk.rs` - chunk scheduling
- `src/scanner/` - CPU signature scanner
- `src/carve/` - file-type handlers
- `src/metadata/` - JSONL sink
