# XZ Carver

## Overview

The XZ carver extracts XZ-compressed streams (`.xz`) from raw forensic
evidence. XZ is the LZMA2-based container format produced by the `xz`
utility and used widely in software distribution, system backups, and
package archives (`.tar.xz`, `.deb` data members, kernel images, etc.).

The format is well specified, frames are CRC-protected, and stream
boundaries can be located deterministically. The carver therefore takes a
**marker + CRC validation** approach: the start is anchored on the magic
bytes and a header CRC32, and the end is located by scanning for the
stream footer magic and validating the footer CRC32 over its CRC-protected
fields.

Source: [src/carve/xz.rs](../../src/carve/xz.rs)

## Signature Detection

**Header magic** (6 bytes): `FD 37 7A 58 5A 00`

This is the only signature pattern registered for XZ; the trailing `00`
disambiguates it from arbitrary `7A 58 5A` (`zXZ`) sequences in random
data.

### Header layout (validated)

```
Offset  Size  Field
------  ----  -----
0       6     Magic              (FD 37 7A 58 5A 00)
6       2     Stream Flags       (CRC-protected)
8       4     CRC32 of bytes 6..8 (little-endian)
```

Pre-validation reads the first 6 bytes and rejects the hit if the magic
does not match. During processing, the next 6 bytes (Stream Flags + CRC32)
are read and the CRC32 over the two Stream Flags bytes is compared
against the stored CRC. A mismatch causes the hit to be dropped silently
(no carved file emitted).

### Footer magic (2 bytes): `59 5A`

The 12-byte stream footer ends with the ASCII `YZ` magic and contains a
CRC32 over its own backward-size and stream-flags fields.

## Carving Algorithm

The XZ stream is locatable by structure rather than known size, so the
carver performs a **streaming forward search** for the footer:

1. **Pre-validate the header**: confirm the 6-byte magic. Reject on
   mismatch or short read.
2. **Validate header CRC32**: read 12 bytes at the hit offset, recompute
   the CRC32 over Stream Flags, compare against the stored CRC. Drop the
   hit on mismatch.
3. **Allocate output path** under the run output root using the standard
   `output_path()` helper, with extension `xz`.
4. **Stream forward in 64 KiB chunks** from `offset + 12`, capped by
   `max_size` (when configured). For each chunk:
   - Maintain a 1-byte carry into the next chunk so the 2-byte footer
     magic is not split across read boundaries.
   - Search the buffered bytes for `59 5A`. For each candidate:
     - Compute the absolute footer-magic offset, then derive the
       footer start (`footer_end - 12`).
     - Read the candidate 12-byte footer and verify it ends in `59 5A`.
     - Recompute the CRC32 over `footer[4..10]` (Backward Size + Stream
       Flags) and compare against the stored CRC at `footer[0..4]`.
     - On a CRC match, set `end_offset` and stop scanning.
5. **Truncation handling**:
   - If `max_size` is reached before a valid footer is found, mark the
     carve as `truncated` with error `"max_size reached before xz end"`
     and emit the partial file.
   - If EOF is reached before a valid footer is found, `write_range()`
     reports it; the carve is marked `truncated` with error
     `"eof before xz end"`.
6. **Write the byte range** `[hit.global_offset, end_offset)` from
   evidence to the output file using `write_range()`, computing MD5 and
   SHA-256 incrementally as data is written.
7. **Apply min-size filter**: if the bytes actually written are below the
   configured `min_size`, discard the writer and drop the record.

### State Machine

```
[hit on FD 37 7A 58 5A 00]
        ↓
[read 12 bytes; verify magic + header CRC32]
        ↓ (drop on mismatch)
[stream forward in 64 KiB chunks, carry 1 byte]
        ↓
   ┌────┴────────────────────────────────┐
   ↓                                     ↓
[scan for 59 5A]                  [reach max_size or EOF]
   ↓                                     ↓
[validate 12-byte footer CRC32]    [TRUNCATED]
   ↓
[VALIDATED → end_offset = footer_end]
```

## Validation

| Field        | Meaning |
|--------------|---------|
| `validated`  | `true` when a stream footer with a passing CRC32 is found. |
| `truncated`  | `true` when `max_size` or EOF was reached before a valid footer. |
| `errors`     | Includes `"max_size reached before xz end"` and/or `"eof before xz end"` when truncation occurs. |

Header CRC32 mismatches are treated as a rejected false positive and
produce no carved file at all (no record, no on-disk file).

## Size Constraints

Defaults from [config/default.yml](../../config/default.yml):

| Setting    | Default       | Notes |
|------------|---------------|-------|
| `min_size` | `32` bytes    | Minimum size of the carved span. Smaller hits are discarded. |
| `max_size` | `1 073 741 824` (1 GiB) | Upper bound on streaming search; `0` means unbounded. |

Files smaller than `min_size` are discarded entirely. Files reaching
`max_size` without a footer are kept and flagged `truncated`.

## Hash Computation

- MD5 and SHA-256 are computed incrementally by `write_range()` over
  exactly the bytes written to the output file (header through footer
  for validated carves, header through truncation point otherwise).
- Hash computation respects the run's `HashConfig`; either or both
  hashes may be disabled via configuration.

## Testing

**Source unit tests**: [src/carve/xz.rs](../../src/carve/xz.rs)
(module `tests`)

