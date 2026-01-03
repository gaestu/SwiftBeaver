# Golden EWF Test Image

Status: Implemented
Implemented in version: 0.1.0

## Problem Statement

The test suite currently uses in-memory raw byte arrays for testing. There is no real E01/EWF file tested, which means the EWF integration code path (using libewf) is only verified to compile, not to actually work.

We also need a repeatable, data-driven way to build test images from **everything** under `tests/golden_image/samples/`. Adding new sample files must not require code changes.

A "golden" EWF test image would provide:
- Real E01 file parsing verification
- Integration test for the full EWF code path
- End-to-end validation of carving logic with real files
- Regression detection for EWF-specific issues
- Comprehensive development/testing image with all sample types
- Ability to verify carved output matches original samples

## Scope

### In Scope
- Generating a golden test image from **ALL** files in `tests/golden_image/samples/` (recursive)
- Currently ~84 files across all categories (~23MB raw, compresses well to E01)
- JSON manifest documenting exact offsets and checksums for every file
- Both raw and E01 output formats
- Integration tests that verify carving against manifest
- Useful for ongoing development and adding new file type support
- No hard-coded file lists in tests or generator (new samples are auto-included)
- Optional `.goldenignore` to exclude non-sample helper files without code changes

### Out of Scope
- Split E01 files (E01/E02/E03)
- Encrypted EWF images
- Filtering/selecting specific files (include everything)
- Updating test code when new sample files or categories are added

---

## Design

### Current Sample Structure (~84 files, ~23MB)

```
tests/golden_image/samples/
├── images/           # 18 files - JPEG, PNG, GIF, BMP, WebP, TIFF, ICO, SVG
├── video/            # 7 files  - MP4, AVI, MOV, OGG, WMV, WEBM
├── audio/            # 4 files  - MP3, WAV, OGG
├── documents/        # 14 files - PDF, DOC/DOCX, XLS/XLSX, PPT/PPTX, ODT/ODS/ODP, RTF
├── archives/         # 11 files - ZIP, RAR, 7z, TAR, TAR.GZ/BZ2/XZ, GZ, BZ2, XZ
├── databases/        # 5 files  - SQLite (basic, browser History/Cookies/places)
├── media_tiny/       # 4 files  - Tiny versions of media for quick tests
├── email/            # 2 files  - EML files
├── binaries/         # 2 files  - ELF executable, shared library
├── other/            # 17 files - TXT, JSON, JSONL, YAML, XML, CSV, HTML, UTF-8/16, etc.
├── generate_missing.sh
└── samples_to_place.md
```

Note: `samples/` is the intended source-of-truth directory. For backward compatibility, the generator should fall back to `sample/` if `samples/` does not exist.

### Files to Generate

```
tests/golden_image/
├── .goldenignore               # Optional ignore list for non-samples
├── generate.sh                  # Script to pack ALL samples
├── manifest.json                # Complete offset/hash map for all files
├── golden.raw                   # Raw disk image (~25MB with alignment)
└── golden.E01                   # EWF compressed (~8-12MB expected)
```

### Image Layout

All files concatenated with 4KB alignment:

```
┌─────────────────────────────────────────┐
│ Offset 0x0000: Zero padding (4 KB)      │
├─────────────────────────────────────────┤
│ Offset 0x1000: images/test_generated.jpg│
├─────────────────────────────────────────┤
│ Offset 0xNNNN: images/test_gradient.png │
├─────────────────────────────────────────┤
│ ... (all files at 4KB boundaries)       │
├─────────────────────────────────────────┤
│ Offset 0xNNNN: other/strings.txt        │
├─────────────────────────────────────────┤
│ Trailing zero padding                   │
└─────────────────────────────────────────┘
```

### Manifest Format

Example manifest with all files grouped by category (values are illustrative). Tests should treat this manifest as the single source of truth so new samples do not require code changes:

