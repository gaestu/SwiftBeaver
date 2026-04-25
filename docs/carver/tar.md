# TAR Carver

## Overview

The TAR carver extracts Unix tape archive (`.tar`) files from raw forensic
evidence by anchoring on the `ustar` magic bytes inside a 512-byte header,
validating the POSIX header checksum, and walking 512-byte blocks until two
consecutive zero blocks mark the end of the archive.

Implementation: [`src/carve/tar.rs`](../../src/carve/tar.rs)

## Signature Detection

**Header Pattern**: `ustar` (5 bytes) at offset **+257** inside the TAR header.

- Bytes: `75 73 74 61 72`
- Pattern id: `tar_ustar`
- The scanner matches the `ustar` literal anywhere in evidence; the carver
  anchors back by 257 bytes to recover the start of the enclosing 512-byte
  TAR header.

### Config Pattern

```yaml
- id: "tar"
  extensions: ["tar"]
  header_patterns:
    - id: "tar_ustar"
      hex: "7573746172"
  footer_patterns: []
  max_size: 1073741824   # 1 GiB
  min_size: 1024         # 1 KiB (two empty blocks)
  validator: "tar"
```

The `tar` validator backs the [`TarCarveHandler`](../../src/carve/tar.rs).

## Carving Algorithm

TAR is a **block-based** format. Every record is a multiple of 512 bytes:

1. **Anchor to header start**: Subtract 257 from the hit offset. If the
   subtraction underflows, drop the hit.
2. **Pre-validate header**: Read 512 bytes at the anchored offset, confirm
   `ustar` is present at +257, and verify the POSIX header checksum.
3. **Walk records**: For each entry until two zero blocks are seen:
   1. Read a 512-byte header.
   2. If the block is all zeros, increment the zero-block counter; two
      consecutive zero blocks terminate the archive (`validated = true`).
   3. Otherwise, reset the zero-block counter, re-validate the checksum, and
      parse the file size from the octal field at bytes 124..136.
   4. Skip `ceil(size / 512) * 512` bytes of payload.
4. **Finalize**: Compute hashes, enforce `min_size` / `max_size`, and emit a
   `CarvedFile` row.

### Header Layout (POSIX ustar)

```
Offset  Size  Field
0       100   File name
100     8     File mode (octal, ASCII)
108     8     Owner uid (octal)
116     8     Group gid (octal)
124     12    File size in bytes (octal)
136     12    Last modification time (octal seconds since epoch)
148     8     Header checksum (octal, ASCII; checksum field treated as spaces)
156     1     Type flag
157     100   Link target name
257     6     Magic ("ustar\0" or "ustar ")
263     2     Version ("00" for ustar; " \0" for GNU)
265     32    Owner user name
297     32    Owner group name
329     8     Device major (octal)
337     8     Device minor (octal)
345     155   Filename prefix
```

### State Machine

```
START
  ↓
[Read header at offset - 257]
  ↓
[Validate ustar magic + checksum]
  ↓
[Loop: read 512-byte block]
  ├─ zero block + previous zero → VALIDATED (break)
  ├─ zero block (first)         → continue
  ├─ valid header               → skip ceil(size/512)*512 payload bytes
  ├─ invalid checksum           → INVALID (discard)
  ├─ EOF / max_size reached     → TRUNCATED (keep)
  └─ continue
```

## Validation

- **Validated**: `true` when two consecutive zero blocks are observed.
- **Truncated**: `true` when `max_size` is reached or evidence ends before the
  terminator. The partial archive is kept and the error is recorded.
- **Invalid (discarded)**: A header fails the POSIX checksum, the `ustar`
  magic disappears mid-stream, or an octal field contains non-octal digits.

### POSIX Header Checksum

The header checksum is the unsigned sum of every byte in the 512-byte header,
treating the 8 bytes of the checksum field itself as ASCII spaces (`0x20`):

```rust
let stored = parse_octal(&header[148..156])? as u32;
let mut sum = 0u32;
for (idx, &b) in header.iter().enumerate() {
    if (148..156).contains(&idx) {
        sum = sum.saturating_add(0x20);
    } else {
        sum = sum.saturating_add(b as u32);
    }
}
sum == stored
```

This guard rejects almost all false positives where `ustar` appears as data
inside another file.

## Size Constraints

- **Default min_size**: `1024` bytes (the two trailing zero blocks alone).
- **Default max_size**: `1 GiB` (`1073741824`).
- Files below `min_size` are discarded.
- Files at or above `max_size` are kept and marked `truncated = true` with
  `errors = ["max_size reached"]`.

## Hash Computation

Hashes are computed incrementally by `CarveStream` while bytes are streamed
to the carved file:

- **MD5**: rolling, full carved span
- **SHA-256**: rolling, full carved span

Both hashes are bound by the `HashConfig` and may be disabled per run.

## Testing

Unit tests live alongside the implementation in
[`src/carve/tar.rs`](../../src/carve/tar.rs):

- `carves_minimal_tar_from_ustar`: builds a single-entry tar with a valid
  POSIX checksum and confirms the carver returns `validated = true` and the
  exact byte count.
