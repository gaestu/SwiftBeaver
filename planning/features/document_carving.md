# Document Carving (OLE Compound, RTF, Office Open XML)

Status: Partially Implemented

## Implementation Status

- ✅ **OLE Compound Documents**: Implemented in `src/carve/ole.rs` (DOC, XLS, PPT)
- ⏳ **RTF**: Not yet implemented
- ✅ **Office Open XML**: Already handled by ZIP classifier (DOCX, XLSX, PPTX)
- ✅ **OpenDocument**: Already handled by ZIP classifier (ODT, ODS, ODP)

## Problem Statement

The golden image contains various document formats that are not fully handled:
- **OLE Compound Documents:** DOC, XLS, PPT (Microsoft Office 97-2003)
- **Office Open XML:** DOCX, XLSX, PPTX, POTX (detected as ZIP but not classified)
- **OpenDocument:** ODT, ODS, ODP (detected as ZIP but not classified)
- **RTF:** Rich Text Format

While ZIP-based formats are detected, they're not properly classified. OLE compound documents and RTF have no carving support.

## Scope

- Add signature patterns and handlers for OLE compound documents.
- Add RTF carving.
- Enhance ZIP-based document classification (OOXML, ODF).
- Wire handlers into the carve registry.
- Add unit tests.
- Update documentation.

## Non-Goals

- Document content extraction (text, metadata).
- Embedded object extraction.
- Macro extraction or analysis.
- Document repair or recovery.

---

## File Format Details

### OLE Compound Documents (DOC, XLS, PPT)

**Signature:**
- `D0 CF 11 E0 A1 B1 1A E1` - OLE/CFB (Compound File Binary) magic

**Structure:**
- 512-byte sectors (or 4096 for v4 files).
- Header at offset 0 (512 bytes).
- FAT (File Allocation Table) for sector chains.
- Directory entries identify streams (e.g., "WordDocument", "Workbook", "PowerPoint Document").

**Size Detection Strategy:**
1. Read header:
   - Bytes 0-7: Signature
   - Bytes 28-29: Minor version
   - Bytes 30-31: Major version (3 = 512-byte sectors, 4 = 4096-byte)
   - Bytes 44-47: Total sectors (v3) or use bytes 72-79 for v4
   - Bytes 48-51: First directory sector
2. Calculate size:
   - V3: (total_sectors + 1) * 512
   - V4: (total_sectors + 1) * 4096
3. Validate by reading directory to confirm document type.

**Document Type Detection:**
After carving, determine type by checking directory entries:
- DOC: Contains "WordDocument" stream
- XLS: Contains "Workbook" or "Book" stream
- PPT: Contains "PowerPoint Document" stream

**Complexity:** Medium - sector-based structure, FAT chains.

**Config Entry:**
```yaml
- id: "ole"
  extensions: ["doc", "xls", "ppt", "msg", "ole"]
  header_patterns:
    - id: "ole_cfb"
      hex: "D0CF11E0A1B11AE1"  # OLE/CFB magic
  max_size: 536870912         # 512 MiB
  min_size: 512               # Minimal header
  validator: "ole"
```

### RTF (Rich Text Format)

**Signature:**
- `7B 5C 72 74 66` (`{\rtf`) - RTF header start

**Structure:**
- Plain text with control words and groups.
- Brace-delimited: `{...}` groups nest.
- Ends when top-level braces balance (final `}`).

**Size Detection Strategy:**
1. Track brace depth: start at 0, `{` increments, `}` decrements.
2. End of file: depth returns to 0.
3. Handle escapes: `\{` and `\}` don't count.
4. Handle binary data: `\binN` skips N bytes.

**Complexity:** Low-Medium - brace counting with escape handling.

**Config Entry:**
```yaml
- id: "rtf"
  extensions: ["rtf"]
  header_patterns:
    - id: "rtf_header"
      hex: "7B5C727466"       # "{\rtf"
  max_size: 104857600         # 100 MiB
  min_size: 7                 # "{\rtf1}"
  validator: "rtf"
```

### Office Open XML (DOCX, XLSX, PPTX, POTX)

**Current Status:** Detected as ZIP, but not classified.

**Enhancement Strategy:**
1. These are ZIP files with specific structure:
   - `[Content_Types].xml` at root
   - `word/` folder (DOCX), `xl/` folder (XLSX), `ppt/` folder (PPTX)

2. **Option A - Post-classification:** After carving ZIP, check internal structure.
3. **Option B - Pattern enhancement:** Detect based on ZIP local file header contents.

**ZIP Internal Pattern Detection:**
- DOCX: First file often `[Content_Types].xml` followed by `docProps/` or `word/`
- Pattern at offset 30+ in ZIP: filename in local file header

**Recommendation:** Enhance ZIP handler to classify based on first filename.

**First Filename Detection:**
```
PK\x03\x04  (4 bytes: ZIP local header)
...        (22 bytes: header fields)
filename   (at offset 30, length from header)
```

Common first filenames:
- DOCX: `[Content_Types].xml`, `_rels/.rels`, `word/`
- XLSX: `[Content_Types].xml`, `xl/`
- PPTX: `[Content_Types].xml`, `ppt/`
- ODT/ODS/ODP: `mimetype` (contains `application/vnd.oasis.opendocument.*`)

### OpenDocument (ODT, ODS, ODP)

**Current Status:** Detected as ZIP.

