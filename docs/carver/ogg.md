# OGG Carver

## Overview

The OGG carver extracts Ogg container streams (`.ogg`, `.oga`, `.ogv`)
from raw forensic evidence. Ogg is a page-oriented multiplex container
used by Vorbis, Opus, FLAC, Theora, and Speex codecs. Each page carries
its own CRC32 and a stream serial number, which the carver uses to
walk the stream forward and detect its precise extent.

The format does not carry an overall stream length in the header, but
each page is self-describing (lacing table → page size) and the final
page of a logical bitstream sets an explicit end-of-stream (EOS) flag.
The carver therefore takes a **structure-based streaming** approach:
it parses pages sequentially from the BOS (beginning-of-stream) page,
verifying the CRC32 and serial number of every page until the EOS flag
is observed. Aggressive bounds (max page count, max page data size)
and CRC validation make the carver highly resistant to false positives
on the short 4-byte `OggS` signature.

Source: [src/carve/ogg.rs](../../src/carve/ogg.rs)

## Signature Detection

**Header pattern** (4 bytes): `4F 67 67 53` (ASCII `OggS`)

Pattern id: `ogg_sync`. The 5th byte is the structure version and must
be `0x00`; any other value is rejected during pre-validation.

### Page header layout (validated)

```
Offset  Size  Field
------  ----  --------------------------------------------------
0       4     Capture pattern        (4F 67 67 53, "OggS")
4       1     Stream structure ver.  (0x00 only)
5       1     Header type flags      (0x01 cont, 0x02 BOS, 0x04 EOS)
6       8     Granule position       (codec-specific, LE)
14      4     Bitstream serial no.   (LE)
18      4     Page sequence number   (LE)
22      4     CRC32 of full page     (LE; computed with this field = 0)
26      1     Number of segments     (0..=255)
27      N     Segment table          (N = number of segments, each 0..=255)
27+N    M     Page data              (M = sum of segment table)
```

### Recognized BOS codec signatures

The first page (BOS) data must begin with one of:

| Codec  | Signature bytes              | Type            |
|--------|------------------------------|-----------------|
| Vorbis | `01 76 6F 72 62 69 73`       | Audio           |
| Opus   | `4F 70 75 73 48 65 61 64`    | Audio           |
| FLAC   | `7F 46 4C 41 43`             | Lossless audio  |
| Theora | `80 74 68 65 6F 72 61`       | Video           |
| Speex  | `53 70 65 65 78 20 20 20`    | Audio (note trailing spaces) |

Streams whose BOS page data starts with any other byte sequence are
rejected during pre-validation.

## Carving Algorithm

1. **Pre-validate the first page** (no file I/O):
   - Read the 27-byte header, then the segment table (`segment_count`
     bytes), then the page data (sum of the segment table).
   - Reject if the read is short, the capture pattern is not `OggS`,
     the version byte is non-zero, the BOS flag (`0x02`) is not set,
     the page data size exceeds `MAX_PAGE_DATA_SIZE` (65,025), the
     stored CRC32 does not match the recomputed CRC, or the BOS data
     does not begin with a known codec signature.
2. **Allocate output path** under the run output root using
   `output_path()` with extension `ogg` (configured per run).
3. **Stream pages sequentially** from `hit.global_offset` via
   `CarveStream`:
   - Read 27-byte header, then `segment_count` bytes of segment table,
     then `sum(segment_table)` bytes of page data.
   - Reject the page if data size exceeds `MAX_PAGE_DATA_SIZE`.
   - Verify CRC32 over `header (with CRC field zeroed)` + segment
     table + page data using the Ogg CRC-32/MPEG-2 polynomial
     (`0x04C11DB7`, init 0, no final XOR, MSB-first).
4. **First page (sequence index 0)**: CRC failure, missing BOS flag,
   or unknown codec are fatal — the carve is discarded entirely.
   Record the BOS serial number as `expected_serial`.
