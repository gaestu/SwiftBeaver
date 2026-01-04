# GIF Carver

## Overview

The GIF carver extracts Graphics Interchange Format files by parsing the header, logical screen descriptor, and block structure defined by GIF87a and GIF89a specifications.

## Signature Detection

**Header Patterns**:
- `GIF87a` (ASCII bytes: `47 49 46 38 37 61`)
- `GIF89a` (ASCII bytes: `47 49 46 38 39 61`)

The scanner triggers on either variant. GIF89a added support for transparency and animation.

## Carving Algorithm

The GIF carver uses block-based parsing with detailed structure validation:

### 1. Header Parsing (6 bytes)
```
Bytes 0-5: "GIF87a" or "GIF89a"
```

### 2. Logical Screen Descriptor (7 bytes)
```
Bytes 0-1: Width (little-endian u16)
Bytes 2-3: Height (little-endian u16)
Byte 4: Packed fields
  Bit 7: Global Color Table flag
  Bits 6-4: Color resolution
  Bit 3: Sort flag
  Bits 2-0: GCT size = 2^(value+1)
Byte 5: Background color index
Byte 6: Pixel aspect ratio
```

### 3. Global Color Table (if present)
```
Size: 3 * 2^(size_bits + 1) bytes
Read if GCT flag is set in packed field
```

### 4. Block Iteration

Loop through blocks until GIF Trailer (`0x3B`) is found:

**Block Types**:
- `0x21`: Extension block
  - Read 1 byte (label)
  - Read sub-blocks until terminator
- `0x2C`: Image descriptor
  - Read 9 bytes (image descriptor)
  - Parse local color table (if present)
  - Read 1 byte (LZW min code size)
  - Read image data sub-blocks
- `0x3B`: GIF Trailer (end of file)

### Sub-Block Reading

Sub-blocks are used for extension data and image data:
```
Loop:
  Read 1 byte → block size
  If size == 0 → terminator, break
  Read size bytes → block data
```

## Validation

- **Validated**: `true` if GIF Trailer (`0x3B`) is found
- **Truncated**: `true` if max_size or EOF reached before trailer
- **Invalid**: Removed if:
  - Header is not "GIF87a" or "GIF89a"
  - Block ID is not recognized (0x21, 0x2C, 0x3B)
  - Structure parsing fails

## Size Constraints

- **Default min_size**: 100 bytes (as of v0.2.1)
- **Default max_size**: 100 MB
- Minimum viable GIF: header(6) + LSD(7) + trailer(1) = 14 bytes
- Files below min_size are discarded

## Hash Computation

- **MD5**: Computed via `CarveStream` as blocks are read
- **SHA-256**: Computed via `CarveStream` as blocks are read
- Covers complete file from header to trailer (or truncation point)

## Testing

**Test file**: `tests/carver_gif.rs`

### Test Strategy

Golden image framework with manifest:

1. **Test data**: Multiple GIF variants in golden image
   - GIF87a (simple)
   - GIF89a (animated)
   - GIF with transparency
   - GIF with local color tables
2. **Verification**:
   - All expected GIFs found
   - Sizes match manifest exactly
   - Files are valid (can be opened by image viewers)
3. **Coverage**: Tests both header variants and complex structures

### Example Test

```rust
#[test]
fn test_gif_carver() {
    let config = default_config();
    let (metadata, _) = carver_for_types(&["gif"], &config);
    verify_manifest_match(metadata, "gif");
}
```

## Edge Cases Handled

1. **Empty sub-blocks**: Correctly handles terminator (size=0) without reading data
2. **Large local color tables**: Validates LCT size before allocation
3. **Unknown extension labels**: Reads and skips sub-blocks without error
4. **Multiple image descriptors**: Handles animated GIFs with multiple frames
5. **Missing trailer**: Marks as truncated but keeps file if substantive
6. **Invalid block IDs**: Returns error to avoid infinite loops

## Performance Characteristics

- **Streaming**: `CarveStream` provides efficient sequential access
- **Memory usage**: Constant (only block headers loaded)
- **I/O pattern**: Many small reads (block sizes, descriptors)
- **Color table skipping**: Direct stream advancement (no memory allocation)

## Forensic Considerations

- **Animation preservation**: Carves complete animated GIFs
- **Metadata retention**: Preserves comment extensions and application extensions
- **Corruption handling**: Tolerates malformed blocks in later frames
- **Transparency data**: Retains graphic control extensions (GIF89a)

## GIF Structure Example

```
[Header: "GIF89a"]
[Logical Screen Descriptor]
[Global Color Table] (if GCT flag set)
[Extension Block: Graphic Control] (0x21 0xF9)
  [Sub-blocks with transparency info]
  [Terminator: 0x00]
[Image Descriptor] (0x2C)
  [Local Color Table] (if LCT flag set)
  [LZW Min Code Size]
  [Image Data Sub-blocks]
  [Terminator: 0x00]
... (more image descriptors for animation)
[GIF Trailer: 0x3B]
```

## Related Carvers

- **PNG**: Alternative image format with better compression
- **BMP**: Simpler bitmap format without compression
- **WEBP**: Modern format supporting animation
