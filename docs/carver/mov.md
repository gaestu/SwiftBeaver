# MOV Carver

## Overview

The MOV carver extracts Apple QuickTime movie files. QuickTime is the historical
predecessor of MP4 and uses the same hierarchical atom/box structure defined in
the QuickTime File Format specification (later standardised as ISO/IEC 14496-12,
the basis of MP4 and HEIC). MOV files are distinguished from MP4 by the
`qt  ` major brand in the `ftyp` box.

The carver shares its parsing strategy with the [MP4 carver](mp4.md) but is a
separate handler so that QuickTime-specific signatures and validation can evolve
independently from MP4. Implementation: [`src/carve/mov.rs`](../../src/carve/mov.rs).

## Signature Detection

**Header Pattern**: `ftyp` box (File Type Box) with `qt  ` major brand.

The configured header patterns combine a leading box size with the `ftypqt  `
sequence so the scanner only triggers on plausible QuickTime files:

```
00 00 00 14 66 74 79 70 71 74 20 20   ftyp size = 0x14 (20)
00 00 00 18 66 74 79 70 71 74 20 20   ftyp size = 0x18 (24)
00 00 00 1C 66 74 79 70 71 74 20 20   ftyp size = 0x1C (28)
00 00 00 20 66 74 79 70 71 74 20 20   ftyp size = 0x20 (32)
```

Pre-validation re-reads the first 12 bytes and rejects the hit unless:

- Bytes 4..8 equal `ftyp`.
- Bytes 8..12 equal `qt  ` (the QuickTime major brand).

Any other brand (`isom`, `mp42`, `heic`, `mif1`, …) is left to the
corresponding carver (MP4, HEIC, …).

## Box Structure

QuickTime atoms are byte-for-byte compatible with ISOBMFF boxes:

```
Standard atom (size < 2^32):
[4 bytes: size (big-endian u32)]
[4 bytes: type (ASCII, e.g. "ftyp", "moov", "mdat")]
[size-8 bytes: payload]

Extended atom (size == 1):
[4 bytes: size = 1]
[4 bytes: type]
[8 bytes: extended size (big-endian u64)]
[extended_size-16 bytes: payload]

Size = 0:
The atom extends to end-of-file. Only honoured when both ftyp and moov have
already been observed; otherwise treated as an invalid stream.
```

## Carving Algorithm

The MOV carver iterates atoms, tracks whether `ftyp` and `moov` have been
seen, and stops when the structure terminates naturally or a guard trips.

### 1. Atom Header Reading

```rust
const BOX_HEADER_LEN: usize = 8;
const EXTENDED_HEADER_LEN: usize = 16;

let header = read_exact_at(ctx, offset, BOX_HEADER_LEN)?;
let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
let box_type = &header[4..8];

let (box_size, header_len) = if size32 == 1 {
    let ext = read_exact_at(ctx, offset, EXTENDED_HEADER_LEN)?;
    let size64 = u64::from_be_bytes([
        ext[8], ext[9], ext[10], ext[11], ext[12], ext[13], ext[14], ext[15],
    ]);
    (size64, EXTENDED_HEADER_LEN as u64)
} else {
    (size32, BOX_HEADER_LEN as u64)
};
```

### 2. File Type Validation

The first atom at the hit offset must be a `ftyp` whose major brand is
`qt  `. Anything else causes the carver to abandon the candidate:

```rust
if offset == hit.global_offset {
    if box_type != b"ftyp" {
        return Ok(None); // Not a QuickTime container
    }
    let brand = read_exact_at(ctx, offset + header_len, 4)?;
    if brand != b"qt  " {
        return Ok(None); // Not QuickTime (likely MP4/HEIC)
    }
    seen_ftyp = true;
}
```

### 3. Atom Iteration

The loop walks each atom, marking when `ftyp` and `moov` are observed, and
terminates on:

- `max_size` reached before the next atom or while consuming the current atom.
- EOF reached while reading the next header. If both `ftyp` and `moov` have
  been seen this is a clean end; otherwise the file is marked truncated.