5. **Subsequent pages**:
   - CRC mismatch → mark `truncated`, append error
     `"CRC32 mismatch on subsequent page (output includes invalid trailing page)"`,
     and stop. The bytes of the bad page are already on disk and
     remain in the carve.
   - Serial number ≠ `expected_serial` → mark `truncated`, append
     error `"serial number mismatch"`, and stop.
6. **End-of-stream**: when a page's header type has the EOS flag
   (`0x04`) set, mark the carve `validated = true` and stop.
7. **Hard limits**:
   - More than `MAX_OGG_PAGES` (100,000) pages → invalid, discard.
   - Fewer than `MIN_PAGE_COUNT` (2) valid pages → invalid, discard.
   - Stream byte count reaches configured `max_size` → mark
     `truncated`, append error `"max_size reached"`, and stop.
8. **Apply min-size filter**: if the bytes actually written are below
   the configured `min_size`, discard the writer and drop the record.

### State machine

```
[hit on 4F 67 67 53]
        ↓
[pre-validate first page (header + lacing + data + CRC + codec)]
        ↓ (drop on any failure)
[stream pages via CarveStream]
        ↓
   ┌────┴───────────────────────────────┬──────────────────────────┐
   ↓                                    ↓                          ↓
[page with EOS flag (0x04)]   [CRC mismatch / serial mismatch]   [page count > 100k
   ↓                              on a non-first page]              OR page data > 65,025
[VALIDATED]                          ↓                              OR < 2 pages]
                                 [TRUNCATED]                          ↓
                                                                  [DROP — invalid]
```

## Validation

| Field        | Meaning |
|--------------|---------|
| `validated`  | `true` when a page with the EOS flag (`0x04`) was reached and parsed without error. |
| `truncated`  | `true` when a CRC/serial mismatch on a non-first page, EOF, or `max_size` stopped the stream before EOS. |
| `errors`     | One or more of: `"CRC32 mismatch on subsequent page (output includes invalid trailing page)"`, `"serial number mismatch"`, `"max_size reached"`, `"unexpected EOF"`, `"truncated stream"`. |

The granule position (codec-specific timestamp) is **not** validated;
SwiftBeaver carves the container, not the codec payload. Per-page
CRC32 validation is exhaustive and covers every byte of every page,
making the carver robust against random false-positive `OggS` matches
in unrelated data.

## Size Constraints

Defaults from [config/default.yml](../../config/default.yml):

| Setting    | Default                | Notes |
|------------|------------------------|-------|
| `min_size` | `28` bytes             | Theoretical minimum (one zero-data page header). Smaller carves are discarded. |
| `max_size` | `104 857 600` (100 MiB) | Upper bound on streaming output; `0` means unbounded. |

Two **internal** ceilings further constrain runtime and are not
user-configurable:

| Constant              | Value   | Purpose |
|-----------------------|---------|---------|
| `MAX_OGG_PAGES`       | 100,000 | Hard stop on pages parsed per stream. |
| `MAX_PAGE_DATA_SIZE`  | 65,025  | Per-page payload ceiling (255 segments × 255 bytes), the format maximum. |
| `MIN_PAGE_COUNT`      | 2       | Reject trivially short matches. |

## Hash Computation

- MD5 and SHA-256 are computed incrementally by `CarveStream` over
  exactly the bytes written to the output file (BOS page through EOS
  page for validated carves, or through the truncation point
  otherwise).
- Hash computation respects the run's `HashConfig`; either or both
  hashes may be disabled via configuration.

## Testing

**Source unit tests**: [src/carve/ogg.rs](../../src/carve/ogg.rs)
(module `tests`)

- `carves_valid_vorbis_stream`: builds a minimal two-page Vorbis
  stream (BOS + EOS) with valid CRC32 and asserts `validated == true`
  and `size == stream_len`.
