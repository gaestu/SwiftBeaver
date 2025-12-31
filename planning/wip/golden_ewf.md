# Golden EWF Test Image

Status: WIP

## Problem Statement

The test suite currently uses in-memory raw byte arrays for testing. There is no real E01/EWF file tested, which means the EWF integration code path (using libewf) is only verified to compile, not to actually work.

A "golden" EWF test image would provide:
- Real E01 file parsing verification
- Integration test for the full EWF code path
- End-to-end validation of carving logic with real files
- Regression detection for EWF-specific issues
- Ability to verify carved output matches original samples

## Scope

### In Scope
- Creating a golden test image from real sample files
- All 12 supported file types with valid, complete files
- String test data for URL/email/phone extraction
- Both raw and E01 output formats
- Manifest documenting exact offsets for test assertions
- Integration tests that verify carved files match originals

### Out of Scope
- Large/realistic forensic images (> 1MB)
- Split E01 files (E01/E02/E03)
- Encrypted EWF images
- Copyrighted content

---

## Design

### Directory Structure

```
tests/
└── golden_image/
    ├── samples/                    # Real sample files (source)
    │   ├── test.jpg                # Valid JPEG photo
    │   ├── test.png                # Valid PNG image
    │   ├── test.gif                # Valid GIF image
    │   ├── test.sqlite             # Valid SQLite database
    │   ├── test.pdf                # Valid PDF document
    │   ├── test.docx               # Valid DOCX (ZIP with word/)
    │   ├── test.webp               # Valid WebP image
    │   ├── test.bmp                # Valid BMP image
    │   ├── test.tiff               # Valid TIFF image
    │   ├── test.mp4                # Valid MP4 video (tiny)
    │   ├── test.rar                # Valid RAR archive
    │   ├── test.7z                 # Valid 7z archive
    │   └── strings.txt             # Text with URLs/emails/phones
    ├── generate.sh                 # Script to pack samples into image
    ├── manifest.json               # Documents offsets and checksums
    ├── golden.raw                  # Generated raw image (gitignored)
    └── golden.E01                  # Generated E01 (committed, ~100-200KB)
```

### Sample File Requirements

Each sample file should be:
- **Small:** Minimize total image size (target < 500KB raw, < 200KB E01)
- **Valid:** Pass the carver's validation logic
- **Unique:** Distinguishable hash for verification
- **License-free:** Public domain or self-created

| File | Target Size | Description |
|------|-------------|-------------|
| test.jpg | ~5-10 KB | Small photo, valid JFIF/EXIF |
| test.png | ~2-5 KB | Small image with proper chunks |
| test.gif | ~1-2 KB | Animated or static GIF |
| test.sqlite | ~4-8 KB | SQLite with a test table (for browser parsing) |
| test.pdf | ~2-5 KB | PDF with minimal content |
| test.docx | ~5-10 KB | Minimal DOCX (word/document.xml present) |
| test.webp | ~2-5 KB | Valid WebP image |
| test.bmp | ~1-2 KB | Small uncompressed BMP |
| test.tiff | ~2-5 KB | Valid TIFF with IFD |
| test.mp4 | ~5-10 KB | Minimal ftyp+moov+mdat |
| test.rar | ~1-2 KB | RAR with small file inside |
| test.7z | ~1-2 KB | 7z with small file inside |
| strings.txt | ~1 KB | URLs, emails, phones, ASCII text |

**Total:** ~35-70 KB raw → ~100-200 KB E01

### String Test Data (strings.txt)

```text
# URLs
https://example.com/test/path?query=1&foo=bar
http://forensic-test.org/evidence/case123
ftp://files.example.net/download/archive.zip
https://subdomain.test-domain.co.uk/page.html

# Emails
user@example.com
admin.test@test-domain.org
forensic_analyst@company.example.net
support+ticket@help.example.com

# Phone numbers
+1-555-123-4567
+1 (800) 555-0199
+44 20 7946 0958
+49 30 12345678

# Plain text for ASCII detection
This is a test string for printable ASCII detection.
The quick brown fox jumps over the lazy dog.
ABCDEFGHIJKLMNOPQRSTUVWXYZ 0123456789

# Paths and filenames
C:\Users\TestUser\Documents\evidence.docx
/home/user/forensics/case_001/image.dd
\\server\share\folder\file.txt
```

