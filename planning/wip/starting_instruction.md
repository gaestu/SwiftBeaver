# High-Speed Forensic Carver with Optional GPU Acceleration

**Architecture & Project Blueprint**

---

## 1. Project Overview

### 1.1 Goal

Build a **high-speed, forensic-grade file and artefact carver** that:

* Scans **large disk images** (multi-TB) and raw devices.
* Carves **files** (images, SQLite DBs, etc.) and **text artefacts** (URLs, emails, phone numbers).
* Uses **parallel CPU processing** and optionally **GPU acceleration** for:

  * Signature scanning (headers/footers).
  * Strings/URL pre-filtering.
* Produces **structured metadata** suitable for further analysis (e.g. in your existing forensic pipelines).

### 1.2 Scope (v1–v3)

* **v1 – CPU-only core carver**

  * Raw & E01 image support.
  * Parallel chunk scanning.
  * JPEG/PNG/GIF carving (contiguous files only).
  * Basic SQLite database carving (whole DBs).
  * Optional basic strings/URL extraction (CPU).

* **v2 – Extended formats & artefacts**

  * Additional file types: PDF, ZIP, DOCX/XLSX, WEBP.
  * Browser history DB parsing (Chromium/Firefox) from carved SQLite DBs.
  * Structured artefact output (URLs, emails, phones).

* **v3 – GPU acceleration**

  * GPU-accelerated signature scanner.
  * GPU-accelerated printable strings scanner (URL/e-mail pre-filter).
  * Entropy-based region detection (optional).

---

## 2. Requirements

### 2.1 Functional Requirements

1. **Input formats**

   * Raw disk images (`.dd`, `.img`, etc.).
   * EWF (`.E01`) images via `libewf`.
   * Optionally direct devices (e.g. `/dev/sdX`) in read-only mode.

2. **Core carving**

   * Detect file headers and, where applicable, footers.
   * Validate candidate files.
   * Extract carved files to an output directory.
   * Compute per-file hashes (MD5, SHA-256).

3. **Text artefact extraction**

   * Find printable strings (ASCII/UTF-8, optionally UTF-16).
   * Extract URLs, emails, and phone numbers from these strings.
   * Associate artefacts with their byte offsets in the source image.

4. **SQLite / browser artefacts**

   * Detect and carve SQLite databases.
   * Recognise browser-related DBs (Chromium/Firefox).
   * Extract history-like records (URL, title, timestamps) into a normalised format.

5. **Metadata & reporting**

   * Record every carved entity in structured form (JSONL/CSV/DB).
   * Provide run-level statistics (bytes scanned, hits, valid files, errors).

### 2.2 Non-Functional Requirements

* **Performance**

  * CPU: saturate fast SSD/NVMe when possible.
  * GPU (when enabled): significantly increase internal scan throughput when data is in RAM or on NVMe.

* **Forensic soundness**

  * Strict read-only access to evidence.
  * No in-place modifications.
  * Clear recording of:

    * evidence path / device,
    * global offset,
    * run identifier,
    * tool version.

* **Extensibility**

  * Adding new file types or artefact parsers requires no major redesign.
  * GPU acceleration should be optional (feature-flagged build).

* **Portability**

  * Linux first (your environment).
  * Aim for minimal OS-specific code (mostly in EvidenceSource).

---

## 3. System Architecture

### 3.1 High-Level Pipeline

1. **EvidenceSource**
   Abstracts reads from raw/E01/device into a linear byte space.

2. **Chunk Scheduler / Reader**
   Splits the image into overlapping chunks; reads them sequentially.

3. **Scan Engines**

   * **SignatureScanner** (CPU or GPU): finds file headers/footers.
   * **StringScanner** (CPU or GPU): finds printable string spans and simple URL-like candidates.

4. **Hit & Span Dispatch**

   * Converts chunk-relative positions to **global offsets**.
   * Sends:

     * file hits → **CarveWorkers**,
     * string spans → **StringArtefactWorkers**.

5. **Carving & Parsing**

   * File carvers validate and extract files (JPEG, PNG, SQLite, etc.).
   * String artefact workers parse URLs, emails, phones.
   * Later: SQLite page and browser history parsers.

6. **Output**

   * Carved files written to disk.
   * Metadata stored via a pluggable **MetadataSink** (JSONL/CSV/SQLite/DuckDB).

