# WMV / ASF Carver

## Overview

The WMV carver extracts files based on Microsoft's Advanced Systems Format
(ASF) container. ASF is the underlying container for `.wmv` (Windows Media
Video), `.wma` (Windows Media Audio), and `.asf` streams. The same carver
handles all three because they share an identical top-level structure: a
fixed `ASF_Header_Object` followed by a sequence of typed objects, with the
overall byte length advertised in the embedded `File_Properties_Object`.

The implementation lives in [src/carve/wmv.rs](../../src/carve/wmv.rs).

## Signature Detection

**Header Pattern** (16-byte ASF_Header_Object GUID, little-endian on the
wire):

```
30 26 B2 75 8E 66 CF 11 A6 D9 00 AA 00 62 CE 6C
```

This GUID identifies the start of every ASF file. The configured pattern in
[config/default.yml](../../config/default.yml) is:

```yaml
- id: "wmv"
  extensions: ["wmv", "wma", "asf"]
  header_patterns:
    - id: "wmv_asf"
      hex: "3026B2758E66CF11A6D900AA0062CE6C"
  footer_patterns: []
  max_size: 10737418240   # 10 GiB
  min_size: 64
  validator: "wmv"
```

Pre-validation re-reads the 16 bytes at the hit offset and rejects the
candidate if the GUID does not match exactly, eliminating false positives
from partial pattern hits before any object parsing occurs.

## Carving Algorithm

ASF is object-based, not marker-based. Every object begins with a 16-byte
GUID followed by a 64-bit little-endian size that includes the header.

1. **Read the ASF header object** (30 bytes minimum):
   - bytes 0..16: ASF_Header_Object GUID
   - bytes 16..24: `header_size` (u64 LE) — total bytes covered by the
     header object, including all nested objects
   - bytes 24..28: `object_count` (u32 LE) — number of nested top-level
     objects inside the header
   - bytes 28..30: reserved, fixed at `0x01 0x02`. Mismatch is treated as a
     false positive and rejected.
2. **Sanity-check `header_size`**: must be `>= 30`, must fit within the
   evidence remaining after the hit, and must respect the configured
   `max_size`.
3. **Sanity-check `object_count`**: must be `>= 1` and `<= 4096` to bound
   the parser against malicious or random data.
4. **Iterate nested objects** within the header until either the
   `File_Properties_Object` is found or the header end is reached:
   - Read each 24-byte object header `(GUID, size)`.
   - Reject if `size < 24` or if the object would extend past the header
     end, or if the offset arithmetic would overflow.
5. **Extract `file_size`** when the `File_Properties_Object` GUID is hit:

   ```
   A1 DC AB 8C 47 A9 CF 11 8E E4 00 C0 0C 20 53 65
   ```

   The `file_size` field is a u64 LE located at offset `+40` from the
   object start. If the field is missing or the object is shorter than 104
   bytes (the spec-required minimum), the candidate is rejected.
6. **Compute the carved extent**:
   - If `file_size` is found, the carve runs from `hit_offset` to
     `hit_offset + file_size`.
   - If `file_size` is `0` (broadcast / live-stream payloads) the
     reported `file_size` would equal zero, which fails the
     `size < header_size` guard and the carver falls back to writing only
     the header. To preserve as much data as possible without a known
     size, broadcast-style files are currently truncated at the
     `header_size` boundary.
   - The end is clamped to the configured `max_size` if exceeded.
7. **Stream the bytes** through `write_range`, computing hashes on the way,
   and discard if the resulting size is below `min_size`.

### State Diagram

```
START
  ↓
[Read ASF_Header_Object GUID + size]
  ↓
[Validate reserved 0x01 0x02 + header_size + object_count]
  ↓
[Iterate nested objects up to header_size or object_count]
  ├─ File_Properties_Object found? → read file_size (u64 LE @ +40) → break
  ├─ Truncated / oversized object?  → REJECT (false positive)
  └─ End of header reached?         → use header_size only
  ↓
[Clamp by max_size]
  ↓
[write_range → hash → emit CarvedFile]
```

## Validation

- **Validated**: `true` if the full extent (`file_size`) was written
  without hitting EOF.
- **Truncated**: `true` if EOF was reached before the declared end.
- **Rejected** during pre-validation or `process_hit`:
  - GUID mismatch
  - reserved bytes ≠ `0x01 0x02`
  - implausible `header_size` (`> remaining` or `> max_size`)
  - implausible `object_count` (`0` or `> 4096`)
  - any nested object that overflows or underflows the header bounds
  - `file_size < header_size` or `file_size > remaining`

## Size Constraints

- **Default min_size**: 64 bytes
- **Default max_size**: 10 GiB (10,737,418,240 bytes)
- A `max_size` of `0` disables the upper bound (treated as "unlimited" by
  the carver helpers).
- Files smaller than `min_size` after writing are discarded.

## Hash Computation

- **MD5** and **SHA-256** are computed incrementally by the shared
  `create_hashers` / `finalize_hashers` helpers as the carved bytes are
  streamed to disk.
- Hashes cover only the carved data — never any pre-hit padding or
  post-extent bytes.
- Either or both hashes can be disabled by `HashConfig`.

## Testing

**Inline unit tests**: [src/carve/wmv.rs](../../src/carve/wmv.rs) (`tests`
module) covers:

- `carves_minimal_wmv`: synthesizes a minimal valid ASF stream consisting
  of an `ASF_Header_Object` plus a single `File_Properties_Object`,
  carves it end-to-end, and asserts the carved size matches the input.
