# BZIP2 Carver

## Overview

The BZIP2 carver extracts bzip2-compressed streams (`.bz2`) from raw
forensic evidence. BZIP2 is a Burrows–Wheeler / Huffman compression
container produced by the `bzip2` utility and commonly seen in
software distribution, log archival, and `.tar.bz2` bundles.

The format does not carry a stream length in the header, but it ends
with a fixed 48-bit end-of-stream marker. The carver therefore takes a
**marker-based streaming** approach: it anchors on the 3-byte magic
plus block-size byte at the start, then streams forward looking for
the end-of-stream marker. To bound work on false positives, the
forward search is capped at a 10 MiB ceiling.

Source: [src/carve/bzip2.rs](../../src/carve/bzip2.rs)

## Signature Detection

**Header pattern** (3 bytes): `42 5A 68` (ASCII `BZh`)

The 4th byte is a block-size selector (`'1'`–`'9'`, ASCII `0x31`–`0x39`)
indicating the 100 KiB–900 KiB Burrows–Wheeler block size. It is
validated during pre-validation but not part of the registered scanner
pattern.

### Header layout (validated)

```
Offset  Size  Field
------  ----  ----------------------------------
0       3     Magic                  (42 5A 68)
3       1     Block size selector    ('1'..='9')
4       N     Compressed blocks (Huffman bitstream)
...     6     End-of-stream marker   (17 72 45 38 50 90, byte-aligned)
...     4     Stream CRC32           (not validated)
```

### End-of-stream marker (6 bytes)

```
17 72 45 38 50 90
```

This is the byte-aligned representation of the 48-bit
`pi`-derived sentinel `0x177245385090` that bzip2 emits at the end of
the final block. The carver searches for this exact byte sequence;
non-byte-aligned occurrences inside compressed payloads are not a
concern in practice because the encoder always pads to a byte boundary
before the marker.

## Carving Algorithm

1. **Pre-validate the header**: read 4 bytes at the hit offset, confirm
   bytes `[0..3]` equal `42 5A 68` and byte `3` is in `b'1'..=b'9'`.
   Reject on short read or mismatch.
2. **Allocate output path** under the run output root using the standard
   `output_path()` helper, with extension `bz2`.
3. **Stream forward in 64 KiB chunks** from `offset + 4`, capped by
   `max_size` when configured. For each chunk:
   - Maintain a `BZIP2_END.len() - 1` (= 5) byte carry into the next
     chunk so the 6-byte end marker is never split across read
     boundaries.
   - Search the carry-prefixed buffer for the literal byte sequence
     `17 72 45 38 50 90`.
   - On a match, set `end_offset = absolute_match_offset + 6`,
     mark `validated = true`, and stop scanning.
4. **False-positive ceiling**: if the cumulative bytes searched past
   the header exceed `BZIP2_SEARCH_LIMIT` (10 MiB) without finding the
   end marker, treat the hit as a false positive and **drop it
   entirely** (no record, no on-disk file).
5. **Truncation handling**:
   - If `max_size` is reached before the end marker is found, mark the
     carve as `truncated` with error
     `"max_size reached before bzip2 end"` and emit the partial file.
   - If EOF is reached before the end marker is found, `write_range()`
     reports it; the carve is marked `truncated` with error
     `"eof before bzip2 end"`.
6. **Write the byte range** `[hit.global_offset, end_offset)` from
   evidence to the output file using `write_range()`, computing MD5
   and SHA-256 incrementally as data is written.
7. **Apply min-size filter**: if the bytes actually written are below
   the configured `min_size`, discard the writer and drop the record.

### State Machine

```
[hit on 42 5A 68]
        ↓
[read 4 bytes; verify magic + block size '1'..'9']
        ↓ (drop on mismatch)
[stream forward in 64 KiB chunks; carry 5 bytes]
        ↓
   ┌────┴──────────────────────────────────────────┐
   ↓                  ↓                            ↓
[find 17 72 45     [bytes searched         [reach max_size or EOF]
 38 50 90]          > 10 MiB]                     ↓
   ↓                  ↓                       [TRUNCATED]
[VALIDATED]        [DROP — false positive]
```