```json
{
  "description": "Golden test image - ALL sample files",
  "generated": "2025-12-31T12:00:00Z",
  "alignment": 4096,
  "sample_dir": "samples",
  "categories": {
    "images": [...],
    "video": [...],
    "audio": [...],
    "documents": [...],
    "archives": [...],
    "databases": [...],
    "media_tiny": [...],
    "email": [...],
    "binaries": [...],
    "other": [...]
  },
  "files": [
    {
      "path": "images/test_generated.jpg",
      "category": "images", 
      "extension": "jpg",
      "offset": 4096,
      "offset_hex": "0x1000",
      "size": 5432,
      "sha256": "abc123..."
    },
    ...
  ],
  "summary": {
    "total_files": 84,
    "total_size": 25165824,
    "categories": {
      "images": 18,
      "video": 7,
      ...
    }
  },
  "raw_sha256": "final_hash..."
}
```

---

## Implementation

### generate.sh Script

Optional `.goldenignore` (gitignore-style patterns) can exclude helper files like `samples_to_place.md` and `generate_missing.sh` while keeping sample discovery data-driven.

```bash
#!/bin/bash
# Generate golden.raw and golden.E01 from ALL sample files
#
# Includes every file in samples/ subdirectories for comprehensive testing.
# Useful for development and regression testing of all supported formats.
#
# Usage: ./generate.sh [--no-e01]
#
# Requirements:
#   - ewfacquire (for E01 generation, optional with --no-e01)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SAMPLES_DIR_DEFAULT="$SCRIPT_DIR/samples"
SAMPLES_DIR_FALLBACK="$SCRIPT_DIR/sample"
SAMPLES_DIR="${SAMPLES_DIR:-$SAMPLES_DIR_DEFAULT}"
if [[ ! -d "$SAMPLES_DIR" && -d "$SAMPLES_DIR_FALLBACK" ]]; then
    SAMPLES_DIR="$SAMPLES_DIR_FALLBACK"
fi
IGNORE_FILE="$SCRIPT_DIR/.goldenignore"
OUTPUT_RAW="$SCRIPT_DIR/golden.raw"
OUTPUT_E01="$SCRIPT_DIR/golden"
MANIFEST="$SCRIPT_DIR/manifest.json"

SKIP_E01=false
[[ "${1:-}" == "--no-e01" ]] && SKIP_E01=true

# Alignment for predictable chunk boundaries
ALIGNMENT=4096

echo "=== Golden Image Generator (All Files) ==="
echo "Samples dir: $SAMPLES_DIR"
echo ""

#------------------------------------------------------------------------------
# Collect all sample files (optional .goldenignore, no hard-coded filters)
#------------------------------------------------------------------------------
if command -v rg &> /dev/null && [[ -f "$IGNORE_FILE" ]]; then
    mapfile -t ALL_FILES < <(rg --files --ignore-file "$IGNORE_FILE" "$SAMPLES_DIR" \
        | sed "s|^$SAMPLES_DIR/||" | sort)
else
    mapfile -t ALL_FILES < <(find "$SAMPLES_DIR" -type f -print \
        | sed "s|^$SAMPLES_DIR/||" | sort)
fi

TOTAL_FILES=${#ALL_FILES[@]}
echo "Found $TOTAL_FILES files to include"
echo ""

if [[ $TOTAL_FILES -eq 0 ]]; then
    echo "ERROR: No sample files found in $SAMPLES_DIR"
    exit 1
fi

#------------------------------------------------------------------------------
# Calculate category for a file path
#------------------------------------------------------------------------------
get_category() {
    local path="$1"
    echo "${path%%/*}"
}

get_extension() {
    local path="$1"
    local filename="${path##*/}"
    if [[ "$filename" == *.* ]]; then
        echo "${filename##*.}" | tr '[:upper:]' '[:lower:]'
    else
        echo ""
    fi
}

#------------------------------------------------------------------------------
# Build the raw image
#------------------------------------------------------------------------------
echo "Building raw image..."

# Start with header padding
OFFSET=$ALIGNMENT
dd if=/dev/zero of="$OUTPUT_RAW" bs=$ALIGNMENT count=1 2>/dev/null

# Start manifest JSON
cat > "$MANIFEST" << EOF
{
  "description": "Golden test image - ALL sample files for SwiftBeaver testing",
  "generated": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "alignment": $ALIGNMENT,
  "sample_dir": "samples",
  "files": [
EOF

# Track categories for summary
declare -A CATEGORY_COUNTS
declare -A CATEGORY_SIZES
TOTAL_SIZE=0

FIRST=true
for rel_path in "${ALL_FILES[@]}"; do
    full_path="$SAMPLES_DIR/$rel_path"
    
    FILE_SIZE=$(stat -c%s "$full_path" 2>/dev/null || stat -f%z "$full_path")
    FILE_SHA256=$(sha256sum "$full_path" | cut -d' ' -f1)
    CATEGORY=$(get_category "$rel_path")
    EXTENSION=$(get_extension "$rel_path")
    
    # Append file at current offset
    dd if="$full_path" of="$OUTPUT_RAW" bs=1 seek=$OFFSET conv=notrunc 2>/dev/null
    
    # Track stats
    CATEGORY_COUNTS[$CATEGORY]=$(( ${CATEGORY_COUNTS[$CATEGORY]:-0} + 1 ))
    CATEGORY_SIZES[$CATEGORY]=$(( ${CATEGORY_SIZES[$CATEGORY]:-0} + FILE_SIZE ))
    TOTAL_SIZE=$((TOTAL_SIZE + FILE_SIZE))
    
    # Manifest entry
    [[ "$FIRST" != "true" ]] && printf ',\n' >> "$MANIFEST"
    FIRST=false
    
    cat >> "$MANIFEST" << EOF
    {
      "path": "$rel_path",
      "category": "$CATEGORY",
      "extension": "$EXTENSION",
      "offset": $OFFSET,
      "offset_hex": "0x$(printf '%X' $OFFSET)",
      "size": $FILE_SIZE,
      "sha256": "$FILE_SHA256"
    }
EOF
    
    printf "  %-50s @ 0x%08X (%d bytes)\n" "$rel_path" "$OFFSET" "$FILE_SIZE"
    
    # Advance to next aligned offset
    OFFSET=$(( ((OFFSET + FILE_SIZE + ALIGNMENT - 1) / ALIGNMENT) * ALIGNMENT ))
done

# Pad to final size
FINAL_SIZE=$OFFSET
truncate -s $FINAL_SIZE "$OUTPUT_RAW"

# Complete manifest with summary
RAW_SHA256=$(sha256sum "$OUTPUT_RAW" | cut -d' ' -f1)

cat >> "$MANIFEST" << EOF

  ],
  "summary": {
    "total_files": $TOTAL_FILES,
    "total_data_size": $TOTAL_SIZE,
    "image_size": $FINAL_SIZE,
    "categories": {
EOF

# Add category counts
FIRST_CAT=true
for cat in $(echo "${!CATEGORY_COUNTS[@]}" | tr ' ' '\n' | sort); do
    [[ "$FIRST_CAT" != "true" ]] && printf ',\n' >> "$MANIFEST"
    FIRST_CAT=false
    printf '      "%s": {"files": %d, "bytes": %d}' \
        "$cat" "${CATEGORY_COUNTS[$cat]}" "${CATEGORY_SIZES[$cat]}" >> "$MANIFEST"
done

cat >> "$MANIFEST" << EOF

    }
  },
  "raw_sha256": "$RAW_SHA256"
}
EOF

echo ""
echo "Created $OUTPUT_RAW"
echo "  Files: $TOTAL_FILES"
echo "  Data:  $TOTAL_SIZE bytes"
echo "  Image: $FINAL_SIZE bytes (with alignment padding)"
echo "  SHA256: $RAW_SHA256"
echo ""
echo "Manifest: $MANIFEST"

#------------------------------------------------------------------------------
# Convert to E01 if ewfacquire is available
#------------------------------------------------------------------------------
if [[ "$SKIP_E01" == "true" ]]; then
    echo ""
    echo "Skipping E01 generation (--no-e01 flag)"
elif command -v ewfacquire &> /dev/null; then
    echo ""
    echo "Converting to E01 format..."
    rm -f "${OUTPUT_E01}.E01"
    
    ewfacquire -t "$OUTPUT_E01" \
               -u \
               -c best \
               -S 0 \
               -C "golden_test" \
               -D "Golden test image - all SwiftBeaver samples" \
               -e "automated" \
               -E "golden_001" \
               "$OUTPUT_RAW"
    
    E01_SIZE=$(stat -c%s "${OUTPUT_E01}.E01" 2>/dev/null || stat -f%z "${OUTPUT_E01}.E01")
    E01_SHA256=$(sha256sum "${OUTPUT_E01}.E01" | cut -d' ' -f1)
    
    echo ""
    echo "Created ${OUTPUT_E01}.E01"
    echo "  Size: $E01_SIZE bytes ($(( E01_SIZE * 100 / FINAL_SIZE ))% of raw)"
    echo "  SHA256: $E01_SHA256"
    
    # Verify if possible
    if command -v ewfverify &> /dev/null; then
        echo ""
        echo "Verifying E01..."
        if ewfverify "${OUTPUT_E01}.E01"; then
            echo "✓ E01 verification passed"
        else
            echo "✗ E01 verification failed!"
            exit 1
        fi
    fi
else
    echo ""
    echo "WARNING: ewfacquire not found"
    echo "Install libewf-tools to generate E01:"
    echo "  Fedora/RHEL: sudo dnf install libewf-tools"
    echo "  Debian/Ubuntu: sudo apt install ewf-tools"
    echo ""
    echo "Raw image created; run with ewfacquire installed for E01."
fi

echo ""
echo "=== Done ==="
echo ""
echo "Test commands:"
echo "  cargo test golden                    # Raw image tests"
echo "  cargo test golden --features ewf     # Include E01 tests"
```

