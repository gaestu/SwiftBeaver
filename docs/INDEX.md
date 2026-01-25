# SwiftBeaver Documentation

Complete documentation for SwiftBeaver forensic file carving tool.

## Quick Navigation

### Getting Started
- **[Getting Started Guide](getting-started.md)** - Installation, first scan, quick reference
- **[Use Cases & Examples](use-cases.md)** - Real-world forensic scenarios
- **[Troubleshooting](troubleshooting.md)** - Common issues and solutions

### Reference Documentation
- **[Configuration Reference](config.md)** - Complete configuration schema
- **[File Format Support](file-formats.md)** - All 34 supported formats
- **[Architecture](architecture.md)** - Pipeline and design overview

### Metadata & Output
- **[JSONL Metadata](metadata_jsonl.md)** - JSON Lines format schema
- **[CSV Metadata](metadata_csv.md)** - CSV format schema
- **[Parquet Metadata](metadata_parquet.md)** - Apache Parquet format schema

### Advanced Topics
- **[Carver Algorithms](carver/README.md)** - Detailed carver documentation
- **[Golden Image Testing](golden_image.md)** - Testing framework

---

## Documentation Overview

### For New Users

**Start Here:**
1. [Getting Started Guide](getting-started.md)
   - Installation instructions (Rust, libewf, GPU dependencies)
   - Your first scan walkthrough
   - Output structure explanation
   - Common scan scenarios

2. [Use Cases & Examples](use-cases.md)
   - Deleted photo recovery
   - Email investigation
   - Browser artifact analysis
   - Document discovery
   - Mobile device forensics
   - 10 real-world scenarios with step-by-step commands

3. [Troubleshooting](troubleshooting.md)
   - Installation issues (libewf, OpenCL, CUDA)
   - Runtime errors (permissions, memory, disk space)
   - Scan problems (no files carved, slow scans, truncated files)
   - E01 issues
   - Checkpoint & resume problems

### For Regular Users

**Core References:**
- [Configuration Reference](config.md)
  - All configuration options explained
  - File type configuration
  - CLI overrides
  - Example configurations

- [File Format Support](file-formats.md)
  - 34 supported formats with details
  - Signature patterns
  - Validation methods
  - Performance characteristics
  - Format-specific notes

- [Metadata Documentation](metadata_jsonl.md)
  - Output metadata schema
  - Field descriptions
  - Example records
  - Querying with jq/DuckDB

### For Developers & Researchers

**Technical Documentation:**
- [Architecture](architecture.md)
  - Pipeline overview
  - GPU backends (OpenCL/CUDA)
  - Concurrency model
  - Module descriptions

- [Carver Algorithms](carver/README.md)
  - Detailed carver documentation (14 formats)
  - Algorithm explanations
  - Testing strategies
  - Edge case handling
  - Forensic considerations

- [Golden Image Testing](golden_image.md)
  - Test infrastructure
  - Generating test images
  - Manifest format
  - Running tests

---

## Feature Matrix

### Core Features

