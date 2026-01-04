# TIFF Carver

## Overview

The TIFF (Tagged Image File Format) carver extracts TIFF images by parsing the IFD (Image File Directory) structure, following strip/tile offsets, and calculating the maximum data extent referenced by the file.

## Signature Detection

**Header Patterns**:
- Little-endian: `II 2A 00` (0x49 0x49 0x2A 0x00)
- Big-endian: `MM 00 2A` (0x4D 0x4D 0x00 0x2A)

The first two bytes determine byte order for the entire file.

## Carving Algorithm

The TIFF carver uses IFD traversal to find all referenced data:

### 1. Header Parsing (8 bytes)

```
Offset  Size  Description
0       2     Byte order ("II" = little, "MM" = big)
2       2     Magic number (42)
4       4     First IFD offset
```

### 2. IFD Traversal

TIFF files contain one or more IFDs (Image File Directories):

```rust
let mut ifd_queue = VecDeque::new();
ifd_queue.push_back(first_ifd_offset);

while let Some(ifd_offset) = ifd_queue.pop_front() {
    // Read entry count (2 bytes)
    let entry_count = read_u16(ctx, offset, endian)?;
    
    // Read each IFD entry (12 bytes each)
    for i in 0..entry_count {
        let entry = read_ifd_entry(ctx, offset + 2 + i*12, endian)?;
        
        // Track maximum offset referenced
        match entry.tag {
            TAG_STRIP_OFFSETS => track_offsets(entry),
            TAG_TILE_OFFSETS => track_offsets(entry),
            TAG_SUB_IFD => queue_sub_ifd(entry),
            TAG_EXIF_IFD => queue_exif_ifd(entry),
            // ... handle other special tags
        }
    }
    
    // Read next IFD offset (4 bytes after entries)
    let next_ifd = read_u32(ctx, offset + 2 + entry_count*12, endian)?;
    if next_ifd > 0 {
        ifd_queue.push_back(next_ifd);
    }
}
```

### 3. IFD Entry Structure (12 bytes)

```
Offset  Size  Description
0       2     Tag ID (e.g., 273 = StripOffsets)
2       2     Field type (1=BYTE, 3=SHORT, 4=LONG, etc.)
4       4     Value count
8       4     Value offset (or value itself if ≤4 bytes)
```

### 4. Critical Tags for Carving

- **TAG_STRIP_OFFSETS (273)**: Offsets to image data strips
- **TAG_STRIP_BYTE_COUNTS (279)**: Byte counts for each strip
- **TAG_TILE_OFFSETS (324)**: Offsets to image tiles
- **TAG_TILE_BYTE_COUNTS (325)**: Byte counts for each tile
- **TAG_SUB_IFD (330)**: Pointer to sub-IFD (thumbnails, etc.)
- **TAG_EXIF_IFD (34665)**: Pointer to EXIF metadata IFD
- **TAG_GPS_IFD (34853)**: Pointer to GPS metadata IFD

### 5. Maximum Extent Calculation

```rust
let mut max_offset = 0u64;

// Track all strip/tile data
for (offset, size) in strips {
    let end = offset + size;
    if end > max_offset {
        max_offset = end;
    }
}

// Track all IFD structures
for ifd_offset in ifds {
    let ifd_end = ifd_offset + 2 + entry_count*12 + 4;
    if ifd_end > max_offset {
        max_offset = ifd_end;
    }
}

let total_size = max_offset - hit.global_offset;
```

## Validation

- **Validated**: `true` if:
  - Byte order marker valid ("II" or "MM")
  - Magic number == 42
  - At least one IFD parsed successfully
- **Truncated**: `true` if:
  - max_size reached before complete file
  - EOF reached before complete file
- **Invalid**: Removed if:
  - Byte order invalid
  - Magic number != 42
  - First IFD offset invalid
  - Entry count > 4096 (safety limit)

## Size Constraints

- **Default min_size**: 100 bytes
- **Default max_size**: 100 MB
- Minimum viable TIFF: ~200 bytes (header + minimal IFD + tiny image)
- Files below min_size are discarded

## Hash Computation

- **MD5**: Computed via `write_range` during copy
- **SHA-256**: Computed via `write_range` during copy
- Covers complete file from header to maximum extent

## Testing

**Test file**: `tests/carver_tiff.rs`

### Test Strategy

Golden image framework with various TIFF types:

1. **Test images**:
   - Uncompressed (no compression)
   - LZW compressed
   - JPEG compressed (TIFF/EP)
   - Tiled images
   - Striped images
   - Multi-page TIFF
   - Little-endian and big-endian
   - With EXIF metadata
   - With GPS metadata
   - Grayscale, RGB, CMYK

2. **Verification**:
   - All TIFFs found at expected offsets
   - Sizes match manifest
   - All marked as validated
   - Files open in image viewers
   - Multi-page TIFFs have all pages

## Edge Cases Handled

1. **Multi-page TIFFs**: Follows linked list of IFDs
2. **Tiled vs. stripped**: Handles both organization methods
3. **Sub-IFDs**: Recursively parses thumbnails and reduced-resolution images
4. **EXIF/GPS IFDs**: Follows pointers to metadata IFDs
5. **Large offset arrays**: Handles thousands of strips/tiles
6. **BigTIFF**: Could be extended (currently handles standard TIFF only)
7. **Sparse files**: Correctly handles non-contiguous strip offsets

## Performance Characteristics

- **IFD traversal**: Multiple small reads (IFD headers and entries)
- **Memory usage**: Moderate (tracks all offsets and IFD queue)
- **I/O pattern**: Random reads for IFDs + sequential copy
- **Complexity**: More complex than header-based carvers

## Forensic Considerations

- **Metadata-rich**: EXIF tags contain camera settings, timestamps, GPS coordinates
- **Compression**: LZW and JPEG compression preserve original image quality
- **Multi-page**: Can contain multiple images (useful for faxes, scans)
- **Private tags**: Proprietary tags may contain vendor-specific data

## TIFF Structure Example

```
[TIFF Header: 8 bytes]
  Byte order: "II" (little-endian)
  Magic: 42
  First IFD offset: 8

[IFD 0: Primary Image]
  Entry count: 12
  Tag 256 (ImageWidth): 1920
  Tag 257 (ImageLength): 1080
  Tag 258 (BitsPerSample): [8, 8, 8]
  Tag 259 (Compression): 1 (no compression)
  Tag 262 (PhotometricInterpretation): 2 (RGB)
  Tag 273 (StripOffsets): [1024, 102424, 203824, ...]
  Tag 277 (SamplesPerPixel): 3
  Tag 278 (RowsPerStrip): 100
  Tag 279 (StripByteCounts): [101400, 101400, 101400, ...]
  Tag 282 (XResolution): 72/1
  Tag 283 (YResolution): 72/1
  Tag 296 (ResolutionUnit): 2 (inches)
  Next IFD offset: 2048 (page 2)

[Strip Data]
  Offset 1024: [pixel data for rows 0-99]
  Offset 102424: [pixel data for rows 100-199]
  ...

[IFD 1: Page 2]
  ...
  Next IFD offset: 0 (end of list)
```

## Known Limitations

1. **BigTIFF not supported**: 64-bit offsets and sizes not handled
2. **No compression validation**: Doesn't decompress to verify data integrity
3. **Private tags ignored**: Vendor-specific tags not parsed
4. **Assumes contiguous**: Doesn't handle external references (TIFF external files)

## Related Carvers

- **JPEG**: Often embedded in TIFF via JPEG compression
- **PNG**: Alternative image format
- **BMP**: Simpler bitmap format
- **WEBP**: Modern alternative with better compression