- Invalid atom (`size32 == 0` or `box_size < header_len`) before `moov` was
  seen — the candidate is rejected. After `moov` was seen the loop stops at
  the last good offset.
- Extended size header that cannot be read.

### 4. Termination Conditions

- **Natural end**: `ftyp` and `moov` both seen and the next header read fails
  at EOF.
- **Size limit**: `max_size` reached. The output is marked `truncated = true`.
- **Invalid structure**: First atom is not `ftyp`, brand is not `qt  `, or an
  atom size is invalid before `moov` was seen.

## Validation

- **Validated** (`validated = true`): `ftyp` (with `qt  ` brand) and `moov`
  were both observed and the loop exited cleanly.
- **Truncated** (`truncated = true`): `max_size` reached, EOF hit before
  reaching the natural end, an extended size could not be read, or a zero-size
  atom was encountered before completion.
- **Invalid** (rejected, returns `None`):
  - First atom is not `ftyp`.
  - First brand is not `qt  `.
  - Atom size is `< header_len` before `moov` was seen.
  - Final size is below `min_size`.

## Size Constraints

- **Default `min_size`**: 16 bytes (matches `config/default.yml`).
- **Default `max_size`**: 10 GiB (`10737418240`).
- Files smaller than `min_size` are discarded after writing and do not appear in
  metadata.

These defaults can be tuned in [`config/default.yml`](../../config/default.yml)
under the `mov` file type.

## Hash Computation

- **MD5** and **SHA-256** are computed incrementally during the range copy.
- The hashed range covers the bytes from the hit offset through `last_good`
  (the end of the last successfully parsed atom), capped at `max_size`.
- Hash computation respects the global `HashConfig` and is skipped when the
  corresponding algorithm is disabled.

## Testing

**Test file**: [`tests/carver_mp4.rs`](../../tests/carver_mp4.rs) — MOV is
exercised together with MP4 because it shares the same handler structure and
manifest grouping.

### Test Strategy

1. The golden image manifest
   ([`tests/golden_image/manifest.json`](../../tests/golden_image/manifest.json))
   includes a real QuickTime file (`file_example_MOV_640_800kB.mov`) embedded
   into the synthetic image alongside MP4/M4A/M4V samples.
2. The carver is invoked for both `mp4` and `mov` and the resulting metadata
   is compared against the manifest:
   - Offsets match.
   - Sizes match.
   - The MOV entry is marked validated.
3. Unit tests in [`src/carve/mov.rs`](../../src/carve/mov.rs) verify a minimal
   synthetic MOV (`ftyp` with `qt  ` brand followed by a `moov` atom) is
   carved and validated.

### Example Test

```rust
#[test]
fn test_mp4_mov_carver() {
    let manifest = load_manifest();
    let expected = get_expected_files(&manifest, &["mp4", "mov", "m4a", "m4v"]);
    let result = run_carver_for_types(&["mp4", "mov"]);
    assert_manifest_match(&result, &expected);
}
```

## Edge Cases Handled

1. **Brand discrimination**: Only `qt  ` is accepted; MP4-family brands are
   left to the MP4 carver to avoid duplicate carves.
2. **Extended size atoms**: 64-bit `largesize` is parsed for atoms with
   `size == 1`, supporting `mdat` payloads larger than 4 GiB.
3. **Size-zero atoms**: An atom with `size == 0` extending to EOF is accepted
   only when `ftyp` and `moov` have been seen; otherwise the candidate is
   marked truncated.
4. **Interleaved `mdat` / `moov`**: QuickTime files often place `mdat` before
   `moov`. The loop continues past `mdat` and recognises `moov` whenever it
   appears.
5. **EOF during iteration**: A short read of the next header is treated as a
   clean end if both required atoms have been seen.
6. **`max_size` guard**: Both before reading and before consuming each atom,
   preventing runaway carves on malformed sizes.
7. **Multiple `ftyp` patterns**: Four header pattern variants cover the most
   common QuickTime `ftyp` sizes (20, 24, 28, 32 bytes).

## Performance Characteristics

