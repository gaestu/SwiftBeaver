# Video File Carving (AVI, MOV, WEBM, WMV)

Status: Partially Implemented

## Implementation Status

- ✅ **AVI**: Implemented in `src/carve/avi.rs` using shared RIFF module
- ⏳ **MOV**: Not yet implemented (can leverage MP4 box-walking)
- ⏳ **WEBM**: Not yet implemented (requires EBML parsing)
- ⏳ **WMV**: Not yet implemented (requires ASF parsing)

## Problem Statement

The golden image contains video files (AVI, MOV, WEBM, WMV) that are currently not carved. While MP4 is already supported, these additional container formats are commonly encountered in forensic investigations.

## Scope

- Add signature patterns and config entries for AVI, MOV, WEBM, and WMV.
- Implement carve handlers leveraging existing parsing approaches where possible.
- Wire handlers into the carve registry.
- Add unit tests with synthetic/minimal samples.
- Update documentation.

## Non-Goals

- Stream-level validation (codec parsing).
- Fragmented file recovery.
- Metadata extraction (timestamps, duration, etc.).
- Thumbnail generation.

---

## File Format Details

### AVI (Audio Video Interleave)

**Signature:**
- `52 49 46 46` ("RIFF") + 4-byte size + `41 56 49 20` ("AVI ")
- Full pattern: `52 49 46 46 xx xx xx xx 41 56 49 20`

**Size Detection Strategy:**
1. Same as WAV: read RIFF chunk size at offset 4 (LE u32).
2. Total file size = RIFF chunk size + 8.
3. Validate "AVI " form type at offset 8.

**Complexity:** Low - leverages RIFF structure (similar to WAV).

**Note:** Shares RIFF signature with WAV and WEBP. Must check form type at offset 8.

**Config Entry:**
```yaml
- id: "avi"
  extensions: ["avi"]
  header_patterns:
    - id: "avi_riff"
      hex: "52494646"        # "RIFF"
  max_size: 4294967296       # 4 GiB (RIFF size limit)
  min_size: 128
  validator: "avi"
```

### MOV (QuickTime Movie)

**Signature:**
- QuickTime containers use the same atom/box structure as MP4.
- Common patterns at offset 4: `66 74 79 70` ("ftyp"), `6D 6F 6F 76` ("moov"), `77 69 64 65` ("wide"), `66 72 65 65` ("free"), `6D 64 61 74` ("mdat")
- `ftyp` brands: `qt  ` (QuickTime), `M4V `, etc.

**Size Detection Strategy:**
1. Reuse MP4 box-walking logic from existing `mp4.rs`.
2. Walk atoms: each atom has 4-byte size + 4-byte type.
3. Extended size (64-bit) if size field == 1.
4. Look for `moov` atom to validate.
5. End at last complete atom or max_size.

**Complexity:** Medium - can leverage existing MP4 handler with minor adaptations.

**Note:** Consider unifying MP4 and MOV handlers as a generic "ISO base media" handler, or keep them separate but share parsing code.

**Config Entry:**
```yaml
- id: "mov"
  extensions: ["mov", "qt"]
  header_patterns:
    - id: "mov_ftyp_qt"
      hex: "0000001466747970"  # ftyp with 20-byte size
    - id: "mov_moov"
      hex: "6D6F6F76"          # "moov" (legacy, no ftyp)
    - id: "mov_wide"
      hex: "77696465"          # "wide" (legacy)
  max_size: 10737418240       # 10 GiB
  min_size: 16
  validator: "mov"
```

### WEBM (WebM Video)

**Signature:**
- `1A 45 DF A3` - EBML header (shared with Matroska/MKV)
- Following EBML header: DocType element contains "webm" or "matroska"

**Size Detection Strategy:**
1. EBML (Extensible Binary Meta Language) structure.
2. Read EBML header: variable-length element IDs and sizes.
3. After EBML header, expect Segment element (`18 53 80 67`).
4. Segment size (variable-length coded) gives total size.
5. If segment size is "unknown" (-1), walk clusters until EOF or invalid.

**Complexity:** High - EBML variable-length encoding requires careful parsing.

**EBML Variable-Length Integer:**
- First byte indicates length: count leading zeros + 1.
- E.g., `0x81` = 1-byte (value 1), `0x4000` = 2-byte (value 0), etc.