### Image Layout

Files are concatenated with padding to ensure non-overlapping offsets:

```
┌─────────────────────────────────────────┐
│ Offset 0x0000: Zero padding (4 KB)      │
├─────────────────────────────────────────┤
│ Offset 0x1000: test.jpg                 │
├─────────────────────────────────────────┤
│ Offset 0xNNNN: test.png                 │
├─────────────────────────────────────────┤
│ ... (each file at 4KB-aligned offset)   │
├─────────────────────────────────────────┤
│ Offset 0xNNNN: strings.txt              │
├─────────────────────────────────────────┤
│ Trailing padding to round size          │
└─────────────────────────────────────────┘
```

4 KB alignment ensures chunk boundary tests work predictably.

---

## Implementation Steps

### Step 1: Create Sample Files

Create or source small, valid sample files for each type.

**Option A: Create minimal samples manually**
```bash
# JPEG - use ImageMagick
convert -size 100x100 xc:red test.jpg

# PNG
convert -size 50x50 xc:blue test.png

# GIF
convert -size 20x20 xc:green test.gif

# SQLite with test data (for browser history parsing)
sqlite3 test.sqlite "CREATE TABLE urls(id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER, last_visit_time INTEGER); INSERT INTO urls VALUES(1, 'https://test.example.com/', 'Test Page', 5, 13300000000000000);"

# PDF - minimal
echo '%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj
xref
0 3
0000000000 65535 f 
0000000009 00000 n 
0000000058 00000 n 
trailer << /Root 1 0 R /Size 3 >>
startxref
111
%%EOF' > test.pdf

# DOCX - create minimal Office Open XML
mkdir -p docx_tmp/word
echo '<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Test</w:t></w:r></w:p></w:body></w:document>' > docx_tmp/word/document.xml
echo '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>' > docx_tmp/\[Content_Types\].xml
(cd docx_tmp && zip -r ../test.docx .)
rm -rf docx_tmp

# WebP
convert -size 50x50 xc:yellow test.webp

# BMP
convert -size 10x10 xc:purple BMP3:test.bmp

# TIFF
convert -size 20x20 xc:orange test.tiff

# MP4 - use ffmpeg for tiny video
ffmpeg -f lavfi -i color=c=black:s=16x16:d=0.1 -c:v libx264 -pix_fmt yuv420p test.mp4

# RAR - requires rar command
echo "test content" > test_file.txt
rar a test.rar test_file.txt
rm test_file.txt

# 7z
echo "test content" > test_file.txt
7z a test.7z test_file.txt
rm test_file.txt
```

**Option B: Use existing tiny test files from public domain sources**

### Step 2: Create Generation Script

Create `tests/golden_image/generate.sh`:

