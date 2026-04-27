# WEBM Carver

## Overview

The WEBM carver extracts WebM and Matroska video files by parsing the EBML
(Extensible Binary Meta Language) document structure. WebM is a Matroska
profile defined by Google that targets VP8/VP9/AV1 video and Vorbis/Opus
audio.

The carver locates the EBML header, validates the `DocType` (`webm` or
`matroska`), then reads the top-level `Segment` element to determine the
file extent. When the segment size is known the carved file ends at the
segment boundary; otherwise the carver falls back to the configured
`max_size` or evidence end.

Source: [src/carve/webm.rs](../../src/carve/webm.rs)

## Signature Detection

**Header pattern**: `1A 45 DF A3` (EBML element ID)

| Offset | Bytes        | Meaning                |
|--------|--------------|------------------------|
| +0..+4 | `1A 45 DF A3`| EBML header element ID |

The scanner registers the pattern under the `webm_ebml` id (see
[config/default.yml](../../config/default.yml)).

## EBML Structure

EBML is a length-prefixed, hierarchical binary format. Every element is:

```
[VINT element ID] [VINT data size] [data ...]
```

VINTs (variable-length integers) encode their byte length in leading zero
bits of the first byte:

| First byte mask | Length (bytes) | Value bits |
|-----------------|----------------|------------|
| `1xxxxxxx`      | 1              | 7          |
| `01xxxxxx`      | 2              | 14         |
| `001xxxxx`      | 3              | 21         |
| ...             | up to 8        | up to 56   |

A size VINT whose value bits are all `1` indicates **unknown size** (the
element extends until a higher-level element ends or EOF is reached).

Top-level WebM elements relevant to carving:

- `EBML` (`0x1A45DFA3`): document header containing `DocType`
- `DocType` (`0x4282`): ASCII string, `webm` or `matroska`
- `Segment` (`0x18538067`): payload container holding all clusters

## Carving Algorithm

### 1. Pre-validation

Read 4 bytes at the hit offset and reject anything that is not exactly
`1A 45 DF A3`.

### 2. EBML Header Parse

```
read EBML element ID  (VINT, must equal 0x1A45DFA3)
read EBML header size (VINT)
read header bytes
scan child elements for DocType (0x4282)
reject unless DocType ∈ { "webm", "matroska" }
```

The header size is bounded by `MAX_EBML_ELEMENT_SIZE` (1 MiB). Anything
larger is treated as corrupt and rejected with `CarveError::Invalid`.

### 3. Segment Discovery

After the EBML header, iterate top-level elements (bounded by a 1 MiB
scan window) until a `Segment` element is found:

- If the segment size is **known**, the carved range is
  `[hit_offset .. segment_data_start + segment_size)`.
- If the segment size is **unknown** (all-ones VINT), the carved range
  extends to `max_size` or evidence EOF, whichever comes first.

If no `Segment` element is found within the scan window the carve is
rejected.

### 4. Range Write & Hashing

The selected byte range is streamed to disk via `write_range`, with MD5
and SHA-256 hashed incrementally according to the active
`HashConfig`. Carves smaller than `min_size` are discarded.

## Validation

A carved WebM is marked `validated = true` when **all** of the following
hold:

- The EBML header parsed cleanly and `DocType` matched.
- The `Segment` element advertised a known size.
- The full segment fit within `max_size` and was not EOF-truncated.

A carve is marked `truncated = true` when:

- Evidence ended before the computed end offset, or
- `max_size` clipped the carve.

## Size Constraints

Defaults from [config/default.yml](../../config/default.yml):

| Parameter | Value             |
|-----------|-------------------|
| `min_size`| 64 bytes          |
| `max_size`| 10 GiB            |

Files below `min_size` after writing are discarded.

## Hash Computation

- **MD5** and **SHA-256** are computed incrementally during the range
  write.
- Hashing is governed by `HashConfig`; either or both may be disabled.
- Hashes cover the full carved range, including any truncation tail.

## Configuration

```yaml
- id: "webm"
  extensions: ["webm", "mkv"]
  header_patterns:
    - id: "webm_ebml"
      hex: "1A45DFA3"
  footer_patterns: []
  max_size: 10737418240
  min_size: 64
  validator: "webm"
```

Both `webm` and `mkv` are accepted as output extensions because the
carver also recognises generic Matroska (`DocType = matroska`) streams.

## Testing

- Unit test: `carves_minimal_webm` in [src/carve/webm.rs](../../src/carve/webm.rs)
  builds a synthetic EBML header + zero-sized Segment and asserts the
  carver returns a validated file of the expected length.
- Golden image: `tests/golden_image/manifest.json` includes
  `media_tiny/tiny.webm` and a real-world WebM sample. The golden image
  test exercises end-to-end discovery, sizing, and hashing.

## Edge Cases

1. **Unknown segment size**: handled by falling back to `max_size`/EOF;
   resulting carve is marked `truncated` and not `validated`.
2. **Matroska (non-WebM)**: accepted because the same parser applies.
3. **Oversized EBML header** (> 1 MiB): rejected as malformed.
4. **Missing Segment within scan window**: rejected with
   `CarveError::Invalid("segment missing")`.
5. **Truncated evidence**: write stops at EOF and `truncated` flag is set.
6. **Carve below `min_size`**: pending writer is discarded, no metadata
   row emitted.

## Performance Characteristics

- **Memory usage**: bounded — at most one EBML header (≤ 1 MiB) is held
  in memory plus the I/O buffer.
- **I/O pattern**: a small number of header reads followed by a single
  sequential range copy.
- **No decoding**: VP8/VP9/AV1 streams are copied as bytes; no decoder
  is invoked.

## Forensic Considerations

- **Evidence integrity**: source is opened read-only; carved bytes are
  written only into the run output directory.
- **Container metadata**: the `Info` and `Tags` clusters inside the
  segment are preserved verbatim, including timestamps and titles.
- **Codec data**: VP8/VP9/AV1 video and Vorbis/Opus audio payloads are
  preserved without modification.
- **Encryption**: WebM Common Encryption (CENC) data is preserved but
  not decrypted.

## Known Limitations

1. **No cluster-level validation**: the carver does not parse individual
   clusters or block groups; it relies on the segment size.
2. **Single segment only**: only the first `Segment` element is followed.
   Multi-segment Matroska files are truncated after the first segment.
3. **DocType allow-list**: only `webm` and `matroska` are accepted.
   Other EBML profiles (e.g. `webmproject-mkv`) would need an allow-list
   change.
4. **Scan window**: the search for `Segment` after the EBML header is
   bounded to 1 MiB. Pathologically large `Void` or `SeekHead` padding
   beyond that could prevent discovery.

## Related Carvers

- **MP4 / MOV**: ISO Base Media File Format (box-based)
- **AVI**: RIFF-based video container
- **WMV**: ASF-based video container
- **OGG**: Page-based audio/video container