- `carves_minimal_xz_with_footer`: builds a hand-crafted minimal XZ
  stream (header magic + Stream Flags + header CRC + dummy index +
  footer with valid CRC + footer magic) and asserts that
  `process_hit()` yields a `validated == true` carve whose size matches
  the synthetic stream length exactly.

The fixture exercises both CRC paths (header CRC and footer CRC) and
the footer-magic search loop. Real `.xz` payloads are also exercised
through the standard golden-image framework when XZ samples are
present in `tests/golden_image/`.

## Edge Cases

- **Multi-stream `.xz` files**: The carver stops at the **first** valid
  stream footer it encounters. Multi-stream concatenations (the format
  permits any number of streams concatenated, optionally separated by
  zero-padded multiples of 4 bytes) are carved as the first stream
  only. Subsequent streams may be re-detected as separate hits at
  their own header offsets.
- **Padding between streams**: Inter-stream zero padding is not
  consumed by the carver; it is left in evidence and either ignored or
  picked up by the next header hit.
- **Spurious `59 5A` bytes**: A naive footer scan would false-match on
  any `YZ` byte pair in compressed data. The carver always validates a
  candidate footer's CRC32 before accepting it, so random `59 5A`
  pairs in LZMA2 output do not terminate the carve early.
- **Header CRC mismatch**: Treated as a false positive on the magic;
  the hit is dropped without emitting a record.
- **Truncated header (< 6 bytes available)**: Pre-validation rejects
  with `"truncated header"`.
- **EOF mid-stream**: Carve is kept and marked `truncated` with error
  `"eof before xz end"`, allowing forensic analysts to attempt partial
  decompression.
- **Read boundary footer**: A 1-byte carry across 64 KiB read
  boundaries ensures the 2-byte footer magic is never split.

## Performance

- **Memory usage**: Constant — a 64 KiB read buffer plus a 1-byte carry
  and a small candidate-footer buffer.
- **I/O pattern**: Sequential 64 KiB reads from evidence, plus one
  random 12-byte read per footer-magic candidate to validate the CRC.
- **CPU**: One CRC32 over the 2-byte Stream Flags at start, plus one
  CRC32 over 6 bytes per validated footer candidate. CRC32 is computed
  with a small inline routine using the standard reflected polynomial
  `0xEDB88320`.
- **Worst-case runtime**: Bounded by `max_size` for evidence with no
  valid footer.

## Forensic Considerations

- **Evidence integrity**: Source evidence is opened read-only and never
  modified.
- **Reproducibility**: Carving is deterministic — same input + same
  config yields identical output bytes and identical hashes.
- **Provenance**: Every emitted record carries `run_id`,
  `global_start`, `global_end`, `size`, `md5`, `sha256`, `validated`,
  `truncated`, `errors`, and `pattern_id` (`"xz_header"`).
- **Truncation transparency**: Partial carves are kept and clearly
  flagged so analysts can attempt salvage decompression with tools
  like `xz --decompress --robot` or `xzcat`.
- **No decompression performed**: The carver never decompresses the
  LZMA2 payload. This avoids decompression-bomb risk and keeps the
  forensic boundary clean: SwiftBeaver carves the container, downstream
  tools decompress.

## Structure Examples

A minimal single-stream `.xz` file:

```
Offset  Bytes                                            Field
------  -----------------------------------------------  --------------------
0x0000  FD 37 7A 58 5A 00                                Stream Header magic
0x0006  00 00                                            Stream Flags
0x0008  CC CC CC CC                                      Header CRC32 (LE)
0x000C  ... compressed blocks (LZMA2) ...                Block(s) + Index
...
0xN-12  CC CC CC CC                                      Footer CRC32 (LE)
0xN-08  BB BB BB BB                                      Backward Size (LE)
0xN-04  00 00                                            Stream Flags (mirror)
0xN-02  59 5A                                            Stream Footer magic
```

The carver covers the byte range `[0x0000, 0xN)` (inclusive of the
footer magic).

## Known Limitations

- **First-stream-only carving** of multi-stream `.xz` files. Subsequent
  streams must be re-detected at their own header offsets.
- **No payload decompression and no Index validation**: the dummy
  Index/Block bytes between header and footer are not parsed. The
  Backward Size field in the footer is not cross-checked against the
  carved span; a CRC-valid footer attached to a corrupted body will
  still produce a `validated` carve.
- **No Stream Flags consistency check** between the Stream Header
  flags and the Stream Footer flags (both are CRC-validated
  individually, but not compared).
- **No detection of the legacy `.lzma` format** (raw LZMA1 streams),
  which has no magic and is not carvable by signature scanning.

## Related Carvers

- [BZIP2](bzip2.md) — older compression format; also marker-based,
  uses a 6-byte end-of-stream marker without CRC validation.
- [GZIP](gzip.md) — older compression format (DEFLATE); marker-based with a
  trailing CRC32 + ISIZE. Documentation pending.
- [7Z](7z.md) — multi-file archive that can use LZMA2 internally;
  metadata-driven (size known from header).
- [TAR](tar.md) — frequently combined with XZ as `.tar.xz` for
  software distribution.

## References

- [The .xz File Format specification (v1.1.0)](https://tukaani.org/xz/xz-file-format.txt)
- [XZ Utils home page](https://tukaani.org/xz/)
