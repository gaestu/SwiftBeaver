# File Type Coverage Expansion - Overview

Status: In Progress

## Summary

This document provides an overview of the file type coverage expansion effort, tracking which formats from the golden image are supported vs. pending implementation.

## Current Coverage (Implemented)

| Format | Extension | Handler | Notes |
|--------|-----------|---------|-------|
| JPEG | jpg, jpeg | `jpeg.rs` | SOI/EOI detection |
| PNG | png | `png.rs` | Chunk-based parsing |
| GIF | gif | `gif.rs` | Block structure |
| BMP | bmp | `bmp.rs` | Header size field |
| TIFF | tiff, tif | `tiff.rs` | IFD chain walking |
| WebP | webp | `webp.rs` | RIFF container |
| PDF | pdf | `pdf.rs` | %%EOF footer search |
| ZIP | zip | `zip.rs` | EOCD detection |
| RAR | rar | `rar.rs` | RAR4/RAR5 support |
| 7z | 7z | `sevenz.rs` | Start header sizes |
| SQLite | sqlite | `sqlite.rs` | Page-based structure |
| MP4 | mp4 | `mp4.rs` | Atom/box walking |
| **WAV** | wav | `wav.rs` | ✅ RIFF container, shared module |
| **AVI** | avi | `avi.rs` | ✅ RIFF container, shared module |
| **MP3** | mp3 | `mp3.rs` | ✅ ID3v2 + frame walking |
| **OLE** | doc, xls, ppt | `ole.rs` | ✅ CFB sector parsing |

## Planned Coverage

### Audio Formats
See: [audio_carving.md](audio_carving.md)

| Format | Extension | Complexity | Priority |
|--------|-----------|------------|----------|
| WAV | wav | Low | High |
| MP3 | mp3 | Medium | High |
| OGG | ogg, oga | Medium | Medium |

### Video Formats  
See: [video_carving.md](video_carving.md)

| Format | Extension | Complexity | Priority |
|--------|-----------|------------|----------|
| AVI | avi | Low | High |
| MOV | mov, qt | Medium | High |
| WMV | wmv, wma, asf | Medium | Medium |
| WEBM | webm, mkv | High | Medium |

### Archive Formats
See: [archive_carving.md](archive_carving.md)

| Format | Extension | Complexity | Priority |
|--------|-----------|------------|----------|
| XZ | xz | Medium | High |
| TAR | tar | Medium | Medium |
| GZIP | gz | High | Medium |
| BZIP2 | bz2 | High | Low |

### Document Formats
See: [document_carving.md](document_carving.md)

| Format | Extension | Complexity | Priority |
|--------|-----------|------------|----------|
| OLE Compound | doc, xls, ppt | Medium | High |
| RTF | rtf | Low-Medium | Medium |
| OOXML Classification | docx, xlsx, pptx | Low | High |
| ODF Classification | odt, ods, odp | Low | High |

### Miscellaneous Formats
See: [misc_format_carving.md](misc_format_carving.md)

| Format | Extension | Complexity | Priority |
|--------|-----------|------------|----------|
| ICO | ico, cur | Low | Medium |
| ELF | (none), so | Medium | Medium |
| MOBI/AZW | mobi, azw3 | Medium | Low |
| EML | eml | High | Low |

## Recommended Implementation Order

### Phase 1: Quick Wins (Low Complexity, High Value)
1. ✅ **WAV** - RIFF structure, size in header - IMPLEMENTED
2. ✅ **AVI** - RIFF structure, size in header - IMPLEMENTED
3. ✅ **OLE Compound** - Important forensic format - IMPLEMENTED
4. **ZIP Classification** - Enhance existing handler (already works)

### Phase 2: Medium Complexity
5. ✅ **MP3** - Frame walking with ID3 handling - IMPLEMENTED
6. **MOV** - Reuse MP4 box-walking
7. **XZ** - Structured block format
8. **RTF** - Brace counting
9. **ICO** - Directory structure
10. **ELF** - Section header parsing

### Phase 3: Higher Complexity
11. **TAR** - Header walking with checksum validation
12. **OGG** - Page-based structure
13. **WMV/ASF** - GUID-based objects
14. **WEBM** - EBML variable-length encoding

### Phase 4: Complex/Low Priority
15. **GZIP** - Deflate stream parsing
16. **BZIP2** - Bit-aligned markers
17. **MOBI/AZW** - PDB container
18. **EML** - Text-based, heuristic end

## Shared Infrastructure Needs

### ✅ RIFF Handler Unification (IMPLEMENTED)
Multiple formats use RIFF container:
- WAV (RIFF + "WAVE") ✅
- AVI (RIFF + "AVI ") ✅
- WebP (RIFF + "WEBP")

**Implemented:** Created shared RIFF parsing in `src/carve/riff.rs`, specific handlers validate form type.

### Offset-Based Pattern Matching
Some formats have signatures at non-zero offsets:
- TAR: "ustar" at offset 257
- MOBI: "BOOKMOBI" at offset 60

**Options:**
1. Enhance scanner to support offset patterns
2. Scan for string, validate structure backwards
3. Scan broad region, let handler verify

### Compression Stream Parsing
GZIP and BZIP2 require understanding compressed streams:
- GZIP: deflate block parsing
- BZIP2: bit-aligned block markers

**Recommendation:** Start with max_size limits, add stream parsing as enhancement.

## Golden Image Coverage Matrix

Files from `tests/golden_image/manifest.json`:

| Category | Files | Currently Covered | After Full Implementation |
|----------|-------|-------------------|--------------------------|
| archives | 11 | ZIP, RAR, 7z (3) | +TAR, GZ, BZ2, XZ (7) |
| audio | 4 | ✅ MP3, WAV (3) | +OGG (4) |
| binaries | 2 | - | ELF (2) |
| databases | 5 | SQLite (5) | (5) |
| documents | 14 | PDF, ZIP-based, ✅ OLE (8) | +RTF (14) |
| email | 2 | - | (deferred) |
| images | 18 | JPG,PNG,GIF,BMP,TIFF,WebP (15) | +ICO (16) |
| media_tiny | 4 | ✅ MP3, WAV, AVI (3) | +WEBM (4) |
| other | 17 | - | (text/misc - deferred) |
| video | 7 | MP4, ✅ AVI (2) | +MOV, WEBM, WMV, OGG (6) |

**Current coverage:** ~36/84 files (43%) - UP FROM 33%
**After Phase 1-3:** ~70/84 files (83%)
**Notes:** 
- "other" category contains mostly text/data formats (JSON, XML, CSV, TXT) not typically carved
- EML deferred due to complexity
- Some formats overlap (e.g., video OGG vs audio OGG)

## Configuration Additions Summary

After full implementation, `config/default.yml` will have these additional file_types:

```yaml
# Audio
- id: "wav"
- id: "mp3"
- id: "ogg"

# Video
- id: "avi"
- id: "mov"
- id: "wmv"
- id: "webm"

# Archives
- id: "xz"
- id: "tar"
- id: "gzip"
- id: "bzip2"

# Documents
- id: "ole"
- id: "rtf"

# Misc
- id: "ico"
- id: "elf"
```

## Related Planning Documents

- [audio_carving.md](audio_carving.md) - MP3, WAV, OGG details
- [video_carving.md](video_carving.md) - AVI, MOV, WEBM, WMV details
- [archive_carving.md](archive_carving.md) - TAR, GZIP, BZIP2, XZ details
- [document_carving.md](document_carving.md) - OLE, RTF, OOXML, ODF details
- [misc_format_carving.md](misc_format_carving.md) - ICO, ELF, ebook details
