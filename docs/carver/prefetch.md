# Prefetch Carver

The Prefetch carver extracts Windows Prefetch files (`.pf`) and records one
structured `windows_artefacts` metadata row per validated hit.

## Detection

Two wire formats are supported:

| Format | Magic Bytes | Versions |
|--------|-------------|----------|
| SCCA (uncompressed) | `<version LE u32> 53 43 43 41` | 17 (XP/2003), 23 (Vista/7), 26 (8/8.1), 30 (10/11), 31 (10 1809+) |
| MAM (compressed) | `4D 41 4D 04` | Windows 10 v1709+ |

- Config validator: `prefetch`
- Output extension: `pf`

### Config Patterns

```yaml
- id: "prefetch"
  extensions: ["pf"]
  header_patterns:
    - id: "prefetch_mam"
      hex: "4D414D04"
    - id: "prefetch_scca_17"
      hex: "1100000053434341"
    - id: "prefetch_scca_23"
      hex: "1700000053434341"
    - id: "prefetch_scca_26"
      hex: "1A00000053434341"
    - id: "prefetch_scca_30"
      hex: "1E00000053434341"
    - id: "prefetch_scca_31"
      hex: "1F00000053434341"
  max_size: 10485760
  min_size: 84
  validator: "prefetch"
```

## Carving Algorithm

### MAM (Compressed) Format

1. Parse the 8-byte MAM header: `4D 41 4D 04` + uncompressed size (u32 LE).
2. Validate the uncompressed size is within `[84, 10 MiB]`.
3. Decompress the payload using LZXPRESS Huffman (per MS-XCA §2.4).
4. Pass the decompressed bytes to the SCCA parser.

### SCCA (Uncompressed) Format

1. Validate the 4-byte magic `SCCA` at offset 4.
2. Read and validate the version field (17, 23, 26, 30, or 31).
3. Read the file size field from the header.
4. Use version-specific field offsets to extract metadata.
5. Emit a carved file record and a `WindowsArtefactRecord::Prefetch` metadata row.

## Extracted Fields

| Field | Description |
|-------|-------------|
| `executable_name` | UTF-16LE name of the traced executable (up to 29 chars) |
| `prefetch_hash` | 4-byte hash of the prefetch file (hex string) |
| `run_count` | Number of times the executable has been launched |
| `last_run_times` | Up to 8 run timestamps (UTC, ISO 8601). Most recent is `last_run_times[0]`. Versions 26/30 store up to 8; older versions store 1. |
| `volume_paths` | Mounted volume paths referenced in the file |
| `volume_paths_truncated` | `true` when the volume-info header claimed more entries than SwiftBeaver decoded under the defensive cap; analysts should treat `volume_paths` as incomplete in that case. |
| `referenced_files` | Deferred. The field is exposed in metadata as nullable state, but filename-strings extraction is not implemented yet, so current rows emit `null` rather than implying "no references found". |
| `version` | Raw SCCA version integer |

## Limits

- Minimum size: configured `min_size` (default `84` bytes), applied to the decoded SCCA byte count. For MAM-compressed records, the carved on-disk span can legitimately be smaller than 84 bytes.
- Maximum size: configured `max_size` (default `10 MiB`)
- Decompressed MAM payload must not exceed the configured `max_size`
- LZXPRESS Huffman decompressor is bounded; invalid compressed data returns an error
- Volume-info iteration is capped at 32 entries even when the SCCA header claims more; if hit, SwiftBeaver emits a `tracing::warn!` under target `prefetch` and marks the metadata row with `volume_paths_truncated=true`

## Metadata

Validated Prefetch hits are written to:

- `metadata/carved_files.*`
- `metadata/windows_artefacts.*` with `artefact_type="prefetch"`

## CLI Example

```bash
cargo run -- --input /cases/disk.dd --output ./out --types prefetch
```

## Testing

- `tests/carver_prefetch.rs` covers end-to-end carving and metadata emission:
  - Win7 uncompressed (version 23) round-trip
  - Executable name extraction
  - Run count extraction
  - Truncated input rejection
  - MAM (compressed) carving with sentinel bytes (verifies exact record boundary)
  - MAM metadata size consistency between `carved_files` and `windows_artefacts`
- Fixture builder: `tests/common/prefetch_fixture.rs`

## References

- [MS-XCA: Xpress Compression Algorithm](https://docs.microsoft.com/en-us/openspecs/windows_protocols/ms-xca)
- [libscca documentation](https://github.com/libyal/libscca/blob/main/documentation/Windows%20Prefetch%20File%20(PF)%20format.asciidoc)