**Enhancement:**
- First file is usually `mimetype` (uncompressed, contains MIME type).
- MIME types:
  - ODT: `application/vnd.oasis.opendocument.text`
  - ODS: `application/vnd.oasis.opendocument.spreadsheet`
  - ODP: `application/vnd.oasis.opendocument.presentation`

---

## Implementation Plan

### Phase 1: OLE Compound Document Handler

1. Create `src/carve/ole.rs`:
   - Validate CFB signature.
   - Parse header: version, sector size, total sectors.
   - Calculate file size from sector count.
   - Optionally: parse directory to determine specific type.
2. Add config entry.
3. Wire into carve registry.
4. Add unit tests with DOC, XLS, PPT samples.

### Phase 2: RTF Handler

1. Create `src/carve/rtf.rs`:
   - Validate `{\rtf` signature.
   - Implement brace-counting parser:
     - Skip escaped braces (`\{`, `\}`).
     - Handle `\binN` binary data.
     - Track depth, end when depth reaches 0.
2. Add config entry.
3. Add unit tests.

### Phase 3: ZIP Document Classification

1. Enhance `src/carve/zip.rs`:
   - After carving, examine first filename(s).
   - Detect OOXML (DOCX, XLSX, PPTX) by structure.
   - Detect ODF (ODT, ODS, ODP) by mimetype file.
   - Update `file_type` and `extension` in CarvedFile.

2. Alternatively, create separate detection:
   - Add new patterns for ZIP + specific content.
   - This requires multi-pattern matching (header + content offset).

**Recommendation:** Post-carve classification in ZIP handler.

### Phase 4: Specialized OOXML/ODF File Types (Optional)

If distinct file_type entries are needed:
1. Add config entries for docx, xlsx, pptx, odt, ods, odp.
2. These would share ZIP carving but have distinct classification.

---

## OLE Compound Document Details

### Header Layout (512 bytes)

| Offset | Size | Description |
|--------|------|-------------|
| 0 | 8 | Signature |
| 8 | 16 | CLSID (usually zero) |
| 24 | 2 | Minor version |
| 26 | 2 | Major version |
| 28 | 2 | Byte order (0xFFFE = little-endian) |
| 30 | 2 | Sector power (9 = 512, 12 = 4096) |
| 32 | 2 | Mini sector power |
| 34 | 6 | Reserved |
| 40 | 4 | Total sectors (V4 only, else in FAT) |
| 44 | 4 | First directory sector SECID |
| 48 | 4 | Transaction signature |
| 52 | 4 | Mini stream cutoff size |
| 56 | 4 | First mini FAT sector SECID |
| 60 | 4 | Number of mini FAT sectors |
| 64 | 4 | First DIFAT sector SECID |
| 68 | 4 | Number of DIFAT sectors |
| 72 | 436 | DIFAT array (109 entries) |

### Size Calculation

**Version 3 (512-byte sectors):**
- Total sectors often unreliable in header.
- Walk FAT to find highest used sector.
- Alternative: Use file size hint from directory.

**Version 4 (4096-byte sectors):**
- Header field at offset 40 has sector count.
- Size = (sector_count + 1) * 4096

**Practical approach:**
1. Read FAT sectors (at DIFAT locations).
2. Find highest allocated sector number.
3. Size = (highest_sector + 1) * sector_size + 512 (header).

---

## RTF Parsing Details

### Control Words

- Start with `\`, followed by letters, optionally a number.
- Example: `\rtf1`, `\b`, `\par`, `\bin1234`

### Binary Data

- `\binN` - next N bytes are binary data (don't parse).
- Example: `\bin100` followed by 100 raw bytes.

### Hex Data

- `\'XX` - hex-encoded byte.
- Example: `\'ab` = byte 0xAB.

### Brace Counting Algorithm

```rust
fn find_rtf_end(data: &[u8]) -> Option<usize> {
    let mut depth = 0;
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            b'\\' if i + 1 < data.len() => {
                match data[i + 1] {
                    b'{' | b'}' | b'\\' => i += 1, // escaped, skip
                    b'b' => {
                        // Check for \bin
                        // Parse number, skip that many bytes
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        i += 1;
    }
    None // Unbalanced
}
```

---

## Expected Tests

- `ole_doc`: Carve DOC file, verify size and type detection.
- `ole_xls`: Carve XLS file, verify size.
- `ole_ppt`: Carve PPT file, verify size.
- `rtf_basic`: Carve simple RTF, verify brace matching.
- `rtf_with_binary`: Carve RTF containing `\bin` data.
- `zip_docx_classification`: Verify DOCX is classified (not just ZIP).
- `zip_odt_classification`: Verify ODT is classified via mimetype.

---

## Impact on Docs/README

- Update file type lists in README.md.
- Add entries to docs/config.md for OLE and RTF validators.
- Document ZIP-based document classification behavior.
- Note that MSG (Outlook) files are OLE compound format.

---

## Open Questions

1. **OLE document type naming:**
   - Output as generic "ole" or specific "doc"/"xls"/"ppt"?
   - Recommendation: Carve as "ole", add `document_type` field in metadata.

2. **ZIP classification granularity:**
   - Keep as "zip" or rename to "docx"/"xlsx"/"odt"/etc.?
   - Recommendation: Update `file_type` to specific format when detected.

3. **Legacy binary formats:**
   - WRI (Windows Write), WK* (Lotus 1-2-3) - worth supporting?
   - Recommendation: Out of scope for now.

4. **Encrypted documents:**
   - OLE and OOXML can be encrypted. How to handle?
   - Recommendation: Carve anyway, mark as encrypted if detectable.