## Validation

| Field        | Meaning |
|--------------|---------|
| `validated`  | `true` when the 6-byte end-of-stream marker is found before `max_size`/EOF/search limit. |
| `truncated`  | `true` when `max_size` or EOF was reached before the end marker. |
| `errors`     | Includes `"max_size reached before bzip2 end"` and/or `"eof before bzip2 end"` when truncation occurs. |

The bzip2 stream-level CRC32 (the 32 bits immediately preceding the
end marker) and the per-block CRC32s are **not** verified; they would
require Huffman/RLE decoding of the compressed payload, which the
carver intentionally avoids.

Hits that exceed the 10 MiB search ceiling without finding the marker
are dropped silently as suspected false positives — no record is
emitted and no file is written.

## Size Constraints

Defaults from [config/default.yml](../../config/default.yml):

| Setting    | Default                | Notes |
|------------|------------------------|-------|
| `min_size` | `14` bytes             | Theoretical minimum (header + selector + minimum payload + end marker). Smaller carves are discarded. |
| `max_size` | `104 857 600` (100 MiB) | Upper bound on streaming search; `0` means unbounded. |

A separate **internal** ceiling (`BZIP2_SEARCH_LIMIT`, 10 MiB)
controls false-positive rejection and is independent of `max_size`.
This ceiling is *not* user-configurable and applies even when
`max_size` is set higher.

## Hash Computation

- MD5 and SHA-256 are computed incrementally by `write_range()` over
  exactly the bytes written to the output file (header through end
  marker for validated carves, header through truncation point
  otherwise).
- Hash computation respects the run's `HashConfig`; either or both
  hashes may be disabled via configuration.

## Testing

**Source unit tests**: [src/carve/bzip2.rs](../../src/carve/bzip2.rs)
(module `tests`)

- `carves_bzip2_with_end_marker`: builds a hand-crafted minimal stream
  (`BZh9` + 10 zero bytes + 6-byte end marker) and asserts that
  `process_hit()` yields a `validated == true` carve whose size matches
  the synthetic stream length exactly.
- `rejects_when_footer_not_found_within_limit`: emits a `BZh9` header
  followed by 11 MiB of zeros with no end marker and asserts that the
  carver returns `Ok(None)` (false-positive rejection beyond the
  10 MiB search ceiling).

Real `.bz2` payloads are exercised through the standard golden-image
framework when bzip2 samples are present in `tests/golden_image/`
(see `tests/golden_image/samples/generate_missing.sh`).

## Edge Cases

- **Multi-stream `.bz2` files**: bzip2 permits any number of
  independent streams concatenated end-to-end. The carver stops at
  the **first** end-of-stream marker. Subsequent streams will be
  re-detected as separate hits at their own header offsets.
- **Embedded `17 72 45 38 50 90` in payload**: theoretically possible
  but extremely unlikely to occur byte-aligned inside a Huffman
  bitstream. The carver does not perform alignment validation; if a
  spurious aligned match occurred mid-stream, the carve would
  terminate early and the residual bytes would be left in evidence.
- **Header CRC**: bzip2 has no per-stream header CRC; the only
  pre-validation possible is the magic + block-size byte check, which
  yields a higher false-positive rate than CRC-validated formats like
  XZ. The 10 MiB search ceiling exists to bound the cost of those
  false positives.
- **Truncated header (< 4 bytes available)**: pre-validation rejects
  with `"truncated header"`.
- **Invalid block size byte** (not `'1'`–`'9'`): pre-validation rejects
  with `"bzip2 block size invalid"`.
- **EOF mid-stream**: the carve is kept and marked `truncated` with
  error `"eof before bzip2 end"`, allowing analysts to attempt partial
  recovery with tools like `bzip2recover`.
- **Read boundary marker**: a 5-byte carry across 64 KiB read
  boundaries ensures the 6-byte end marker is never split.
- **`max_size = 0`**: interpreted as unbounded streaming; the 10 MiB
  false-positive ceiling still applies.