### 3.2 Concurrency Model

* **Reader thread**
  Reads chunks and pushes them to a `scan_jobs` channel.

* **Scan workers**
  Consume chunks, perform signature/strings scanning, emit hits/spans.

* **Carve workers**
  Consume file hits, perform validation and extraction.

* **String artefact workers**
  Consume string spans, perform parsing and emit artefacts.

* **Metadata writer**
  Single thread consuming metadata events and writing to storage.

The stages are connected via bounded channels to avoid unbounded memory growth.

---

## 4. Detailed Module Specifications

### 4.1 CLI & Configuration (`cli`, `config` modules)

**Responsibilities**

* Parse command-line arguments.
* Load a configuration file for:

  * enabled file types
  * patterns, validators
  * GPU usage
  * paths and output settings.

**Key CLI options (examples)**

* `--input /path/to/image.E01`
* `--output /cases/1234/carving/`
* `--types jpeg,png,gif,sqlite`
* `--scan-strings`
* `--scan-urls`
* `--gpu` (use GPU for scanning)
* `--chunk-size 512M`
* `--overlap 32K`
* `--workers 16`
* `--metadata-backend jsonl|csv|sqlite|duckdb`

### 4.2 EvidenceSource Abstraction (`evidence` module)

```rust
pub trait EvidenceSource: Send + Sync {
    fn len(&self) -> u64;
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, EvidenceError>;
}
```

Implementations:

* `RawFileSource` – backed by an OS file descriptor.
* `EwfSource` – backed by libewf (via FFI).
* (Later) `DeviceSource` – direct read from `/dev/sdX` with safety checks.

### 4.3 Chunk Scheduler & Reader (`chunk` module)

* Generate a list of `ScanChunk`:

```rust
pub struct ScanChunk {
    pub id: u64,
    pub start: u64,   // global offset
    pub length: u64,  // includes overlap where applicable
}
```

* Logic:

  * For image size `L`, with `chunk_size` and `overlap`:

    * chunk 0: start=0, length = min(L, chunk_size + overlap)
    * chunk 1: start=chunk_size, length = min(L - chunk_size, chunk_size + overlap)
    * etc.

* Reader:

  * Uses `EvidenceSource` to fill buffers for each chunk.
  * Sends `(ScanChunk, Arc<Vec<u8>>)` into `scan_jobs` channel.

### 4.4 Signature Scanning Subsystem (`scanner` module)

#### Traits

```rust
pub struct Hit {
    pub chunk_id: u64,
    pub local_offset: u64,
    pub pattern_id: String,
    pub file_type_id: String,
}

pub trait SignatureScanner: Send + Sync {
    fn scan_chunk(&self, chunk: &ScanChunk, data: &[u8]) -> Vec<Hit>;
}
```

#### CPU implementation (`scanner::cpu`)

* Uses fast substring search (`memchr`/`memmem`) per pattern.
* Optionally uses Rayon internally for intra-chunk parallelism.
* Configurable patterns loaded from configuration.

#### GPU implementation (`scanner::opencl`/`scanner::cuda`, features `gpu-opencl`/`gpu-cuda`)

* Available when compiled with `--features gpu-opencl` (alias `gpu`) or `--features gpu-cuda`.
* Uses CUDA or OpenCL to:

  * Copy chunk data to device (or use mapped memory).
  * Launch kernel that checks multiple patterns in parallel.
  * Returns hits as `(offset, pattern_index)`.

**Normalization**

* Convert `Hit` → `NormalizedHit`:

```rust
pub struct NormalizedHit {
    pub global_offset: u64,
    pub file_type_id: String,
    pub pattern_id: String,
}
```

### 4.5 String Scanning & Text Artefacts (`strings` module)

#### String scanner trait

```rust
pub struct StringSpan {
    pub chunk_id: u64,
    pub local_start: u64,
    pub length: u32,
    pub flags: u32, // bitmask: URL-like, email-like, phone-like, etc.
}

pub trait StringScanner: Send + Sync {
    fn scan_chunk(&self, chunk: &ScanChunk, data: &[u8]) -> Vec<StringSpan>;
}
```

#### GPU StringScanner (later)

