# Parquet Extension – Design & Module Specification

Status: Implemented  
Implemented in version: 0.1.0

## 1. Objective

Add a **Parquet-based metadata backend** to the carver so that, for each run, the tool writes:

* One or more **Parquet files per category / handler**, e.g.

  * `files_jpeg.parquet`, `files_png.parquet`, `files_sqlite.parquet`
  * `strings_spans.parquet`
  * `artefacts_urls.parquet`
  * `browser_history.parquet`

Each file contains structured, columnar data suitable for:

* Reproducing the carving decisions later.
* Downstream analytics (DuckDB, Polars, pandas, etc.).
* Integration with other forensic tools.

Parquet writing should be encapsulated in a **`ParquetSink`** implementation of the existing `MetadataSink` trait.

---

## 2. Design Goals

1. **Modular**: Parquet support is one of several metadata backends (`Jsonl`, `Csv`, `Sqlite`, `Duckdb`, `Parquet`).
2. **Category-based files**: one Parquet file per main category / handler per run.
3. **Forensic provenance** included in each row:

   * `run_id`, `tool_version`, `config_hash`, `evidence_path`, `evidence_sha256`.
4. **Efficient**: use reasonable batching (e.g. 1k–10k rows per row group).
5. **Robust**:

   * Cleanly flushed/closed on normal exit.
   * No corruption if process is interrupted partway (best-effort; Parquet writer will typically close row groups properly).

---

## 3. Affected / New Modules

* `metadata/mod.rs`  (existing – extend)
* `metadata/parquet.rs`  (new – main implementation)
* `config.rs`            (extend config with Parquet options)
* `cli.rs`               (add flag to choose Parquet backend, if not already generic)
* `util.rs` / pipeline   (no big change; just builds Parquet sink via `build_sink`)

---

## 4. Core Types & Categories

### 4.1 Logical categories

Define **logical Parquet output categories**:

* **Files (per handler)**

  * `files_jpeg`
  * `files_png`
  * `files_gif`
  * `files_sqlite`
  * (later more: pdf, zip, docx, …)

* **Raw hits (optional)**

  * `hits_files`

* **Strings**

  * `strings_spans` (not implemented; only artefacts are persisted)

* **Artefacts**

  * `artefacts_urls`
  * `artefacts_emails`
  * `artefacts_phones`

* **Browser history**

  * `browser_history`

Each logical category maps to **one Parquet file per run** in `<run_dir>/parquet/`.

### 4.2 Existing Rust structs (from earlier blueprint)

Assume you have or will have:

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
    pub encoding: String,
    pub global_start: u64,
    pub global_end: u64,
}

pub struct BrowserHistoryRecord {
    pub run_id: String,
    pub browser: String,
    pub profile: String,
    pub url: String,
    pub title: Option<String>,
    pub visit_time: Option<chrono::NaiveDateTime>,
    pub visit_source: Option<String>,
    pub source_file: std::path::PathBuf,
}
```

You will add additional internal types for **raw hits** (`Hit` / `NormalizedHit`) and **string spans** later; here we focus on what gets persisted.

---

## 5. Parquet Schemas (specification)

### 5.1 Files per handler (`files_*.parquet`)

Single schema reused for all file-type Parquet outputs:

**Columns**

* `run_id: string`

* `tool_version: string`

* `config_hash: string`

* `evidence_path: string`

* `evidence_sha256: string`

* `handler_id: string`     // e.g. "jpeg", "png", "sqlite_db"

* `file_type: string`      // could mirror handler_id

* `carved_path: string`    // relative path under carved_files/

* `global_start: long`

* `global_end: long`

* `size: long`

* `md5: string` (nullable)

* `sha256: string` (nullable)

* `pattern_id: string` (nullable)

* `magic_bytes: binary` (nullable)    // optional: few header bytes

* `validated: boolean`

* `truncated: boolean`

* `error: string` (nullable)

**Mapping from `CarvedFile`**

* `run_id` ← `CarvedFile.run_id`
* `handler_id` ← consistent mapping (e.g. from `file.file_type`)
* `carved_path` ← `file.path` relative to the run’s output root
* `global_start/global_end/size/md5/sha256/validated/truncated` from struct fields
* `error` ← `errors.join("; ")` if non-empty, else null

---

### 5.2 String spans (`strings_spans.parquet`)

Low-level spans from `StringScanner` (GPU/CPU):

**Columns**

* `run_id: string`

* `tool_version: string`

* `config_hash: string`

* `evidence_path: string`

* `chunk_id: long`

* `chunk_start: long`

* `local_start: long`

* `global_start: long`

* `length: int`

* `flags: int`           // bitmask: URL-like, email-like, phone-like, etc.

* `charset_hint: string` // "ascii","utf-8","unknown" etc.

You will define an internal struct, e.g.:

```rust
pub struct StringSpanRecord {
    pub run_id: String,
    pub tool_version: String,
    pub config_hash: String,
    pub evidence_path: String,

