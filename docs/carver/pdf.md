# PDF Carver

## Overview

The PDF carver extracts Portable Document Format files by detecting the header signature and searching for the `%%EOF` trailer marker that indicates the end of the document.

## Signature Detection

**Header Pattern**: `%PDF-` (ASCII bytes: `25 50 44 46 2D`)

Typically followed by version (e.g., `%PDF-1.4`, `%PDF-1.7`), but the carver only requires the `%PDF-` prefix.

## Carving Algorithm

The PDF carver uses a streaming search with a sliding window:

### 1. Signature Validation
```rust
Verify first 5 bytes match "%PDF-"
If mismatch → return invalid (remove file)
```

### 2. Streaming Search for EOF Marker

The carver searches for `%%EOF` (bytes: `25 25 45 4F 46`) using a carry-over buffer:

```
Initialize carry buffer (empty)
Loop:
  Read up to 64KB chunk from evidence
  Combine: search_buf = carry + chunk
  Search for "%%EOF" in search_buf
  If found:
    Calculate how many bytes from chunk to write
    Write partial chunk
    Mark as validated
    Break
  If not found:
    Write entire chunk
    Update carry = last 4 bytes of search_buf
    Continue
```

### Carry Buffer Logic

The carry buffer ensures `%%EOF` is detected even when it spans chunk boundaries:

```
Chunk 1: "...trailer\nstartxr"
Chunk 2: "ef\n1234\n%%EOF\n"
                  ^-- Would miss without carry

With carry:
Carry: "txr"
Search buf: "txr" + "ef\n1234\n%%EOF\n"
Found at position in combined buffer
```

## Validation

- **Validated**: `true` if `%%EOF` marker is found
- **Truncated**: `true` if max_size or EOF reached before `%%EOF`
- **Invalid**: Removed if header doesn't match `%PDF-`

## Size Constraints

- **Default min_size**: 16 bytes (configurable)
- **Default max_size**: 500 MB
- Minimum viable PDF: `%PDF-1.0\n%%EOF\n` (~15 bytes)
- Files below min_size are discarded

## Hash Computation

- **MD5**: Computed incrementally as chunks are written
- **SHA-256**: Computed incrementally as chunks are written
- Hashes cover data from header to end of `%%EOF` (inclusive)

## Testing

**Test file**: `tests/carver_pdf.rs`

### Test Strategy

Golden image framework:

1. **Test PDFs**: Multiple PDF versions in golden image
   - PDF 1.4 (older version)
   - PDF 1.7 (modern version)
   - PDFs with images
   - PDFs with fonts
   - Linearized PDFs
2. **Manifest verification**:
   - All PDFs found at expected offsets
   - Sizes match exactly
   - All marked as validated (%%EOF found)
3. **Validation**: Optionally verify PDFs open in viewers

### Example Test

```rust
#[test]
fn test_pdf_carver() {
    let config = default_config();
    let (metadata, _) = carver_for_types(&["pdf"], &config);
    verify_manifest_match(metadata, "pdf");
}
```

## Edge Cases Handled

1. **%%EOF in PDF stream data**: May cause early termination (known limitation)
2. **Multiple %%EOF markers**: Stops at first occurrence
3. **Linearized PDFs**: Handles alternate structure (header at end)
4. **Incremental updates**: Includes all updates (multiple trailers)
5. **Large embedded files**: Truncates at max_size if needed
6. **Missing %%EOF**: Keeps file as truncated if substantive data exists

## Performance Characteristics

- **Streaming**: Direct write to output file (no buffering entire PDF)
- **Memory usage**: Constant (~64KB chunk buffer)
- **I/O pattern**: Sequential 64KB reads
- **Search efficiency**: Boyer-Moore-style search could optimize (currently linear)

## Forensic Considerations

- **Damaged PDFs**: Preserves files missing %%EOF for manual recovery
- **Encrypted PDFs**: Carves complete file (decryption is not performed)
- **JavaScript/embedded content**: Retains all embedded objects
- **Metadata preservation**: Includes PDF info dictionary and XMP metadata

## PDF Structure Overview

```
%PDF-1.7
%âãÏÓ
1 0 obj
<<
  /Type /Catalog
  /Pages 2 0 R
>>
endobj

2 0 obj
<<
  /Type /Pages
  /Kids [3 0 R]
  /Count 1
>>
endobj

3 0 obj
<<
  /Type /Page
  /Parent 2 0 R
  /Resources <<
    /Font <<
      /F1 4 0 R
    >>
  >>
  /Contents 5 0 R
>>
endobj

... (more objects)

xref
0 6
0000000000 65535 f 
0000000015 00000 n 
...
trailer
<<
  /Size 6
  /Root 1 0 R
>>
startxref
1234
%%EOF
```

## Known Limitations

1. **Embedded %%EOF in streams**: If `%%EOF` appears in compressed stream data, carving may terminate early. This is a known PDF carving challenge.
2. **No xref validation**: Does not validate cross-reference table consistency
3. **No object parsing**: Treats PDF as byte stream (doesn't parse object structure)

## Related Carvers

- **DOC/OLE**: Office documents (different structure)
- **RTF**: Rich Text Format (simpler text-based format)
- **EML**: Email format (also text-based with markers)