* Kernel 1: mark printable characters.
* Kernel 2: detect continuous runs (string spans).
* Kernel 3: simple substring checks for `"http"`, `"https"`, `"www."`, `"@"`, `"+"` etc. to set flags.
* Return compact list of spans; CPU then fetches bytes and runs full regex/validation.

#### String artefact workers (`strings::artifacts`)

Responsibilities:

* Consume normalised spans.
* Read bytes from chunk buffer or `EvidenceSource`.
* Run regex-based parsing on CPU for:

  * URLs
  * Emails
  * Phone numbers
* Emit artefacts:

```rust
pub enum ArtefactKind {
    Url,
    Email,
    Phone,
    GenericString,
}

pub struct StringArtefact {
    pub run_id: String,
    pub artefact_kind: ArtefactKind,
    pub content: String,
    pub encoding: String,      // "ascii", "utf-8", "utf-16le", etc.
    pub global_start: u64,
    pub global_end: u64,
}
```

### 4.6 File Carving Subsystem (`carve` module)

#### Traits

```rust
pub struct CarvedFile {
    pub run_id: String,
    pub file_type: String,
    pub path: std::path::PathBuf,
    pub extension: String,
    pub global_start: u64,
    pub global_end: u64,
    pub size: u64,
    pub md5: Option<String>,
    pub sha256: Option<String>,
    pub validated: bool,
    pub truncated: bool,
    pub errors: Vec<String>,
}

pub struct ExtractionContext<'a> {
    pub run_id: &'a str,
    pub output_root: &'a std::path::Path,
    pub evidence: &'a dyn EvidenceSource,
}

pub trait CarveHandler: Send + Sync {
    fn file_type(&self) -> &str;

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &mut ExtractionContext
    ) -> Result<Option<CarvedFile>, CarveError>;
}
```

#### Example handlers

* `jpeg::JpegCarveHandler`

  * Start at SOI, parse segments, stop at EOI or size limit.
* `png::PngCarveHandler`

  * Start at PNG header, parse chunks until IEND.
* `gif::GifCarveHandler`
* `sqlite::SqliteCarveHandler`

  * On header hit “SQLite format 3\0”:

    * Determine page size.
    * Read until pages stop validating or max length.
    * Output `.sqlite` file and mark status.

**Carve workers**

* Pool of N workers.
* Each worker:

  * Receives `NormalizedHit`.
  * Finds appropriate `CarveHandler` by `file_type_id`.
  * Calls `process_hit`.
  * Writes resulting `CarvedFile` to metadata channel.

### 4.7 SQLite & Browser Artefact Parsers (`parsers` module or separate binary)

This can be part of the same project or a separate tool.

Sub-modules:

* `parsers::sqlite_db`

  * Open carved `.sqlite` files.
  * For known schemas (browser `History`, `places.sqlite`, etc.), run SQL queries to extract history, cookies, etc.

* `parsers::browser`

  * Normalise browser history records into a unified format:

```rust
pub struct BrowserHistoryRecord {
    pub run_id: String,
    pub browser: String,       // "chrome", "edge", "firefox", …
    pub profile: String,
    pub url: String,
    pub title: Option<String>,
    pub visit_time: Option<chrono::NaiveDateTime>,
    pub visit_source: Option<String>,
    pub source_file: std::path::PathBuf,
}
```

* `parsers::sqlite_pages` (later)

  * Directly parse B-tree pages to recover rows from damaged DBs.

### 4.8 Metadata & Storage (`metadata` module)

#### Trait

```rust
pub trait MetadataSink: Send + Sync {
    fn record_file(&self, file: &CarvedFile) -> Result<(), MetadataError>;
    fn record_string(&self, artefact: &StringArtefact) -> Result<(), MetadataError>;
    fn record_history(&self, record: &BrowserHistoryRecord) -> Result<(), MetadataError>;
    fn flush(&self) -> Result<(), MetadataError>;
}
```

#### Implementations

* `JsonlSink` – writes one JSON object per line.
* `CsvSink` – separate CSV files for file records, strings, history.
* `SqliteSink` – tables:

  * `carved_files`
  * `string_artefacts`
  * `browser_history`
* `DuckdbSink` – similar schema, optimised for analytics.

Start with `JsonlSink` + `CsvSink` (simplest for v1).

### 4.9 Logging & Metrics (`logging` module)

