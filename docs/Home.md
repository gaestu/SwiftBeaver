# SwiftBeaver Wiki

**High-speed forensic file carver with GPU acceleration**

[![Release](https://img.shields.io/github/v/release/gaestu/SwiftBeaver)](https://github.com/gaestu/SwiftBeaver/releases)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](../LICENSE)

---

## 🚀 Quick Start

```bash
# Install from source
cargo install --path .

# Basic scan
swiftbeaver /path/to/image.dd -o ./output

# With GPU acceleration (OpenCL)
swiftbeaver /path/to/image.dd -o ./output --gpu
```

**→ [Full Getting Started Guide](getting-started)**

---

## 📚 Documentation

### Core Guides
| Guide | Description |
|-------|-------------|
| [Getting Started](getting-started) | Installation, first scan, quick reference |
| [Configuration](config) | Complete YAML configuration schema |
| [Use Cases](use-cases) | Real-world forensic scenarios |
| [Troubleshooting](troubleshooting) | Common issues and solutions |

### Architecture & Internals
| Document | Description |
|----------|-------------|
| [Architecture](architecture) | Pipeline design, threading model, GPU integration |
| [File Formats](file-formats) | All 34+ supported file formats |
| [Golden Image Testing](golden_image) | Testing framework for carver validation |

### Output Formats
| Format | Description |
|--------|-------------|
| [JSONL Metadata](metadata_jsonl) | JSON Lines schema for carved file metadata |
| [CSV Metadata](metadata_csv) | CSV format for spreadsheet analysis |
| [Parquet Metadata](metadata_parquet) | Apache Parquet for big data tools |

---

## 🔧 Supported File Types

SwiftBeaver carves **34+ file formats** across multiple categories:

### Images
[JPEG](carver-jpeg) • [PNG](carver-png) • [GIF](carver-gif) • [BMP](carver-bmp) • [TIFF](carver-tiff) • [WebP](carver-webp)

### Documents
[PDF](carver-pdf) • [SQLite](carver-sqlite)

### Audio/Video
[MP3](carver-mp3) • [MP4](carver-mp4) • [WAV](carver-wav)

### Archives
[ZIP](carver-zip) • [RAR](carver-rar) • [7z](carver-7z)

**→ [Complete Carver Documentation](carver-index)**

---

## 🎯 Key Features

- **High Performance**: Multi-threaded pipeline, memory-mapped I/O
- **GPU Acceleration**: OpenCL and CUDA support for signature scanning
- **Forensic Grade**: SHA-256 hashing, run provenance, evidence integrity
- **Multiple Formats**: EWF (E01), raw DD, split images
- **Rich Metadata**: Parquet, JSONL, CSV, SQLite output
- **Checkpoint/Resume**: Interrupt and continue long scans

---

## 📖 Index

For a complete list of all documentation pages, see the **[Documentation Index](INDEX)**.