| Feature | Status | Documentation |
|---------|--------|---------------|
| **File Carving** | ✅ Production | [File Formats](file-formats.md), [Carvers](carver/README.md) |
| **String Scanning** | ✅ Production | [Getting Started](getting-started.md#scan-with-string-extraction) |
| **GPU Acceleration** | ✅ Production | [Getting Started](getting-started.md#gpu-support---opencl) |
| **Browser Artifacts** | ✅ Production | [Use Cases](use-cases.md#browser-artifact-analysis) |
| **E01 Support** | ✅ Production | [Getting Started](getting-started.md#scan-e01-image) |
| **Checkpointing** | ✅ Production | [Use Cases](use-cases.md#use-checkpointing) |
| **Metadata Backends** | ✅ Production | [JSONL](metadata_jsonl.md), [CSV](metadata_csv.md), [Parquet](metadata_parquet.md) |
| **Entropy Detection** | ✅ Production | [Config Reference](config.md#entropy-detection) |
| **SQLite Page Recovery** | ✅ Production | [Use Cases](use-cases.md#browser-artifact-analysis) |

### Supported Platforms

| Platform | Status | Notes |
|----------|--------|-------|
| **Linux** | ✅ Fully Supported | Primary development platform |
| **macOS** | ✅ Supported | OpenCL only (Apple Silicon limited) |
| **Windows** | ⚠️ Via WSL2 | Native Windows build not tested |

### GPU Backends

| Backend | Supported GPUs | Documentation |
|---------|----------------|---------------|
| **OpenCL** | NVIDIA, AMD, Intel | [Getting Started](getting-started.md#gpu-support---opencl) |
| **CUDA** | NVIDIA only | [Getting Started](getting-started.md#gpu-support---cuda) |

---

## Common Workflows

### Quick Triage Scan

```bash
# Scan for high-value formats only
swiftbeaver \
    --input suspect_disk.dd \
    --output ./triage \
    --enable-types jpeg,pdf,docx,xlsx,sqlite \
    --gpu \
    --dry-run
```

See: [Use Cases - Fast Triage](use-cases.md#for-fast-triage)

### Comprehensive Forensic Analysis

```bash
# Enable all features
swiftbeaver \
    --input evidence.E01 \
    --output ./full_analysis \
    --gpu \
    --scan-strings \
    --scan-utf16 \
    --scan-urls \
    --scan-emails \
    --scan-phones \
    --scan-entropy \
    --scan-sqlite-pages \
    --metadata-backend parquet
```

See: [Use Cases - Detailed Analysis](use-cases.md#for-detailed-analysis)

### Large Image Processing

```bash
# Scan with resource limits and checkpointing
swiftbeaver \
    --input huge_disk.dd \
    --output ./large_scan \
    --gpu \
    --max-memory-mib 8192 \
    --checkpoint-path checkpoint.json \
    --metadata-backend parquet
```

See: [Use Cases - Large-Scale Processing](use-cases.md#large-scale-image-processing)

---

## Output Formats

### Directory Structure

```
output/
└── 20250104T120000Z_abc123def/     # Run ID (timestamp + hash)
    ├── carved/                      # Extracted files by type
    │   ├── jpeg/
    │   │   ├── 0000000051200.jpg
    │   │   └── ...
    │   ├── png/
    │   ├── pdf/
    │   ├── zip/
    │   ├── docx/                    # ZIP-classified Office docs
    │   ├── sqlite/
    │   └── ...
    ├── metadata/                    # Forensic metadata
    │   ├── carved_files.jsonl       # All carved files
    │   ├── string_artefacts.jsonl   # URLs, emails, phones
    │   ├── browser_history.jsonl    # Browser browsing history
    │   ├── browser_cookies.jsonl    # Browser cookies
    │   ├── browser_downloads.jsonl  # Browser download records
    │   ├── entropy_regions.jsonl    # High-entropy regions
    │   └── run_summary.jsonl        # Scan statistics
    └── checkpoint.json              # Resume point (if created)
```

### Metadata Backends

| Backend | Use Case | Query Tools | Documentation |
|---------|----------|-------------|---------------|
| **JSONL** | General purpose, text processing | `jq`, `grep` | [metadata_jsonl.md](metadata_jsonl.md) |
| **CSV** | Spreadsheet import, legacy tools | Excel, LibreOffice | [metadata_csv.md](metadata_csv.md) |
| **Parquet** | Large datasets, SQL queries | DuckDB, pandas | [metadata_parquet.md](metadata_parquet.md) |

---

## File Format Categories

### Image Formats (7)
JPEG, PNG, GIF, BMP, TIFF, WEBP, ICO

**Documentation**: [File Formats - Image](file-formats.md#image-formats)

### Document Formats (9)
PDF, DOCX, XLSX, PPTX, DOC, XLS, PPT, RTF, ODT, ODS, ODP

**Documentation**: [File Formats - Document](file-formats.md#document-formats)

### Archive Formats (7)
ZIP, RAR, 7Z, TAR, GZIP, BZIP2, XZ

**Documentation**: [File Formats - Archive](file-formats.md#archive-formats)

### Multimedia Formats (8)
MP4, MOV, MP3, WAV, AVI, OGG, WEBM, WMV

**Documentation**: [File Formats - Multimedia](file-formats.md#multimedia-formats)

### Database & Special (3)
SQLite, ELF, EML, MOBI, FB2, LRF

**Documentation**: [File Formats - Database & Special](file-formats.md#database--special-formats)

---

## Performance Tuning

### Hardware Recommendations

| Component | Minimum | Recommended | Optimal |
|-----------|---------|-------------|---------|
| **CPU** | 4 cores | 8 cores | 16+ cores |
| **RAM** | 4 GB | 8 GB | 16+ GB |
| **Storage** | HDD | SSD (output) | NVMe SSD |
| **GPU** | None | Any OpenCL | NVIDIA (CUDA) |

### Optimization Tips

**For Speed:**
```bash
# Enable GPU, reduce overlap, limit formats
swiftbeaver \
    --input image.dd \
    --output ./output \
    --gpu \
    --overlap-kib 32 \
    --enable-types jpeg,png,pdf
```

**For Completeness:**
```bash
# Increase overlap, enable all features
swiftbeaver \
    --input image.dd \
    --output ./output \
    --gpu \
    --overlap-kib 128 \
    --scan-strings \
    --scan-utf16 \
    --scan-entropy \
    --scan-sqlite-pages
```

**For Large Images:**
```bash
# Use Parquet, checkpointing, resource limits
swiftbeaver \
    --input large.dd \
    --output ./output \
    --gpu \
    --metadata-backend parquet \
    --checkpoint-path checkpoint.json \
    --max-memory-mib 8192
```

---

## Forensic Best Practices

### Evidence Handling

1. **Create Write-Protected Image**
   ```bash
   # Hardware write blocker + imaging tool
   dc3dd if=/dev/sdb of=evidence.dd hash=md5 hash=sha256
   ```

2. **Verify Image Integrity**
   ```bash
   # Compute hash
   sha256sum evidence.dd > evidence.dd.sha256
   
   # Record hash in SwiftBeaver
   swiftbeaver \
       --input evidence.dd \
       --output ./output \
       --evidence-sha256 "$(cat evidence.dd.sha256 | cut -d' ' -f1)"
   ```

3. **Use E01 for Compression & Metadata**
   ```bash
   # Create E01 with ewfacquire
   ewfacquire -t evidence.E01 -C "Case XYZ" -e "Examiner Name" /dev/sdb
   
   # Scan E01
   swiftbeaver --input evidence.E01 --output ./output
   ```

### Chain of Custody

- **Run ID**: Unique identifier for each scan (timestamp + hash)
- **Provenance**: All carved files include:
  - `run_id`: Links to scan session
  - `global_start` / `global_end`: Byte offsets in evidence
  - `md5` / `sha256`: File hashes for verification
  - `evidence_path`: Source evidence identifier
  - `tool_version`: SwiftBeaver version used

### Documentation

```bash
# Generate comprehensive report
cat metadata/run_summary.jsonl | jq '.' > scan_report.json
cat metadata/carved_files.jsonl | jq -s '.' > carved_files_report.json

# Create file list with hashes
cat metadata/carved_files.jsonl | \
    jq -r '[.path, .sha256, .size] | @csv' > file_inventory.csv
```

---

## Quick Reference Card

### Essential Commands

```bash
# Basic scan
swiftbeaver --input image.dd --output ./out

# With E01 support
swiftbeaver --input image.E01 --output ./out

# With GPU acceleration
swiftbeaver --input image.dd --output ./out --gpu

# With string scanning
swiftbeaver --input image.dd --output ./out --scan-strings

# Limit to specific formats
swiftbeaver --input image.dd --output ./out --enable-types jpeg,png,pdf

# Resume interrupted scan
swiftbeaver --input image.dd --output ./out --resume-from checkpoint.json

# Use Parquet output
swiftbeaver --input image.dd --output ./out --metadata-backend parquet
```

### Metadata Queries

```bash
# Count files by type (JSONL)
cat metadata/carved_files.jsonl | jq -r '.file_type' | sort | uniq -c

# Find large files
cat metadata/carved_files.jsonl | jq 'select(.size > 10000000)'

# Extract all URLs
cat metadata/string_artefacts.jsonl | jq -r 'select(.artefact_type == "url") | .value'

# Query Parquet with DuckDB
duckdb -c "SELECT file_type, COUNT(*) FROM 'metadata/carved_files.parquet' GROUP BY file_type"
```

---

## Getting Help

### Documentation

- **This wiki** - Comprehensive guides and references
- **README.md** - Project overview and quick start
- **docs/** - Detailed technical documentation
- **examples/** - Example usage scenarios

### Community

- **GitHub Issues**: Report bugs, request features
- **Discussions**: Ask questions, share use cases

### Contributing

See [CONTRIBUTING.md](../CONTRIBUTING.md) for:
- Code style guidelines
- Testing requirements
- Pull request process
- Development setup

---

## Document Change Log

| Date | Changes |
|------|---------|
| 2025-01-04 | Initial comprehensive documentation created |
| | - Added Getting Started Guide |
| | - Added Use Cases & Examples |
| | - Added Troubleshooting Guide |
| | - Added File Format Support Reference |
| | - Created Documentation Index (this file) |

---

## Related Documentation

- **[README.md](../README.md)** - Project overview
- **[CHANGELOG.md](../CHANGELOG.md)** - Version history
- **[LICENSE](../LICENSE)** - Apache License 2.0
- **[CONTRIBUTING.md](../CONTRIBUTING.md)** - Contribution guidelines
