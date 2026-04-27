# ICO Carver

## Overview

The ICO carver extracts Windows Icon (`.ico`) and Cursor (`.cur`) files. Both
formats share an identical container structure: a small header that lists one
or more image directory entries, each pointing to embedded BMP (DIB) or PNG
image data. Total size is derived from the directory entries themselves, so
ICO is a metadata-driven format with strict per-entry validation to suppress
false positives from the very short signature.

Implementation: [src/carve/ico.rs](../../src/carve/ico.rs).

## Signature Detection

**Header Pattern**: `00 00 01 00` (ICO) or `00 00 02 00` (CUR)

The first six bytes form `ICONDIR`:

```
Offset  Size  Description
0       2     Reserved (must be 0x0000)
2       2     Image type: 1 = ICO, 2 = CUR
4       2     Number of images in the file
```

Because the magic is only four bytes and includes two zero bytes, false
positives are common. The pre-validator and the carver both perform structural
checks before any data is written.

### Config Patterns

```yaml
- id: "ico"
  extensions: ["ico", "cur"]
  header_patterns:
    - id: "ico_header"
      hex: "00000100"
    - id: "cur_header"
      hex: "00000200"
  footer_patterns: []
  max_size: 10485760
  min_size: 22
  validator: "ico"
```

## Carving Algorithm

ICO is a structure-based, metadata-driven carver:

1. **Pre-validate header** (6 bytes):
   - Reserved bytes must be `0x0000`.
   - Image type must be `1` (ICO) or `2` (CUR).
   - Image count must be in `1..=64`.
2. **Read the `ICONDIRENTRY` array**: `count * 16` bytes immediately after
   the header. Each entry is:

   ```
   Offset  Size  Description
   0       1     Width  (0 means 256)
   1       1     Height (0 means 256)
   2       1     Color count (0 if >= 8 bpp)
   3       1     Reserved
   4       2     Color planes (ICO) / hotspot X (CUR)
   6       2     Bits per pixel (ICO) / hotspot Y (CUR)
   8       4     Bytes in resource (image size)
   12      4     Image data offset (from start of file)
   ```
3. **Walk the directory** and for each entry:
   - Reject if `bytes_in_res == 0` or `image_offset` is before the end of the
     `ICONDIR`/`ICONDIRENTRY` table.
   - Reject if `bytes_in_res` exceeds the per-image cap (512 KiB).
   - Reject if `image_offset + bytes_in_res` overflows or exceeds the
     effective total cap.
   - Probe the payload at `start + image_offset` and require either a PNG
     signature (`89 50 4E 47 0D 0A 1A 0A`) or a BMP DIB header. BMP validation
     reads the first 16 bytes and checks the DIB header size (`40`, `52`, `56`,
     `108`, or `124`), width, height, planes, and bit depth.
   - Track the maximum `image_offset + bytes_in_res` across all entries.
4. **Reject the candidate** if any declared entry does not point to a valid
   BMP/PNG image resource.
5. **Compute total size** as `max(image_offset + bytes_in_res)`. Oversized
   declared spans are rejected rather than silently capped.
6. **Copy bytes** to the output file via `write_range`, computing hashes
   on the fly.
7. **Drop** the carve if the on-disk size is below the configured `min_size`.

## Validation

- **Validated** (`validated = true`): all directory entries pass sanity checks
  and every entry is backed by a recognisable BMP or PNG payload.
- **Truncated** (`truncated = true`): the declared end of the last image
  extends past EOF. The partial file is still written so analysts can inspect
  it and the metadata includes `eof before ICO end`.
- **Rejected** (no file emitted) when:
  - Reserved/type/count fields are out of range.
  - Any entry has `size == 0`, an offset before the end of the
    `ICONDIR`/`ICONDIRENTRY` table, or `size` greater than 512 KiB.
  - Any entry's declared end overflows or exceeds the effective total cap.
  - Any entry does not point at a valid BMP DIB or PNG header.

## Size Constraints

| Constant | Value | Purpose |
|----------|-------|---------|
| `min_size` (config) | 22 bytes | Smallest plausible single-entry ICO |
| `max_size` (config) | 10 MiB | Hard upper bound from configuration |
| `MAX_ICON_ENTRIES` | 64 | Stricter than the spec's 255 to limit false positives |
| `MAX_SINGLE_IMAGE_SIZE` | 512 KiB | Per-image sanity cap |
| `MAX_REASONABLE_ICO_SIZE` | 4 MiB | Internal upper bound on total carved span |

The effective maximum is `min(MAX_REASONABLE_ICO_SIZE, max_size)`. Candidates
whose declared span exceeds that cap are rejected. Files whose carved span
falls below `min_size` are discarded.

## Hash Computation

- **MD5** and **SHA-256** are produced incrementally by `write_range` as the
  carved bytes are copied to the output file.
- Hashes cover the exact byte range that was written, including the trailing
  bytes of the last image referenced by the directory.

## Testing

- Unit tests in [src/carve/ico.rs](../../src/carve/ico.rs) build synthetic ICO
  and CUR containers with BMP and PNG entries, verify exact directory-sized
  output, reject malformed entries, and assert EOF truncation metadata.