* Use `tracing` + `tracing-subscriber`:

  * structured logs at INFO/DEBUG.
* Maintain counters:

  * bytes_scanned
  * chunks_processed
  * hits_found
  * files_carved
  * strings_found
  * artefacts_extracted
* Emit final summary at end of run.

---

## 5. GPU Integration Strategy

* Build with feature flag: `--features gpu-opencl` (alias `gpu`) or `--features gpu-cuda`.
* Under `scanner::opencl`/`scanner::cuda` and `strings::opencl`/`strings::cuda`:

  * use CUDA/OpenCL wrappers.
* If GPU is unavailable or fails:

  * fall back to CPU scanners.
* Isolate all GPU-specific code behind traits:

  * `SignatureScanner` and `StringScanner`.

This keeps the rest of the system pure Rust and simple.

---

## 6. Data Models & Example Output

### 6.1 Example `CarvedFile` JSONL entry

```json
{
  "run_id": "2025-12-23T20:15:00Z_abc123",
  "file_type": "jpeg",
  "path": "run_2025-12-23T20-15-00/jpeg/jpeg_00001234.jpg",
  "extension": "jpg",
  "global_start": 1048576,
  "global_end": 1062345,
  "size": 13770,
  "md5": "ae2f1b5c0a...",
  "sha256": "3fa9c0d5...",
  "validated": true,
  "truncated": false,
  "errors": []
}
```

### 6.2 Example `StringArtefact` JSONL entry

```json
{
  "run_id": "2025-12-23T20:15:00Z_abc123",
  "kind": "Url",
  "content": "https://example.com/login",
  "encoding": "ascii",
  "global_start": 987654321,
  "global_end": 987654355
}
```

### 6.3 Example `BrowserHistoryRecord` JSONL entry

```json
{
  "run_id": "2025-12-23T20:15:00Z_abc123",
  "browser": "chrome",
  "profile": "Default",
  "url": "https://example.com/login",
  "title": "Example Login",
  "visit_time": "2025-12-21T19:42:11",
  "visit_source": "typed",
  "source_file": "run_2025-12-23T20-15-00/sqlite/history_00000001.sqlite"
}
```

---

## 7. Rust Project Skeleton

### 7.1 Directory Layout

```text
fastcarve/
├─ Cargo.toml
├─ README.md
├─ LICENSE
├─ config/
│  └─ default.yml
├─ src/
│  ├─ main.rs
│  ├─ lib.rs
│  ├─ cli.rs
│  ├─ config.rs
│  ├─ evidence.rs
│  ├─ chunk.rs
│  ├─ scanner/
│  │  ├─ mod.rs
│  │  ├─ cpu.rs
│  │  └─ gpu.rs         # behind "gpu" feature
│  ├─ strings/
│  │  ├─ mod.rs
│  │  ├─ cpu.rs
│  │  └─ gpu.rs         # behind "gpu" feature
│  ├─ carve/
│  │  ├─ mod.rs
│  │  ├─ jpeg.rs
│  │  ├─ png.rs
│  │  ├─ gif.rs
│  │  └─ sqlite.rs
│  ├─ metadata/
│  │  ├─ mod.rs
│  │  ├─ jsonl.rs
│  │  ├─ csv.rs
│  │  └─ sqlite.rs      # later
│  ├─ parsers/
│  │  ├─ mod.rs
│  │  ├─ sqlite_db.rs
│  │  ├─ browser.rs
│  │  └─ sqlite_pages.rs
│  ├─ logging.rs
│  └─ util.rs
├─ tests/
│  ├─ integration_basic.rs
│  └─ perf_basics.rs
└─ examples/
   └─ minimal_run.rs
```

### 7.2 `Cargo.toml` (skeleton)

```toml
[package]
name = "fastcarve"
version = "0.1.0"
edition = "2021"
authors = ["<your name>"]
description = "High-speed forensic file and artefact carver with optional GPU acceleration."
license = "MIT"

[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
thiserror = "1"
anyhow = "1"
rayon = "1"
crossbeam-channel = "0.5"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["fmt", "env-filter"] }
hex = "0.4"
chrono = { version = "0.4", features = ["serde"] }

# Optional: for SQLite / DuckDB metadata backends
rusqlite = { version = "0.32", optional = true, features = ["bundled"] }
duckdb = { version = "0.10", optional = true }

[features]
default = []
gpu = []          # later: add CUDA/OpenCL wrapper crates here
sqlite-meta = ["rusqlite"]
duckdb-meta = ["duckdb"]

[dev-dependencies]
tempfile = "3"
```

