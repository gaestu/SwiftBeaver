# XZ Carver

## Overview

The XZ carver extracts XZ-compressed streams (`.xz`) from raw forensic
evidence. XZ is the LZMA2-based container format produced by the `xz`
utility and used widely in software distribution, system backups, and
package archives (`.tar.xz`, `.deb` data members, kernel images, etc.).

The format is well specified, frames are CRC-protected, and stream
boundaries can be located deterministically. The carver therefore takes a
**marker + structural validation** approach: the start is anchored on the
magic bytes and a header CRC32, and the end is accepted only when the
stream footer and Index are internally consistent.

Source: [src/carve/xz.rs](../../src/carve/xz.rs)

## Signature Detection

**Header magic** (6 bytes): `FD 37 7A 58 5A 00`

This is the only signature pattern registered for XZ; the trailing `00`
disambiguates it from arbitrary `7A 58 5A` (`zXZ`) sequences in random
data.

### Header layout (validated)

```text
Offset  Size  Field
------  ----  -----
0       6     Magic              (FD 37 7A 58 5A 00)
6       2     Stream Flags       (CRC-protected)
8       4     CRC32 of bytes 6..8 (little-endian)
```

Pre-validation reads the first 6 bytes and rejects the hit if the magic
does not match. During processing, the next 6 bytes (Stream Flags + CRC32)
are read, the Stream Flags reserved bits are checked, and the CRC32 over
the two Stream Flags bytes is compared against the stored CRC. A mismatch
or reserved Stream Flags value causes the hit to be dropped silently (no
carved file emitted).

### Footer magic (2 bytes): `59 5A`

The 12-byte stream footer ends with the ASCII `YZ` magic and contains a
CRC32 over its own backward-size and stream-flags fields.

The footer's Stream Flags must match the header Stream Flags. Its stored
Backward Size is decoded as `(stored + 1) * 4` and points to the XZ Index
immediately before the footer.

## Carving Algorithm

The XZ stream is locatable by structure rather than known size, so the
carver performs a **streaming forward search** for the footer:

1. **Pre-validate the header**: confirm the 6-byte magic. Reject on
   mismatch or short read.
2. **Validate header CRC32**: read 12 bytes at the hit offset, recompute
   the CRC32 over Stream Flags, compare against the stored CRC. Drop the
   hit on mismatch.
3. **Stream forward in 64 KiB chunks** from `offset + 12`, capped by
  `max_size` (when configured). For each chunk:
   - Maintain a 1-byte carry into the next chunk so the 2-byte footer
     magic is not split across read boundaries.
   - Search the buffered bytes for `59 5A`. For each candidate:
     - Compute the absolute footer-magic offset, then derive the
       footer start (`footer_end - 12`).
     - Read the candidate 12-byte footer and verify it ends in `59 5A`.
     - Recompute the CRC32 over `footer[4..10]` (Backward Size + Stream
       Flags) and compare against the stored CRC at `footer[0..4]`.
     - Require footer Stream Flags to match the header Stream Flags.
     - Decode Backward Size and read the referenced Index.
     - Validate the Index indicator, VLI record table, zero padding,
       Index CRC32, and the sum of padded block sizes against the bytes
       between the Stream Header and Index.
     - Walk the indexed Block extents and validate each Block Header
       size, flags, optional size fields, filter-field bounds, zero
       padding, and Block Header CRC32.
     - On a complete footer + Index match, set `end_offset` and stop scanning.
4. **Rejection handling**:
   - If EOF or `max_size` is reached before a structurally valid footer
     and Index are found, reject the candidate and emit no carved file.
   - The carver does not persist `validated=false`, `truncated=true` XZ
     fallback files by default.
5. **Allocate the output path** under the run output root only after a
   valid footer and Index are found.
6. **Write the byte range** `[hit.global_offset, end_offset)` from
   evidence to the output file using `write_range()`, computing MD5 and
   SHA-256 incrementally as data is written.
7. **Apply min-size filter**: if the bytes actually written are below the
   configured `min_size`, discard the writer and drop the record.

### State Machine

```text
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
[validate footer + Index]          [REJECT]
   ↓
[VALIDATED → end_offset = footer_end]
```

## Validation