- Integration tests in [tests/carver_ico.rs](../../tests/carver_ico.rs)
  exercise public pre-validation for plausible directories and malformed
  counts.
- End-to-end golden-image coverage can be added through
  `tests/golden_image/manifest.json` when an ICO fixture is available.

## Edge Cases

1. **CUR cursors**: Identical container to ICO but `type = 2`. The carver
   accepts both. Cursor entries reuse the planes/bits-per-pixel slots for
   hotspot X/Y; the carver does not interpret these fields, so cursors carve
   correctly without special-casing.
2. **PNG-encoded icons**: Vista+ allows entries to contain a complete PNG
   stream instead of a DIB. The validator detects the PNG signature and
   accepts the entry.
3. **256×256 icons**: Width and height of `0` in `ICONDIRENTRY` mean 256.
   The carver does not interpret pixel dimensions, so this is handled
   transparently.
4. **Directories listed out of order**: Entries may declare image data in any
   order, including past the directory itself. The carver tracks the maximum
   `offset + size` rather than assuming sequential layout.
5. **Overlapping entries**: Tolerated; the carved span is `max(end)` across
   all entries.
6. **Partial trailing image**: When the last image runs past EOF, the file is
   written truncated and flagged with `truncated = true`. When a declared span
   exceeds the effective total size cap, the candidate is rejected instead of
   being capped and marked valid.
7. **Implausible counts**: Counts above 64 are rejected even though the spec
   allows 255, because high counts are almost always coincidental matches on
   the four-byte magic.

## Performance

- **Pattern**: Structure-based, metadata-driven.
- **Memory usage**: Constant — the carver only buffers the 6-byte header,
  the directory (at most `64 * 16 = 1024` bytes), and one or two short payload
  probes per entry.
- **I/O pattern**: A header read, a directory read, an 8-byte PNG probe plus a
  16-byte DIB probe for BMP-like entries, then a single sequential copy of the
  carved range.
- **CPU**: Minimal; no decompression or pixel decoding is performed.

## Forensic Considerations

- ICO carries no native timestamps; provenance is established via the
  standard SwiftBeaver fields (`run_id`, `tool_version`, `config_hash`,
  `evidence_path`, `global_start`, `global_end`, hashes).
- ICO files are commonly embedded inside PE executables (`RT_GROUP_ICON` /
  `RT_ICON` resources). Carved ICOs from a disk image may originate from
  resource sections of binaries rather than standalone files; correlate with
  the surrounding PE/ELF carves where available.
- CUR files preserve hotspot coordinates inside each `ICONDIRENTRY` even
  though the carver does not interpret them.
- The ICO handler preserves the ICO/CUR container as one carve. Embedded PNG
  payloads inside ICO entries may also be emitted as separate PNG carves when
  their signatures are independently detected and validated.

## Structure Examples

### Minimal single-entry ICO (BMP payload)

```
[ICONDIR: 6 bytes]
  Reserved : 00 00
  Type     : 01 00      (ICO)
  Count    : 01 00      (1 image)

[ICONDIRENTRY[0]: 16 bytes]
  Width    : 16
  Height   : 16
  Colors   : 0
  Reserved : 0
  Planes   : 01 00
  BitCount : 20 00      (32 bpp)
  Size     : LE u32     (DIB header + pixels + AND mask)
  Offset   : 16 00 00 00  (= 22)

[BITMAPINFOHEADER + XOR + AND mask: starts at offset 22]
  biSize   : 28 00 00 00
  biWidth  : 10 00 00 00
  biHeight : 20 00 00 00  (height * 2 for XOR + AND)
  ...
```

### Multi-entry ICO (mixed BMP and PNG)

```
[ICONDIR]
  Type  : ICO
  Count : 3

[ICONDIRENTRY[0]] -> 16x16  BMP  @ offset 54
[ICONDIRENTRY[1]] -> 32x32  BMP  @ offset 1238
[ICONDIRENTRY[2]] -> 256x256 PNG @ offset 5430

[BMP DIB #1] ...
[BMP DIB #2] ...
[PNG stream]  89 50 4E 47 0D 0A 1A 0A ...
```

## Known Limitations

1. **No PNG/BMP payload depth-validation**: image bytes are not decoded. PNG
   entries are checked by their 8-byte signature, and BMP/DIB entries are
   checked by a 16-byte header probe.
2. **Total span capped at 4 MiB** (`MAX_REASONABLE_ICO_SIZE`). Candidates
   declaring a larger span are rejected even when the configured `max_size`
   permits more.
3. **Per-image cap of 512 KiB**: rejects entries that legitimately contain
   very large 256×256 PNG payloads above this size.
4. **Entry count capped at 64** rather than the spec's 255 to limit false
   positives.
5. **No CUR-specific hotspot extraction**: cursor hotspots are preserved
   in the carved bytes but not surfaced as metadata.
6. **No validation that image resource ranges are non-overlapping with each
    other** or contiguous.

## Related Carvers

- [BMP](bmp.md): ICO image entries usually contain a `BITMAPINFOHEADER`
  payload; the BMP carver targets standalone `BM`-prefixed files instead.
- [PNG](png.md): Vista+ ICOs may embed full PNG streams in their entries; the
  PNG carver targets standalone PNG files.
