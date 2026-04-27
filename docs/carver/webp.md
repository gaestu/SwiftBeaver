# WEBP Carver

## Overview

The WEBP carver extracts WebP image files by parsing the RIFF container structure and using the embedded file size to determine the complete file extent.

## Signature Detection

**Scanner Pattern**: `RIFF`

The raw signature scanner detects:

- Bytes 0-3: `RIFF` (ASCII: 0x52 0x49 0x46 0x46)

WebP pre-validation then requires:

- Bytes 8-11: `WEBP` (ASCII: 0x57 0x45 0x42 0x50)
- Bytes 12-15: one of `VP8 `, `VP8L`, or `VP8X`

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
if max_size > 0 && total_size > max_size {
    reject_candidate();
}

walk_and_stream_webp_chunks(total_size)?;
```

The outer RIFF size is authoritative. SwiftBeaver does not extend a WebP carve to `max_size` when the RIFF size is corrupt or implausible.
The chunk walker streams each chunk header and payload while validating RIFF chunk bounds.

### 4. Chunk Walk

Complete WebP containers are walked chunk by chunk inside the declared RIFF extent. The first chunk must be a primary image chunk:

- `VP8 `
- `VP8L`
- `VP8X`

Common subsequent chunks include:

- `VP8 `, `VP8L`, `VP8X`
- `ALPH`
- `ANIM`, `ANMF`
- `EXIF`, `XMP `, `ICCP`

Unknown non-primary chunks are preserved as long as their declared size fits within the outer RIFF container after applying RIFF word padding for odd-sized payloads.

## Validation

- **Validated**: `true` if:
  - "RIFF" signature matches
  - "WEBP" form type matches
  - The RIFF size is at least 20 bytes total and does not exceed the configured `max_size`
  - The first chunk is `VP8 `, `VP8L`, or `VP8X`
  - All walked chunks fit inside the declared RIFF container
- **Truncated**: `true` if:
  - EOF is reached before the complete declared RIFF extent
- **Invalid**: Removed if:
  - "RIFF" signature mismatch
  - "WEBP" form type mismatch
  - First chunk fourcc is not a primary WebP chunk
  - RIFF size is too small or exceeds `max_size`
  - A chunk exceeds the declared RIFF container

## Size Constraints

- **Default min_size**: 20 bytes
- **Default max_size**: 100 MB
- Minimum structurally accepted WebP: 20 bytes (RIFF header + one zero-length primary chunk header)
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
  - Deterministic malformed RIFF and chunk-layout cases are rejected or marked truncated as expected

## Edge Cases Handled

1. **Animated WebP**: Contains multiple VP8/VP8L frames (ANMF chunks)
2. **Extended format**: VP8X chunk enables alpha channel and animation
3. **Metadata chunks**: EXIF, XMP, ICCP chunks preserved
4. **Truncated evidence**: If the declared RIFF extent is larger than the available evidence, remaining bytes are carved and the metadata row is marked `validated=false`, `truncated=true`
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
  Size: 12344 bytes
  "WEBP"

[VP8 Chunk: 12340 bytes total]
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

1. **No bitstream decoding**: VP8/VP8L payload data is copied as-is and not decoded
2. **No payload semantic validation**: Chunk payload contents are not interpreted beyond RIFF chunk bounds and the required primary first chunk
3. **Contiguous carving only**: Fragmented WebP containers cannot be reconstructed
4. **RIFF extent required**: The outer RIFF size is authoritative; corrupt oversize declarations are rejected rather than scanned forward

## Related Carvers

- **PNG**: Alternative lossless format
- **JPEG**: Alternative lossy format
- **GIF**: Alternative format for animation
- **WAV/AVI**: Also use RIFF container format