| Field       | Meaning                                                                                  |
|-------------|------------------------------------------------------------------------------------------|
| `validated` | `true` when a stream footer and Index pass structural validation.                        |
| `truncated` | Normally `false`; invalid or incomplete XZ candidates are rejected before writing.       |
| `errors`    | Empty for accepted XZ carves unless a later evidence short read occurs after validation. |

Header CRC32 mismatches are treated as a rejected false positive and
produce no carved file at all (no record, no on-disk file).

## Size Constraints

Defaults from [config/default.yml](../../config/default.yml):

| Setting    | Default                 | Notes                                                            |
|------------|-------------------------|------------------------------------------------------------------|
| `min_size` | `32` bytes              | Minimum size of the carved span. Smaller hits are discarded.     |
| `max_size` | `1 073 741 824` (1 GiB) | Upper bound on streaming search; `0` means unbounded.            |

Files smaller than `min_size` are discarded entirely. Files reaching
`max_size` without a structurally valid footer and Index are rejected and
not written.

## Hash Computation

- MD5 and SHA-256 are computed incrementally by `write_range()` over
  exactly the bytes written to the output file (header through footer
  for validated carves).
- Hash computation respects the run's `HashConfig`; either or both
  hashes may be disabled via configuration.

## Testing

**Source unit tests**: [src/carve/xz.rs](../../src/carve/xz.rs)
(module `tests`)

- `carves_minimal_xz_with_footer`: builds a hand-crafted minimal XZ
  stream (header magic + Stream Flags + header CRC + empty Index +
  footer with valid CRC + footer magic) and asserts that
  `process_hit()` yields a `validated == true` carve whose size matches
  the synthetic stream length exactly.
- Rejection tests cover valid-header/no-footer candidates, `max_size`
  fallback avoidance, footer CRC matches with mismatched Stream Flags,
  and footer-like bytes with an invalid Index.

The fixtures exercise header CRC, footer CRC, Index CRC, Index VLI
record parsing, Block Header validation, and the footer-magic search
loop. Real `.xz` payloads are also exercised through the standard
golden-image framework when XZ samples are present in
`tests/golden_image/`.

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
- **Footer Stream Flags mismatch**: Rejected even when the footer CRC is
  valid.
- **Footer CRC coincidence with invalid Index or Block metadata**:
  Rejected before writing.
- **Truncated header (< 6 bytes available)**: Pre-validation rejects
  with `"truncated header"`.
- **EOF mid-stream**: Rejected before writing because no complete footer
  and Index relationship can be proven.
- **Read boundary footer**: A 1-byte carry across 64 KiB read
  boundaries ensures the 2-byte footer magic is never split.

## Performance

- **Memory usage**: Bounded — a 64 KiB read buffer, a 1-byte carry, a
  small candidate-footer buffer, and the referenced Index capped at
  16 MiB.
- **I/O pattern**: Sequential 64 KiB reads from evidence, plus one
  random 12-byte footer read and bounded Index read per plausible
  footer-magic candidate. Cumulative Index validation reads are capped
  per hit to avoid repeated expensive validation of crafted footer
  candidates, and Index validation is capped at 65,536 records.
- **CPU**: CRC32 checks cover Stream Flags, candidate footer fields, and
  the referenced Index and Block Headers. Index VLI records and Block
  metadata are parsed without decompression. CRC32 is computed with a
  small inline routine using the standard reflected polynomial
  `0xEDB88320`.
- **Worst-case runtime**: Bounded by `max_size` for evidence with no
  valid footer and Index.

## Forensic Considerations

- **Evidence integrity**: Source evidence is opened read-only and never
  modified.
- **Reproducibility**: Carving is deterministic — same input + same
  config yields identical output bytes and identical hashes.
- **Provenance**: Every emitted record carries `run_id`,
  `global_start`, `global_end`, `size`, `md5`, `sha256`, `validated`,
  `truncated`, `errors`, and `pattern_id` (`"xz_header"`).
- **Invalid candidate handling**: Corrupt and truncated candidates are
  rejected before output is written, avoiding large fallback files from
  footerless hits.
- **No decompression performed**: The carver never decompresses the
  LZMA2 payload. This avoids decompression-bomb risk and keeps the
  forensic boundary clean: SwiftBeaver carves the container, downstream
  tools decompress.

## Structure Examples

A minimal single-stream `.xz` file:

```text
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
- **No payload decompression**: the carver validates the XZ container
  Index/footer relationship, padded Block extents, and Block Headers but
  does not decompress LZMA2 block payloads.
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
