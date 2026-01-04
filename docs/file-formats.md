# File Format Support

Complete reference of all file formats supported by SwiftBeaver, organized by category.

## Summary Statistics

- **Total Formats**: 34
- **Image Formats**: 7
- **Document Formats**: 9  
- **Archive Formats**: 7
- **Multimedia Formats**: 8
- **Database & Special**: 3

---

## Image Formats

| Format | Extensions | Signature | Max Size (Default) | Validated | Notes |
|--------|-----------|-----------|-------------------|-----------|-------|
| **JPEG** | jpg, jpeg | `FF D8` | 100 MB | Yes (EOI marker) | Streams to `FF D9`, handles restart markers |
| **PNG** | png | `89 50 4E 47 0D 0A 1A 0A` | 100 MB | Yes (IEND chunk) | Chunk-based validation, preserves metadata |
| **GIF** | gif | `47 49 46 38 [37\|39] 61` | 100 MB | Yes (trailer 0x3B) | Supports GIF87a and GIF89a, animation preserved |
| **BMP** | bmp | `42 4D` | 100 MB | Yes | DIB header validation, multiple formats supported |
| **TIFF** | tif, tiff | `49 49 2A 00` (LE)<br>`4D 4D 00 2A` (BE) | 100 MB | Yes | IFD traversal, supports multi-page, EXIF, GPS |
| **WEBP** | webp | `52 49 46 46 ... 57 45 42 50` | 100 MB | Yes | RIFF container, VP8/VP8L/VP8X support, animation |
| **ICO** | ico | `00 00 01 00` | 4 MB | Yes | Multiple resolutions, validates BMP/PNG data |

### Image Format Details

**JPEG**:
- Detection: Start of Image (SOI) marker `FF D8`
- Termination: End of Image (EOI) marker `FF D9`
- Validation: Streaming parser, validates marker sequence
- Metadata: Preserves EXIF, JFIF data
- Edge Cases: Handles embedded restart markers (FF D0-D7)

**PNG**:
- Detection: 8-byte PNG signature
- Termination: IEND chunk
- Validation: Parses all chunks, validates chunk types
- Metadata: Preserves all chunks (tEXt, iTXt, tIME, etc.)
- Edge Cases: Handles multiple IDAT chunks, palettes, transparency

**GIF**:
- Detection: GIF87a or GIF89a header
- Termination: GIF trailer (0x3B)
- Validation: Block-by-block parsing with sub-blocks
- Metadata: Preserves comments, application extensions
- Edge Cases: Animated GIFs with multiple frames, local color tables

---

## Document Formats

| Format | Extensions | Signature | Max Size (Default) | Validated | Notes |
|--------|-----------|-----------|-------------------|-----------|-------|
| **PDF** | pdf | `25 50 44 46 2D` | 500 MB | Yes (%%EOF) | Searches for `%%EOF` marker, preserves structure |
| **OLE/CFB** | doc, xls, ppt, msg | `D0 CF 11 E0 A1 B1 1A E1` | 200 MB | Yes | MS Office 97-2003, uses FAT-based sectors |
| **DOCX** | docx | `50 4B 03 04` + ZIP structure | 100 MB | Yes | ZIP-based, validates central directory entries |
| **XLSX** | xlsx | `50 4B 03 04` + ZIP structure | 100 MB | Yes | ZIP-based, Office Open XML format |
| **PPTX** | pptx | `50 4B 03 04` + ZIP structure | 100 MB | Yes | ZIP-based, Office Open XML format |
| **RTF** | rtf | `7B 5C 72 74 66` | 50 MB | Yes | Rich Text Format, brace-balanced parsing |
| **ODT** | odt | `50 4B 03 04` + ZIP structure | 100 MB | Yes | OpenDocument Text (ZIP-based) |
| **ODS** | ods | `50 4B 03 04` + ZIP structure | 100 MB | Yes | OpenDocument Spreadsheet (ZIP-based) |
| **ODP** | odp | `50 4B 03 04` + ZIP structure | 100 MB | Yes | OpenDocument Presentation (ZIP-based) |

