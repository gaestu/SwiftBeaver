# AVI Carver

## Overview

The AVI carver extracts Audio Video Interleave (AVI) files by parsing the RIFF
container structure and using the embedded file size. AVI is a multimedia
container introduced by Microsoft as part of the Video for Windows technology
and is built on the same RIFF foundation as WAV and WebP.

## Signature Detection

**Header Pattern**: `RIFF` followed by `AVI ` at offset +8

Scanner detects:

- Bytes 0-3: `RIFF` (ASCII: 0x52 0x49 0x46 0x46)
- Bytes 4-7: RIFF chunk size (little-endian u32; ignored at scan time)
- Bytes 8-11: `AVI ` (ASCII: 0x41 0x56 0x49 0x20 — note the trailing space)

Byte signature: `52 49 46 46 xx xx xx xx 41 56 49 20`

The carver registers under the `avi_riff` pattern id (see
[config/default.yml](../../config/default.yml)).

## Carving Algorithm

AVI uses the RIFF (Resource Interchange File Format) container, so size is
metadata-driven — the file length is read directly from the RIFF header.

### 1. Pre-Validation (12 bytes)

Before allocating an output stream, `pre_validate` reads the first 12 bytes
and rejects the candidate when:

- `RIFF` magic does not match
- The form type at offset +8 is not `AVI `
- `chunk_size + 8` exceeds the remaining evidence length
- `chunk_size + 8` exceeds the configured `max_size`

This prevents speculative output files from being created for clearly invalid
hits.

### 2. RIFF Header Parsing (12 bytes)

```
Offset  Size  Description
0       4     "RIFF" signature
4       4     File size - 8 (little-endian u32)
8       4     "AVI " form type (note trailing space)
```

### 3. Size Calculation

```rust
let riff_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as u64;
let total_size = riff_size + 8;  // RIFF size field excludes the first 8 bytes
```

### 4. Header List Validation (next 12 bytes)

The carver reads the next sub-chunk header and requires that the first
sub-chunk inside the RIFF body is the `hdrl` LIST chunk:

```
Offset  Size  Description
12      4     "LIST"
16      4     LIST chunk size (little-endian u32)
20      4     "hdrl"
```

The `hdrl` LIST is rejected when its size is zero or larger than the enclosing
RIFF size.

### 5. Data Streaming

```rust
let target_size = total_size.min(max_size);
let remaining = target_size.saturating_sub(24); // 12 RIFF + 12 LIST/hdrl already read
stream.consume_remaining(remaining)?;
```

The data is copied byte-for-byte from the evidence into the output stream
without re-encoding.

## Validation

### RIFF Structure Validation

- **Validated**: `true` if all of the following hold:
  - `RIFF` signature matches
  - Form type equals `AVI `
  - Total size ≥ 12 bytes
  - First sub-chunk is `LIST` of type `hdrl`
  - `hdrl` LIST size is non-zero and ≤ total size

### Validation Status

- **Truncated**: `true` if:
  - `max_size` is reached before the complete file has been written, or
  - EOF is reached before the complete file has been read
- **Invalid (discarded)**: the carve is dropped without producing an output
  file when:
  - `RIFF` or `AVI ` magic mismatches
  - Total size < 12 bytes
  - First sub-chunk is not `LIST`/`hdrl`
  - `hdrl` LIST size is 0 or larger than the RIFF total size

Note: unlike WAV, the AVI carver does not currently inspect the `avih`
sub-chunk fields (frame rate, dimensions, stream count) for plausibility. The
`hdrl` LIST presence and size sanity checks are the primary structural
defences against false positives on random `RIFF` data.

## Size Constraints

- **Default `min_size`**: 4096 bytes
- **Default `max_size`**: 4 GiB (`4294967296`, the practical RIFF limit since
  the size field is a `u32`)
- Files below `min_size` are discarded after carving completes
- Files at or above `max_size` are kept and marked `truncated = true`

See [config/default.yml](../../config/default.yml) for the active values.

## Hash Computation

- **MD5**: Computed via `CarveStream` as data is read
- **SHA-256**: Computed via `CarveStream` as data is read
- Both hashes cover the complete carved file, from the `RIFF` header through
  the last byte written

## Testing

**Test file**: [tests/carver_avi.rs](../../tests/carver_avi.rs)

### Test Strategy

Golden image framework with deterministic AVI fixtures:

1. The integration test `finds_all_avi_files` runs the AVI carver across
   `tests/golden_image/golden.bin` and verifies every AVI entry from the
   manifest is recovered with the expected offset and size.
2. Unit tests in [src/carve/avi.rs](../../src/carve/avi.rs) cover:
   - Carving a hand-built minimal AVI (RIFF + `hdrl` LIST + `movi` LIST)
   - Rejecting non-AVI RIFF containers (e.g. `WAVE`)
   - Honouring `max_size` (truncated output)
   - Honouring `min_size` (carve dropped)

### Verification

- Count matches manifest expectation
- Sizes match exactly
- Carved files are marked `validated = true`
- Files written to the output directory match the source bytes

## Edge Cases