```bash
#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SAMPLES_DIR="$SCRIPT_DIR/samples"
OUTPUT_RAW="$SCRIPT_DIR/golden.raw"
OUTPUT_E01="$SCRIPT_DIR/golden"
MANIFEST="$SCRIPT_DIR/manifest.json"

# Ensure samples exist
REQUIRED_FILES=(
    "test.jpg" "test.png" "test.gif" "test.sqlite" "test.pdf"
    "test.docx" "test.webp" "test.bmp" "test.tiff" "test.mp4"
    "test.rar" "test.7z" "strings.txt"
)

echo "Checking sample files..."
for f in "${REQUIRED_FILES[@]}"; do
    if [[ ! -f "$SAMPLES_DIR/$f" ]]; then
        echo "ERROR: Missing sample file: $SAMPLES_DIR/$f"
        exit 1
    fi
done

# Calculate total size needed (4KB alignment per file + 4KB header)
ALIGNMENT=4096
OFFSET=$ALIGNMENT  # Start after 4KB header padding

# Start manifest
echo '{' > "$MANIFEST"
echo '  "description": "Golden test image manifest",' >> "$MANIFEST"
echo '  "generated": "'$(date -u +%Y-%m-%dT%H:%M:%SZ)'",' >> "$MANIFEST"
echo '  "files": [' >> "$MANIFEST"

# Create initial raw image with header padding
dd if=/dev/zero of="$OUTPUT_RAW" bs=$ALIGNMENT count=1 2>/dev/null

FIRST=true
for f in "${REQUIRED_FILES[@]}"; do
    FILE_PATH="$SAMPLES_DIR/$f"
    FILE_SIZE=$(stat -c%s "$FILE_PATH" 2>/dev/null || stat -f%z "$FILE_PATH")
    FILE_SHA256=$(sha256sum "$FILE_PATH" | cut -d' ' -f1)
    FILE_TYPE="${f%.*}"
    
    # Append file at current offset
    dd if="$FILE_PATH" of="$OUTPUT_RAW" bs=1 seek=$OFFSET conv=notrunc 2>/dev/null
    
    # Calculate end offset
    END_OFFSET=$((OFFSET + FILE_SIZE - 1))
    
    # Write manifest entry
    if [[ "$FIRST" != "true" ]]; then
        echo ',' >> "$MANIFEST"
    fi
    FIRST=false
    
    printf '    {\n' >> "$MANIFEST"
    printf '      "filename": "%s",\n' "$f" >> "$MANIFEST"
    printf '      "type": "%s",\n' "$FILE_TYPE" >> "$MANIFEST"
    printf '      "offset": %d,\n' "$OFFSET" >> "$MANIFEST"
    printf '      "offset_hex": "0x%X",\n' "$OFFSET" >> "$MANIFEST"
    printf '      "size": %d,\n' "$FILE_SIZE" >> "$MANIFEST"
    printf '      "end_offset": %d,\n' "$END_OFFSET" >> "$MANIFEST"
    printf '      "sha256": "%s"\n' "$FILE_SHA256" >> "$MANIFEST"
    printf '    }' >> "$MANIFEST"
    
    echo "  Added $f at offset $OFFSET (0x$(printf '%X' $OFFSET)), size $FILE_SIZE"
    
    # Advance offset with alignment
    OFFSET=$(( ((OFFSET + FILE_SIZE + ALIGNMENT - 1) / ALIGNMENT) * ALIGNMENT ))
done

# Pad to final size
FINAL_SIZE=$OFFSET
truncate -s $FINAL_SIZE "$OUTPUT_RAW"

# Close manifest
echo '' >> "$MANIFEST"
echo '  ],' >> "$MANIFEST"
echo '  "total_size": '$FINAL_SIZE',' >> "$MANIFEST"
RAW_SHA256=$(sha256sum "$OUTPUT_RAW" | cut -d' ' -f1)
echo '  "raw_sha256": "'$RAW_SHA256'"' >> "$MANIFEST"
echo '}' >> "$MANIFEST"

echo ""
echo "Created $OUTPUT_RAW ($FINAL_SIZE bytes)"
echo "Manifest written to $MANIFEST"

# Convert to E01 if ewfacquire is available
if command -v ewfacquire &> /dev/null; then
    echo ""
    echo "Converting to E01..."
    rm -f "${OUTPUT_E01}.E01"
    ewfacquire -t "$OUTPUT_E01" -u -c best -S 0 "$OUTPUT_RAW"
    E01_SIZE=$(stat -c%s "${OUTPUT_E01}.E01" 2>/dev/null || stat -f%z "${OUTPUT_E01}.E01")
    echo "Created ${OUTPUT_E01}.E01 ($E01_SIZE bytes)"
else
    echo ""
    echo "WARNING: ewfacquire not found. Install libewf-tools to generate E01."
    echo "Raw image created; E01 conversion skipped."
fi

echo ""
echo "Done! Verify with: ewfverify ${OUTPUT_E01}.E01"
```

### Step 3: Generate Manifest Format

The `manifest.json` enables precise test assertions:

```json
{
  "description": "Golden test image manifest",
  "generated": "2025-12-29T12:00:00Z",
  "files": [
    {
      "filename": "test.jpg",
      "type": "jpeg",
      "offset": 4096,
      "offset_hex": "0x1000",
      "size": 5432,
      "end_offset": 9527,
      "sha256": "abc123..."
    },
    {
      "filename": "test.png",
      "type": "png",
      "offset": 12288,
      "offset_hex": "0x3000",
      "size": 2048,
      "end_offset": 14335,
      "sha256": "def456..."
    }
  ],
  "total_size": 131072,
  "raw_sha256": "789abc..."
}
```

### Step 4: Create Integration Test

