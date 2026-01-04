# WEBP Carver

## Overview

The WEBP carver extracts WebP image files by parsing the RIFF container structure and using the embedded file size to determine the complete file extent.

## Signature Detection

**Header Pattern**: `RIFF` followed by `WEBP` at offset +8

Scanner detects:
- Bytes 0-3: `RIFF` (ASCII: 0x52 0x49 0x46 0x46)
- Bytes 8-11: `WEBP` (ASCII: 0x57 0x45 0x42 0x50)

## Carving Algorithm

WebP uses RIFF (Resource Interchange File Format) container:

### 1. RIFF Header Parsing (12 bytes)

```
Offset  Size  Description
0       4     "RIFF" signature
4       4     File size - 8 (little-endian u32)
8       4     "WEBP" form type
```

### 2. Size Calculation

```rust
let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as u64;
let total_size = size + 8;  // RIFF size field doesn't include first 8 bytes
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
  - "WEBP" form type matches
  - Size field is reasonable (>= 4)
- **Truncated**: `true` if:
  - max_size reached before complete file
  - EOF reached before complete file
- **Invalid**: Removed if:
  - "RIFF" signature mismatch
  - "WEBP" form type mismatch
  - Size field < 4 bytes

## Size Constraints

- **Default min_size**: 100 bytes
- **Default max_size**: 100 MB
- Minimum viable WebP: 30 bytes (header + VP8 bitstream header + minimal data)
- Files below min_size are discarded

## Hash Computation

- **MD5**: Computed via `CarveStream` as data is read
- **SHA-256**: Computed via `CarveStream` as data is read
- Covers complete file from RIFF header to end

## Testing

**Test file**: `tests/carver_webp.rs`

### Test Strategy

Golden image framework with various WebP types:

1. **Test images**:
   - VP8 (lossy compression)
   - VP8L (lossless compression)
   - VP8X (extended format with alpha/animation)
   - Animated WebP
   - WebP with EXIF metadata
   - WebP with XMP metadata
   - WebP with ICC color profile

2. **Verification**:
   - All WebPs found at expected offsets
   - Sizes match manifest (total_size = RIFF size + 8)
   - All marked as validated
   - Files open in browsers/viewers

## Edge Cases Handled

1. **Animated WebP**: Contains multiple VP8/VP8L frames (ANMF chunks)
2. **Extended format**: VP8X chunk enables alpha channel and animation
3. **Metadata chunks**: EXIF, XMP, ICCP chunks preserved
4. **Size field edge case**: Handles size=0 (though invalid per spec)
5. **Chunk alignment**: RIFF chunks are word-aligned (2-byte boundary)

## Performance Characteristics

- **Metadata-driven**: Size known from header (very efficient)
- **Memory usage**: Constant (reads header, streams rest)
- **I/O pattern**: Small header read + sequential stream
- **No decoding**: Image data copied as-is (not decompressed)

## Forensic Considerations

- **Modern format**: Increasingly common on web and mobile devices
- **Metadata preservation**: EXIF data may contain GPS, timestamps, device info
- **Lossless mode**: VP8L provides lossless compression (no quality loss)
- **Animation support**: Can contain multiple frames (like GIF)

## WebP Structure Example

### Simple lossy WebP (VP8)
```
[RIFF Header: 12 bytes]
  "RIFF"
  Size: 12340 bytes
  "WEBP"

[VP8 Chunk: 12344 bytes total]
  "VP8 " (note space)
  Size: 12332
  [VP8 bitstream data]
```

### Extended WebP with alpha (VP8X)
```
[RIFF Header: 12 bytes]
  "RIFF"
  Size: 45678
  "WEBP"

[VP8X Chunk]
  "VP8X"
  Size: 10
  Flags: 0x10 (alpha channel)
  Canvas width: 1920
  Canvas height: 1080

[ALPH Chunk]
  "ALPH"
  Size: 5432
  [Alpha channel data]

[VP8 Chunk]
  "VP8 "
  Size: 40000
  [Color data]
```

### Animated WebP
```
[RIFF Header]
  "RIFF"
  Size: 123456
  "WEBP"

[VP8X Chunk]
  Flags: 0x02 (animation)

[ANIM Chunk]
  Background color: 0xFFFFFFFF
  Loop count: 0 (infinite)

[ANMF Chunk - Frame 0]
  Frame duration: 100ms
  [VP8/VP8L data for frame 0]

[ANMF Chunk - Frame 1]
  Frame duration: 100ms
  [VP8/VP8L data for frame 1]
...
```

## Compression Formats

### VP8 (Lossy)
- Based on H.264 intra-frame coding
- Similar quality to JPEG at 25-35% smaller file size
- Lossy compression (some detail lost)

### VP8L (Lossless)
- Specialized for photographic lossless compression
- Typically 25-35% smaller than PNG
- Preserves exact pixel values

### VP8X (Extended)
- Enables additional features:
  - Alpha channel
  - Animation
  - EXIF metadata
  - XMP metadata
  - ICC color profiles

## Known Limitations

1. **No chunk validation**: Doesn't verify chunk CRCs or structure
2. **No bitstream parsing**: VP8/VP8L data not validated
3. **Assumes contiguous**: Doesn't handle malformed chunk layout
4. **Size field trusted**: Relies on embedded size (could be incorrect)

## Related Carvers

- **PNG**: Alternative lossless format
- **JPEG**: Alternative lossy format
- **GIF**: Alternative format for animation
- **WAV/AVI**: Also use RIFF container format