### 7.3 `src/main.rs` (stub)

```rust
mod cli;
mod config;
mod evidence;
mod chunk;
mod scanner;
mod strings;
mod carve;
mod metadata;
mod parsers;
mod logging;
mod util;

use anyhow::Result;
use tracing::info;

fn main() -> Result<()> {
    logging::init_logging();

    let cli_opts = cli::parse();
    let cfg = config::load_config(cli_opts.config_path.as_deref())?;

    info!("Starting fastcarve run_id={} input={}",
        cfg.run_id,
        cli_opts.input.display()
    );

    // 1. Open evidence source
    let evidence = evidence::open_source(&cli_opts)?;

    // 2. Build metadata sink
    let meta_sink = metadata::build_sink(&cli_opts, &cfg)?;

    // 3. Build scanners (signature + strings)
    let sig_scanner: Box<dyn scanner::SignatureScanner> =
        scanner::build_signature_scanner(&cfg)?;
    let string_scanner: Option<Box<dyn strings::StringScanner>> =
        if cfg.enable_string_scan {
            Some(strings::build_string_scanner(&cfg)?)
        } else {
            None
        };

    // 4. Run pipeline (reader + workers)
    util::run_pipeline(&cfg, &*evidence, sig_scanner, string_scanner, meta_sink)?;

    info!("fastcarve run finished");
    Ok(())
}
```

### 7.4 Key Module Stubs

#### `src/cli.rs`

```rust
use std::path::PathBuf;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct CliOptions {
    /// Input image (raw, E01, or device)
    #[arg(short, long)]
    pub input: PathBuf,

    /// Output directory for carved files and metadata
    #[arg(short, long, default_value = "./output")]
    pub output: PathBuf,

    /// Optional path to config file (YAML)
    #[arg(long)]
    pub config_path: Option<PathBuf>,

    /// Enable GPU acceleration if available
    #[arg(long)]
    pub gpu: bool,

    /// Number of worker threads
    #[arg(long, default_value_t = num_cpus::get())]
    pub workers: usize,

    /// Chunk size, in MiB
    #[arg(long, default_value_t = 512)]
    pub chunk_size_mib: u64,
}

pub fn parse() -> CliOptions {
    CliOptions::parse()
}
```

#### `src/config.rs` (simplified)

```rust
use serde::Deserialize;
use anyhow::Result;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct FileTypeConfig {
    pub id: String,
    pub extensions: Vec<String>,
    pub header_patterns: Vec<PatternConfig>,
    pub footer_patterns: Vec<PatternConfig>,
    pub max_size: u64,
    pub min_size: u64,
    pub validator: String,
}

#[derive(Debug, Deserialize)]
pub struct PatternConfig {
    pub id: String,
    pub hex: String,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub run_id: String,
    pub overlap_bytes: u64,
    pub enable_string_scan: bool,
    pub file_types: Vec<FileTypeConfig>,
}

pub fn load_config(path: Option<&Path>) -> Result<Config> {
    let cfg: Config = if let Some(p) = path {
        let bytes = std::fs::read(p)?;
        serde_yaml::from_slice(&bytes)?
    } else {
        // load built-in default.yml at compile time
        let bytes = include_bytes!("../config/default.yml");
        serde_yaml::from_slice(bytes)?
    };
    Ok(cfg)
}
```

#### `src/evidence.rs` (skeleton)