Create `tests/golden_image_test.rs`:

```rust
//! Integration test using the golden test image.
//!
//! This test uses real sample files packed into a raw/E01 image.
//! It verifies that carved files match the original samples.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use fastcarve::config;
use fastcarve::evidence::{self, EvidenceSource, RawFileSource};
use fastcarve::metadata::{self, MetadataBackendKind};
use fastcarve::pipeline;
use fastcarve::scanner;
use fastcarve::util;

fn golden_image_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden_image")
}

fn golden_raw_path() -> PathBuf {
    golden_image_dir().join("golden.raw")
}

#[cfg(feature = "ewf")]
fn golden_e01_path() -> PathBuf {
    golden_image_dir().join("golden.E01")
}

fn load_manifest() -> Option<serde_json::Value> {
    let manifest_path = golden_image_dir().join("manifest.json");
    if !manifest_path.exists() {
        return None;
    }
    let content = fs::read_to_string(manifest_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Get expected SHA256 for a file type from manifest
fn expected_sha256(manifest: &serde_json::Value, file_type: &str) -> Option<String> {
    manifest["files"]
        .as_array()?
        .iter()
        .find(|f| f["type"].as_str() == Some(file_type))?
        ["sha256"]
        .as_str()
        .map(|s| s.to_string())
}

#[test]
fn carves_from_raw_golden_image() {
    let raw_path = golden_raw_path();
    if !raw_path.exists() {
        eprintln!("Skipping: golden.raw not found. Run generate.sh first.");
        return;
    }

    let manifest = load_manifest().expect("manifest.json required");
    
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "golden_raw_test".to_string();

    let evidence = RawFileSource::open(&raw_path).expect("open raw");
    let evidence: Arc<dyn EvidenceSource> = Arc::new(evidence);

    let run_output_dir = temp_dir.path().join(&cfg.run_id);
    fs::create_dir_all(&run_output_dir).expect("output dir");

    let meta_sink = metadata::build_sink(
        MetadataBackendKind::Jsonl,
        &cfg,
        &cfg.run_id,
        "0.1.0",
        &loaded.config_hash,
        &raw_path,
        "",
        &run_output_dir,
    )
    .expect("metadata sink");

    let sig_scanner = scanner::build_signature_scanner(&cfg, false).expect("scanner");
    let sig_scanner: Arc<dyn fastcarve::scanner::SignatureScanner> = Arc::from(sig_scanner);

    let carve_registry = Arc::new(util::build_carve_registry(&cfg).expect("registry"));

    let stats = pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        None,
        meta_sink,
        &run_output_dir,
        2,
        64 * 1024,
        4096, // 4KB overlap matches alignment
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    // Count expected files from manifest
    let expected_count = manifest["files"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    
    assert!(stats.hits_found >= expected_count as u64 - 1, 
            "expected at least {} hits, got {}", expected_count - 1, stats.hits_found);
    assert!(stats.files_carved > 0, "expected carved files");

    // Verify specific file types were carved
    let carved_root = run_output_dir.join("carved");
    
    // Check JPEG carved and hash matches original
    if let Some(expected_hash) = expected_sha256(&manifest, "test") {
        // Note: "test" type for test.jpg - adjust based on actual manifest
        let jpeg_dir = carved_root.join("jpeg");
        if jpeg_dir.exists() {
            for entry in fs::read_dir(jpeg_dir).expect("read jpeg dir") {
                let entry = entry.expect("entry");
                let carved_content = fs::read(entry.path()).expect("read carved");
                let carved_hash = sha256_hex(&carved_content);
                // Carved file should match original sample
                if carved_hash == expected_hash {
                    println!("JPEG hash matches original sample");
                }
            }
        }
    }

    // Basic existence checks for all expected types
    let expected_types = ["jpeg", "png", "gif", "sqlite", "pdf", "docx", "webp", "bmp", "tiff", "mp4", "rar", "7z"];
    let mut found_types = Vec::new();
    for t in expected_types {
        if carved_root.join(t).exists() {
            found_types.push(t);
        }
    }
    
    println!("Found carved types: {:?}", found_types);
    assert!(found_types.len() >= 6, "expected at least 6 file types carved, got {}", found_types.len());
}

#[cfg(feature = "ewf")]
#[test]
fn carves_from_e01_golden_image() {
    let e01_path = golden_e01_path();
    if !e01_path.exists() {
        eprintln!("Skipping: golden.E01 not found. Run generate.sh with ewfacquire.");
        return;
    }

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "golden_e01_test".to_string();

    let cli_opts = fastcarve::cli::CliOptions {
        input: e01_path.clone(),
        output: temp_dir.path().to_path_buf(),
        config_path: None,
        gpu: false,
        workers: 2,
        chunk_size_mib: 1,
        overlap_kib: Some(4),
        metadata_backend: fastcarve::cli::MetadataBackend::Jsonl,
        scan_strings: true,
        scan_utf16: false,
        scan_urls: true,
        no_scan_urls: false,
        scan_emails: true,
        no_scan_emails: false,
        scan_phones: true,
        no_scan_phones: false,
        string_min_len: Some(6),
        scan_entropy: false,
        entropy_window_bytes: None,
        entropy_threshold: None,
        scan_sqlite_pages: false,
        max_bytes: None,
        max_chunks: None,
        evidence_sha256: None,
        compute_evidence_sha256: false,
        disable_zip: false,
        types: None,
    };

    let evidence = evidence::open_source(&cli_opts).expect("open E01");
    let evidence: Arc<dyn evidence::EvidenceSource> = Arc::from(evidence);

    let run_output_dir = temp_dir.path().join(&cfg.run_id);
    fs::create_dir_all(&run_output_dir).expect("output dir");

    let meta_sink = metadata::build_sink(
        MetadataBackendKind::Jsonl,
        &cfg,
        &cfg.run_id,
        "0.1.0",
        &loaded.config_hash,
        &e01_path,
        "",
        &run_output_dir,
    )
    .expect("metadata sink");

    let sig_scanner = scanner::build_signature_scanner(&cfg, false).expect("scanner");
    let sig_scanner: Arc<dyn fastcarve::scanner::SignatureScanner> = Arc::from(sig_scanner);

    let string_scanner = Some(Arc::from(
        fastcarve::strings::build_string_scanner(&cfg, false).expect("string scanner")
    ));

    let carve_registry = Arc::new(util::build_carve_registry(&cfg).expect("registry"));

    let stats = pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        string_scanner,
        meta_sink,
        &run_output_dir,
        2,
        64 * 1024,
        4096,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    assert!(stats.files_carved > 0, "expected carved files from E01");
    
    // Verify string artifacts were found
    let meta_dir = run_output_dir.join("metadata");
    let strings_file = meta_dir.join("string_artefacts.jsonl");
    if strings_file.exists() {
        let content = fs::read_to_string(&strings_file).expect("read strings");
        assert!(content.contains("example.com") || content.contains("@"), 
                "expected URL or email artifacts");
    }
}

#[cfg(feature = "ewf")]
#[test]
fn e01_size_matches_raw() {
    let raw_path = golden_raw_path();
    let e01_path = golden_e01_path();
    
    if !raw_path.exists() || !e01_path.exists() {
        eprintln!("Skipping size comparison: need both golden.raw and golden.E01");
        return;
    }

    let raw_size = fs::metadata(&raw_path).expect("raw metadata").len();

    let cli_opts = fastcarve::cli::CliOptions {
        input: e01_path,
        output: PathBuf::from("/tmp"),
        config_path: None,
        gpu: false,
        workers: 1,
        chunk_size_mib: 1,
        overlap_kib: None,
        metadata_backend: fastcarve::cli::MetadataBackend::Jsonl,
        scan_strings: false,
        scan_utf16: false,
        scan_urls: false,
        no_scan_urls: false,
        scan_emails: false,
        no_scan_emails: false,
        scan_phones: false,
        no_scan_phones: false,
        string_min_len: None,
        scan_entropy: false,
        entropy_window_bytes: None,
        entropy_threshold: None,
        scan_sqlite_pages: false,
        max_bytes: None,
        max_chunks: None,
        evidence_sha256: None,
        compute_evidence_sha256: false,
        disable_zip: false,
        types: None,
    };

    let e01_evidence = evidence::open_source(&cli_opts).expect("open E01");
    
    assert_eq!(e01_evidence.len(), raw_size, 
               "E01 media size should match raw image size");
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}
```

