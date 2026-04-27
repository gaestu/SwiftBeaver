# EVTX (Windows Event Log) Carver

The EVTX carver extracts Windows Event Log files (`ElfFile\0` format,
Vista and later) and records one structured `windows_artefacts` metadata
row per carved hit.

This is a **carve-and-identify** carver. It does not parse individual
event records (BinXML); it carves the on-disk byte range and extracts a
small set of header-derived identification fields. Full BinXML record
parsing is out of scope (separate feature).

## Detection

| Format | Magic Bytes | Notes |
|--------|-------------|-------|
| EVTX file header | `45 6C 66 46 69 6C 65 00` (`ElfFile\0`) | 8-byte signature at the start of every EVTX file |

- Config validator: `evtx`
- Output extension: `evtx`

### Config Patterns

```yaml
- id: "evtx"
  extensions: ["evtx"]
  header_patterns:
    - id: "evtx_header"
      hex: "456C6646696C6500"
  footer_patterns: []
  max_size: 1073741824   # 1 GiB
  # 4096 (file header) + 65536 (one chunk). Header-only carves are
  # rejected because pre_validate requires the first ElfChnk\0 magic
  # at +0x1000, so the floor is one header plus one chunk.
  min_size: 69632
  validator: "evtx"
```

## Carving Algorithm

1. **Pre-validate** by reading 8 bytes at the candidate offset and confirming
   the `ElfFile\0` magic, then a second 8-byte read at offset `+0x1000`
   confirming the `ElfChnk\0` magic at the start of chunk 0. The two-stage
   magic check rejects false-positive `ElfFile\0` byte sequences in
   compressed/random evidence before they trigger speculative carve I/O.
2. **Read the 4096-byte file header** and validate:
   - magic is `ElfFile\0`
   - `header_block_size` (`0x28`, u16 LE) is `4096`
   - `major_version` (`0x26`, u16 LE) is `3` (Windows Vista+); any `minor_version`
     (`0x24`, u16 LE) is accepted (Win7=`1`, Win11=`2`, future minors are additive)
   - `chunk_count` (`0x2A`, u16 LE) is in `1 ..= 16383` (the upper bound is a
     defensive plausibility cap that matches the default `max_size`)
   - `first_chunk` (`0x08`, u64 LE) and `last_chunk` (`0x10`, u64 LE) are both
     `< chunk_count`. We deliberately do **not** require `first_chunk <= last_chunk`:
     in *circular* (overwrite) channels, `first_chunk` advances past `last_chunk`
     once the log wraps, and those files are still valid EVTX.
3. **Walk declared chunks** to determine the verified contiguous chunk count
   and sum the per-chunk record counts in a single pass:
   - The carver only walks chunks `0 .. chunk_count`. Extra `ElfChnk\0` blocks
     after the header-declared extent are not included because raw carving
     cannot prove they belong to the EVTX file rather than adjacent evidence.
   - For each chunk with a valid `ElfChnk\0` magic, sum
     `(last_record_number - first_record_number + 1)` into
     `record_count_estimate`. The first missing or corrupt declared chunk
     stops the walk; it does not extend the carve range and the output is
     marked unvalidated.
4. **Compute total size** = `4096 + verified_chunks * 65536`. Reject if this
   exceeds the configured `max_size` or is below `min_size`.
5. **Stream the full record** (header + chunks) to disk while computing
   MD5/SHA-256 in the same pass. Truncation by the evidence boundary marks
   the carved row `truncated=true` rather than discarding.

## Extracted Fields

| Field | Description |
|-------|-------------|
| `first_chunk` | `FirstChunkNumber` from the file header (normally `0`). Reflects the source header verbatim — when `truncated=true`, this value may reference a chunk index past the actually-carved extent. |
| `last_chunk` | `LastChunkNumber` from the file header — index of the last in-use chunk. Same truncation caveat as `first_chunk`: callers indexing into the carved file by `last_chunk * 65536 + 4096` must check `size` (and `truncated`) first. |
| `record_count_estimate` | Sum of per-chunk record counts derived by walking each verified declared `ElfChnk\0` header. Lower bound when chunks are corrupt or past the evidence boundary. `null` when one or more chunk headers declare implausible record-number ranges (e.g. `last_record_number == u64::MAX`) or when the running sum would exceed `i64::MAX`, since the downstream Parquet column is `int64`. |
| `log_name` | Always `null` for raw-image carving — the channel name (e.g. `Microsoft-Windows-PowerShell/Operational`) is encoded only in the on-disk filename, not in the EVTX file body |

