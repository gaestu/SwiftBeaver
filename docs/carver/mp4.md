# MP4 Carver

## Overview

The MP4 carver extracts MPEG-4 video files (and QuickTime MOV files) by parsing the hierarchical box structure defined in ISO/IEC 14496-12. It validates the file type and traverses boxes until a natural endpoint is reached.

## Signature Detection

**Header Pattern**: `ftyp` box (File Type Box)

The scanner detects:
- Offset +4 to +8: ASCII "ftyp"
- Offset +0 to +4: Box size (big-endian u32)

## Box Structure

Every MP4 atom/box follows this structure:

```
Standard box (size < 2^32):
[4 bytes: size (big-endian u32)]
[4 bytes: type (ASCII, e.g. "ftyp", "moov", "mdat")]
[size-8 bytes: data]

Extended box (size >= 2^32):
[4 bytes: size = 1]
[4 bytes: type]
[8 bytes: extended size (big-endian u64)]
[extended_size-16 bytes: data]
```

## Carving Algorithm

The MP4 carver uses box-based iteration with validation:

### 1. Box Header Reading

```rust
const BOX_HEADER_LEN: usize = 8;
const EXTENDED_HEADER_LEN: usize = 16;

let header = read_exact_at(ctx, offset, BOX_HEADER_LEN)?;
let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
let box_type = &header[4..8];

let (box_size, header_len) = if size32 == 1 {
    // Extended size
    let ext = read_exact_at(ctx, offset, EXTENDED_HEADER_LEN)?;
    let size64 = u64::from_be_bytes([ext[8], ext[9], ext[10], ext[11], 
                                      ext[12], ext[13], ext[14], ext[15]]);
    (size64, EXTENDED_HEADER_LEN as u64)
} else {
    (size32, BOX_HEADER_LEN as u64)
};
```

### 2. File Type Validation

First box must be `ftyp`:

```rust
if offset == hit.global_offset {
    if box_type != b"ftyp" {
        return Ok(None);  // Invalid MP4
    }
    
    // Read major brand
    let brand = read_exact_at(ctx, offset + header_len, 4)?;
    if brand == b"qt  " && !self.allow_quicktime {
        return Ok(None);  // QuickTime file (optional exclusion)
    }
}
```

### 3. Box Iteration

Loop through boxes tracking critical types:

```rust
loop {
    // Read box header
    // Check termination conditions:
    //   - max_size reached
    //   - EOF reached
    //   - Both ftyp and moov seen
    
    if box_type == b"ftyp" {
        seen_ftyp = true;
    }
    if box_type == b"moov" {
        seen_moov = true;
    }
    
    // If size == 0: box extends to EOF (terminate)
    // If both ftyp and moov seen: can terminate
    
    offset += box_size;
}
```

### 4. Termination Conditions

- **Natural end**: Both `ftyp` and `moov` boxes seen, EOF or next box read fails
- **Size limit**: max_size reached
- **Invalid structure**: Box size invalid or box type unexpected

## Validation

- **Validated**: `true` if:
  - First box is `ftyp`
  - `moov` box is found
  - File structure is sound
- **Truncated**: `true` if:
  - max_size reached before natural end
  - EOF reached before `moov`
- **Invalid**: Removed if:
  - First box is not `ftyp`
  - Brand is "qt  " (QuickTime) when not allowed
  - Box size < header length

## Size Constraints

- **Default min_size**: 32 bytes (minimum for ftyp box)
- **Default max_size**: 2 GB
- Files below min_size are discarded

## Hash Computation

- **MD5**: Computed incrementally during range copy
- **SHA-256**: Computed incrementally during range copy
- Covers complete file from ftyp to last box (or truncation point)

## Testing

**Test file**: `tests/carver_mp4.rs`

### Test Strategy

Comprehensive MP4 variant testing:

1. **Test videos**:
   - H.264 video codec (AVC)
   - H.265 video codec (HEVC)
   - AAC audio codec
   - MP3 audio codec
   - Various resolutions (480p, 720p, 1080p, 4K)
   - Fragmented MP4 (DASH/HLS)
   - QuickTime MOV files
   - Files with metadata (EXIF, GPS, etc.)

2. **Verification**:
   - All MP4s found at expected offsets
   - Sizes match manifest
   - All marked as validated (ftyp + moov found)
   - Files playable in media players
   - Video/audio streams intact

### Example Test

```rust
#[test]
fn test_mp4_carver() {
    let config = default_config();
    let (metadata, output_dir) = carver_for_types(&["mp4"], &config);
    verify_manifest_match(metadata, "mp4");
    
    // Verify videos are playable
    for entry in metadata {
        let video_path = output_dir.join(&entry.path);
        assert!(verify_video_playable(&video_path));
    }
}
```

## Edge Cases Handled

1. **Extended size boxes**: Correctly handles 64-bit sizes for large videos
2. **Size=0 boxes**: Terminates when box extends to EOF
3. **QuickTime compatibility**: Optionally allows or rejects MOV files
4. **Fragmented MP4**: Handles `moof` boxes (fragmented movie)
5. **Metadata boxes**: Preserves `meta`, `udta` boxes with GPS/EXIF
6. **EOF during box**: Terminates gracefully if EOF in middle of box

## Performance Characteristics

- **Box skipping**: Efficient (reads headers only, seeks over data)
- **Memory usage**: Constant (only headers loaded, ~16 bytes)
- **I/O pattern**: Many small reads (headers), large seeks (box data)
- **No decoding**: Video data not decoded (copied as-is)

## Forensic Considerations

- **Deleted frames**: Truncated videos may be missing end frames
- **Metadata preservation**: GPS coordinates, creation time, device info retained
- **Codec information**: Preserved in `stsd` box (Sample Description)
- **Encryption**: Some MP4s use DRM (not detected or removed)

## Common Box Types

### Container Boxes
- `ftyp`: File Type (brand, version)
- `moov`: Movie metadata (required)
- `mdat`: Media data (video/audio frames)
- `moof`: Movie fragment (fragmented MP4)

### Metadata Boxes
- `meta`: Metadata container
- `udta`: User data (GPS, timestamps, etc.)
- `uuid`: Extended user data

### Track Boxes
- `trak`: Track container
- `mdia`: Media information
- `minf`: Media information
- `stbl`: Sample table
- `stsd`: Sample descriptions (codec info)

## MP4 Structure Example

```
[ftyp: File Type Box]
  Brand: isom
  Compatible brands: iso2, avc1, mp41
[moov: Movie Box]
  [mvhd: Movie Header]
    Duration, timescale, creation time
  [trak: Video Track]
    [tkhd: Track Header]
    [mdia: Media]
      [mdhd: Media Header]
      [hdlr: Handler] (vide = video)
      [minf: Media Info]
        [stbl: Sample Table]
          [stsd: Sample Description] (avc1 = H.264)
          [stts: Time-to-Sample]
          [stsc: Sample-to-Chunk]
          [stsz: Sample Size]
          [stco: Chunk Offset]
  [trak: Audio Track]
    ... (similar structure)
  [udta: User Data] (optional)
    [meta: Metadata] (GPS, device info)
[mdat: Media Data]
  [Video samples]
  [Audio samples]
```

## Known Limitations

1. **No fragment reassembly**: Fragmented MP4 (DASH) carves fragments separately
2. **No codec validation**: Doesn't verify video/audio codec is valid
3. **Assumes sequential boxes**: Doesn't handle non-sequential mdat references
4. **Large file truncation**: Files >max_size are truncated (may lose end frames)

## Related Carvers

- **MOV**: QuickTime format (same structure, different brand)
- **AVI**: Alternative video format (RIFF-based)
- **WMV**: Windows Media Video (ASF-based)
- **WEBM**: Alternative format (Matroska-based)