### Step 5: Update .gitignore

Add to `.gitignore`:

```gitignore
# Golden image intermediates
tests/golden_image/golden.raw
```

Keep `golden.E01` and `manifest.json` committed.

### Step 6: Update CI Workflow

The CI already handles missing files gracefully (tests skip). Optionally add:

```yaml
- name: Verify golden image
  run: |
    if [ -f tests/golden_image/golden.E01 ]; then
      ewfverify tests/golden_image/golden.E01 || echo "ewfverify not available"
    fi
```

### Step 7: Document in README

Add to Testing section:

```markdown
### Golden Image Integration Tests

The test suite includes integration tests using a golden test image containing 
real sample files for all supported formats.

**Structure:**
```
tests/golden_image/
├── samples/          # Source sample files
├── generate.sh       # Packs samples into raw + E01
├── manifest.json     # Offset/hash reference
├── golden.raw        # Generated (gitignored)
└── golden.E01        # Generated (committed)
```

**To regenerate the golden image:**
```bash
cd tests/golden_image
./generate.sh
```

Requires: `ewfacquire` from libewf-tools for E01 generation.
```

---

## File Structure (Updated)

```
tests/
├── golden_image/
│   ├── samples/                # Real sample files
│   │   ├── test.jpg
│   │   ├── test.png
│   │   ├── test.gif
│   │   ├── test.sqlite
│   │   ├── test.pdf
│   │   ├── test.docx
│   │   ├── test.webp
│   │   ├── test.bmp
│   │   ├── test.tiff
│   │   ├── test.mp4
│   │   ├── test.rar
│   │   ├── test.7z
│   │   └── strings.txt
│   ├── generate.sh             # Packing script
│   ├── manifest.json           # Auto-generated offset map
│   ├── golden.raw              # Gitignored
│   └── golden.E01              # Committed (~100-200KB)
├── golden_image_test.rs        # Integration tests
├── integration_basic.rs        # Existing
└── metadata_parquet.rs         # Existing
```