    pub chunk_id: u64,
    pub chunk_start: u64,
    pub local_start: u64,
    pub global_start: u64,
    pub length: u32,

    pub flags: u32,
    pub charset_hint: String,
}
```

And write that to Parquet.

---

### 5.3 String artefacts (URLs/emails/phones)

You want **separate Parquet files** for each artefact type.

#### `artefacts_urls.parquet`

**Columns**

* `run_id: string`

* `tool_version: string`

* `config_hash: string`

* `evidence_path: string`

* `global_start: long`

* `global_end: long`

* `url: string`

* `scheme: string`

* `host: string`

* `port: int`      // nullable

* `path: string`   // nullable

* `query: string`  // nullable

* `fragment: string` // nullable

* `source_kind: string`   // "string_span", "sqlite_history", "html_file", ...

* `source_detail: string` // "strings_spans", "chrome_history", ...

* `certainty: double`     // optional: start with 1.0

#### `artefacts_emails.parquet`

**Columns**

* `run_id: string`

* `tool_version: string`

* `config_hash: string`

* `evidence_path: string`

* `global_start: long`

* `global_end: long`

* `email: string`

* `local_part: string`

* `domain: string`

* `source_kind: string`

* `source_detail: string`

* `certainty: double`  // optional

#### `artefacts_phones.parquet`

**Columns**

* `run_id: string`

* `tool_version: string`

* `config_hash: string`

* `evidence_path: string`

* `global_start: long`

* `global_end: long`

* `phone_raw: string`

* `phone_e164: string`     // nullable; normalised phone

* `country: string`        // nullable; ISO country code if known

* `source_kind: string`

* `source_detail: string`

* `certainty: double`

Mapping from `StringArtefact`:

* Use `artefact_kind` to route to correct Parquet writer.
* `global_start/global_end` pass-through.
* `content` → `url` / `email` / `phone_raw`.
* Additional parsed fields (scheme, host, e164, etc.) are output of your parsing logic.

---

### 5.4 Browser history – `browser_history.parquet`

**Columns**

* `run_id: string`

* `tool_version: string`

* `config_hash: string`

* `evidence_path: string`

* `source_file: string`   // path to carved SQLite or direct file path

* `browser: string`       // chrome/edge/firefox/…

* `profile: string`       // "Default", etc.

* `url: string`

* `title: string`         // nullable

* `visit_time_utc: timestamp`  // or string

* `visit_source: string`  // nullable; "typed","link","redirect","bookmark",…

* `row_id: long`          // nullable; original SQLite rowid

* `table_name: string`    // nullable; "urls","moz_places", etc.

Mapping from `BrowserHistoryRecord` is direct plus extra provenance fields.

---

### 5.5 Raw hits – `hits_files.parquet` (optional, advanced)

If you decide to persist raw ScanEngine hits:

**Columns**

* `run_id: string`

* `tool_version: string`

* `config_hash: string`

* `evidence_path: string`

* `chunk_id: long`

* `chunk_start: long`

* `hit_local_offset: long`

* `global_offset: long`

* `file_type_id: string`  // candidate

* `pattern_id: string`

* `magic_bytes: binary`   // optional

* `raw_score: double`     // optional; initial 1.0

You can derive these from `Hit` / `NormalizedHit` records.

---

## 6. `MetadataSink` and `ParquetSink` – Spec

### 6.1 Extend `MetadataBackendKind`

In `metadata/mod.rs`:

```rust
pub enum MetadataBackendKind {
    Jsonl,
    Csv,
    Sqlite,
    Duckdb,
    Parquet,
}
```

And ensure CLI/config can select `Parquet`.

### 6.2 `MetadataSink` trait (recap)

```rust
pub trait MetadataSink: Send + Sync {
    fn record_file(&self, file: &CarvedFile) -> Result<(), MetadataError>;
    fn record_string(&self, artefact: &StringArtefact) -> Result<(), MetadataError>;
    fn record_history(&self, record: &BrowserHistoryRecord) -> Result<(), MetadataError>;
    fn flush(&self) -> Result<(), MetadataError>;
}
```

### 6.3 New module `metadata/parquet.rs`

#### 6.3.1 Internal design

Design `ParquetSink` as a **wrapper** around multiple **category writers**.

```rust
pub struct ParquetSink {
    run_id: String,
    tool_version: String,
    config_hash: String,
    evidence_path: String,
    evidence_sha256: String,

    output_dir: std::path::PathBuf,

    // Option<Writer> so we lazily initialize only when needed
    files_jpeg: Option<CategoryWriter>,
    files_png: Option<CategoryWriter>,
    files_gif: Option<CategoryWriter>,
    files_sqlite: Option<CategoryWriter>,
    // ...

    strings_spans: Option<CategoryWriter>,
    artefacts_urls: Option<CategoryWriter>,
    artefacts_emails: Option<CategoryWriter>,
    artefacts_phones: Option<CategoryWriter>,
    browser_history: Option<CategoryWriter>,