## Performance

- **Memory usage**: Constant — a 64 KiB read buffer plus a 5-byte
  carry vector.
- **I/O pattern**: Sequential 64 KiB reads from evidence. No random
  reads after the initial header peek.
- **CPU**: A simple byte-by-byte search anchored on the first byte of
  the end marker. No CRC, no decompression, no Huffman parsing.
- **Worst-case runtime**: bounded by `min(max_size, BZIP2_SEARCH_LIMIT
  + bytes_to_first_marker)` for evidence with no valid end marker.
  False positives cost at most ~10 MiB of sequential reads each.

## Forensic Considerations

- **Evidence integrity**: source evidence is opened read-only and
  never modified.
- **Reproducibility**: carving is deterministic — same input + same
  config yields identical output bytes and identical hashes.
- **Provenance**: every emitted record carries `run_id`,
  `global_start`, `global_end`, `size`, `md5`, `sha256`, `validated`,
  `truncated`, `errors`, and `pattern_id` (`"bzip2_header"`).
- **Truncation transparency**: partial carves are kept and clearly
  flagged so analysts can attempt salvage with `bzip2recover`.
- **Silent false-positive drops**: hits rejected by the 10 MiB search
  ceiling produce no record. Raising `max_size` does *not* relax this
  ceiling — it is internal. If exhaustive recall is required for an
  investigation, the raw byte ranges can still be inspected via the
  scanner output before extraction.
- **No decompression performed**: the carver never decodes the
  Burrows–Wheeler / Huffman payload. This keeps the forensic boundary
  clean and avoids decompression-bomb risk; SwiftBeaver carves the
  container, downstream tools decompress.

## Structure Examples

A minimal single-stream `.bz2` file:

```
Offset  Bytes                                            Field
------  -----------------------------------------------  --------------------
0x0000  42 5A 68                                         Magic ("BZh")
0x0003  39                                               Block size '9' (900 KiB)
0x0004  ... compressed Huffman bitstream ...             Block(s)
...
0xN-10  CC CC CC CC                                      Stream CRC32 (not validated)
0xN-06  17 72 45 38 50 90                                End-of-stream marker
```

The carver covers the byte range `[0x0000, 0xN)` (inclusive of the
6-byte end marker). A valid `.bz2` file may contain additional
concatenated streams beyond `0xN`; those are not consumed by this
carve.

## Known Limitations

- **First-stream-only carving** of multi-stream `.bz2` files.
  Subsequent streams must be re-detected at their own header offsets.
- **No CRC validation**: neither the per-block CRC32s nor the
  trailing stream CRC32 are checked. A corrupted body terminated by a
  valid end marker still produces a `validated` carve.
- **No block-structure parsing**: the carver does not decode block
  headers, randomization flags, or Huffman tables. Detection relies
  entirely on the magic, block-size byte, and end marker.
- **10 MiB false-positive ceiling is hard-coded** in
  `BZIP2_SEARCH_LIMIT`. Genuine bzip2 streams whose first end marker
  lies more than 10 MiB past the header (uncommon for typical block
  sizes but possible for highly compressible data with the largest
  block size and one very long block) will be silently dropped.
- **No detection of legacy bzip1 (`BZ0`)** streams; these are
  effectively extinct and not registered as a pattern.

## Related Carvers

- [XZ](xz.md) — newer LZMA2-based compression container with
  CRC-validated header and footer; the structural counterpart to
  BZIP2.
- [GZIP](gzip.md) — older DEFLATE-based compression with a trailing
  CRC32 + ISIZE epilogue; header-anchored with decoder validation.
- [7Z](7z.md) — multi-file archive that can use BZIP2 internally as a
  codec; metadata-driven (size known from header).
- [TAR](tar.md) — frequently combined with BZIP2 as `.tar.bz2` for
  software distribution.

## References

- [bzip2 and libbzip2 manual (Julian Seward)](https://sourceware.org/bzip2/manual/manual.html)
- [bzip2 file format notes (Wikipedia)](https://en.wikipedia.org/wiki/Bzip2#File_format)
