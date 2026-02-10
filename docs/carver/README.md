# Carver Documentation Index

This directory contains detailed documentation for all 34 file format carvers implemented in SwiftBeaver.

## Documentation Structure

Each carver document includes:
- **Overview**: Purpose and high-level approach
- **Signature Detection**: Byte patterns that trigger the carver
- **Carving Algorithm**: Step-by-step extraction logic
- **Validation**: How files are verified as valid/truncated/invalid
- **Size Constraints**: Min/max size limits and defaults
- **Hash Computation**: MD5/SHA256 calculation approach
- **Testing**: Test strategy and golden image framework
- **Edge Cases**: Special conditions and how they're handled
- **Performance**: Memory usage and I/O patterns
- **Forensic Considerations**: Metadata, timestamps, encryption, etc.
- **Structure Examples**: Visual representation of file format
- **Known Limitations**: Current restrictions or unsupported features
- **Related Carvers**: Similar or related file formats

## Image Format Carvers

| Carver | Documentation | Status | Description |
|--------|--------------|--------|-------------|
| [JPEG](jpeg.md) | ✅ Complete | Production | JPEG/JFIF images (FF D8 → FF D9) |
| [PNG](png.md) | ✅ Complete | Production | PNG images (chunk-based) |
| [GIF](gif.md) | ✅ Complete | Production | GIF87a/89a images (block-based) |
| [BMP](bmp.md) | ✅ Complete | Production | Windows Bitmap images |
| [TIFF](tiff.md) | ✅ Complete | Production | Tagged Image File Format (IFD-based) |
| [WEBP](webp.md) | ✅ Complete | Production | WebP images (RIFF container) |
| ICO | ⏳ TBD | Production | Windows Icon Format |

## Archive Format Carvers

| Carver | Documentation | Status | Description |
|--------|--------------|--------|-------------|
| [ZIP](zip.md) | ✅ Complete | Production | ZIP archives (PK\\x03\\x04 → EOCD) |
| [RAR](rar.md) | ✅ Complete | Production | RAR 4.x and RAR 5.x archives |
| [7Z](7z.md) | ✅ Complete | Production | 7-Zip archives (LZMA/LZMA2) |
| TAR | ⏳ TBD | Production | TAR archives (ustar format) |
| GZIP | ⏳ TBD | Production | GZIP compressed files |
| BZIP2 | ⏳ TBD | Production | BZip2 compressed files |
| XZ | ⏳ TBD | Production | XZ/LZMA compressed files |

## Document Format Carvers

| Carver | Documentation | Status | Description |
|--------|--------------|--------|-------------|
| [PDF](pdf.md) | ✅ Complete | Production | Portable Document Format |
| OLE | ⏳ TBD | Production | OLE/CFB (DOC, XLS, PPT, MSG) |
| RTF | ⏳ TBD | Production | Rich Text Format |
| EML | ⏳ TBD | Production | Email message format |

## Multimedia Carvers

| Carver | Documentation | Status | Description |
|--------|--------------|--------|-------------|
| [MP4](mp4.md) | ✅ Complete | Production | MPEG-4 video (box-based) |
| [MP3](mp3.md) | ✅ Complete | Production | MPEG Audio Layer III |
| [WAV](wav.md) | ✅ Complete | Production | Waveform Audio (RIFF) |
| MOV | ⏳ TBD | Production | QuickTime video (box-based) |
| AVI | ⏳ TBD | Production | Audio Video Interleave (RIFF) |
| WMV | ⏳ TBD | Production | Windows Media Video (ASF) |
| WEBM | ⏳ TBD | Production | WebM video (Matroska) |
| OGG | ⏳ TBD | Production | Ogg Vorbis/Opus audio |

## Database & Special Carvers

| Carver | Documentation | Status | Description |
|--------|--------------|--------|-------------|
| [SQLite](sqlite.md) | ✅ Complete | Production | SQLite3 database files |
| [SQLite WAL](sqlite_wal.md) | ✅ Complete | Production | SQLite Write-Ahead Log files |
| [SQLite Page](sqlite_page.md) | ✅ Complete | Production | SQLite leaf page fragments |
| ELF | ⏳ TBD | Production | Executable and Linkable Format |
| MOBI | ⏳ TBD | Production | Mobipocket ebook format |
| FB2 | ⏳ TBD | Production | FictionBook 2.0 ebook format |
| LRF | ⏳ TBD | Production | Sony Portable Reader format |

## Quick Reference by Signature

### Common Signatures