    row_group_size: usize,
}
```

`CategoryWriter` is a generic wrapper around the Parquet library:

```rust
struct CategoryWriter {
    file: std::fs::File,
    writer: parquet::file::writer::SerializedFileWriter<std::fs::File>,
    // or parquet2-equivalent
    buffer: Vec<Row>,       // or your own column buffers; see below
    row_group_size: usize,
}
```

**Choice for the agent:**
You may represent rows as:

* Parquet `Row` / `RowGroup` types (if using `parquet` crate), or
* your own struct arrays + `arrow2` record batches, then `parquet2`.

The exact wire type is up to the implementing agent, but **schema must match the spec above**.

#### 6.3.2 Construction

A `build_parquet_sink` function:

```rust
pub fn build_parquet_sink(
    run_id: &str,
    tool_version: &str,
    config_hash: &str,
    evidence_path: &std::path::Path,
    evidence_sha256: &str,
    output_dir: &std::path::Path,
    row_group_size: usize,
) -> Result<ParquetSink, MetadataError>
```

Responsibilities:

* Create `<output_dir>/parquet/` if not exists.
* Initialize `ParquetSink` struct with all `Option<CategoryWriter>` = `None`.
* Store `run_id`, `tool_version`, `config_hash`, `evidence_path`, `evidence_sha256`.

### 6.4 CategoryWriter behaviour

A `CategoryWriter` should:

* Lazily write:

  * When first record arrives:

    * Create `<output_dir>/parquet/<category>.parquet`
    * Build Parquet schema for that category.
    * Create `SerializedFileWriter`.
* Buffer rows in memory until `buffer.len() >= row_group_size`.
* When flush threshold is reached:

  * Write a new row group to the Parquet file.
  * Clear the buffer.
* On `flush()` and on `Drop`:

  * Write remaining buffered rows as a row group.
  * Close the writer cleanly.

---

## 7. Agent Instructions – Implementation Steps

Below is a concise task list you can give to AI coding agents.

---

### Task 0 – Add Parquet library dependency

**Instructions for agent**

1. Edit `Cargo.toml`.

2. Add an appropriate Parquet crate. You may choose either:

   * `parquet` crate (Apache Arrow’s Parquet implementation), or
   * `parquet2` (lighter, column-oriented).

3. Example (if choosing `parquet`):

   ```toml
   [dependencies]
   parquet = { version = "x.y", features = ["async", "serde"] }
   ```

4. Ensure the crate compiles with the current Rust edition.

---

### Task 1 – Extend configuration and CLI for metadata backend

**Instructions for agent**

1. In `config.rs`, ensure there is a setting or mapping to `MetadataBackendKind`.
2. In `cli.rs`, add:

   * A flag (if missing) such as `--metadata-backend parquet`.
3. In `metadata/mod.rs`:

   * Extend `MetadataBackendKind` with `Parquet`.
   * In `build_sink(...)`, add a match arm that instantiates `ParquetSink` when this backend is selected.

---

### Task 2 – Create `metadata/parquet.rs` and define `ParquetSink`

**Instructions for agent**

1. Create new file `src/metadata/parquet.rs`.

2. Define:

   * `ParquetSink` struct as specified.
   * `CategoryWriter` struct.
   * Function `build_parquet_sink(...)`.

3. `ParquetSink` should store run-level provenance:

   * `run_id`
   * `tool_version`
   * `config_hash`
   * `evidence_path` (as string)
   * `evidence_sha256`
   * `output_dir`
   * `row_group_size` (constant or configurable)

4. Implement `MetadataSink` for `ParquetSink`:

   * `record_file(&self, file: &CarvedFile)`:

     * Derive handler/category from `file.file_type` (e.g. if `"jpeg"` → `files_jpeg`).
     * Lazily initialise the corresponding `CategoryWriter`.
     * Convert `CarvedFile` to a row with the `files_*` schema.
     * Append to that writer’s buffer.

   * `record_string(&self, artefact: &StringArtefact)`:

     * If `artefact_kind` is:

       * `Url` → `artefacts_urls`
       * `Email` → `artefacts_emails`
       * `Phone` → `artefacts_phones`
       * `GenericString` → optional (either ignore or send to a separate Parquet).
     * Convert to row according to the relevant schema.
     * Append to buffer.

   * `record_history(&self, record: &BrowserHistoryRecord)`:

     * Lazily initialise `browser_history` writer.
     * Map to row and append.

   * `flush(&self)`:

     * For each `CategoryWriter` that is `Some`, call its `flush()` method to write any remaining buffered rows and close the file.

---

### Task 3 – Implement `CategoryWriter`

**Instructions for agent**

1. `CategoryWriter` should encapsulate all interactions with the Parquet library.

2. Provide methods:

   ```rust
   impl CategoryWriter {
       fn new(output_path: &Path, schema: SchemaRef, row_group_size: usize) -> Result<Self, MetadataError>;
       fn append_row(&mut self, row: &YourRowStruct) -> Result<(), MetadataError>;
       fn flush(&mut self) -> Result<(), MetadataError>;
   }
   ```

3. Design an internal representation of buffered rows:

   * Either:

     * Use Parquet’s `Row` type.
     * Or maintain column-wise vectors and build row groups from them.

4. In `append_row`, add the row to in-memory buffer; if `buffer.len() >= row_group_size`, write a row group.

5. In `flush`, write remaining rows if any, then close the underlying writer.

6. Ensure `CategoryWriter` is not `Sync` unless necessary; it will be used from a **single metadata thread**, so `&mut self` methods are fine.

---

### Task 4 – Schema builders for each category

**Instructions for agent**

1. For each category, implement a `fn schema_files()`, `fn schema_strings_spans()`, `fn schema_artefacts_urls()`, `fn schema_browser_history()`, etc.

2. Example for files schema:

   ```rust
   fn schema_files() -> Result<SchemaRef, MetadataError> {
       // build using parquet/arrow schema builder
   }
   ```

3. Ensure column names and types match the spec in section 5.

4. The `schema_*` functions will be called when the relevant `CategoryWriter` is initialised.

---

### Task 5 – Mapping helpers from internal structs to Parquet rows

**Instructions for agent**

1. Implement helper functions to map internal structs:

   * `CarvedFile` → `FilesRow` (a dedicated internal struct for files)
   * `StringSpanRecord` → `StringSpanRow`
   * `StringArtefact` → `UrlRow` / `EmailRow` / `PhoneRow`
   * `BrowserHistoryRecord` → `BrowserHistoryRow`

2. Each `*Row` struct should match the columns of the schema and have simple types ready for Parquet writing.

3. Implement conversions as `From<T> for RowType` or as explicit `fn to_row(file: &CarvedFile, ctx: &GlobalContext) -> FilesRow`.

4. Include:

   * run-level context (`run_id`, `tool_version`, `config_hash`, `evidence_path`, `evidence_sha256`).
   * per-record fields from the struct.

---

### Task 6 – Integrate into pipeline / ensure single-threaded sink usage

**Instructions for agent**

1. Confirm that all `MetadataSink` calls happen from a **single metadata writer thread** that consumes events from a channel.
   If that is not yet implemented, implement it in `util::run_pipeline()`.

   * Carve workers and artefact workers should send metadata events (enums) through a channel.
   * Metadata writer thread receives events and calls `sink.record_*()`.

2. This design ensures `ParquetSink` does not need to be thread-safe beyond `Send`.

---

### Task 7 – Tests

**Instructions for agent**

1. Add tests in `tests/`:

   * `test_parquet_files_output()`:

     * Create a temporary output directory.
     * Instantiate a `ParquetSink` with fake run context.
     * Call `record_file` with a few `CarvedFile` samples for different handlers.
     * Call `flush()`.
     * Assert that the expected Parquet files exist (`files_jpeg.parquet`, etc.).
     * Optionally read them back using the Parquet library and check row counts / values.

   * `test_parquet_artefacts_output()`:

     * Same pattern for `StringArtefact` and `BrowserHistoryRecord`.

2. Ensure tests run successfully with `cargo test`.

---

## 8. Final Notes for Agents

* **Do not change the semantics** of the existing `MetadataSink` interface; only implement a new backend.
* **Respect the schemas** as defined; downstream tools will rely on column names/types.
* **Keep category logic simple**:

  * Map `file.file_type == "jpeg"` → `files_jpeg.parquet`
  * Map `ArtefactKind::Url` → `artefacts_urls.parquet`, etc.
* **Log on errors** but do not panic for individual record failures; prefer to record an error message in a dedicated column and continue if possible.
* All paths written to Parquet should be **relative to the run’s output root**, so that the entire run directory is relocatable.

---

Here is a ready-to-paste `src/metadata/parquet.rs` skeleton with all core types, functions, and TODO markers for AI agents to fill in.

You will still need to adjust some import paths (`crate::carve::CarvedFile`, etc.) to match your actual project layout, but the structure is complete.

```rust
//! Parquet metadata backend.
//!
//! This module implements `MetadataSink` for Parquet output.
//! It writes one Parquet file per category / handler within a single run.
//!
//! Categories (per run):
//!   - files_jpeg.parquet
//!   - files_png.parquet
//!   - files_gif.parquet
//!   - files_sqlite.parquet
//!   - strings_spans.parquet
//!   - artefacts_urls.parquet
//!   - artefacts_emails.parquet
//!   - artefacts_phones.parquet
//!   - browser_history.parquet
//!   - (optional) hits_files.parquet
//!
//! IMPORTANT FOR IMPLEMENTORS (AI AGENTS):
//!   - Do NOT change the `MetadataSink` trait – only implement it.
//!   - Respect the column names and types described in the design spec.
//!   - Parquet writing should be buffered and flushed in row groups.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::metadata::{MetadataSink, MetadataError};
use crate::config::Config;