- `pre_validate_accepts_valid_tar_ustar_hit`: checks that a well-formed
  header passes the cheap pre-validation gate.
- `pre_validate_rejects_invalid_tar_checksum`: corrupts the checksum field
  and confirms the hit is rejected before any I/O is wasted.

The shared golden image at [`tests/golden_image/manifest.json`](../../tests/golden_image/manifest.json)
also exercises the TAR carver end-to-end via `archives/tar/test.tar`
(offset `0x5000`, size `10240` bytes) and asserts the expected SHA-256.

## Edge Cases

- **PAX extended headers** (typeflag `x` / `g`): treated as ordinary entries.
  The carver reads the `512`-byte header and skips the declared payload, so
  the surrounding archive boundary is preserved even though the extended
  attributes themselves are not parsed.
- **GNU long names** (typeflag `L` / `K`): handled the same way — the long
  name payload is skipped as data, and the next header continues normal
  iteration.
- **Sparse files** (GNU typeflag `S`): the size field is parsed as written
  in the header. SwiftBeaver does **not** reconstruct the sparse map; for
  malformed sparse headers the archive may be marked truncated.
- **Embedded `ustar` strings**: a stray `ustar` literal in random data is
  almost always rejected because the surrounding 512-byte header will fail
  the POSIX checksum.
- **Pre-POSIX `tar` (v7)**: not detected. The historical v7 format has no
  `ustar` magic and is therefore invisible to the signature scanner.
- **Two valid headers without a terminator**: the carver keeps reading until
  it hits `max_size` or EOF, then marks the result `truncated`.

## Performance

- **Memory**: constant. Each iteration reads one 512-byte header and then
  streams payload bytes through the standard `CarveStream` buffer (~64 KiB).
- **I/O pattern**: strictly sequential reads from the evidence source.
- **CPU**: dominated by hash computation; the checksum walk is O(512) per
  header.
- **Pre-validation**: a single 512-byte read plus a checksum sum rejects
  most false positives before any output file is created.

## Forensic Considerations

- **Evidence integrity**: the source is opened read-only via `EvidenceSource`;
  the carver never writes back to evidence.
- **Reproducibility**: same input and same config produce the same carved
  bytes, hashes, and metadata.
- **Provenance**: every carved row carries `run_id`, `tool_version`,
  `config_hash`, `evidence_path`, and (where available) `evidence_sha256`,
  alongside `global_start`, `global_end`, `validated`, `truncated`, and
  `errors`.
- **Corruption tolerance**: truncated archives are preserved with the
  `truncated` flag set and the failure recorded in `errors`.
- **Metadata extraction**: TAR per-entry metadata (filenames, mtimes, modes)
  is **not** decomposed into a separate metadata table — only the archive
  envelope is recorded. Per-entry inspection is left to downstream tooling.

## Structure Examples

A minimal valid TAR (one file, then two zero blocks):

```
+----------------------------------+  offset 0
|  Header (512 bytes, ustar @+257) |
+----------------------------------+  offset 512
|  File payload (rounded to 512)   |
+----------------------------------+  offset 512 + ceil(size/512)*512
|  Zero block (512 bytes)          |
+----------------------------------+
|  Zero block (512 bytes)          |  ← terminator
+----------------------------------+  end of archive
```

Multi-entry archive:

```
[Header A][Payload A → padded to 512]
[Header B][Payload B → padded to 512]
[Header C][Payload C → padded to 512]
[Zero block][Zero block]              ← terminator
```

## Known Limitations

- **No per-entry metadata table**: only the outer archive is emitted as a
  carved file; individual member filenames and timestamps are not surfaced.
- **No GNU sparse reconstruction**: sparse-file payloads are skipped using
  the declared size only.
- **No pre-POSIX (v7) detection**: requires the `ustar` magic to be present.
- **No transparent decompression**: `.tar.gz`, `.tar.bz2`, and `.tar.xz`
  payloads are surfaced by the GZIP, BZIP2, and XZ carvers respectively;
  SwiftBeaver does not auto-decompress them before TAR parsing.
- **Block-aligned padding only**: any non-512-byte trailing data after the
  two zero blocks (some writers append junk) is left to evidence and not
  appended to the carved archive.

## Related Carvers

- [ZIP](zip.md) — archive format that walks local file headers and EOCD.
- [7Z](7z.md) — header-driven archive carver (LZMA-based).
- [GZIP](gzip.md) — frequently wraps `.tar` payloads (`.tar.gz`); see [src/carve/gzip.rs](../../src/carve/gzip.rs).
- **BZIP2** — frequently wraps `.tar` payloads (`.tar.bz2`); see [src/carve/bzip2.rs](../../src/carve/bzip2.rs).

## References

- [POSIX.1-1988 / ustar header format](https://pubs.opengroup.org/onlinepubs/9699919799/utilities/pax.html#tag_20_92_13_06)
- [GNU tar manual — Basic Tar Format](https://www.gnu.org/software/tar/manual/html_node/Standard.html)