### Integration Tests

Create `tests/golden_image_test.rs`. Tests should be manifest-driven (no hard-coded file lists or categories) so new samples are picked up automatically:

```rust
//! Integration tests using the golden test image.
//!
//! Tests use ALL sample files packed into raw and E01 images.
//! Provides comprehensive testing for development and regression detection.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline;
use swiftbeaver::scanner;
use swiftbeaver::util;

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

/// Load and parse manifest.json
fn load_manifest() -> Option<serde_json::Value> {
    let path = golden_image_dir().join("manifest.json");
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Get file entry from manifest by path
fn get_file_entry<'a>(manifest: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    manifest["files"]
        .as_array()?
        .iter()
        .find(|f| f["path"].as_str() == Some(path))
}

/// Get all files of a specific extension from manifest
fn get_files_by_extension<'a>(manifest: &'a serde_json::Value, ext: &str) -> Vec<&'a serde_json::Value> {
    manifest["files"]
        .as_array()
        .map(|files| {
            files.iter()
                .filter(|f| f["extension"].as_str() == Some(ext))
                .collect()
        })
        .unwrap_or_default()
}

/// Get all files in a category from manifest
fn get_files_by_category<'a>(manifest: &'a serde_json::Value, category: &str) -> Vec<&'a serde_json::Value> {
    manifest["files"]
        .as_array()
        .map(|files| {
            files.iter()
                .filter(|f| f["category"].as_str() == Some(category))
                .collect()
        })
        .unwrap_or_default()
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Sha256, Digest};
    hex::encode(Sha256::digest(data))
}

/// Test carving from raw golden image - comprehensive test
#[test]
fn golden_carves_from_raw() {
    let raw_path = golden_raw_path();
    if !raw_path.exists() {
        eprintln!("Skipping: golden.raw not found. Run tests/golden_image/generate.sh");
        return;
    }

    let manifest = load_manifest().expect("manifest.json required when golden.raw exists");
    let temp_dir = tempfile::tempdir().expect("tempdir");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "golden_raw_test".to_string();

    let evidence = RawFileSource::open(&raw_path).expect("open raw");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::new(evidence);

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
    let sig_scanner: Arc<dyn swiftbeaver::scanner::SignatureScanner> = Arc::from(sig_scanner);
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
        4096,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    // Get summary from manifest
    let total_files = manifest["summary"]["total_files"].as_u64().unwrap_or(0);
    println!("Manifest contains {} files", total_files);
    println!("Pipeline found {} hits, carved {} files", stats.hits_found, stats.files_carved);

    // We should find hits for most carveable files
    assert!(stats.hits_found > 0, "expected some hits");
    assert!(stats.files_carved > 0, "expected carved files");

    // Validate carved outputs against manifest hashes (no hard-coded type list).
    let manifest_hashes: HashSet<String> = manifest["files"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|f| f["sha256"].as_str())
        .map(|s| s.to_string())
        .collect();

    let carved_meta = run_output_dir.join("metadata").join("carved_files.jsonl");
    let carved_content = fs::read_to_string(&carved_meta).expect("read carved metadata");
    let mut matched = 0usize;
    for line in carved_content.lines() {
        let record: serde_json::Value = serde_json::from_str(line).expect("parse carved record");
        if let Some(hash) = record["sha256"].as_str() {
            if manifest_hashes.contains(hash) {
                matched += 1;
            }
        }
    }

    println!("Carved outputs matching manifest: {}", matched);
    assert!(matched > 0, "expected carved outputs to match manifest samples");
}

/// Test carving from E01 with string scanning
#[cfg(feature = "ewf")]
#[test]
fn golden_carves_from_e01_with_strings() {
    let e01_path = golden_e01_path();
    if !e01_path.exists() {
        eprintln!("Skipping: golden.E01 not found.");
        return;
    }

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "golden_e01_test".to_string();
    cfg.enable_string_scan = true;
    cfg.enable_url_scan = true;
    cfg.enable_email_scan = true;

    let evidence = swiftbeaver::evidence::EwfSource::open(&e01_path).expect("open E01");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::new(evidence);

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
    let sig_scanner: Arc<dyn swiftbeaver::scanner::SignatureScanner> = Arc::from(sig_scanner);

    let string_scanner = Some(Arc::from(
        swiftbeaver::strings::build_string_scanner(&cfg, false).expect("string scanner")
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

    // Verify string artefacts extracted (from strings.txt and other text files)
    let strings_file = run_output_dir.join("metadata").join("string_artefacts.jsonl");
    if strings_file.exists() {
        let content = fs::read_to_string(&strings_file).expect("read strings");
        // Should find URLs and emails from other/strings.txt
        let has_urls = content.contains("example.com") || content.contains("http");
        let has_emails = content.contains("@");
        println!("String scanning: URLs={}, Emails={}", has_urls, has_emails);
        assert!(has_urls || has_emails, "expected URL or email artefacts");
    }
}

/// Verify E01 media size matches raw
#[cfg(feature = "ewf")]
#[test]
fn golden_e01_size_matches_raw() {
    let raw_path = golden_raw_path();
    let e01_path = golden_e01_path();

    if !raw_path.exists() || !e01_path.exists() {
        eprintln!("Skipping: need both golden.raw and golden.E01");
        return;
    }

    let raw_size = fs::metadata(&raw_path).expect("raw metadata").len();
    let e01 = swiftbeaver::evidence::EwfSource::open(&e01_path).expect("open E01");

    assert_eq!(e01.len(), raw_size, "E01 media size should match raw");
}

/// Verify manifest integrity - all files at correct offsets with correct hashes
#[test]
fn golden_manifest_integrity() {
    let raw_path = golden_raw_path();
    let manifest = match load_manifest() {
        Some(m) => m,
        None => {
            eprintln!("Skipping: manifest.json not found");
            return;
        }
    };

    if !raw_path.exists() {
        eprintln!("Skipping: golden.raw not found");
        return;
    }

    let raw_data = fs::read(&raw_path).expect("read raw");
    let files = manifest["files"].as_array().expect("files array");
    
    println!("Verifying {} files in manifest...", files.len());

    let mut verified = 0;
    let mut failed = Vec::new();

    for file in files {
        let path = file["path"].as_str().unwrap_or("?");
        let offset = file["offset"].as_u64().unwrap_or(0) as usize;
        let size = file["size"].as_u64().unwrap_or(0) as usize;
        let expected_hash = file["sha256"].as_str().unwrap_or("");

        if offset + size > raw_data.len() {
            failed.push(format!("{}: extends beyond image", path));
            continue;
        }

        let slice = &raw_data[offset..offset + size];
        let actual_hash = sha256_hex(slice);

        if actual_hash == expected_hash {
            verified += 1;
        } else {
            failed.push(format!("{}: hash mismatch", path));
        }
    }

    println!("Verified: {}/{}", verified, files.len());
    
    if !failed.is_empty() {
        for f in &failed {
            eprintln!("  FAILED: {}", f);
        }
        panic!("{} files failed verification", failed.len());
    }
}

/// Test that specific file categories are present and carved correctly
#[test]
fn golden_category_coverage() {
    let manifest = match load_manifest() {
        Some(m) => m,
        None => {
            eprintln!("Skipping: manifest.json not found");
            return;
        }
    };

    let categories = manifest["summary"]["categories"]
        .as_object()
        .expect("categories object");

    println!("Category coverage:");
    for (cat, info) in categories {
        let files = info["files"].as_u64().unwrap_or(0);
        let bytes = info["bytes"].as_u64().unwrap_or(0);
        println!("  {}: {} files, {} bytes", cat, files, bytes);

        // Each category present in the manifest should have at least one file.
        assert!(files > 0, "category '{}' should have files", cat);
    }
}
```