// TODO: Adjust imports to your actual module structure.
use crate::carve::CarvedFile;
use crate::strings::artifacts::StringArtefact;
use crate::parsers::browser::BrowserHistoryRecord;

// TODO: Add parquet crate dependency in Cargo.toml, e.g.:
// parquet = { version = "X.Y", features = ["serde"] }
// and then import the needed types here:
//
// use parquet::schema::types::TypePtr;
// use parquet::schema::types::Type;
// use parquet::file::writer::SerializedFileWriter;
// use parquet::file::properties::WriterProperties;
// use parquet::record::Row;
// etc.

/// Logical Parquet category identifier.
///
/// Each category maps to exactly one Parquet file within a run directory.
/// The file name convention is `<category>.parquet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParquetCategory {
    FilesJpeg,
    FilesPng,
    FilesGif,
    FilesSqlite,
    StringsSpans,
    ArtefactsUrls,
    ArtefactsEmails,
    ArtefactsPhones,
    BrowserHistory,
    HitsFiles, // optional, for raw hits
}

impl ParquetCategory {
    /// Return the output filename (no directory) for this category.
    pub fn filename(self) -> &'static str {
        match self {
            ParquetCategory::FilesJpeg => "files_jpeg.parquet",
            ParquetCategory::FilesPng => "files_png.parquet",
            ParquetCategory::FilesGif => "files_gif.parquet",
            ParquetCategory::FilesSqlite => "files_sqlite.parquet",
            ParquetCategory::StringsSpans => "strings_spans.parquet",
            ParquetCategory::ArtefactsUrls => "artefacts_urls.parquet",
            ParquetCategory::ArtefactsEmails => "artefacts_emails.parquet",
            ParquetCategory::ArtefactsPhones => "artefacts_phones.parquet",
            ParquetCategory::BrowserHistory => "browser_history.parquet",
            ParquetCategory::HitsFiles => "hits_files.parquet",
        }
    }
}

