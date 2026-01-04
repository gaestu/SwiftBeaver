# BMP Carver

## Overview

The BMP (Bitmap) carver extracts Windows bitmap image files by parsing the file header, validating the DIB (Device Independent Bitmap) header structure, and using embedded file size metadata.

## Signature Detection

**Header Pattern**: `BM` (ASCII bytes: `42 4D`)

This 2-byte magic number is mandatory for all BMP files.

## Carving Algorithm

The BMP carver uses metadata-driven size calculation with extensive validation:

### 1. BMP File Header (14 bytes)

```
Offset  Size  Description
0       2     Magic number ("BM" = 0x42 0x4D)
2       4     File size (little-endian u32)
6       4     Reserved (application-specific)
10      4     Pixel data offset (little-endian u32)
```

### 2. DIB Header Validation

The carver validates the DIB header size to reduce false positives:

```rust
const VALID_DIB_SIZES: [u32; 6] = [12, 40, 52, 56, 108, 124];
// 12:  BITMAPCOREHEADER
// 40:  BITMAPINFOHEADER (most common)
// 52:  BITMAPV2INFOHEADER
// 56:  BITMAPV3INFOHEADER
// 108: BITMAPV4HEADER
// 124: BITMAPV5HEADER
```

### 3. Dimension Validation (BITMAPINFOHEADER+)

For DIB headers ≥40 bytes:

```
Offset  Size  Description
18      4     Width (little-endian i32, must be > 0)
22      4     Height (little-endian i32, can be negative)
26      2     Color planes (must be 1)
28      2     Bits per pixel (1, 4, 8, 16, 24, 32)
30      4     Compression method
34      4     Image size (can be 0 for uncompressed)
```

Validation:
- Width must be positive
- Width and height must be ≤ 32768 pixels
- Color planes must equal 1
- Bits per pixel must be valid value

### 4. Structure Validation

```rust
// pixel_offset must be >= header_size
if pixel_offset < (BMP_HEADER_LEN + dib_header_size) {
    return Ok(None);  // Invalid structure
}

// file_size must be >= pixel_offset
if file_size < pixel_offset {
    return Ok(None);
}
```

## Validation

- **Validated**: `true` if all structural checks pass
- **Truncated**: `true` if:
  - max_size reached before complete file
  - EOF reached before file_size
- **Invalid**: Removed if:
  - Magic number != "BM"
  - DIB header size not in valid list
  - Width/height out of range
  - Color planes != 1
  - Pixel offset < header size

## Size Constraints

- **Default min_size**: 200 bytes (as of v0.2.1)
- **Default max_size**: 100 MB
- Minimum viable BMP: 54 bytes (header + BITMAPINFOHEADER + minimal pixel data)
- Files below min_size are discarded

## Hash Computation

- **MD5**: Computed via `write_range` during copy
- **SHA-256**: Computed via `write_range` during copy
- Covers complete file from header to end (or truncation point)

## Testing

**Test file**: `tests/carver_bmp.rs`

### Test Strategy

Golden image framework with various BMP types:

1. **Test images**:
   - 1-bit monochrome
   - 4-bit indexed color
   - 8-bit indexed color
   - 24-bit RGB
   - 32-bit RGBA
   - Top-down DIB (negative height)
   - Compressed (RLE4, RLE8)
   - Various DIB header versions

2. **Verification**:
   - All BMPs found at expected offsets
   - Sizes match manifest
   - Dimension validation correct
   - Files open in image viewers

## Edge Cases Handled

1. **Top-down DIBs**: Negative height indicates top-down pixel order
2. **Various DIB headers**: Supports 6 different header formats
3. **Compressed BMPs**: Handles RLE4/RLE8 compression
4. **Large images**: Respects max dimension limit (32768 pixels)
5. **Zero image size**: Handles uncompressed images with image_size=0
6. **Padding bytes**: Correctly calculates row padding (aligned to 4 bytes)

## Performance Characteristics

- **Metadata-driven**: Size known from header (efficient)
- **Memory usage**: Constant (reads header only, then copies)
- **I/O pattern**: Small header read + sequential copy
- **No parsing**: Pixel data copied as-is (not decoded)

## Forensic Considerations

- **Uncompressed data**: Most BMPs store pixels uncompressed (easy analysis)
- **Color table**: Indexed color images contain embedded palettes
- **Metadata**: Some DIB versions contain color space and ICC profile info
- **Simplicity**: Format is straightforward (good for forensic integrity)

## BMP Structure Example

```
[BMP File Header: 14 bytes]
  Magic: "BM"
  File size: 54054 bytes
  Reserved: 0
  Pixel offset: 54 bytes

[DIB Header (BITMAPINFOHEADER): 40 bytes]
  Header size: 40
  Width: 100 pixels
  Height: 100 pixels
  Color planes: 1
  Bits per pixel: 24
  Compression: 0 (BI_RGB, no compression)
  Image size: 0
  X pixels per meter: 0
  Y pixels per meter: 0
  Colors used: 0
  Important colors: 0

[Pixel Data: 30000 bytes]
  Row 0: [BGR BGR BGR ...] + padding
  Row 1: [BGR BGR BGR ...] + padding
  ...
  Row 99: [BGR BGR BGR ...] + padding
```

## Known Limitations

1. **No compression validation**: Doesn't validate RLE data is correctly formed
2. **No color table parsing**: Indexed images not validated for palette correctness
3. **Assumes contiguous**: Doesn't handle external color profiles
4. **File size trusted**: Relies on embedded file_size field

## Related Carvers

- **PNG**: Alternative format with compression
- **GIF**: Alternative format with indexed color
- **TIFF**: Alternative format with flexible structure
- **ICO**: Similar format for icons (multiple images)