1. **OpenDML AVI 2.0 (multi-RIFF / AVIX)**: Files larger than 1 GB are
   typically split across multiple RIFF chunks (`RIFF…AVI ` followed by one
   or more `RIFF…AVIX` chunks). The carver only reads the first RIFF chunk
   and uses its embedded size. Trailing `AVIX` chunks are not concatenated;
   they may be detected separately as raw RIFF candidates but will be
   rejected because their form type is not `AVI `. The result is that
   OpenDML files are recovered up to the end of the first RIFF chunk only.
2. **Oversized / corrupt RIFF size fields**: Historically the AVI carver
   triggered I/O issues when the `chunk_size` field was much larger than
   the remaining evidence (see closed
   [#18](https://github.com/HighlandAlpha/SwiftBeaver/issues/18)).
   This is now mitigated by `pre_validate`, which rejects any hit whose
   `chunk_size + 8` exceeds either the remaining evidence length or the
   configured `max_size`.
3. **Padding bytes**: RIFF chunks are word-aligned (16-bit), so an extra
   pad byte may be appended to odd-length sub-chunks. The carver does not
   strip or interpret padding; it copies the bytes covered by the RIFF size
   field verbatim.
4. **`JUNK` and `LIST/INFO` metadata chunks**: Preserved verbatim — they sit
   inside the RIFF body and are streamed as part of the file payload.
5. **Missing `movi` chunk**: Only the leading `hdrl` LIST is structurally
   validated. A file that lacks a `movi` chunk but is otherwise well-formed
   is still carved; downstream tooling can flag it.
6. **Unaligned hits**: The scanner reports the `RIFF` offset directly; no
   re-alignment is necessary.

## Performance

- **Metadata-driven**: Total size is known after reading 12 bytes, so the
  carver never has to scan for an end marker.
- **Memory usage**: Constant — the RIFF header, the `hdrl` LIST header, and
  the streaming I/O buffer are all bounded.
- **I/O pattern**: One small header read followed by a single sequential
  copy of the body.
- **No decoding**: Audio/video payload is copied as-is; the carver does not
  parse codec data.

## Forensic Considerations

- **Container, not codec**: AVI is a wrapper. Recovered files may contain
  any combination of video/audio codecs (DivX, Xvid, MJPEG, MP3, PCM, etc.).
  Playability depends on the recipient's codec stack, not on this carver.
- **Embedded metadata**: `LIST/INFO` sub-chunks (e.g. `INAM`, `IART`,
  `ICMT`, `ISFT`) and `IDIT` (creation date) are preserved inside the
  carved file and can be inspected by downstream tools.
- **Size truthfulness**: AVI files written by streaming or crashed
  applications often have `chunk_size = 0` or `0xFFFFFFFF`. The
  `pre_validate` step rejects such candidates because the implied total
  size exceeds the remaining evidence.
- **Index data (`idx1` / `indx`)**: Present at the end of well-formed AVIs
  and useful for re-syncing playback. The carver retains them implicitly
  because they fall within the RIFF size.

## Structure Examples

### Minimal AVI

```
[RIFF Header: 12 bytes]
  "RIFF"
  Size: N - 8 bytes
  "AVI "

[hdrl LIST: variable]
  "LIST"
  Size: M
  "hdrl"
  [avih chunk: 56 bytes of main AVI header]
  [strl LIST chunks: one per stream — video / audio]

[movi LIST: variable]
  "LIST"
  Size: P
  "movi"
  [Interleaved 'NNwb' (audio) and 'NNdc' (video) chunks]

[idx1 chunk: optional]
  "idx1"
  Size: Q
  [16-byte index entries]
```

### OpenDML AVI 2.0 (multi-RIFF, only first RIFF is recovered)

```
[RIFF #0]
  "RIFF" / size / "AVI "
  hdrl LIST
  movi LIST (≤ ~1 GB)
  idx1 (optional)

[RIFF #1]                ← not concatenated by this carver
  "RIFF" / size / "AVIX"
  movi LIST

[RIFF #2]                ← not concatenated by this carver
  "RIFF" / size / "AVIX"
  movi LIST
```

## Known Limitations

1. **OpenDML / AVIX continuation chunks are not concatenated.** Only the
   first `RIFF…AVI ` block is recovered; extension RIFFs are skipped.
2. **No codec-level validation.** The carver does not verify that
   `strl`/`strh` describe a real codec or that frames are decodable.
3. **No `avih` field plausibility check.** Frame rate, width/height, and
   stream count are not range-checked (compare with WAV's `fmt ` validation).
4. **Trusts the RIFF size field.** Beyond the
   `chunk_size + 8 ≤ remaining_evidence` and `≤ max_size` checks, the
   embedded size is assumed correct.
5. **Single-RIFF assumption.** Files larger than 4 GB cannot be represented
   in a single RIFF chunk and are not supported as a contiguous unit.

## Related Carvers

- **[WAV](wav.md)**: Audio in a RIFF container — closest structural relative.
- **[WEBP](webp.md)**: Image in a RIFF container.
- **[WMV](wmv.md)**: Microsoft video container, but built on ASF rather than
  RIFF.
- **[MP4](mp4.md)** / **MOV**: Box-based multimedia containers (ISOBMFF).
- **[WEBM](webm.md)**: Matroska-based video container.