/// Internal row type for file carve results (files_* categories).
///
/// This struct should map 1:1 to the Parquet schema for files.
/// Agents must keep it in sync with the schema builder.
#[derive(Debug, Clone)]
pub struct FilesRow {
    pub run_id: String,
    pub tool_version: String,
    pub config_hash: String,
    pub evidence_path: String,
    pub evidence_sha256: String,

    pub handler_id: String,
    pub file_type: String,
    pub carved_path: String,

    pub global_start: i64,
    pub global_end: i64,
    pub size: i64,

    pub md5: Option<String>,
    pub sha256: Option<String>,

    pub pattern_id: Option<String>,
    pub magic_bytes: Option<Vec<u8>>,

    pub validated: bool,
    pub truncated: bool,
    pub error: Option<String>,
}

/// Internal row type for string spans (strings_spans category).
#[derive(Debug, Clone)]
pub struct StringSpanRow {
    pub run_id: String,
    pub tool_version: String,
    pub config_hash: String,
    pub evidence_path: String,

    pub chunk_id: i64,
    pub chunk_start: i64,
    pub local_start: i64,
    pub global_start: i64,
    pub length: i32,

    pub flags: i32,
    pub charset_hint: String,
}

/// Internal row type for URL artefacts (artefacts_urls category).
#[derive(Debug, Clone)]
pub struct UrlArtefactRow {
    pub run_id: String,
    pub tool_version: String,
    pub config_hash: String,
    pub evidence_path: String,

    pub global_start: i64,
    pub global_end: i64,

    pub url: String,
    pub scheme: String,
    pub host: String,
    pub port: Option<i32>,
    pub path: Option<String>,
    pub query: Option<String>,
    pub fragment: Option<String>,

    pub source_kind: String,
    pub source_detail: String,
    pub certainty: f64,
}

/// Internal row type for email artefacts (artefacts_emails category).
#[derive(Debug, Clone)]
pub struct EmailArtefactRow {
    pub run_id: String,
    pub tool_version: String,
    pub config_hash: String,
    pub evidence_path: String,

    pub global_start: i64,
    pub global_end: i64,

    pub email: String,
    pub local_part: String,
    pub domain: String,

    pub source_kind: String,
    pub source_detail: String,
    pub certainty: f64,
}

/// Internal row type for phone artefacts (artefacts_phones category).
#[derive(Debug, Clone)]
pub struct PhoneArtefactRow {
    pub run_id: String,
    pub tool_version: String,
    pub config_hash: String,
    pub evidence_path: String,

    pub global_start: i64,
    pub global_end: i64,

    pub phone_raw: String,
    pub phone_e164: Option<String>,
    pub country: Option<String>,

    pub source_kind: String,
    pub source_detail: String,
    pub certainty: f64,
}

/// Internal row type for browser history records (browser_history category).
#[derive(Debug, Clone)]
pub struct BrowserHistoryRow {
    pub run_id: String,
    pub tool_version: String,
    pub config_hash: String,
    pub evidence_path: String,