## Limits

- Minimum size: configured `min_size` (default `69632` bytes — one file
  header plus one chunk; pre-validation requires the first `ElfChnk\0`
  magic at `+0x1000`, so header-only carves are not recoverable)
- Maximum size: configured `max_size` (default `1 GiB`)
- Defensive plausibility cap on declared chunk count: `16383` chunks (just under
  the default `max_size`)
- The CRC32 fields (file-header at `0x7C` and per-chunk at `0x7C`) are
  intentionally **not** validated: real-world Win10/Win11 EVTX files in the
  wild frequently fail strict CRC checks while still being recoverable. The
  carver follows "carve the bytes" semantics — header magic, version sanity,
  and chunk count are sufficient.

## Pre-allocation and Logical vs Physical Size

The Windows Event Log service may pre-allocate EVTX files to their configured
maximum size and pad chunks past `chunk_count` (i.e. `LastChunkNumber + 1 ..
allocated_chunks`) with empty data. From a raw image those blocks are
indistinguishable from arbitrary adjacent evidence, so signature-based carving
**cannot** prove the original allocated file size. The carver instead recovers
the **declared logical extent**: header (4 KiB) plus verified contiguous chunks
within `chunk_count`.

The carved file is a fully valid EVTX (parsable by `evtx_dump`,
`python-evtx`, etc.) and contains every event record present in the source.
Its SHA-256 will differ from the on-disk file's SHA-256 whenever the source
had pre-allocated empty trailing chunks.

## Metadata

Carved EVTX hits are written to:

- `metadata/carved_files.*` — `validated` is `true` only when the carve was
  not truncated by the evidence boundary and every declared chunk in the carved
  extent had valid `ElfChnk\0` magic; on truncation or corruption an
  explanatory message is appended to `errors`.
- `metadata/windows_artefacts.*` with `artefact_type="evtx"`.

### `validated` vs `truncated` — which to filter on

The two flags answer different questions and are **not** interchangeable:

- `truncated=true` means the **evidence boundary** cut the carve short — the
  source image ended before all declared chunks could be read. Use this to
  distinguish "the disk image was incomplete" from "the EVTX was complete on
  disk".
- `validated=false` means the **carved file is not a complete EVTX** — either
  it was truncated (above) **or** a declared chunk had a corrupt `ElfChnk\0`
  magic so the walk stopped early. In the corruption case the file's own
  header still advertises `chunk_count` chunks but the body contains fewer,
  and downstream parsers (`evtx_dump`, `python-evtx`) will hit EOF before the
  declared end.

**Downstream consumers wanting only "complete, parseable" EVTX should filter
on `validated=true`, not on `truncated=false`.** A carve with `validated=false,
truncated=false` is the corrupt-chunk case and is still short relative to its
own header.

## CLI Example

```bash
cargo run -- --input /cases/disk.dd --output ./out --types evtx
```

## Testing

- `tests/carver_evtx.rs` covers end-to-end carving and metadata emission:
  - `test_evtx_basic_carve` — single-chunk synthetic round-trip
  - `test_evtx_chunk_count` — multi-chunk size + record-count walk (including
    an empty middle chunk)
  - `test_evtx_truncated` — truncation by evidence boundary preserves bytes
    and marks `validated=false, truncated=true`
  - `test_evtx_size_limit` — `total_size > max_size` is dropped
  - `test_evtx_corrupt_declared_chunk_stops_extent` — a corrupt declared chunk
    stops the carve before adjacent chunk-like bytes
  - `test_evtx_dirty_flag` — `file_flags=1` (dirty) still carves cleanly
  - `test_evtx_invalid_version` — `major != 3` is rejected
- Fixture builder: `tests/common/evtx_fixture.rs` (mirrors
  `tests/golden_image/samples/windows/evtx/generate.py`).
- Golden-image coverage: `golden_carves_from_raw` asserts that all 11 EVTX
  fixtures bundled in `tests/golden_image/samples/windows/evtx/` are
  recovered from `golden.raw` with matching SHA-256.

## References

- [libyal/libevtx — Windows XML Event Log (EVTX) format](https://github.com/libyal/libevtx/blob/main/documentation/Windows%20XML%20Event%20Log%20%28EVTX%29.asciidoc)
- Microsoft `[MS-EVEN6]` Event Log Remoting Protocol (cross-references the
  on-disk format)
