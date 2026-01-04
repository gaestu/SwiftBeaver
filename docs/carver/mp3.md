# MP3 Carver

## Overview

The MP3 carver extracts MPEG Audio Layer III files by detecting ID3 tags and/or MPEG frame sync words, then parsing frames sequentially until end of valid audio data.

## Signature Detection

**ID3v2 Pattern**: `ID3` (ASCII bytes: 0x49 0x44 0x33)
- Preferred starting point when present
- Contains tag size information for accurate positioning

**MPEG Frame Sync Patterns**:
- `0xFF 0xFB`: MPEG1 Layer III, no CRC
- `0xFF 0xFA`: MPEG1 Layer III, with CRC
- `0xFF 0xF3`: MPEG2/2.5 Layer III, no CRC
- `0xFF 0xF2`: MPEG2/2.5 Layer III, with CRC

## Carving Algorithm

The MP3 carver uses frame-by-frame validation:

### 1. ID3v2 Tag Handling (if present)

```
Offset  Size  Description
0       3     "ID3" signature
3       1     Version major
4       1     Version minor
5       1     Flags
6       4     Tag size (syncsafe integer)
```

Syncsafe integer encoding (28 bits):
```rust
let size = ((byte[0] & 0x7F) << 21) |
           ((byte[1] & 0x7F) << 14) |
           ((byte[2] & 0x7F) << 7) |
           (byte[3] & 0x7F);
let total_tag_size = 10 + size;  // 10-byte header + tag data
```

### 2. MPEG Frame Parsing

Each frame has a 4-byte header followed by audio data:

```
Bits 0-10: Frame sync (all 1s = 0x7FF)
Bits 11-12: MPEG version (00=2.5, 10=2, 11=1)
Bits 13-14: Layer (01=III, 10=II, 11=I)
Bit 15: Protection (0=CRC, 1=no CRC)
Bits 16-19: Bitrate index
Bits 20-21: Sample rate index
Bit 22: Padding
Bit 23: Private bit
Bits 24-25: Channel mode
Bits 26-27: Mode extension
Bit 28: Copyright
Bit 29: Original
Bits 30-31: Emphasis
```

### 3. Frame Size Calculation

```rust
// MPEG1 Layer III
let frame_size = (144 * bitrate * 1000) / sample_rate + padding;

// MPEG2/2.5 Layer III
let frame_size = (72 * bitrate * 1000) / sample_rate + padding;
```

### 4. Frame Validation

For sync-word detection (no ID3), requires minimum 3 consecutive valid frames:

```rust
let mut valid_frames = 0;
loop {
    let header = stream.read_exact(4)?;
    if !is_valid_mpeg_header(&header) {
        break;
    }
    valid_frames += 1;
    
    let frame_size = calculate_frame_size(&header)?;
    stream.read_exact(frame_size - 4)?;  // Read frame data
}

if valid_frames < MIN_FRAMES_FOR_SYNC_VALIDATION {
    return Ok(None);  // Too few frames, likely false positive
}
```

## Validation

- **Validated**: `true` if:
  - ID3 tag found and parsed, OR
  - At least 3 consecutive valid MPEG frames found
- **Truncated**: `true` if:
  - max_size reached during frame parsing
  - EOF reached mid-frame
- **Invalid**: Removed if:
  - Less than 3 valid frames (sync-word detection)
  - Frame header validation fails

## Size Constraints

- **Default min_size**: 128 bytes
- **Default max_size**: 50 MB
- Minimum viable MP3: ~200 bytes (ID3 tag + few frames)
- Files below min_size are discarded

## Hash Computation

- **MD5**: Computed via `CarveStream` as frames are read
- **SHA-256**: Computed via `CarveStream` as frames are read
- Covers complete file from ID3 tag (or first frame) to end

## Testing

**Test file**: `tests/carver_mp3.rs`

### Test Strategy

Golden image framework with various MP3 types:

1. **Test audio files**:
   - With ID3v2 tags
   - Without ID3 tags (raw frames)
   - With ID3v1 tags (at end)
   - Constant bitrate (CBR)
   - Variable bitrate (VBR)
   - MPEG1 Layer III
   - MPEG2 Layer III
   - Various bitrates (128, 192, 256, 320 kbps)
   - Various sample rates (44.1kHz, 48kHz)

2. **Verification**:
   - All MP3s found at expected offsets
   - Sizes match manifest
   - Valid frame count correct
   - Files playable in media players

## Edge Cases Handled

1. **VBR files**: Xing/VBRI headers parsed (contain frame count)
2. **ID3v1 tags**: 128-byte tag at end of file (optional)
3. **APE tags**: AudioPhileEncoder tags (v1/v2) at end
4. **Padding frames**: Frames with padding bit set
5. **CRC protection**: Frames with 16-bit CRC after header
6. **Free bitrate**: Bitrate index 0000 (not commonly used)

## Performance Characteristics

- **Frame-by-frame**: Sequential frame parsing (moderate complexity)
- **Memory usage**: Constant (only current frame loaded)
- **I/O pattern**: Many small reads (4-byte headers + variable frame data)
- **No decoding**: Audio data not decoded (copied as-is)

## Forensic Considerations

- **ID3 metadata**: Contains artist, title, album, year, genre, embedded artwork
- **Timestamps**: ID3v2.4 includes recording time (TDRC frame)
- **Embedded images**: APIC frames can contain album art (JPEG/PNG)
- **VBR headers**: Xing header includes encoder version and quality settings

## MP3 Structure Example

### With ID3v2
```
[ID3v2 Tag: variable]
  "ID3"
  Version: 2.4
  Flags: 0x00
  Size: 2048 bytes (syncsafe)
  [TIT2 frame: "Song Title"]
  [TPE1 frame: "Artist Name"]
  [TALB frame: "Album Name"]
  [APIC frame: [JPEG image data]]

[MPEG Frames: variable]
  [Frame 1: 417 bytes]
    Sync: 0xFFFB
    MPEG1 Layer III
    Bitrate: 128 kbps
    Sample rate: 44100 Hz
    [Audio data: 413 bytes]
  
  [Frame 2: 417 bytes]
    ...
  
  [Frame N: 417 bytes]
    ...

[ID3v1 Tag: 128 bytes (optional)]
  "TAG"
  Title: [30 bytes]
  Artist: [30 bytes]
  Album: [30 bytes]
  Year: [4 bytes]
  Comment: [30 bytes]
  Genre: [1 byte]
```

## Bitrate Tables

### MPEG1 Layer III
```
Index  Bitrate (kbps)
0      free
1      32
2      40
3      48
4      56
5      64
6      80
7      96
8      112
9      128
10     160
11     192
12     224
13     256
14     320
15     reserved
```

### MPEG2/2.5 Layer III
```
Index  Bitrate (kbps)
0      free
1      8
2      16
3      24
4      32
5      40
6      48
7      56
8      64
9      80
10     96
11     112
12     128
13     144
14     160
15     reserved
```

## Known Limitations

1. **False positive risk**: Sync words (0xFFFA, 0xFFFB) can appear in random data
2. **Partial files**: Requires minimum 3 frames for validation
3. **Free bitrate**: Not fully supported (uncommon)
4. **Corrupted frames**: Stops at first invalid frame (may truncate file)

## Related Carvers

- **WAV**: Uncompressed audio format
- **OGG**: Alternative compressed format (Vorbis/Opus)
- **MP4**: Container format (can contain AAC audio)