    pub source_file: String,

    pub browser: String,
    pub profile: String,

    pub url: String,
    pub title: Option<String>,
    pub visit_time_utc: Option<chrono::NaiveDateTime>,
    pub visit_source: Option<String>,

    pub row_id: Option<i64>,
    pub table_name: Option<String>,
}

/// Generic Parquet category writer.
///
/// This struct is responsible for:
///   - Holding the Parquet writer and schema for a specific category.
///   - Buffering rows in memory.
///   - Writing row groups when buffer size reaches `row_group_size`.
///
/// IMPLEMENTATION NOTES FOR AGENTS:
///   - Choose a representation for buffered rows (e.g., Vec<Row> or column vectors).
///   - Use the `parquet` crate to create a `SerializedFileWriter<File>`.
///   - Implement `flush()` to write any remaining buffered rows and close the writer.
struct CategoryWriter {
    /// Target file path for this category (absolute or run-relative).
    path: PathBuf,

    /// TODO: Add actual Parquet writer type here.
    /// Example with `parquet` crate:
    /// writer: SerializedFileWriter<File>,
    ///
    /// For now, we keep it as an Option to allow lazy initialisation.
    /// Implementors must replace this with the actual type.
    ///
    /// NOTE: You may also want to store the underlying `File`.
    writer: Option<()>, // TODO: replace with real Parquet writer type

    /// Buffered rows awaiting flush.
    ///
    /// NOTE: Use the correct row type per category:
    ///   - FilesRow, StringSpanRow, UrlArtefactRow, etc.
    /// For simplicity, we store `Vec<Box<dyn Any>>` here and downcast,
    /// or define one CategoryWriter per row type via generics.
    ///
    /// For the skeleton, we keep this generic and let agents refine it.
    buffer_len: usize,

    /// Maximum number of rows per row group (flush threshold).
    row_group_size: usize,

    /// Category (used for schema selection and mapping).
    category: ParquetCategory,
}

impl CategoryWriter {
    /// Create a new CategoryWriter for the given category.
    ///
    /// This function:
    ///   - Constructs the Parquet schema for `category`.
    ///   - Opens the file at `path`.
    ///   - Creates the Parquet file writer.
    ///
    /// TODO (Agent): Implement using the chosen Parquet crate.
    fn new(path: PathBuf, category: ParquetCategory, row_group_size: usize) -> Result<Self, MetadataError> {
        // TODO: build schema based on category (see schema_* functions below)
        // TODO: open file and create Parquet writer
        Ok(Self {
            path,
            writer: None, // TODO: set Some(writer)
            buffer_len: 0,
            row_group_size,
            category,
        })
    }

    /// Append a `FilesRow` to the buffer (for file_* categories).
    ///
    /// IMPLEMENTATION NOTE:
    ///   - This is a category-specific append method.
    ///   - Agents should add similar methods for other row types.
    fn append_files_row(&mut self, _row: FilesRow) -> Result<(), MetadataError> {
        // TODO:
        //   - push row into internal buffer
        //   - if buffer_len >= row_group_size, write row group to file
        //   - update buffer_len
        Ok(())
    }

    /// Append a `StringSpanRow` to the buffer.
    fn append_string_span_row(&mut self, _row: StringSpanRow) -> Result<(), MetadataError> {
        // TODO: similar to append_files_row
        Ok(())
    }

    /// Append a `UrlArtefactRow`.
    fn append_url_row(&mut self, _row: UrlArtefactRow) -> Result<(), MetadataError> {
        // TODO
        Ok(())
    }

    /// Append an `EmailArtefactRow`.
    fn append_email_row(&mut self, _row: EmailArtefactRow) -> Result<(), MetadataError> {
        // TODO
        Ok(())
    }

    /// Append a `PhoneArtefactRow`.
    fn append_phone_row(&mut self, _row: PhoneArtefactRow) -> Result<(), MetadataError> {
        // TODO
        Ok(())
    }

    /// Append a `BrowserHistoryRow`.
    fn append_browser_history_row(&mut self, _row: BrowserHistoryRow) -> Result<(), MetadataError> {
        // TODO
        Ok(())
    }

    /// Flush any buffered rows and close the Parquet writer.
    ///
    /// IMPLEMENTATION NOTE:
    ///   - Called from ParquetSink::flush() and Drop.
    fn flush(&mut self) -> Result<(), MetadataError> {
        // TODO:
        //   - if buffer_len > 0, write final row group
        //   - close writer if not already closed
        Ok(())
    }
}

/// ParquetSink – `MetadataSink` implementation that writes Parquet files.
///
/// This object is created per run and lives in the metadata writer thread.
/// It holds one lazy-initialised `CategoryWriter` per logical category.
pub struct ParquetSink {
    // Run-level provenance
    run_id: String,
    tool_version: String,
    config_hash: String,
    evidence_path: String,
    evidence_sha256: String,

