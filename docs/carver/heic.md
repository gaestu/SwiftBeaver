# HEIC/HEIF Carver

## Overview

The HEIC carver extracts High Efficiency Image Container (HEIC) and High Efficiency Image Format (HEIF) files. These are the default photo formats on modern iOS devices (iPhone 7+, iOS 11+) and are increasingly common on Android devices.

HEIC/HEIF uses the ISO Base Media File Format (ISOBMFF), the same box-based structure as MP4/MOV.

## Signature Detection

**Header Patterns** (ftyp box with HEIC/HEIF brand):

HEIC files with 24-byte ftyp (0x18):
- `00 00 00 18 66 74 79 70 68 65 69 63` — ftyp + "heic" brand

HEIC files with 28-byte ftyp (0x1C):
- `00 00 00 1C 66 74 79 70 68 65 69 63` — ftyp + "heic" brand

HEIC files with 32-byte ftyp (0x20):
- `00 00 00 20 66 74 79 70 68 65 69 63` — ftyp + "heic" brand

HEIF (mif1) files with 24-byte ftyp:
- `00 00 00 18 66 74 79 70 6D 69 66 31` — ftyp + "mif1" brand

HEIF (mif1) files with 28-byte ftyp:
- `00 00 00 1C 66 74 79 70 6D 69 66 31` — ftyp + "mif1" brand

HEIF (mif1) files with 32-byte ftyp:
- `00 00 00 20 66 74 79 70 6D 69 66 31` — ftyp + "mif1" brand

## Supported Brands

The carver recognizes these major brands in the ftyp box:

| Brand | Description |
|-------|-------------|
| `heic` | HEIC image |
| `heix` | HEIC image extended |
| `heim` | HEIC image sequence |
| `heis` | HEIC image sequence |
| `mif1` | HEIF image (MIAF) |
| `msf1` | HEIF image sequence |
| `hevc` | HEVC video (can contain images) |
| `hevx` | HEVC extended |

## Carving Algorithm

The HEIC carver uses box-based parsing:

### 1. ftyp Box Validation

The first box must be `ftyp` containing a recognized HEIC/HEIF brand.

```
Bytes 0-3: Box size (big-endian u32)
Bytes 4-7: Box type ("ftyp")
Bytes 8-11: Major brand (heic/mif1/etc.)
Bytes 12-15: Minor version
Bytes 16+: Compatible brands (optional)
```

### 2. Box Iteration

Sequential box parsing continues until:
- End of evidence data
- Invalid box structure
- max_size limit reached

**Box Header Format**:
```
Bytes 0-3: Size (big-endian u32)
  - If size == 1: Extended size in bytes 8-15 (u64)
  - If size == 0: Box extends to EOF
Bytes 4-7: Type (4-byte ASCII)
```

**Key Box Types**:
- `ftyp`: File type (must be first)
- `meta`: Metadata containing image properties and item locations
- `mdat`: Media data (actual image pixels)

### 3. Size Determination

The file ends when:
1. End of evidence data is reached after valid boxes
2. An invalid box structure is encountered
3. max_size limit is reached (truncated output)

## Validation

1. **ftyp validation**: First box must be ftyp with HEIC/HEIF brand
2. **Box structure**: All boxes must have valid size/type headers
3. **Size limits**: Respects configured max_size

## Configuration

```yaml
- id: "heic"
  extensions: ["heic", "heif", "hif"]
  header_patterns:
    - id: "heic_ftyp_18"
      hex: "000000186674797068656963"
    - id: "heic_ftyp_1c"
      hex: "0000001C6674797068656963"
    - id: "heic_ftyp_20"
      hex: "000000206674797068656963"
    - id: "mif1_ftyp_18"
      hex: "00000018667479706D696631"
    - id: "mif1_ftyp_1c"
      hex: "0000001C667479706D696631"
    - id: "mif1_ftyp_20"
      hex: "00000020667479706D696631"
  footer_patterns: []
  max_size: 104857600  # 100 MB
  min_size: 100
  validator: "heic"
```

## Output

Carved HEIC/HEIF files are written to `<output>/carved/heic/` with naming:
```
heic_<12-digit-hex-offset>.heic
```

Example: `heic_000000001000.heic`

## Limitations

- Does not extract EXIF metadata (can be added later)
- Does not extract embedded thumbnails
- Does not convert to JPEG
- AVIF files (similar ISOBMFF structure) are not handled by this carver

## See Also

- [MP4 Carver](mp4.md) — Similar ISOBMFF structure
- [Architecture](../architecture.md) — Overall pipeline design