- `rejects_unreasonable_header_size`: feeds a header with
  `header_size = u64::MAX` and asserts the candidate is rejected
  (overflow / range guard).

**Golden image coverage**: the golden image at
[tests/golden_image/manifest.json](../../tests/golden_image/manifest.json)
includes `video/wmv/file_example_WMV_640_1_6MB.wmv` (1,604,429 bytes,
SHA-256 `79f392cdb87d0ba22807717c3613456862e264f086c7d0f50b59c7cc4e83c7fc`)
and is exercised by the golden-image integration test.

## Edge Cases

- **DRM-protected streams** (`Content_Encryption_Object`,
  `Extended_Content_Encryption_Object`, `Advanced_Content_Encryption_Object`):
  the carver does not attempt decryption. The encrypted bytes are still
  written verbatim, preserving the original cipher-text for downstream
  analysis.
- **Broadcast / live-capture files** with `file_size = 0`: the spec allows
  this for indefinite streams. SwiftBeaver currently rejects a zero
  `file_size` (it fails the `size < header_size` guard) and falls back to
  writing the header object only. These captures are flagged as
  `truncated`. Forensic recovery of the data payload past the header for
  such files is a known limitation (see below).
- **Object-count cap**: the inner header parser walks at most 4,096
  top-level objects to bound CPU use against fuzzed/malicious GUIDs.
- **Overflow safety**: every offset addition is performed with
  `checked_add`; any overflow rejects the hit.
- **Truncated nested object**: any object whose size would extend past
  `header_end` rejects the hit instead of being silently clamped.

## Performance

- **I/O pattern**: one bounded read of the 30-byte ASF header, one bounded
  read per nested object header (24 bytes) until the
  `File_Properties_Object` is located, then a single sequential
  `write_range` over the carved extent.
- **Memory**: constant — only the ~30-byte header buffer plus per-object
  24-byte buffers are kept on the stack/heap; the body is streamed.
- **`is_fast`**: returns `true`. The carver does not perform decompression
  or full structural validation, so it is eligible for the fast scan
  scheduling tier.

## Forensic Considerations

- **Read-only evidence**: source data is opened via `EvidenceSource` and
  never mutated.
- **Provenance**: every emitted `CarvedFile` carries:
  - `run_id`
  - `file_type` (`"wmv"`)
  - `pattern_id` (`"wmv_asf"`)
  - `global_start`, `global_end`, `size`
  - `md5`, `sha256` (when enabled)
  - `validated`, `truncated`
- **Deterministic output**: identical evidence + identical config produce
  byte-identical carved files, identical hashes, and identical metadata
  rows.
- **Encrypted payloads**: ciphertext is preserved as-is; SwiftBeaver does
  not record or imply that decryption was performed.

## Structure Examples

Minimal valid ASF layout produced by the unit-test fixture:

```
+----------------------------------------------------+ offset 0
| ASF_Header_Object GUID (16 bytes)                  |
| 30 26 B2 75 8E 66 CF 11 A6 D9 00 AA 00 62 CE 6C    |
+----------------------------------------------------+
| header_size (u64 LE) = 134                         |
+----------------------------------------------------+
| object_count (u32 LE) = 1                          |
+----------------------------------------------------+
| reserved = 0x01 0x02                               |
+----------------------------------------------------+ offset 30
| File_Properties_Object GUID (16 bytes)             |
| A1 DC AB 8C 47 A9 CF 11 8E E4 00 C0 0C 20 53 65    |
+----------------------------------------------------+
| object_size (u64 LE) = 104                         |
+----------------------------------------------------+
| file_id (16 bytes)                                 |
+----------------------------------------------------+ offset +40 of object
| file_size (u64 LE) = total ASF length              |
+----------------------------------------------------+
| ... remaining File_Properties fields (56 bytes) ...|
+----------------------------------------------------+ offset 134 = EOF
```

Real WMV/WMA/ASF files extend the header with additional objects (Stream
Properties, Header Extension, Codec List, Content Description, ...) and
follow the header with one or more `Data_Object` payloads, all of which
are covered by the `file_size` reported in `File_Properties_Object`.

## Known Limitations

- **Live / broadcast captures** (`file_size == 0`) cannot be carved past
  the ASF header. Recovery of the body for such captures is not currently
  implemented.
- **No structural validation of `Data_Object` payload**: the carver trusts
  the `file_size` reported in `File_Properties_Object`. Tampered files
  that report a smaller size than their actual payload will lose trailing
  bytes; files that report a larger size are caught by the `> remaining`
  guard.
- **No decryption** of DRM-protected streams.
- **No deep object validation** beyond the `ASF_Header_Object` and
  `File_Properties_Object`. Other top-level objects are skipped by their
  declared size.
- **Wide GUID collisions**: the 16-byte GUID is a strong signature, but
  random data may still match. The reserved-byte check (`0x01 0x02`) and
  the bounded `object_count`/`header_size` checks reduce false positives
  to a negligible level in practice.

## Related Carvers

- **AVI** — RIFF-based legacy Microsoft container (`RIFF` / `AVI ` magic).
  Different structure but a peer multimedia carver. Documentation pending.
- **[WEBM](webm.md)** — Matroska-based container. Object/element-driven
  parsing similar in spirit to ASF.
- **[MP4](mp4.md)** — ISOBMFF box-based container. Also size-prefixed
  object iteration to find the carved extent.
