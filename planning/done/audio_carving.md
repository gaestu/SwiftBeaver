# Audio File Carving (MP3, WAV, OGG)

Status: Implemented
Implemented in version: 0.2.1

## Implementation Status

- ✅ **WAV**: Implemented in `src/carve/wav.rs` with RIFF validation
- ✅ **MP3**: Implemented in `src/carve/mp3.rs` with ID3v2 and frame walking
- ⏳ **OGG**: Not yet implemented

## Problem Statement

The golden image contains audio files (MP3, WAV, OGG) that are currently not carved by SwiftBeaver. These formats are commonly encountered in forensic investigations and should be supported.

## Scope

- Add signature patterns and config entries for MP3, WAV, and OGG.
- Implement carve handlers for each format with best-effort size detection.
- Wire handlers into the carve registry.
- Add unit tests with synthetic samples.
- Update documentation.

## Non-Goals

- Deep metadata extraction (ID3 tags, Vorbis comments).
- Audio stream validation (codec-level checks).
- Fragmented file recovery.

---

## File Format Details

### MP3 (MPEG Audio Layer III)

**Signatures:**
- ID3v2 header: `49 44 33` ("ID3") - present at start of most modern MP3s
- MPEG audio frame sync: `FF FB`, `FF FA`, `FF F3`, `FF F2` (varies by MPEG version/layer)

**Size Detection Strategy:**
1. If ID3v2 present: read ID3v2 size from header (syncsafe integer at bytes 6-9), skip to audio data.
2. Walk MPEG audio frames: each frame has a header with bitrate/sample rate that determines frame length.
3. Frame length formula: `frame_size = 144 * bitrate / sample_rate + padding`
4. Continue until: invalid frame header, EOF, or ID3v1 footer (`TAG` at -128).
5. ID3v1 footer (128 bytes): `54 41 47` ("TAG") - appears at end if present.

**Complexity:** Medium - frame walking required, variable bitrate handling.

**Config Entry:**
```yaml
- id: "mp3"
  extensions: ["mp3"]
  header_patterns:
    - id: "mp3_id3v2"
      hex: "494433"          # "ID3"
    - id: "mp3_sync_fb"
      hex: "FFFB"            # MPEG1 Layer3
    - id: "mp3_sync_fa"
      hex: "FFFA"            # MPEG1 Layer3 (no CRC)
  max_size: 104857600        # 100 MiB
  min_size: 128
  validator: "mp3"
```

### WAV (RIFF WAVE)

**Signature:**
- `52 49 46 46` ("RIFF") + 4-byte little-endian size + `57 41 56 45` ("WAVE")
- Full 12-byte header: `52 49 46 46 xx xx xx xx 57 41 56 45`

**Size Detection Strategy:**
1. Read RIFF chunk size at offset 4 (little-endian u32).
2. Total file size = RIFF chunk size + 8.
3. Validate by checking "WAVE" marker at offset 8.

**Complexity:** Low - size is embedded in header.

**Config Entry:**
```yaml
- id: "wav"
  extensions: ["wav"]
  header_patterns:
    - id: "wav_riff"
      hex: "52494646"        # "RIFF" (size follows, then "WAVE")
  max_size: 1073741824       # 1 GiB
  min_size: 44               # Minimal header
  validator: "wav"
```

**Note:** RIFF signature is shared with WEBP. Handler must validate "WAVE" at offset 8 to distinguish from WEBP ("WEBP").

### OGG (Ogg Vorbis/Opus/FLAC)

**Signature:**
- `4F 67 67 53` ("OggS") - Ogg page sync pattern

**Size Detection Strategy:**
1. Ogg files are a sequence of pages, each starting with "OggS".
2. Page header structure:
   - Bytes 0-3: "OggS"
   - Byte 4: version (0)
   - Byte 5: header type flags (bit 2 = end of stream)
   - Bytes 6-13: granule position
   - Bytes 14-17: serial number
   - Bytes 18-21: page sequence number
   - Bytes 22-25: CRC
   - Byte 26: number of segments
   - Following bytes: segment table (each byte is segment length)
   - Page data follows
3. Walk pages until end-of-stream flag (header type & 0x04).
4. Alternatively: limit search to max_size.

**Complexity:** Medium - page-walking required.

**Config Entry:**
```yaml
- id: "ogg"
  extensions: ["ogg", "oga", "ogv"]
  header_patterns:
    - id: "ogg_sync"
      hex: "4F676753"        # "OggS"
  max_size: 1073741824       # 1 GiB (also used for video)
  min_size: 28               # Minimal page
  validator: "ogg"
```

---

## Implementation Plan

### Phase 1: WAV Handler (Simplest)

1. Create `src/carve/wav.rs`:
   - Validate RIFF signature and "WAVE" form type.
   - Read size from header (offset 4, LE u32).
   - Basic validation: check for "fmt " subchunk.
2. Add config entry.
3. Wire into carve registry.
4. Add unit test with synthetic WAV.

### Phase 2: MP3 Handler

1. Create `src/carve/mp3.rs`:
   - Detect ID3v2 header, parse syncsafe size to skip tag.
   - Implement MPEG frame header parser.
   - Walk frames to determine end.
   - Check for ID3v1 footer at candidate end.
2. Handle VBR/CBR differences (frame size calculation).
3. Add config entries.
4. Add unit tests.

### Phase 3: OGG Handler

1. Create `src/carve/ogg.rs`:
   - Validate "OggS" signature.
   - Parse page headers, walk pages.
   - Stop at end-of-stream flag or invalid page.
2. Add config entry.
3. Add unit tests.

---

## Expected Tests

- `wav_basic`: Carve minimal WAV, verify size matches header.
- `mp3_id3v2`: Carve MP3 with ID3v2 tag, verify tag is included.
- `mp3_no_id3`: Carve MP3 starting with sync word.
- `ogg_basic`: Carve OGG file, verify end-of-stream detection.

---

## Impact on Docs/README

- Update file type lists in README.md.
- Add entries to docs/config.md for new validators.
- Document any special considerations (e.g., RIFF/WEBP disambiguation).

---

## Open Questions

1. Should we extract ID3 metadata as a separate artefact?
   - Recommendation: No, keep carving simple. Metadata extraction is out of scope.

2. MP3 frame sync false positives:
   - `FF FB` and similar patterns can appear in random data.
   - Mitigation: Require valid frame header fields (bitrate index != 15, sample rate != 3).

3. OGG video vs audio:
   - Same container format, different codecs.
   - Recommendation: Carve as "ogg", classify by first stream header (Vorbis vs Theora vs Opus).