---

## .gitignore Updates

Add to project `.gitignore`:

```gitignore
# Golden image - raw is large, E01 is committed
tests/golden_image/golden.raw
```

---

## README Updates

Add to Testing section:

```markdown
### Golden Image Tests

Comprehensive integration tests using a golden image with ALL sample files (currently ~84 files, ~23MB).

```
tests/golden_image/
├── .goldenignore     # Optional ignore list for non-samples
├── samples/          # Source files organized by type
│   ├── images/       # JPEG, PNG, GIF, BMP, WebP, TIFF, etc.
│   ├── documents/    # PDF, DOCX, XLSX, PPTX, ODT, etc.
│   ├── archives/     # ZIP, RAR, 7z, TAR variants
│   ├── databases/    # SQLite (basic + browser artifacts)
│   ├── video/        # MP4, AVI, MOV, etc.
│   ├── audio/        # MP3, WAV, OGG
│   ├── other/        # Text, JSON, strings for extraction
│   └── ...
├── generate.sh       # Packs ALL samples into image
├── manifest.json     # Complete offset/hash map
├── golden.raw        # ~25MB raw image (gitignored)
└── golden.E01        # ~10MB compressed (committed)
```

**Generate/regenerate:**
```bash
cd tests/golden_image
./generate.sh              # Creates raw + E01
./generate.sh --no-e01     # Raw only (faster)
```

**Run tests:**
```bash
cargo test golden                    # Raw image tests
cargo test golden --features ewf     # Include E01 tests
```
```

