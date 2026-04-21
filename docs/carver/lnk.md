# LNK Carver

The LNK carver extracts Windows Shell Link shortcut files and records one
structured `windows_artefacts` metadata row per validated hit.

## Detection

- Signature: `4C 00 00 00 01 14 02 00 00 00 00 00 C0 00 00 00 00 00 00 46`
- Config validator: `lnk`
- Output extension: `lnk`

## Carving Algorithm

1. Validate the 76-byte Shell Link header.
2. Read `LinkFlags` and walk optional sections in order:
   - `LinkTargetIDList`
   - `LinkInfo`
   - `StringData`
   - `ExtraData`
3. Stop at the 4-byte TerminalBlock (`00 00 00 00`) and carve the exact byte range.
4. Emit a carved file record and a `WindowsArtefactRecord::Lnk` metadata row.

When a Unicode string variant is absent, ANSI path strings are decoded as
Windows-1252.

## Extracted Fields

- `target_path`
- `working_dir`
- `creation_time`
- `access_time`
- `write_time`
- `file_size`
- `volume_serial`
- `local_base_path`
- `network_path`

## Limits

- Minimum size: `76`
- Maximum size: `64 KiB`
- Default concurrency cap: `carver_limits.lnk.max_concurrent = 2`
- ExtraData parsing is limited to locating block boundaries; deep block decoding is out of scope.

## Metadata

Validated LNK hits are written to:

- `metadata/carved_files.*`
- `metadata/windows_artefacts.*` with `artefact_type="lnk"`
- `metadata/windows_artefacts.parquet` uses `creation_time_utc`, `access_time_utc`, and `write_time_utc`

## CLI Example

```bash
cargo run -- --input /cases/disk.dd --output ./out --types lnk
```

## Testing

- Unit tests cover target-path parsing, timestamp extraction, non-ASCII ANSI decoding, network-share parsing, ExtraData chaining, and truncated input handling.
- `tests/carver_lnk.rs` covers end-to-end carving and metadata emission through the pipeline.
