# HEIC/HEIF Image Carver

**Status:** WIP  
**Priority:** High  
**Effort:** Low  

---

## Problem Statement

HEIC (High Efficiency Image Container) and HEIF (High Efficiency Image Format) are the default photo formats on modern iOS devices (iPhone 7+, iOS 11+) and increasingly on Android. When analyzing mobile device images or cloud backups, HEIC/HEIF files are extremely common.

Currently, `SwiftBeaver` cannot carve these files, leaving a significant gap in mobile forensics capabilities.

---

## Scope

### In Scope

1. **Detect HEIC/HEIF files** by their ISO Base Media File Format (ISOBMFF) signatures
2. **Carve complete HEIC/HEIF files** using box-based structure parsing
3. **Handle common variants:**
   - `.heic` — HEIC image container
   - `.heif` — HEIF image container  
   - `.hif` — Alternative extension
4. **Support both single images and image sequences**
5. **Metadata recording** in all backends

### Out of Scope

- Thumbnail extraction from HEIC files
- EXIF metadata parsing (can be added later)
- Conversion to JPEG (not a carver responsibility)
- AVIF carving (separate feature, similar structure)

---

## Design Notes

### File Format Structure

HEIC/HEIF uses ISO Base Media File Format (ISOBMFF), same as MP4/MOV:

```
[ftyp box] - File type, brand identifies HEIC/HEIF
[meta box] - Metadata
[mdat box] - Media data (image pixels)
```

**Key ftyp brands:**
| Brand | Meaning |
|-------|---------|
| `heic` | HEIC image |
| `heix` | HEIC image extended |
| `hevc` | HEVC video (can contain images) |
| `hevx` | HEVC extended |
| `heim` | HEIC image sequence |
| `heis` | HEIC image sequence |
| `mif1` | HEIF image |
| `msf1` | HEIF image sequence |

### Header Signatures

Unlike MP4 which has variable ftyp sizes, HEIC commonly uses:

```
Offset 4-7: "ftyp"
Offset 8-11: brand (heic, mif1, etc.)
```

Common patterns:
- `00 00 00 18 66 74 79 70 68 65 69 63` — ftyp size 24, brand "heic"
- `00 00 00 18 66 74 79 70 6D 69 66 31` — ftyp size 24, brand "mif1"
- `00 00 00 1C 66 74 79 70 68 65 69 63` — ftyp size 28, brand "heic"

### Size Determination

Like MP4, parse sequential boxes until EOF or invalid box:

```rust
fn determine_heic_size(data: &[u8]) -> Option<u64> {
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let box_size = read_u32_be(&data[pos..]) as u64;
        let box_type = &data[pos+4..pos+8];
        
        if box_size == 0 {
            // Box extends to EOF - need external size limit
            return None; 
        }
        if box_size == 1 {
            // Extended size in next 8 bytes
            let extended_size = read_u64_be(&data[pos+8..]);
            pos += extended_size as usize;
        } else {
            pos += box_size as usize;
        }
    }
    Some(pos as u64)
}
```

### Validation

1. Verify ftyp box is first
2. Verify brand is HEIC/HEIF-related
3. Verify meta box exists (contains item info)
4. Verify mdat box exists (contains image data)
5. Optional: verify box sizes don't exceed max_size

---

## Implementation Plan

### Phase 1: Add File Type Configuration

1. **Update `config/default.yml`:**
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
         hex: "000000186674797 06D696631"
       - id: "mif1_ftyp_1c"
         hex: "0000001C6674797 06D696631"
     footer_patterns: []
     max_size: 104857600  # 100 MB
     min_size: 100
     validator: "heic"
   ```

### Phase 2: Implement Carver

2. **Create `src/carve/heic.rs`:**
   ```rust
   //! HEIC/HEIF image carver
   //! 
   //! Carves HEIC and HEIF images using ISOBMFF box structure.
   
   use crate::carve::{CarveHandler, CarveResult, CarveError};
   
   pub struct HeicCarver;
   
   impl CarveHandler for HeicCarver {
       fn validate_and_carve(
           &self,
           data: &[u8],
           offset: u64,
           max_size: u64,
       ) -> Result<CarveResult, CarveError> {
           // 1. Verify ftyp box and brand
           // 2. Parse boxes to determine size
           // 3. Validate required boxes exist
           // 4. Return carved data
       }
   }
   ```

3. **Leverage existing ISOBMFF code:**
   - The MP4 and MOV carvers already parse ISOBMFF boxes
   - Extract common box-parsing logic to `src/carve/isobmff.rs`
   - Share between MP4, MOV, and HEIC carvers

### Phase 3: Register Carver

4. **Update `src/carve/mod.rs`:**
   - Add `pub mod heic;`
   - Register HeicCarver in CarveRegistry for "heic" validator

### Phase 4: Testing

5. **Create `tests/carver_heic.rs`:**
   - Test with minimal valid HEIC (can construct programmatically)
   - Test brand detection (heic, mif1, etc.)
   - Test truncated file handling
   - Test max_size enforcement

6. **Add sample HEIC to test resources:**
   - Create minimal HEIC test files or use public domain samples

### Phase 5: Documentation

7. **Update README.md:**
   - Add heic to carved file types list

8. **Update docs/architecture.md:**
   - Mention HEIC support

---

## Expected Tests

- `tests/carver_heic.rs`:
  - `test_heic_basic_carve` — carve valid HEIC file
  - `test_heic_mif1_brand` — carve HEIF with mif1 brand
  - `test_heic_truncated` — handle truncated gracefully
  - `test_heic_max_size` — enforce size limits
  - `test_heic_invalid_brand` — reject non-HEIC ISOBMFF

---

## Impact on Docs and README

- **README.md:** Add `heic` to the carved file types list in output section
- **docs/architecture.md:** Add HEIC to supported formats
- **config/default.yml:** Add heic file type configuration

---

## Open Questions

1. Should we also add AVIF in the same PR (very similar format)?
2. Should we extract a shared ISOBMFF module for MP4/MOV/HEIC/AVIF?
3. Any interest in extracting embedded thumbnails (HEIC often contains JPEG thumbnails)?