```rust
use anyhow::Result;

pub enum EvidenceError {
    Io(std::io::Error),
    Other(String),
}

pub trait EvidenceSource: Send + Sync {
    fn len(&self) -> u64;
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, EvidenceError>;
}

pub struct RawFileSource {
    file: std::fs::File,
    len: u64,
}

impl RawFileSource {
    pub fn open(path: &std::path::Path) -> Result<Self, EvidenceError> {
        let file = std::fs::File::open(path).map_err(EvidenceError::Io)?;
        let len = file.metadata().map_err(EvidenceError::Io)?.len();
        Ok(Self { file, len })
    }
}

impl EvidenceSource for RawFileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, EvidenceError> {
        use std::io::{Seek, SeekFrom, Read};
        let mut f = &self.file;
        f.seek(SeekFrom::Start(offset)).map_err(EvidenceError::Io)?;
        let n = f.read(buf).map_err(EvidenceError::Io)?;
        Ok(n)
    }
}

// TODO: EwfSource with libewf bindings

use crate::cli::CliOptions;

pub fn open_source(opts: &CliOptions) -> Result<Box<dyn EvidenceSource>, EvidenceError> {
    // For now assume raw file; later detect by extension or header
    let src = RawFileSource::open(&opts.input)?;
    Ok(Box::new(src))
}
```

#### `src/scanner/mod.rs`

```rust
pub mod cpu;
#[cfg(feature = "gpu-opencl")]
pub mod opencl;
#[cfg(feature = "gpu-cuda")]
pub mod cuda;

use crate::chunk::ScanChunk;

#[derive(Debug, Clone)]
pub struct Hit {
    pub chunk_id: u64,
    pub local_offset: u64,
    pub pattern_id: String,
}

#[derive(Debug, Clone)]
pub struct NormalizedHit {
    pub global_offset: u64,
    pub file_type_id: String,
    pub pattern_id: String,
}

pub trait SignatureScanner: Send + Sync {
    fn scan_chunk(&self, chunk: &ScanChunk, data: &[u8]) -> Vec<Hit>;
}

// builder
use crate::config::Config;
use anyhow::Result;

pub fn build_signature_scanner(cfg: &Config) -> Result<Box<dyn SignatureScanner>> {
    // For now only CPU implementation; later choose based on cfg + feature
    Ok(Box::new(cpu::CpuScanner::new(cfg)?))
}
```

…and similar small stubs for `scanner::cpu`, `strings::mod`, `carve::mod`, `metadata::mod`, etc., with comments explaining what to implement.

---

## 8. Implementation Roadmap

### Phase 1 – Minimal CPU Carver

* Implement:

  * RawFileSource
  * Chunk scheduler & reader
  * CPU SignatureScanner with hard-coded JPEG/PNG/GIF patterns
  * JPEG and PNG CarveHandlers
  * JSONL metadata sink
  * Basic CLI + logging
* Test on small disk images with known content.

### Phase 2 – Format & Artefact Expansion

* Add:

  * GifCarveHandler
  * SqliteCarveHandler (whole DB carving)
  * StringScanner (CPU) + StringArtefactWorkers
  * Browser history parser for carved SQLite DBs (Chromium + Firefox basic)
  * CSV metadata sink
* Test on browser profiles & sample forensic images.

### Phase 3 – GPU Acceleration

* Add GPU feature:

  * GpuSignatureScanner
  * GpuStringScanner
* Benchmark vs CPU:

  * On NVMe + RAM disk, measure scan speed.
* Add config switches & fallback logic.

### Phase 4 – Advanced Forensics

* SQLite page parsing for damaged DBs.
* Entropy-based region detection.
* Additional file types and artefact types.
* Integration with your existing forensic toolkit (DB ingestion, GUIs).

---

## 9. Testing & Validation

* **Unit tests** for each module (chunking logic, pattern matching, validators).
* **Property tests** for parsers (e.g. random JPEGs, fuzzing invalid input).
* **Golden-image tests**:

  * Known images where ground truth carved files and URLs are known.
* **Performance tests**:

  * Synthetic large files (random + embedded artefacts).
  * Record throughput and memory usage.

---

## 10. Refined Considerations / Pitfalls

* **I/O bottlenecks**
  The largest constraint will often be disk throughput. Design for sequential reads and large buffers, avoid random access.

* **Chunk overlap**
  Choose overlap large enough to handle:

  * longest header/footer patterns,
  * multi-byte string boundaries between chunks.
    16–64 KiB is usually safe.

* **False positives vs speed**
  It’s better to:

  * have more candidates from the scanner, and
  * rely on validators to filter,
    than to miss real files.

* **GPU benefit**
  Real benefit appears on:

  * fast storage,
  * cached images (RAM disk),
  * environments where you redo scans often (e.g. tuning signatures).

* **Forensic reproducibility**
  Always log:

  * tool version,
  * configuration file hash,
  * command line,
  * run ID.