### Document Format Details

**PDF**:
- Detection: `%PDF-` header (any version)
- Termination: `%%EOF` marker
- Validation: Streaming search with carry buffer for boundary detection
- Metadata: Preserves all PDF objects, info dictionary
- Edge Cases: Handles linearized PDFs, incremental updates, large embedded files

**OLE/CFB** (DOC, XLS, PPT):
- Detection: 8-byte OLE signature
- Size Calculation: Parses FAT sectors and directory entries
- Validation: Header version (3 or 4), sector size, directory structure
- Metadata: Preserves all streams (content, VBA, properties)
- Edge Cases: Supports both 512-byte (v3) and 4096-byte (v4) sectors

**Office Open XML** (DOCX, XLSX, PPTX):
- Detection: ZIP signature + specific directory structure
- Classification: Examines central directory entries to distinguish types
- Validation: Verifies EOCD, validates ZIP structure
- Metadata: Preserves all XML parts, relationships, media
- Edge Cases: Handles encrypted documents (preserved but not decrypted)

---

## Archive Formats

| Format | Extensions | Signature | Max Size (Default) | Validated | Notes |
|--------|-----------|-----------|-------------------|-----------|-------|
| **ZIP** | zip, jar, apk, epub | `50 4B 03 04` | 100 MB | Yes (EOCD) | Finds End of Central Directory, classifies by content |
| **RAR** | rar | `52 61 72 21` (RAR4/5) | 500 MB | Yes | Supports RAR 4.x and 5.x formats |
| **7Z** | 7z | `37 7A BC AF 27 1C` | 2 GB | Yes | Metadata-driven, LZMA/LZMA2 compression |
| **TAR** | tar | ustar magic at offset 257 | 2 GB | Yes | Block-based, validates checksums, finds two zero blocks |
| **GZIP** | gz | `1F 8B` | 500 MB | Yes | Deflate compression, reads size from footer |
| **BZIP2** | bz2 | `42 5A 68` | 500 MB | Yes | Block-based compression, validates magic |
| **XZ** | xz | `FD 37 7A 58 5A 00` | 500 MB | Yes | LZMA2 compression, parses stream header |

### Archive Format Details

**ZIP**:
- Detection: Local file header `PK\x03\x04`
- Termination: End of Central Directory (EOCD) `PK\x05\x06`
- Validation: Searches for EOCD, parses directory
- Classification: DOCX/XLSX/PPTX/JAR/APK/EPUB/ODT/ODS/ODP based on contents
- Edge Cases: ZIP64 support, encrypted archives, split archives

**RAR**:
- Detection: `Rar!` magic with version byte
- Validation: Parses headers sequentially until end marker
- Formats: RAR 4.x (block-based), RAR 5.x (vint encoding)
- Edge Cases: Solid archives, recovery records, encrypted archives

**7Z**:
- Detection: 6-byte signature
- Size Calculation: Header offset + header size (metadata-driven)
- Validation: Parses start header structure
- Edge Cases: Solid archives, header compression, AES encryption

---

## Multimedia Formats

| Format | Extensions | Signature | Max Size (Default) | Validated | Notes |
|--------|-----------|-----------|-------------------|-----------|-------|
| **MP4** | mp4, m4v, m4a | `66 74 79 70` at offset +4 | 2 GB | Yes | Box-based structure, H.264/H.265 support |
| **MOV** | mov | `66 74 79 70` at offset +4 | 2 GB | Yes | QuickTime format, configurable MP4 compatibility |
| **MP3** | mp3 | `49 44 33` (ID3v2)<br>`FF FB`, `FF FA` (MPEG) | 50 MB | Yes | Frame-by-frame validation, ID3 tag support |
| **WAV** | wav | `52 49 46 46 ... 57 41 56 45` | 2 GB | Yes | RIFF container, PCM and compressed formats |
| **AVI** | avi | `52 49 46 46 ... 41 56 49 20` | 2 GB | Yes | RIFF container, multiple codec support |
| **OGG** | ogg | `4F 67 67 53` | 500 MB | Yes | Page-based container, Vorbis/Opus/Theora |
| **WEBM** | webm | Matroska/EBML signature | 2 GB | Yes | Matroska container, VP8/VP9/AV1 video |
| **WMV** | wmv, asf | ASF GUID signature | 2 GB | Yes | Windows Media container, ASF structure |

