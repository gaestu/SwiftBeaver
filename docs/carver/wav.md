# WAV Carver

## Overview

The WAV carver extracts Waveform Audio File Format (WAV) files by parsing the RIFF container structure and using the embedded file size.

## Signature Detection

**Header Pattern**: `RIFF` followed by `WAVE` at offset +8

Scanner detects:
- Bytes 0-3: `RIFF` (ASCII: 0x52 0x49 0x46 0x46)
- Bytes 8-11: `WAVE` (ASCII: 0x57 0x41 0x56 0x45)

## Carving Algorithm

WAV uses RIFF (Resource Interchange File Format) container:

### 1. RIFF Header Parsing (12 bytes)

```
Offset  Size  Description
0       4     "RIFF" signature
4       4     File size - 8 (little-endian u32)
8       4     "WAVE" form type
```

### 2. Size Calculation

```rust
let riff_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as u64;
let total_size = riff_size + 8;  // RIFF size field doesn't include first 8 bytes
```

### 3. Data Streaming

```rust
let target_size = total_size.min(max_size);
let remaining = target_size - 12;  // Already read 12-byte header
stream.read_exact(remaining as usize)?;
```

## Validation

- **Validated**: `true` if:
  - "RIFF" signature matches
  - "WAVE" form type matches
  - Size field is reasonable (>= 4)
- **Truncated**: `true` if:
  - max_size reached before complete file
  - EOF reached before complete file
- **Invalid**: Removed if:
  - "RIFF" signature mismatch
  - "WAVE" form type mismatch
  - Size field < 4 bytes

## Size Constraints

- **Default min_size**: 44 bytes (minimal WAV with tiny PCM data)
- **Default max_size**: 2 GB
- Minimum viable WAV: 44 bytes (RIFF header + fmt chunk + data chunk header)
- Files below min_size are discarded

## Hash Computation

- **MD5**: Computed via `CarveStream` as data is read
- **SHA-256**: Computed via `CarveStream` as data is read
- Covers complete file from RIFF header to end

## Testing

**Test file**: `tests/carver_wav.rs`

### Test Strategy

Golden image framework with various WAV types:

1. **Test audio files**:
   - PCM uncompressed (most common)
   - 8-bit mono
   - 16-bit stereo
   - 24-bit audio
   - 32-bit float
   - Various sample rates (8kHz, 44.1kHz, 48kHz, 96kHz, 192kHz)
   - Compressed (ADPCM, μ-law, A-law)

2. **Verification**:
   - All WAVs found at expected offsets
   - Sizes match manifest
   - All marked as validated
   - Files playable in media players

## Edge Cases Handled

1. **Large files**: >4GB WAV files use extended format (RF64)
2. **Metadata chunks**: INFO, BEXT, cart chunks preserved
3. **Broadcast extensions**: BWF (Broadcast Wave Format) metadata retained
4. **Padding chunks**: JUNK and PAD chunks preserved
5. **Multiple data chunks**: Some WAVs have multiple 'data' chunks

## Performance Characteristics

- **Metadata-driven**: Size known from header (very efficient)
- **Memory usage**: Constant (reads header, streams rest)
- **I/O pattern**: Small header read + sequential stream
- **No decoding**: Audio data copied as-is (not decoded)

## Forensic Considerations

- **Uncompressed common**: Most WAVs use PCM (lossless, uncompressed)
- **Metadata rich**: BWF format includes creation time, originator, description
- **Forensic audio**: Commonly used for audio recordings (interviews, surveillance)
- **Editing artifacts**: Multiple data chunks may indicate editing

## WAV Structure Example

### Simple PCM WAV
```
[RIFF Header: 12 bytes]
  "RIFF"
  Size: 1000036 bytes
  "WAVE"

[fmt Chunk: 24 bytes]
  "fmt "
  Chunk size: 16
  Audio format: 1 (PCM)
  Channels: 2 (stereo)
  Sample rate: 44100 Hz
  Byte rate: 176400 bytes/sec
  Block align: 4 bytes
  Bits per sample: 16

[data Chunk: 1000008 bytes]
  "data"
  Chunk size: 1000000
  [1,000,000 bytes of PCM audio data]
```

### WAV with Metadata
```
[RIFF Header]
  "RIFF"
  Size: 1234567
  "WAVE"

[fmt Chunk]
  ... (audio format info)

[fact Chunk]
  "fact"
  Chunk size: 4
  Sample frames: 250000

[LIST-INFO Chunk]
  "LIST"
  Chunk size: 234
  List type: "INFO"
  [INAM: "Recording Title"]
  [IART: "Artist Name"]
  [ICMT: "Comments"]

[bext Chunk - Broadcast Wave Extension]
  "bext"
  Chunk size: 602
  Description: "Interview with suspect"
  Originator: "Recorder Model XYZ"
  Originator reference: "CASE-2024-001"
  Origination date: "2024-01-15"
  Origination time: "14:30:00"
  Time reference: 0
  Version: 1
  UMID: [64 bytes]
  Loudness value: -23 LUFS
  Loudness range: 10 LU
  Coding history: "PCM,F=48000,W=24,M=stereo"

[data Chunk]
  "data"
  Chunk size: 1200000
  [Audio samples]
```

## Audio Formats

### PCM (Audio Format 1)
- Uncompressed linear pulse code modulation
- Most common WAV format
- Bit depths: 8, 16, 24, 32 bits
- Can be integer or floating-point

### Compressed Formats
- **ADPCM (Format 2)**: Adaptive differential PCM
- **μ-law (Format 7)**: Telephone quality (8kHz, 8-bit)
- **A-law (Format 6)**: European telephone standard
- **MP3 (Format 85)**: MP3-compressed WAV (rare)

## Common Sample Rates

- **8 kHz**: Telephone quality
- **11.025 kHz**: Low quality
- **22.05 kHz**: FM radio quality
- **44.1 kHz**: CD quality (standard)
- **48 kHz**: Professional audio/video
- **96 kHz**: High-resolution audio
- **192 kHz**: Ultra high-resolution

## Known Limitations

1. **RF64 not fully supported**: Files >4GB require extended format
2. **No audio validation**: Doesn't verify PCM data is valid
3. **Assumes contiguous**: Doesn't handle fragmented chunks
4. **Size field trusted**: Relies on embedded size metadata

## Related Carvers

- **MP3**: Compressed audio format
- **AVI**: Video format (also RIFF-based)
- **WEBP**: Image format (also RIFF-based)
- **OGG**: Alternative audio format