```
FF D8                  → JPEG
89 50 4E 47           → PNG  
47 49 46 38 37 61     → GIF87a
47 49 46 38 39 61     → GIF89a
42 4D                  → BMP
49 49 2A 00           → TIFF (little-endian)
4D 4D 00 2A           → TIFF (big-endian)
52 49 46 46 xx xx xx xx 57 45 42 50  → WEBP

50 4B 03 04           → ZIP
52 61 72 21           → RAR
37 7A BC AF 27 1C     → 7Z
1F 8B                  → GZIP
42 5A 68               → BZIP2
FD 37 7A 58 5A 00     → XZ

25 50 44 46           → PDF
D0 CF 11 E0           → OLE/CFB
7B 5C 72 74 66        → RTF
46 72 6F 6D 3A        → EML (From:)

66 74 79 70           → MP4/MOV (at offset +4)
49 44 33               → MP3 (ID3v2)
FF FB / FF FA          → MP3 (MPEG frames)
52 49 46 46 xx xx xx xx 57 41 56 45  → WAV
52 49 46 46 xx xx xx xx 41 56 49 20  → AVI

53 51 4C 69 74 65     → SQLite
37 7F 06 82 / 83      → SQLite WAL
0D / 0A               → SQLite page fragment candidates
7F 45 4C 46           → ELF
```

## Testing Coverage

All carvers use the golden image framework:

1. **Golden Image**: `tests/golden_image/golden.bin` contains known files
2. **Manifest**: `tests/golden_image/manifest.json` lists expected files with:
   - `offset`: Where the file starts
   - `size`: Expected file size
   - `file_type`: Carver type
   - Optional: `validated`, `sha256`, etc.

3. **Test Structure**:
   ```rust
   #[test]
   fn test_<format>_carver() {
       let config = default_config();
       let (metadata, _) = carver_for_types(&["<format>"], &config);
       verify_manifest_match(metadata, "<format>");
   }
   ```

4. **Verification**:
   - Count matches expected
   - Sizes match exactly
   - All files exist on disk
   - Hashes match (if provided)

## Configuration

All carvers respect configuration in `config/default.yml`:

```yaml
file_types:
  - name: jpeg
    enabled: true
    pattern: "\\xFF\\xD8"
    extension: jpg
    min_size: 500
    max_size: 104857600  # 100 MB
```

## Common Patterns

### Metadata-Driven Carvers
Size known from header (very efficient):
- **SQLite**: page_count × page_size
- **BMP**: file_size field in header
- **WEBP**: RIFF size + 8
- **WAV**: RIFF size + 8
- **7Z**: 32 + next_header_offset + next_header_size

### Marker-Based Carvers
Search for end marker (streaming):
- **JPEG**: FF D8 → FF D9
- **PDF**: %PDF → %%EOF
- **GIF**: GIF8?a → 0x3B

### Structure-Based Carvers
Parse internal structure to find extent:
- **PNG**: Parse chunks until IEND
- **ZIP**: Find EOCD (PK\\x05\\x06)
- **MP4**: Parse boxes until ftyp+moov seen
- **TIFF**: Parse IFDs and track max offset

### Frame-Based Carvers
Validate sequential frames:
- **MP3**: Parse MPEG frames sequentially
- **TAR**: Parse 512-byte blocks until two zero blocks

## Performance Characteristics

| Pattern | Memory Usage | I/O Pattern | Complexity |
|---------|--------------|-------------|------------|
| Metadata-driven | Minimal | Single read + copy | Low |
| Marker-based | Constant (~64KB) | Sequential scanning | Medium |
| Structure-based | Moderate (tracking offsets) | Random reads + copy | High |
| Frame-based | Constant | Many small reads | Medium-High |

## Forensic Best Practices

All carvers follow these principles:

1. **Evidence Integrity**: Never modify source evidence
2. **Reproducibility**: Same input → same output (deterministic)
3. **Provenance**: Record run_id, global_start, global_end, hashes
4. **Corruption Tolerance**: Keep truncated/damaged files when possible
5. **Metadata Preservation**: Retain all embedded metadata (EXIF, ID3, etc.)
6. **Size Limits**: Respect min_size and max_size to prevent resource exhaustion

## Future Documentation

The following carvers are production-ready but documentation is pending:

- **GZIP, BZIP2, XZ**: Compression formats (metadata-driven)
- **TAR**: Archive format (block-based)
- **ICO**: Icon format (directory structure)
- **OLE**: Office documents (FAT-based sectors)
- **RTF**: Rich text (marker-based)
- **EML**: Email format (marker-based)
- **MOV**: QuickTime video (box-based, similar to MP4)
- **AVI, WMV, WEBM**: Video formats (RIFF/ASF/Matroska)
- **OGG**: Audio format (page-based)
- **ELF**: Executable format (section-based)
- **MOBI, FB2, LRF**: Ebook formats (various structures)

For implementation details, consult source code in [src/carve/](../../src/carve/).