### Multimedia Format Details

**MP4/MOV**:
- Detection: `ftyp` box at offset 4
- Structure: Hierarchical box structure (ftyp, moov, mdat, etc.)
- Validation: Parses boxes, verifies ftyp and moov presence
- QuickTime: Configurable handling (separate MOV output or merge with MP4)
- Edge Cases: Fragmented MP4 (DASH/HLS), extended sizes (64-bit), metadata preservation

**MP3**:
- Detection: ID3v2 tag or MPEG frame sync word
- Validation: Parses frames sequentially, requires minimum 3 valid frames
- Formats: MPEG1/2/2.5 Layer III
- Metadata: Preserves ID3v1 and ID3v2 tags, embedded artwork
- Edge Cases: VBR files (Xing/VBRI headers), free bitrate, APE tags

**WAV/AVI**:
- Detection: RIFF header + WAVE/AVI form type
- Size Calculation: RIFF size field + 8 bytes
- Validation: Verifies RIFF structure and form type
- Metadata: Preserves all chunks/lists
- Edge Cases: RF64 for files >4GB (WAV), OpenDML extended format (AVI)

---

## Database & Special Formats

| Format | Extensions | Signature | Max Size (Default) | Validated | Notes |
|--------|-----------|-----------|-------------------|-----------|-------|
| **SQLite** | sqlite, db, sqlite3 | `53 51 4C 69 74 65 20 66 6F 72 6D 61 74 20 33 00` | 1 GB | Yes | Browser history extraction, page-level recovery |
| **ELF** | (none), bin | `7F 45 4C 46` | 100 MB | Yes | Linux executables, section-based structure |
| **EML** | eml | `46 72 6F 6D 3A` or RFC 2822 headers | 50 MB | Yes | Email message format, preserves headers and body |

### Database & Special Format Details

**SQLite**:
- Detection: 16-byte "SQLite format 3\0" header
- Size Calculation: page_count × page_size (from header)
- Validation: Parses header, validates page size and version
- Browser Artifacts: Automatically extracts history, cookies, downloads from Chromium-based browsers
- Page Recovery: Optional deep scan for individual pages when database is corrupted
- Edge Cases: Empty databases (page_count=0), WAL files, various page sizes (512-65536 bytes)

**ELF**:
- Detection: ELF magic number + class/endianness
- Structure: Program headers and section headers
- Validation: Parses ELF header, calculates extent from tables
- Edge Cases: Stripped binaries, core dumps, shared libraries

---

## Ebook Formats

| Format | Extensions | Signature | Max Size (Default) | Validated | Notes |
|--------|-----------|-----------|-------------------|-----------|-------|
| **EPUB** | epub | `50 4B 03 04` + ZIP structure | 100 MB | Yes | ZIP-based, validates mimetype file |
| **MOBI** | mobi, azw | `4D 4F 42 49` or PalmDOC header | 50 MB | Yes | Amazon Kindle format, PDB structure |
| **FB2** | fb2 | XML-based FictionBook signature | 20 MB | Yes | XML structure with validation |
| **LRF** | lrf | Sony BBeB format signature | 20 MB | Yes | Sony Reader format |

---

## Configuration Examples

### Enable Only Image Formats

```yaml
# config/images_only.yml
file_types:
  - id: jpeg
    enabled: true
  - id: png
    enabled: true
  - id: gif
    enabled: true
  - id: bmp
    enabled: true
  - id: tiff
    enabled: true
  - id: webp
    enabled: true
```

Or use CLI:
```bash
swiftbeaver --input image.dd --output ./out --enable-types jpeg,png,gif,bmp,tiff,webp
```

### High-Performance Settings

