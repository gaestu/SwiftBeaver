# GZIP Carver

## Overview

The GZIP carver extracts gzip-compressed streams (`.gz`) from raw
forensic evidence. GZIP is a single-member DEFLATE container defined
by [RFC 1952](https://www.ietf.org/rfc/rfc1952.txt) and is one of the
most common compression formats on Unix-like systems, frequently seen
as standalone log archives, package payloads, HTTP `Content-Encoding`
captures, and the inner layer of `.tar.gz` bundles.

GZIP carries a CRC32 and an uncompressed-size (ISIZE) field in its
8-byte trailer, but the trailer is not byte-pattern searchable: it is
preceded only by an arbitrary-length DEFLATE bitstream with no
self-synchronizing end marker. The carver therefore uses a hybrid
**header-anchored, decoder-validated** approach:

1. Anchor on the 3-byte magic `1F 8B 08` and parse the variable-length
   GZIP header (RFC 1952 §2.3).
2. Stream forward looking for the **next** `1F 8B 08` header (or EOF /
   `max_size`) to bound the carve extent.
3. Validate the resulting range by passing it through `flate2`'s
   `GzDecoder` and confirming the decoder consumes **exactly** the
   carved byte count.

Source: [src/carve/gzip.rs](../../src/carve/gzip.rs)

## Signature Detection

**Header pattern** (3 bytes): `1F 8B 08` (magic + DEFLATE compression
method)

```
Offset  Size  Field          Notes
------  ----  -------------  --------------------------------------------
0       2     ID1, ID2       1F 8B
2       1     CM             Compression method; 08 = DEFLATE (only one
                             defined by RFC 1952; carver requires it)
3       1     FLG            Flag byte (FTEXT, FHCRC, FEXTRA, FNAME,
                             FCOMMENT)
4       4     MTIME          Little-endian Unix mtime (0 = unset)
8       1     XFL            Extra flags (compression hint)
9       1     OS             Source OS code
10      ?     [FEXTRA]       2-byte XLEN + XLEN bytes  (FLG.FEXTRA = 0x04)
?       ?     [FNAME]        NUL-terminated original filename
                             (FLG.FNAME = 0x08)
?       ?     [FCOMMENT]     NUL-terminated comment (FLG.FCOMMENT = 0x10)
?       2     [FHCRC]        Header CRC16 (FLG.FHCRC = 0x02)
?       N     CDATA          DEFLATE-compressed blocks
N+0     4     CRC32          Uncompressed CRC32 (little-endian)
N+4     4     ISIZE          Uncompressed size mod 2^32 (little-endian)
```

Including `08` in the registered scanner pattern eliminates the bulk
of false positives from unrelated `1F 8B` byte pairs in binary noise.

## Carving Algorithm

1. **Pre-validate the header** (`pre_validate`): read 3 bytes at the
   hit offset and require them to equal `1F 8B 08`. Reject on short
   read or mismatch.
2. **Parse the variable-length header** (`parse_gzip_header`):
   - Read the fixed 10-byte prefix; reject if `CM != 8`.
   - If `FLG.FEXTRA` (`0x04`) is set, read XLEN and skip
     `2 + XLEN` bytes.
   - If `FLG.FNAME` (`0x08`) is set, skip a NUL-terminated string
     (capped at 1 MiB to bound malformed-header cost).
   - If `FLG.FCOMMENT` (`0x10`) is set, skip a NUL-terminated string
     (same cap).
   - If `FLG.FHCRC` (`0x02`) is set, skip 2 bytes (CRC16). The CRC16
     is **not** verified.
   - Return the total header length. On any header parse failure
     (`CarveError::Invalid`) the hit is silently dropped.
3. **Allocate output path** under the run output root with extension
   `gz`.
4. **Stream forward in 64 KiB chunks** from `offset + header_len`,
   capped by `max_size` when configured. For each chunk:
   - Maintain a `GZIP_MAGIC.len() - 1` (= 2) byte carry into the next
     chunk so the 3-byte next-member magic is never split.
   - Search the carry-prefixed buffer for `1F 8B 08`. The first match
     **strictly after** `hit.global_offset` becomes `end_offset`.
5. **Truncation handling**:
   - If `max_size` is reached before another header is found, mark the
     carve as `truncated` with error
     `"max_size reached before gzip end"`.
   - If `write_range()` reports EOF before `end_offset`, append
     `"eof before gzip end"`.
6. **Write the byte range** `[hit.global_offset, end_offset)` from
   evidence to the output file using `write_range()`, computing MD5
   and SHA-256 incrementally.
7. **Apply min-size filter**: if bytes written are below `min_size`,
   discard the writer and drop the record.
8. **Decoder validation** (`validate_gzip_member`): open the freshly
   written file, wrap it in `flate2::read::GzDecoder`, drain to
   `io::sink()`, and require that the underlying file's stream
   position after draining equals the carved byte count.
   - If decoding errors out, or if the decoder consumes a different
     number of bytes than the carved range, **drop the carve**
     (writer is discarded — no record, no on-disk file).

### State Machine

```
[hit on 1F 8B 08]
        ↓
[pre_validate: 3-byte magic check]
        ↓ (drop on mismatch / truncated header)
[parse_gzip_header: FEXTRA / FNAME / FCOMMENT / FHCRC]
        ↓ (drop on Invalid)
[stream forward in 64 KiB chunks; 2-byte carry]
        ↓
   ┌────┴──────────────────┬─────────────────────┐
   ↓                       ↓                     ↓
[find 1F 8B 08         [reach max_size]     [reach EOF]
 strictly after hit]        ↓                     ↓
   ↓                    [TRUNCATED]          [TRUNCATED]
[end_offset known]
        ↓
[write_range → MD5/SHA-256]
        ↓
[GzDecoder drains exactly `written` bytes?]
        ↓ yes                       ↓ no
[VALIDATED]                     [DROP — invalid stream]
```

## Validation

| Field        | Meaning |
|--------------|---------|
| `validated`  | Always `true` when a record is emitted: the carve has been confirmed by `flate2`'s `GzDecoder` to be a complete, well-formed gzip member whose extent matches the carved range exactly. |
| `truncated`  | `true` when `max_size` or EOF was reached before another header boundary was found. A truncated carve is only emitted if it still passes decoder validation (rare — typically requires the trailer to be inside the truncated range by coincidence). |
| `errors`     | Includes `"max_size reached before gzip end"` and/or `"eof before gzip end"` when truncation occurs. |

The decoder validation is strict: the `flate2` decoder verifies the
stream's CRC32 and ISIZE trailer fields as part of normal
decompression. A carve whose range is too short, too long, or
internally corrupt fails validation and is dropped silently. There is
no separate "invalid" record — only validated emissions or dropped
hits.

## Size Constraints

Defaults from [config/default.yml](../../config/default.yml):

| Setting    | Default                  | Notes |
|------------|--------------------------|-------|
| `min_size` | `18` bytes               | Theoretical minimum (10-byte header + minimum DEFLATE block + 8-byte trailer). Smaller carves are discarded before validation. |
| `max_size` | `1 073 741 824` (1 GiB)  | Upper bound on streaming search; `0` means unbounded. |

`max_size` for GZIP is intentionally larger than for BZIP2/XZ because
gzip is routinely used for very large compressed log files and tarball
payloads.

## Hash Computation

- MD5 and SHA-256 are computed incrementally by `write_range()` over
  exactly the bytes written to the output file (header through
  CRC32+ISIZE trailer for validated carves).
- Hash computation respects the run's `HashConfig`; either or both
  hashes may be disabled via configuration.

## Testing

**Source unit tests**: [src/carve/gzip.rs](../../src/carve/gzip.rs)
(module `tests`)

- `carves_until_next_gzip_header`: encodes two minimal gzip members
  back-to-back with `flate2::GzEncoder`, runs `process_hit()` on the
  first, and asserts a `validated` carve whose size equals the first
  member's exact length. This exercises both the next-header
  termination path and decoder validation.
- `rejects_invalid_gzip_stream`: feeds a syntactically plausible
  10-byte gzip header followed by garbage and asserts `process_hit()`
  returns `Ok(None)` (decoder validation fails, carve is dropped, no
  file written).

Real `.gz` payloads are exercised through the standard golden-image
framework when gzip samples are present in `tests/golden_image/`
(see `tests/golden_image/samples/generate_missing.sh`).

## Edge Cases

- **Multi-member gzip files** (e.g. concatenated `.gz` streams,
  `pigz`-style parallel output): RFC 1952 §2.2 permits any number of
  independent members concatenated end-to-end. The carver detects
  the next member's `1F 8B 08` header and **terminates the current
  carve at that boundary**. Each member is then re-detected as its
  own hit and emitted as a separate carved file. Joining members
  back together is left to downstream tooling.
- **Optional FNAME field**: parsed and skipped (NUL-terminated, with
  a 1 MiB safety cap). The original filename is **not** preserved in
  metadata; output filenames follow the standard offset-based naming
  scheme. Analysts who need the embedded filename can recover it
  trivially with `gzip -lN` against the carved file.
- **Optional FCOMMENT field**: parsed and skipped under the same
  1 MiB cap. Not preserved in metadata.
- **Optional FHCRC field**: skipped without verification. Header CRC16
  mismatches do not cause rejection — the decoder validation step
  catches body-level corruption regardless.
- **FEXTRA subfields**: skipped opaquely. Vendor-specific subfields
  (e.g. `RA` for random access, `BC` for `BGZF`/bgzip) are not
  parsed.
- **Embedded `1F 8B 08` in DEFLATE payload**: the next-header search
  may match a coincidental occurrence inside the compressed bitstream.
  This would terminate the carve early; decoder validation then fails
  (consumed bytes ≠ carved bytes) and the carve is dropped silently.
  The genuine member at its true offset is unaffected — it remains a
  separate hit.
- **Truncated header (< 3 bytes available)**: pre-validation rejects
  with `"truncated header"`.
- **Unsupported compression method** (`CM != 8`): header parse rejects
  with `"gzip method unsupported"`. Only DEFLATE is supported, which
  matches RFC 1952 reality (no other CM values have ever been
  defined).
- **Header longer than 1 MiB** (malformed `FNAME`/`FCOMMENT` with no
  NUL): header parse rejects with `"gzip string too long"`. Bounds
  the cost of pathological headers.
- **EOF mid-member**: the carve is written up to EOF and marked
  `truncated`, but decoder validation almost always rejects it
  because the CRC32/ISIZE trailer is missing. Such carves are
  therefore dropped silently in practice.
- **`max_size = 0`**: interpreted as unbounded streaming; the carve
  proceeds until the next header or EOF.
- **Read boundary header**: a 2-byte carry across 64 KiB read
  boundaries ensures the 3-byte `1F 8B 08` is never split.

## Performance

- **Memory usage**: Constant during streaming — a 64 KiB read buffer
  plus a 2-byte carry vector. Decoder validation re-opens the carved
  file and streams it through `GzDecoder` to `io::sink()`, which uses
  bounded internal buffers.
- **I/O pattern**: Sequential 64 KiB reads from evidence during
  streaming, followed by one full sequential read of the carved file
  during decoder validation.
- **CPU**: Byte-level scan for the next header (anchored on the first
  byte of the 3-byte magic) plus DEFLATE decoding for validation.
  Validation cost scales linearly with the compressed size.
- **Worst-case runtime**: bounded by `min(max_size, evidence_size -
  hit_offset)` plus one decode pass over the carved bytes.

## Forensic Considerations

- **Evidence integrity**: source evidence is opened read-only and
  never modified.
- **Reproducibility**: carving is deterministic — same input + same
  config yields identical output bytes and identical hashes.
  Decoder validation is also deterministic for a given `flate2`
  version; upgrades to `flate2` could in principle change validation
  outcomes for malformed streams, but well-formed RFC 1952 members
  are stable across versions.
- **Provenance**: every emitted record carries `run_id`,
  `global_start`, `global_end`, `size`, `md5`, `sha256`, `validated`,
  `truncated`, `errors`, and `pattern_id` (`"gzip_header"`).
- **Strict validation, no recovery records**: unlike BZIP2, GZIP
  carves that fail validation are dropped entirely rather than
  emitted as "invalid". This favours precision over recall: a
  GZIP record in the metadata can be trusted to decompress cleanly.
  Analysts requiring partial-stream salvage should consult the raw
  scanner output for `gzip_header` hits before extraction.
- **No decompression beyond validation**: the carver decompresses the
  member only to confirm well-formedness; the decompressed bytes are
  discarded (`io::sink()`). SwiftBeaver carves the container;
  downstream tools decompress for analysis.
- **Embedded original filename / mtime are not surfaced** in
  metadata. They remain available inside the carved `.gz` file for
  any analyst who runs `gzip -lN` or equivalent.

## Structure Examples

A minimal single-member `.gz` file with no optional header fields:

```
Offset  Bytes                           Field
------  ------------------------------  ----------------------------
0x0000  1F 8B 08                        Magic + CM (DEFLATE)
0x0003  00                              FLG (no optional fields)
0x0004  00 00 00 00                     MTIME
0x0008  00                              XFL
0x0009  03                              OS (Unix)
0x000A  ... DEFLATE blocks ...          CDATA
0xN-08  CC CC CC CC                     CRC32 of uncompressed data
0xN-04  SS SS SS SS                     ISIZE (uncompressed size mod 2^32)
```

The carver covers the byte range `[0x0000, 0xN)` (inclusive of the
8-byte trailer). Two concatenated members appear as:

```
0x0000   1F 8B 08 ... CRC ISIZE          Member 1  → carved as one file
0xN      1F 8B 08 ... CRC ISIZE          Member 2  → carved as a separate file
```

A `.tar.gz` archive is carved as its outer GZIP container only; the
inner TAR is reconstructed by downstream tooling after decompression.

## Known Limitations

- **One member per emitted file**: concatenated multi-member streams
  are split at member boundaries. Re-joining requires post-processing
  (e.g. `cat *.gz > combined.gz`).
- **Embedded original filename is not preserved in metadata**.
  It remains inside the carved `.gz` payload (FNAME field) but is
  not surfaced as a Parquet column.
- **No header-CRC enforcement**: when `FLG.FHCRC` is set, the
  declared CRC16 is skipped without verification. Decoder validation
  catches body corruption but not header-only corruption.
- **No support for non-DEFLATE compression methods**: per RFC 1952
  this is not a real-world limitation, but a malformed header
  declaring `CM != 8` is rejected.
- **No vendor-extension awareness**: `BGZF` (bgzip) and other FEXTRA
  subfield-driven random-access variants are carved as ordinary GZIP
  members. Their block structure is not exposed.
- **Strict validation drops corrupt-but-recoverable carves**: streams
  with body-level CRC mismatches or ISIZE drift fail
  `GzDecoder::read_to_end` and are dropped. This is intentional
  (favouring precision) but means partial recovery for damaged gzip
  streams must be performed externally.

## Related Carvers

- [BZIP2](bzip2.md) — older block-sorting compression with a
  byte-aligned 6-byte end-of-stream marker; marker-based.
- [XZ](xz.md) — newer LZMA2-based compression container with
  CRC-validated header and footer; structurally the modern successor
  to GZIP.
- [TAR](tar.md) — frequently combined with GZIP as `.tar.gz` for
  software distribution and backup. The TAR carver does not see
  through the GZIP layer; carved `.gz` files must be decompressed
  before TAR carving can recover their contents.
- [7Z](7z.md) — multi-file archive that can use DEFLATE internally as
  one of several codecs; metadata-driven (size known from header).

## References

- [RFC 1952 — GZIP file format specification version 4.3](https://www.ietf.org/rfc/rfc1952.txt)
- [RFC 1951 — DEFLATE compressed data format specification](https://www.ietf.org/rfc/rfc1951.txt)
- [`flate2` crate documentation](https://docs.rs/flate2/)