    // Output directory for this run, typically `<run_root>/parquet/`
    parquet_dir: PathBuf,

    // Category writers, created lazily on first use
    files_jpeg: Option<CategoryWriter>,
    files_png: Option<CategoryWriter>,
    files_gif: Option<CategoryWriter>,
    files_sqlite: Option<CategoryWriter>,

    strings_spans: Option<CategoryWriter>,

    artefacts_urls: Option<CategoryWriter>,
    artefacts_emails: Option<CategoryWriter>,
    artefacts_phones: Option<CategoryWriter>,

    browser_history: Option<CategoryWriter>,

    // Optional: raw hits writer
    hits_files: Option<CategoryWriter>,

    // Row group size for all categories
    row_group_size: usize,
}

impl ParquetSink {
    /// Build a new ParquetSink for a given run.
    ///
    /// PARAMETERS:
    ///   - cfg: global configuration (used to derive row_group_size or other settings).
    ///   - run_id: unique run identifier.
    ///   - tool_version: version string of this tool.
    ///   - config_hash: hash (e.g., SHA-256) of the effective config.
    ///   - evidence_path: path to evidence (raw/E01/device).
    ///   - evidence_sha256: hash of the evidence (if known).
    ///   - run_output_dir: root directory for this run; this function will
    ///     create `run_output_dir/parquet/` if it does not exist.
    pub fn new(
        cfg: &Config,
        run_id: String,
        tool_version: String,
        config_hash: String,
        evidence_path: PathBuf,
        evidence_sha256: String,
        run_output_dir: PathBuf,
    ) -> Result<Self, MetadataError> {
        // Determine row_group_size from config or use a reasonable default.
        // TODO: read actual value from cfg; for now, use 10_000.
        let row_group_size = 10_000;

        let parquet_dir = run_output_dir.join("parquet");
        std::fs::create_dir_all(&parquet_dir)
            .map_err(|e| MetadataError::Io(format!("Failed to create parquet dir: {e}")))?;

        Ok(Self {
            run_id,
            tool_version,
            config_hash,
            evidence_path: evidence_path.to_string_lossy().to_string(),
            evidence_sha256,
            parquet_dir,
            files_jpeg: None,
            files_png: None,
            files_gif: None,
            files_sqlite: None,
            strings_spans: None,
            artefacts_urls: None,
            artefacts_emails: None,
            artefacts_phones: None,
            browser_history: None,
            hits_files: None,
            row_group_size,
        })
    }

    /// Helper: get or create a `CategoryWriter` for a given category.
    ///
    /// IMPLEMENTATION NOTE:
    ///   - This function must choose the correct field in `self` and
    ///     initialise it with `CategoryWriter::new()` if it is None.
    fn get_or_create_writer(
        &mut self,
        category: ParquetCategory,
    ) -> Result<&mut CategoryWriter, MetadataError> {
        let slot: &mut Option<CategoryWriter> = match category {
            ParquetCategory::FilesJpeg => &mut self.files_jpeg,
            ParquetCategory::FilesPng => &mut self.files_png,
            ParquetCategory::FilesGif => &mut self.files_gif,
            ParquetCategory::FilesSqlite => &mut self.files_sqlite,
            ParquetCategory::StringsSpans => &mut self.strings_spans,
            ParquetCategory::ArtefactsUrls => &mut self.artefacts_urls,
            ParquetCategory::ArtefactsEmails => &mut self.artefacts_emails,
            ParquetCategory::ArtefactsPhones => &mut self.artefacts_phones,
            ParquetCategory::BrowserHistory => &mut self.browser_history,
            ParquetCategory::HitsFiles => &mut self.hits_files,
        };

        if slot.is_none() {
            let filename = category.filename();
            let path = self.parquet_dir.join(filename);
            let writer = CategoryWriter::new(path, category, self.row_group_size)?;
            *slot = Some(writer);
        }

        // Safe to unwrap: ensured above.
        Ok(slot.as_mut().unwrap())
    }

    /// Helper: derive the ParquetCategory for a given file type.
    ///
    /// IMPLEMENTATION NOTE:
    ///   - For now, use simple mapping: file.file_type == "jpeg" → FilesJpeg, etc.
    ///   - Extend when you add more handlers.
    fn category_for_file(file: &CarvedFile) -> Option<ParquetCategory> {
        match file.file_type.as_str() {
            "jpeg" | "jpg" => Some(ParquetCategory::FilesJpeg),
            "png" => Some(ParquetCategory::FilesPng),
            "gif" => Some(ParquetCategory::FilesGif),
            "sqlite" | "sqlite_db" => Some(ParquetCategory::FilesSqlite),
            _ => None, // unknown/unsupported for Parquet
        }
    }
}

