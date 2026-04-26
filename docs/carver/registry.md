# Registry Hive Carver

The Registry Hive carver extracts Windows Registry hive files (`regf` format)
and records one structured `windows_artefacts` metadata row per carved hit.
Validation state for each carved artefact is carried by the corresponding
`carved_files` row (`validated`, `truncated`, `errors`); the
`windows_artefacts` row itself is emitted for every non-duplicate carve,
including truncated ones.
Hives covered by this carver include `SAM`, `SYSTEM`, `SOFTWARE`, `SECURITY`,
`DEFAULT`, `NTUSER.DAT`, `UsrClass.dat`, `BCD`, `BCD-Template`, `ELAM`, `BBI`,
`COMPONENTS`, and the various per-user diff/template hives, as well as
transaction-log files (`.LOG`) that share the `regf` magic.

This is a **carve-and-identify** carver. It does not parse keys, values, or
class data; it carves the on-disk byte range and emits a small set of
identification fields. Full key/value parsing is out of scope (see issue #43).

## Detection

| Format | Magic Bytes | Notes |
|--------|-------------|-------|
| Windows Registry hive | `72 65 67 66` (`regf`) | Same magic for primary hives, transaction logs, and BCD-Template files |

- Config validator: `registry`
- Output extension: `hve` (carved hive bytes are opaque binary; `.reg` is
  reserved by convention for textual REGEDIT exports). The `file_type` field
  in the base block distinguishes primary hives (`0`), transaction logs
  (`1`), and external hives (`2`).

### Config Patterns

```yaml
- id: "registry"
  extensions: ["hve"]
  header_patterns:
    - id: "regf_header"
      hex: "72656766"
  footer_patterns: []
  max_size: 536870912    # 512 MiB
  min_size: 4096         # one base block
  validator: "registry"
```

## Carving Algorithm

1. **Pre-validate** by reading 4 bytes and confirming the `regf` magic.
2. **Read the 4096-byte base block** and validate:
   - magic is `regf`
   - primary and secondary sequence numbers are both non-zero for primary and
     external hives; a transaction-log file may legitimately have one of them
     zero until its first commit cycle, but not both
   - major version is `1`, minor version is `<= 15`
   - file type is `0` (primary), `1` (transaction log), or `2` (external)
3. **Extract metadata** from the base block:
   - timestamp (Windows FILETIME at offset `0x0C`)
   - root cell offset (`0x24`)
   - hive bins data size (`0x28`)
   - embedded filename (UTF-16LE, 64 bytes at offset `0x30`, NUL-terminated)
4. **Compute total size** = `4096 + hive_bins_data_size`. Reject if this
   exceeds either the configured `max_size` or the defensive 1 GiB plausibility
   ceiling.
5. **Stream the full hive** (base block + hive bins) to disk while computing
   MD5/SHA-256 in the same pass. Truncation by the evidence boundary marks the
   carved row `truncated=true` rather than discarding.
6. **Best-effort root-key name extraction**: read the first cell of the first
   `hbin`, validate the `nk` magic, and decode the name (ASCII or UTF-16LE
   depending on the cell flags). Failure here is non-fatal.
7. **Hive-type identification**: the trailing path component of the embedded
   filename is matched against a canonical list of well-known hive names.
   Unknown components are surfaced as-is in `hive_type`.

## Extracted Fields

| Field | Description |
|-------|-------------|
| `timestamp` | Last-write FILETIME from the base block (UTC) |
| `hive_name` | Embedded UTF-16LE filename (often a `\??\C:\…` style path or a short name like `SYSTEM`) |
| `hive_type` | Canonical hive label (e.g. `SAM`, `SYSTEM`, `NTUSER.DAT`) inferred from the trailing component of `hive_name`; falls back to the raw component when not in the canonical set |
| `root_key_name` | Name of the root `nk` cell (best-effort; `null` on parse failure) |

## Limits

- Minimum size: configured `min_size` (default `4096` bytes — one base block)
- Maximum size: configured `max_size` (default `512 MiB`)
- A defensive plausibility ceiling of **1 GiB** is applied to the
  `hive_bins_data_size` field independent of `max_size`, to reject pathological
  garbage headers
- The XOR checksum at offset `0x1FC` is intentionally **not** validated:
  real-world hives (especially those undergoing crash-recovery or carved from
  unallocated space) frequently have invalid checksums while still being useful
  evidence

## Metadata

Carved registry hits are written to:

- `metadata/carved_files.*` — the `validated` flag is `true` only when both
  the base block and the first `hbin` magic at offset `0x1000` are sound
  **and** the hive was not truncated by the evidence boundary; otherwise an
  explanatory message is appended to `errors`.
- `metadata/windows_artefacts.*` with `artefact_type="registry"` — emitted
  for every carved hit (duplicates excluded), regardless of validation state.

The `windows_artefacts` flat schema (see [docs/metadata_parquet.md](../metadata_parquet.md))
includes the registry-specific fields: `timestamp`, `hive_name`, `hive_type`,
`root_key_name`.

> Note: in the Parquet output the timestamp column is named `timestamp_utc`
> (the JSONL/CSV sinks emit it as `timestamp`). Other registry-specific
> column names are identical across all sinks.

## CLI Example

```bash
cargo run -- --input /cases/disk.dd --output ./out --types registry
```

## Testing

- `tests/carver_registry.rs` covers end-to-end carving and metadata emission:
  - basic carve of a synthetic hive
  - hive-type detection (`SAM`, `SYSTEM`, `SOFTWARE`, `NTUSER.DAT`, `SECURITY`,
    path-prefixed names)
  - embedded filename extraction
  - graceful handling of evidence truncation (carved row marked `truncated`)
  - `max_size` enforcement (oversize hives are skipped, not partially carved)
  - dirty-hive handling (primary != secondary sequence still emits bytes)
  - transaction-log acceptance when exactly one sequence number is zero
  - rejection of primary hives with a zero sequence number
- Fixture builder: `tests/common/registry_fixture.rs`
- Golden-image samples (synthetic + real, scrubbed):
  `tests/golden_image/samples/windows/registry/`

## Forensic Considerations

- **Read-only**: the carver never modifies evidence and only writes to the
  configured output directory.
- **Provenance**: every emitted row carries `run_id`, `tool_version`,
  `config_hash`, and `evidence_path`.
- **Dirty hives** (where primary and secondary sequence numbers differ) are
  carved as-is. Recovery via transaction logs is the analyst's responsibility
  and is not attempted here.
- **Transaction logs** (`.LOG`, `.LOG1`, `.LOG2`) that begin with the `regf`
  magic are carved with the same path. Higher-level discrimination between
  primary hive and transaction log can be made downstream via the `file_type`
  field in the base block.

## Known Limitations

- Key/value parsing, security descriptors, and class data are not extracted.
- Transaction log application (CLFS-based recovery for `.blf` /
  `.regtrans-ms` files) is out of scope.
- The XOR base-block checksum is not used as a validation gate.
- Headers where **both** `primary_sequence` and `secondary_sequence` are
  zero are rejected (combined with the version, `file_type`, and `hbin`
  magic checks this filters most random-byte false positives). Hives where
  only one sequence number is zero are accepted, since freshly initialised
  transaction-log files (`.LOG`/`.LOG1`/`.LOG2`) can legitimately have
  `secondary_sequence == 0` until their first commit cycle. Dirty hives
  where the two sequence numbers simply disagree are carved as-is.

## References

- [Windows registry file format](https://github.com/libyal/libregf/blob/main/documentation/Windows%20NT%20Registry%20File%20%28REGF%29%20format.asciidoc)
  (libregf documentation)
- [Microsoft: registry hive overview](https://learn.microsoft.com/en-us/windows/win32/sysinfo/registry-hives)
