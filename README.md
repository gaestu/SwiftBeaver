# fastcarve

High-speed, forensic-grade file and artefact carver (Phase 1: CPU-only core).

## Quick start

```bash
cargo run -- --input /path/to/image.dd --output ./output
```

E01 input (requires `libewf`):

```bash
cargo run --features ewf -- --input /path/to/image.E01 --output ./output
```

This creates a run directory under `./output/<run_id>/` with:

- `carved/` - carved files per type
- `metadata/` - JSONL records for carved files

## Configuration

The default configuration lives in `config/default.yml`. You can override it with:

```bash
cargo run -- --input /path/to/image.dd --output ./output --config-path /path/to/config.yml
```

Key settings:

- `overlap_bytes`: chunk overlap in bytes
- `file_types`: enabled formats, header patterns, size limits

CLI overrides:

- `--overlap-kib`: overrides `overlap_bytes` when set

See `docs/config.md` for the full schema.

## Output metadata (JSONL)

Carved files are recorded to `metadata/carved_files.jsonl` with run-level provenance.

See `docs/metadata_jsonl.md` for the schema.

## Architecture

The Phase 1 pipeline:

1. Evidence source (raw file)
2. Chunk scheduler + reader
3. CPU signature scanner
4. Carve workers (JPEG/PNG/GIF)
5. JSONL metadata sink

See `docs/architecture.md` for details.

## Notes

- E01 support is available when built with `--features ewf` and requires `libewf` installed.
- GPU and string scanning are not implemented yet.

## License

MIT (see `LICENSE`).