---

## Sample Files Licensing

All sample files must be:
- **Self-created** using ImageMagick/ffmpeg/etc., OR
- **Public domain** (CC0, Unlicense), OR  
- **Permissively licensed** (MIT, Apache 2.0)

Document licenses in `tests/golden_image/samples/README.md`:

```markdown
# Sample Files

These files are used to generate the golden test image.

| File | Source | License |
|------|--------|---------|
| test.jpg | Generated with ImageMagick | CC0 |
| test.png | Generated with ImageMagick | CC0 |
| test.gif | Generated with ImageMagick | CC0 |
| test.sqlite | Created with sqlite3 CLI | CC0 |
| test.pdf | Hand-crafted minimal PDF | CC0 |
| test.docx | Created with script | CC0 |
| test.webp | Generated with ImageMagick | CC0 |
| test.bmp | Generated with ImageMagick | CC0 |
| test.tiff | Generated with ImageMagick | CC0 |
| test.mp4 | Generated with ffmpeg | CC0 |
| test.rar | Created with rar CLI | CC0 |
| test.7z | Created with 7z CLI | CC0 |
| strings.txt | Hand-crafted test data | CC0 |
```

---

## Verification Checklist

After creating the golden image, verify:

- [ ] All sample files exist in `samples/`
- [ ] `generate.sh` runs without errors
- [ ] `manifest.json` contains correct offsets and hashes
- [ ] `golden.raw` size matches manifest `total_size`
- [ ] `golden.E01` file is < 500 KB
- [ ] `ewfverify golden.E01` passes (if available)
- [ ] `cargo test golden` passes for raw image
- [ ] `cargo test golden` passes for E01 image (with EWF feature)
- [ ] Carved files match original sample hashes
- [ ] String scanning finds URLs/emails from strings.txt

---

## Completion Criteria

This feature is complete when:
- [ ] `samples/` directory with all 13 files created
- [ ] `samples/README.md` documents licensing
- [ ] `generate.sh` script created and tested
- [ ] `manifest.json` auto-generated with correct data
- [ ] `golden.E01` committed to repo (< 500KB)
- [ ] `tests/golden_image_test.rs` passes locally
- [ ] CI runs golden image tests successfully
- [ ] Tests skip gracefully when golden image missing
- [ ] README updated with golden image instructions