- `carves_opus_stream`: same shape with an `OpusHead` BOS payload.
- `rejects_unknown_codec`: BOS page contains `UnknownCodec`; carver
  returns `Ok(None)`.
- `rejects_bad_first_page_crc`: corrupts the BOS CRC field; carver
  returns `Ok(None)`.

A test helper (`build_ogg_page`) constructs syntactically correct
pages with valid CRC32 from arbitrary header flags, serial, sequence,
granule, and payload — used to exercise BOS, EOS, and corruption
scenarios.

Real `.ogg` payloads are exercised through the standard golden-image
framework when Ogg samples are present in `tests/golden_image/`
(see `tests/golden_image/samples/generate_missing.sh`).

## Edge Cases

- **Multiplexed (chained) streams**: Ogg permits multiple logical
  bitstreams concatenated end-to-end, each with its own BOS/EOS pair.
  The carver stops at the first EOS page with the BOS serial number
  and does **not** follow chained continuations. Each subsequent
  logical stream is re-detected as a separate hit at its own BOS.
- **Multiplexed (interleaved) streams**: a single physical stream can
  interleave pages from multiple logical streams (different serial
  numbers). The carver currently enforces a single serial number per
  carve and treats a serial change as truncation. This means
  multi-stream Ogg files (e.g., Theora video with a Vorbis audio
  track) carve only the first logical stream.
- **CRC mismatch on the first page**: fatal — the entire hit is
  rejected with no record and no on-disk file.
- **CRC mismatch on a subsequent page**: the page bytes are already
  written by `CarveStream` when the mismatch is detected; the carve
  is kept and marked `truncated`, with an explicit error noting that
  the output includes the invalid trailing page.
- **Continuation packets** (header type bit `0x01`): handled
  transparently — packet boundaries are irrelevant to the carver,
  which operates on pages.
- **Empty pages** (zero-data segments): allowed; the BOS or EOS page
  may legitimately have zero or minimal payload.
- **Page count overflow** (> 100,000 pages): treated as malformed and
  discarded; protects against pathological or attacker-crafted input.
- **Truncated BOS read**: pre-validation rejects with
  `"truncated first page"`.
- **Unsupported version byte** (≠ 0): rejected with
  `"ogg version unsupported"`. No Ogg version other than 0 has ever
  been deployed.
- **`max_size = 0`**: interpreted as unbounded streaming; the
  100,000-page and 65,025-byte page ceilings still apply.

## Performance

- **Memory usage**: One full page in memory at a time (≤ 65 KiB
  payload + 27-byte header + 255-byte segment table). No buffering
  beyond the active page.
- **I/O pattern**: Sequential reads through `CarveStream`, plus a
  one-shot pre-validation read of the first page from evidence.
- **CPU**: One CRC32 (CRC-32/MPEG-2) computation per page using a
  precomputed 256-entry table. No decompression, no codec parsing.
- **Worst-case runtime**: bounded by `min(max_size, MAX_OGG_PAGES *
  (27 + 255 + 65,025))` ≈ 6.2 GiB per stream — but in practice
  capped by `max_size` (100 MiB default) long before the page limit.

## Forensic Considerations

- **Evidence integrity**: source evidence is opened read-only and
  never modified. Pre-validation reads from evidence directly without
  allocating output state.
- **Reproducibility**: carving is deterministic — same input + same
  config yields identical output bytes and identical hashes.
- **Provenance**: every emitted record carries `run_id`,
  `global_start`, `global_end`, `size`, `md5`, `sha256`, `validated`,
  `truncated`, `errors`, and `pattern_id` (`"ogg_sync"`).
- **Truncation transparency**: partial carves are kept and clearly
  flagged so analysts can attempt salvage with codec-aware tools
  (`oggz-validate`, `opusinfo`, `ogginfo`).
- **No payload decoding**: the carver never decodes Vorbis, Opus,
  FLAC, Theora, or Speex frames. This keeps the forensic boundary
  clean and avoids decoder-bomb risk; SwiftBeaver carves the
  container, downstream tools decode.