/// Implement the MetadataSink trait for ParquetSink.
///
/// NOTE: Calls to this sink are expected to come from a single metadata writer
/// thread, so internal mutability via &mut self is sufficient.
impl MetadataSink for ParquetSink {
    fn record_file(&self, _file: &CarvedFile) -> Result<(), MetadataError> {
        // IMPORTANT:
        //   - This trait method uses &self, but we need &mut self to append rows.
        //   - In practice, the metadata writer thread will own a mutable ParquetSink.
        //   - Adjust trait and usage accordingly in crate::metadata, or wrap self in a Mutex.
        //
        // For the skeleton, we only provide the logical steps as comments.

        // TODO (Agent):
        //   1. Change MetadataSink trait to take &mut self if allowed,
        //      OR wrap ParquetSink in a Mutex/RwLock at the call site.
        //   2. Implement the following logic:
        //
        //   - Determine category via `ParquetSink::category_for_file(file)`.
        //   - If None, return Ok(()) to silently ignore unsupported types.
        //   - Build a `FilesRow` from `file` and run-level context
        //     (run_id, tool_version, config_hash, evidence_path, evidence_sha256).
        //   - Acquire mutable reference to the relevant CategoryWriter via
        //     `get_or_create_writer(category)`.
        //   - Call `writer.append_files_row(row)`.
        //
        Ok(())
    }

    fn record_string(&self, _artefact: &StringArtefact) -> Result<(), MetadataError> {
        // TODO (Agent):
        //   - Similar mutability issue as record_file.
        //   - Decide which category to use based on `_artefact.artefact_kind`:
        //       Url   -> ParquetCategory::ArtefactsUrls
        //       Email -> ParquetCategory::ArtefactsEmails
        //       Phone -> ParquetCategory::ArtefactsPhones
        //       GenericString -> optionally ignore or store in another file.
        //   - Build corresponding Row type:
        //       - UrlArtefactRow
        //       - EmailArtefactRow
        //       - PhoneArtefactRow
        //   - Populate fields:
        //       run_id, tool_version, config_hash, evidence_path
        //       global_start/global_end from artefact
        //       parsed URL/email/phone fields
        //       source_kind/source_detail (e.g. "string_span"/"strings_spans")
        //       certainty (default 1.0)
        //   - Append to category writer.
        Ok(())
    }

    fn record_history(&self, _record: &BrowserHistoryRecord) -> Result<(), MetadataError> {
        // TODO (Agent):
        //   - Mutability as above.
        //   - Use ParquetCategory::BrowserHistory.
        //   - Build BrowserHistoryRow from record + run-level context.
        //   - Append to browser_history writer via get_or_create_writer.
        Ok(())
    }

    fn flush(&self) -> Result<(), MetadataError> {
        // TODO (Agent):
        //   - Ensure all CategoryWriter instances flush buffered rows and close files.
        //   - This may require &mut self; adjust trait or usage.
        //
        // Pseudocode if &mut self were available:
        //
        //   if let Some(w) = &mut self.files_jpeg { w.flush()?; }
        //   if let Some(w) = &mut self.files_png { w.flush()?; }
        //   ...
        //
        Ok(())
    }
}

// OPTIONAL: Implement Drop for ParquetSink to auto-flush on drop.
// NOTE: Errors in Drop cannot be handled cleanly; primary flush path
// should be via MetadataSink::flush().
impl Drop for ParquetSink {
    fn drop(&mut self) {
        // TODO (Agent):
        //   - Best-effort flush; ignore errors or log them using a global logger
        //     if one is available.
        //
        //   self.files_jpeg.as_mut().map(|w| { let _ = w.flush(); });
        //   self.files_png.as_mut().map(|w| { let _ = w.flush(); });
        //   ...
    }
}

/// Factory function used by crate::metadata::build_sink to construct a ParquetSink.
///
/// PARAMETERS:
///   - cfg: global configuration.
///   - run_id: unique identifier for this run.
///   - tool_version: version string of the program.
///   - config_hash: hash of the active configuration.
///   - evidence_path: path to evidence file/device.
///   - evidence_sha256: hash of evidence (if known).
///   - run_output_dir: directory for this run (will contain `parquet/`).
///
/// RETURN:
///   - A boxed ParquetSink implementing MetadataSink.
pub fn build_parquet_sink(
    cfg: &Config,
    run_id: String,
    tool_version: String,
    config_hash: String,
    evidence_path: PathBuf,
    evidence_sha256: String,
    run_output_dir: PathBuf,
) -> Result<Box<dyn MetadataSink>, MetadataError> {
    let sink = ParquetSink::new(
        cfg,
        run_id,
        tool_version,
        config_hash,
        evidence_path,
        evidence_sha256,
        run_output_dir,
    )?;
    Ok(Box::new(sink))
}

// TODO (Agent): add unit tests for ParquetSink in a separate test module/file:
//
//   - Create temporary directory.
//   - Instantiate ParquetSink with fake run context.
//   - Call record_file / record_string / record_history with sample data.
//   - Call flush().
//   - Assert that expected .parquet files exist.
//   - Optionally, read back Parquet files and assert row counts/values.
```