```yaml
# config/fast.yml
overlap_bytes: 32768  # Reduce overlap
file_types:
  - id: jpeg
    max_size: 52428800  # 50 MB (reduce from 100 MB)
  - id: pdf
    max_size: 104857600  # 100 MB (reduce from 500 MB)
```

### Forensic Mode (Maximum Recovery)

```yaml
# config/forensic.yml
overlap_bytes: 131072  # Increase overlap
enable_string_scan: true
string_scan_utf16: true
enable_entropy_detection: true
enable_sqlite_page_recovery: true

file_types:
  - id: jpeg
    min_size: 100  # Lower threshold (from 500)
```

---

## Format Capabilities Matrix

| Capability | JPEG | PNG | PDF | ZIP | MP4 | SQLite | OLE |
|------------|------|-----|-----|-----|-----|--------|-----|
| Header validation | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Structure parsing | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ |
| End marker detection | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ |
| Metadata preservation | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Corruption tolerance | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ |
| Fragmentation handling | ❌ | ❌ | ❌ | ⚠️ | ⚠️ | ❌ | ❌ |
| Encryption detection | ❌ | ❌ | ❌ | ⚠️ | ⚠️ | ⚠️ | ❌ |
| Multi-page/frame | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ |

**Legend**:
- ✅ Fully supported
- ⚠️ Partially supported or detected but not handled
- ❌ Not applicable or not supported

---

## Format-Specific Notes

### ZIP-Based Classification

SwiftBeaver automatically classifies ZIP files based on internal structure:

- **DOCX**: Contains `word/document.xml`
- **XLSX**: Contains `xl/workbook.xml`
- **PPTX**: Contains `ppt/presentation.xml`
- **ODT**: Contains `content.xml` + mimetype `application/vnd.oasis.opendocument.text`
- **ODS**: Contains `content.xml` + mimetype `application/vnd.oasis.opendocument.spreadsheet`
- **ODP**: Contains `content.xml` + mimetype `application/vnd.oasis.opendocument.presentation`
- **EPUB**: Contains `mimetype` file with `application/epub+zip`
- **JAR**: Contains `META-INF/MANIFEST.MF`
- **APK**: Contains `AndroidManifest.xml`

### QuickTime vs MP4

QuickTime (MOV) and MP4 use the same box-based structure. Configuration options:

```yaml
# config/default.yml
quicktime_mode: "mov"  # Keep MOV separate (default)
# OR
quicktime_mode: "mp4"  # Treat QuickTime as MP4
```

### String Artefact Extraction

When `--scan-strings` is enabled, SwiftBeaver extracts:

- **URLs**: HTTP/HTTPS/FTP URLs from printable spans
- **Emails**: RFC 5322 email addresses
- **Phones**: E.164-format phone numbers (with validation)

Supports both ASCII/UTF-8 and UTF-16LE/BE encodings.

---

## Performance Characteristics

| Format | Carving Speed | CPU Intensive | I/O Intensive | Memory Usage |
|--------|--------------|---------------|---------------|--------------|
| JPEG | ⚡⚡⚡ Fast | Low | Medium | Low |
| PNG | ⚡⚡ Medium | Medium | Medium | Low |
| PDF | ⚡⚡ Medium | Low | High | Low |
| ZIP | ⚡ Slow | Medium | High | Medium |
| MP4 | ⚡⚡ Medium | Medium | Medium | Low |
| SQLite | ⚡⚡⚡ Fast | Low | Low | Low |
| TIFF | ⚡ Slow | High | Medium | Medium |

**Speed ratings**:
- ⚡⚡⚡ Fast: Metadata-driven or simple marker detection
- ⚡⚡ Medium: Structure parsing with moderate complexity
- ⚡ Slow: Complex traversal or extensive validation

---

## See Also

- **[Carver Documentation](carver/README.md)** - Detailed algorithm explanations
- **[Configuration Guide](config.md)** - Full configuration reference
- **[Performance Tuning](performance.md)** - Optimize for specific formats
- **[Use Cases](use-cases.md)** - Real-world forensic scenarios