- **CRC-validated extents**: because every byte of every page is
  covered by CRC32, the carved extent is byte-exact when `validated`
  is true. This is a stronger guarantee than marker-only carvers
  (e.g., JPEG, PDF) can provide.

## Structure Examples

A minimal validated single-stream Vorbis `.ogg` file (BOS + EOS):

```
Offset  Bytes                                            Field
------  -----------------------------------------------  --------------------
0x0000  4F 67 67 53                                      Capture ("OggS")
0x0004  00                                               Version
0x0005  02                                               Header type (BOS)
0x0006  00 00 00 00 00 00 00 00                          Granule position
0x000E  01 00 00 00                                      Serial number
0x0012  00 00 00 00                                      Page sequence (0)
0x0016  CC CC CC CC                                      CRC32 (computed)
0x001A  01                                               Segment count
0x001B  1E                                               Segment lengths (30)
0x001C  01 76 6F 72 62 69 73 ...                         Vorbis ID header (30 B)
0x003A  4F 67 67 53                                      Capture ("OggS")
0x003E  00                                               Version
0x003F  04                                               Header type (EOS)
0x0040  00 00 00 00 00 00 00 00                          Granule position
0x0048  01 00 00 00                                      Serial number (matches BOS)
0x004C  01 00 00 00                                      Page sequence (1)
0x0050  DD DD DD DD                                      CRC32 (computed)
0x0054  01                                               Segment count
0x0055  00                                               Segment lengths (0 — empty page)
0x0056  ...                                              (no page data)
```

The carver covers the byte range `[0x0000, 0x0056)` (BOS through end
of EOS page). The exact end offset depends on the BOS payload size
and number of segments.

## Known Limitations

- **First-stream-only carving** of chained Ogg files. Subsequent
  logical streams must be re-detected at their own BOS offsets.
- **No interleaved multiplex support**: a serial number change is
  treated as truncation rather than as a parallel logical stream, so
  multi-track Ogg (e.g., Vorbis audio + Theora video) yields only the
  first track.
- **No codec-level validation**: Vorbis comment / Opus tag / FLAC
  STREAMINFO contents are not parsed or verified beyond the BOS
  identification signature.
- **No granule position validation**: monotonicity of granule
  positions is not checked; corrupt timestamps embedded in valid
  pages do not affect the carve.
- **Speex signature requires exact 8-byte match** including trailing
  spaces (`"Speex   "`); non-spec encoders that omit padding will be
  rejected as unknown codec.
- **CRC mismatch on a subsequent page leaves the bad page in the
  output** because `CarveStream` cannot rewind. The carve is flagged
  `truncated` and the error message names this behavior explicitly.
- **`MAX_OGG_PAGES` (100,000) is hard-coded**. Genuine streams
  exceeding this page count (extremely long-form audio with very
  small pages) will be silently rejected as invalid.

## Related Carvers

- [WAV](wav.md) — RIFF-based PCM audio container; structurally
  metadata-driven (size known from header) rather than page-walked.
- [MP3](mp3.md) — frame-based audio carver; analogous in that
  per-frame structures are validated sequentially.
- [WEBM](webm.md) — Matroska/EBML container often used alongside
  Ogg/Theora for free-codec video.
- [FLAC](#) — when stored in its native container, not Ogg-encapsulated;
  Ogg-FLAC is handled here.

## References

- [RFC 3533 — The Ogg Encapsulation Format Version 0](https://www.rfc-editor.org/rfc/rfc3533)
- [Xiph.org Ogg documentation](https://xiph.org/ogg/doc/)
- [Vorbis I specification](https://xiph.org/vorbis/doc/Vorbis_I_spec.html)
- [RFC 7845 — Ogg Encapsulation for the Opus Audio Codec](https://www.rfc-editor.org/rfc/rfc7845)