- **Atom skipping**: Only 8- or 16-byte headers are read; payloads are seeked
  past during parsing and streamed during the final range copy.
- **Memory usage**: Constant — a single header buffer plus the I/O buffer
  reused from the extraction context.
- **I/O pattern**: Many small header reads followed by one sequential range
  copy when the carve is finalised.
- **No decoding**: Audio/video samples are copied verbatim; codecs are not
  inspected.

## Forensic Considerations

- **Provenance**: Every carved row carries `run_id`, `tool_version`,
  `config_hash`, `evidence_path`, `global_start`, `global_end`, and (when
  enabled) MD5/SHA-256, satisfying the project-wide invariants.
- **Metadata preservation**: `moov/udta`, `meta`, and `uuid` atoms are
  preserved as-is, retaining GPS, creation/modification timestamps, device
  model, and other QuickTime user data.
- **Codec information**: Sample descriptions in `stsd` (e.g. `avc1`, `hvc1`,
  `mp4a`) are preserved; the carver does not validate or rewrite them.
- **Truncation**: Files cut at `max_size` are still emitted with
  `truncated = true` so partial recoveries are auditable.
- **DRM / encryption**: Atoms such as `sinf` are passed through untouched;
  the carver does not detect or attempt to decrypt protected content.
- **Read-only evidence**: Source data is opened read-only via the standard
  `EvidenceSource` abstraction; no writes are made back to the image.

## Common Atom Types

### Container Atoms
- `ftyp`: File Type (brand, version) — required first atom.
- `moov`: Movie metadata — required.
- `mdat`: Media data (samples).
- `free` / `skip` / `wide`: Padding / placeholder atoms.

### Metadata Atoms
- `udta`: User data (GPS, device model, timestamps).
- `meta`: Generic metadata container.
- `uuid`: Vendor-extension data (e.g. Apple-specific).

### Track Atoms
- `trak`: Track container.
- `mdia` / `minf` / `stbl`: Media → media information → sample table.
- `stsd`: Sample descriptions (codec metadata).

## MOV Structure Example

```
[ftyp: File Type Box]
  Major brand: qt
  Minor version: 0x00000200
  Compatible brands: qt
[wide]                          (8-byte placeholder, common in Apple files)
[mdat: Media Data]              (often appears before moov in QuickTime)
  [Video samples]
  [Audio samples]
[moov: Movie Box]
  [mvhd: Movie Header]
  [trak: Video Track]
    [tkhd: Track Header]
    [mdia]
      [mdhd: Media Header]
      [hdlr: Handler] (vide)
      [minf]
        [stbl]
          [stsd] (avc1 / hvc1 / etc.)
          [stts] [stsc] [stsz] [stco]
  [trak: Audio Track]
    ... (similar structure, hdlr = soun)
  [udta: User Data]             (optional: ©day, ©too, GPS, etc.)
```

## Known Limitations

1. **Brand-restricted**: Only files with the exact `qt  ` major brand are
   carved. QuickTime-compatible files that advertise an MP4 brand fall under
   the MP4 carver.
2. **No fragment reassembly**: Fragmented streams (`moof` / `mfra`) are
   walked but not stitched together with prior fragments.
3. **No codec validation**: The carver does not verify that referenced codecs
   are well-formed.
4. **`max_size` truncation**: Files larger than the configured `max_size` are
   truncated, potentially losing trailing samples.
5. **Reference offsets are not followed**: The carver determines extent from
   atom sizes, not from chunk offset tables (`stco` / `co64`). External media
   references (`dref` pointing outside the file) are not resolved.

## Related Carvers

- **[MP4](mp4.md)**: ISOBMFF sibling that handles `isom`, `mp41`, `mp42`,
  `avc1`, `iso2`, etc.
- **[HEIC](heic.md)**: ISOBMFF still-image variant (`heic`, `mif1`, `heix`,
  `hevc`).
- **[AVI](avi.md)**: Alternative video container (RIFF-based).
- **[WMV](wmv.md)**: ASF-based Windows Media video.
- **[WEBM](webm.md)**: Matroska-based open video container.