**Config Entry:**
```yaml
- id: "webm"
  extensions: ["webm", "mkv"]
  header_patterns:
    - id: "webm_ebml"
      hex: "1A45DFA3"        # EBML header
  max_size: 10737418240      # 10 GiB
  min_size: 64
  validator: "webm"
```

### WMV (Windows Media Video)

**Signature:**
- ASF (Advanced Systems Format) header GUID:
- `30 26 B2 75 8E 66 CF 11 A6 D9 00 AA 00 62 CE 6C`

**Size Detection Strategy:**
1. ASF header object contains total file size at offset 16 (LE u64, includes header).
2. Actually, the structure is:
   - Bytes 0-15: ASF Header Object GUID
   - Bytes 16-23: Object size (LE u64)
   - Bytes 24-27: Number of header objects (LE u32)
   - Bytes 28-29: Reserved
3. To get total file size: read ASF File Properties Object (GUID `A1 DC AB 8C 47 A9 CF 11 8E E4 00 C0 0C 20 53 65`) within header.
4. File Properties Object contains file size at offset 40 from object start.

**Alternative simple approach:** Use header object size + walk subsequent top-level objects (Data Object, Index Objects) via their size fields.

**Complexity:** Medium - GUID-based objects but sizes are embedded.

**Config Entry:**
```yaml
- id: "wmv"
  extensions: ["wmv", "wma", "asf"]
  header_patterns:
    - id: "wmv_asf"
      hex: "3026B2758E66CF11A6D900AA0062CE6C"  # ASF Header GUID
  max_size: 10737418240      # 10 GiB
  min_size: 64
  validator: "wmv"
```

---

## Implementation Plan

### Phase 1: AVI Handler (Simplest - RIFF reuse)

1. Create `src/carve/avi.rs` or extend RIFF handling:
   - If creating shared RIFF module: factor out common logic from webp.rs.
   - Validate RIFF signature and "AVI " form type.
   - Read size from header.
2. Add config entry.
3. Wire into carve registry.
4. Add unit test.

### Phase 2: MOV Handler (Leverage MP4)

1. Consider options:
   - **Option A:** Add MOV patterns to existing MP4 handler, rename to "isobmff" or "quicktime".
   - **Option B:** Create `src/carve/mov.rs` that reuses MP4 box-walking functions.
2. Extract shared box-walking code from `mp4.rs` if needed.
3. Handle MOV-specific quirks (legacy atoms without ftyp).
4. Add config entries.
5. Add unit tests.

### Phase 3: WMV Handler

1. Create `src/carve/wmv.rs`:
   - Validate ASF header GUID.
   - Parse ASF header to find File Properties Object.
   - Extract file size from File Properties.
2. Handle edge cases (streaming ASF without file size).
3. Add config entry.
4. Add unit tests.

### Phase 4: WEBM Handler (Most Complex)

1. Create `src/carve/webm.rs`:
   - Implement EBML variable-length integer parser.
   - Parse EBML header, validate doctype.
   - Parse Segment element size.
   - If size unknown, walk Clusters.
2. Consider sharing with MKV (same format, different doctype).
3. Add config entry.
4. Add unit tests.

---

## Expected Tests

- `avi_basic`: Carve minimal AVI, verify RIFF size parsing.
- `mov_with_ftyp`: Carve MOV with ftyp atom.
- `mov_legacy`: Carve legacy MOV starting with moov.
- `wmv_basic`: Carve WMV, verify ASF size extraction.
- `webm_known_size`: Carve WEBM with known segment size.
- `webm_unknown_size`: Carve WEBM with streaming/unknown size.

---

## Impact on Docs/README

- Update file type lists in README.md.
- Add entries to docs/config.md for new validators.
- Document RIFF-based format disambiguation.
- Note MOV/MP4 relationship.

---

## Open Questions

1. **RIFF handler unification:**
   - Should we create a generic RIFF handler that dispatches to WAV/AVI/WEBP based on form type?
   - Recommendation: Yes, create `riff.rs` with shared logic, specific handlers validate form type.

2. **MP4/MOV unification:**
   - These are very similar. Should we merge them?
   - Recommendation: Start with separate handlers, share box-walking code, consider merging later if complexity warrants.

3. **MKV vs WEBM:**
   - Same format, different branding. Handle as one type?
   - Recommendation: Single handler, detect doctype, output extension based on content.

4. **Video frame validation:**
   - Should we verify that video frames exist?
   - Recommendation: No, keep it simple. Container validation only.