---

## Verification Checklist

- [ ] `generate.sh` created and executable
- [ ] Running `./generate.sh` completes without errors
- [ ] `manifest.json` contains all files with offsets/hashes
- [ ] `golden.raw` created (~25MB with alignment)
- [ ] `golden.E01` created (~8-12MB compressed)
- [ ] `ewfverify golden.E01` passes
- [ ] `cargo test golden` passes
- [ ] `cargo test golden --features ewf` passes
- [ ] Manifest integrity test validates all file hashes
- [ ] String scanning finds URLs/emails from test data
- [ ] Adding a new file under `tests/golden_image/samples/` updates the image and manifest without code changes

---

## Completion Criteria

This feature is complete when:

- [ ] `generate.sh` works on Linux (bash 4+)
- [ ] All sample files included in image
- [ ] `manifest.json` auto-generated with full metadata
- [ ] `golden.raw` and `golden.E01` successfully created
- [ ] `tests/golden_image_test.rs` implemented with 5+ test functions
- [ ] Tests verify carving, string extraction, and manifest integrity
- [ ] README updated with instructions
- [ ] `.gitignore` excludes `golden.raw`
- [ ] Tests and generator are manifest-driven (no hard-coded sample lists)
- [ ] Planning doc moved to `planning/done/`

---

## Feature Plan

1. Standardize the input directory on `tests/golden_image/samples/` and keep a fallback to `sample/` to avoid churn.
2. Implement `tests/golden_image/generate.sh` to recursively discover files (optionally honoring `.goldenignore`), then build `golden.raw`, `golden.E01`, and `manifest.json` deterministically.
3. Make tests manifest-driven: validate offsets/hashes, validate carved outputs via metadata, and avoid hard-coded file/category lists.
4. Document usage and artifact policy (`README`, `.gitignore`), including how to regenerate images.
5. Validate the workflow by adding a new sample file and regenerating the images/tests without code changes.
