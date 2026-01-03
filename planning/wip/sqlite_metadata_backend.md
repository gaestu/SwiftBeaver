# SQLite Metadata Backend

**Status:** WIP  
**Priority:** High  
**Effort:** Medium  

---

## Problem Statement

`SwiftBeaver` currently supports three metadata backends: JSONL, CSV, and Parquet. While Parquet is excellent for large-scale analytics, many forensic analysts prefer to query results immediately using SQL without needing specialized tools.

SQLite offers:
- **Zero-setup querying** — any SQLite client works (sqlite3 CLI, DB Browser, Python sqlite3)
- **Single-file output** — easy to share and archive
- **Full SQL support** — JOINs, aggregations, CTEs, window functions
- **Wide tool compatibility** — integrates with virtually everything

---

## Scope

### In Scope

1. **New metadata backend: SQLite**
   - Write all metadata categories to a single `.sqlite` file
   - One table per category (carved_files, string_artefacts, browser_history, etc.)
   - Proper schema with indexes for common queries

2. **CLI integration:**
   - `--metadata-backend sqlite`

3. **Same data model** as existing backends:
   - carved_files
   - string_artefacts
   - browser_history
   - browser_cookies
   - browser_downloads
   - run_summary
   - entropy_regions

4. **Transactional writes:**
   - Batch inserts for performance
   - Proper commit on flush/close

5. **Optional: DuckDB support** (stretch goal)
   - DuckDB offers better analytical query performance
   - Similar API to SQLite

### Out of Scope

- Query interface within SwiftBeaver
- Database migration tooling
- Remote database support (PostgreSQL, MySQL)

---

## Design Notes

### Schema Design

Each category gets its own table with appropriate types:

```sql
-- Run provenance (shared fields in all tables)
-- run_id, tool_version, config_hash, evidence_path, evidence_sha256

CREATE TABLE carved_files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    tool_version TEXT NOT NULL,
    config_hash TEXT NOT NULL,
    evidence_path TEXT NOT NULL,
    evidence_sha256 TEXT,
    
    file_type TEXT NOT NULL,
    offset INTEGER NOT NULL,
    size INTEGER NOT NULL,
    output_path TEXT NOT NULL,
    md5 TEXT,
    sha1 TEXT,
    sha256 TEXT,
    is_duplicate INTEGER DEFAULT 0,
    duplicate_of_offset INTEGER,
    carved_at TEXT NOT NULL  -- ISO 8601 timestamp
);

CREATE INDEX idx_carved_files_type ON carved_files(file_type);
CREATE INDEX idx_carved_files_offset ON carved_files(offset);
CREATE INDEX idx_carved_files_sha256 ON carved_files(sha256);

CREATE TABLE string_artefacts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    tool_version TEXT NOT NULL,
    config_hash TEXT NOT NULL,
    evidence_path TEXT NOT NULL,
    evidence_sha256 TEXT,
    
    artefact_type TEXT NOT NULL,  -- url, email, phone
    value TEXT NOT NULL,
    offset INTEGER NOT NULL,
    length INTEGER NOT NULL,
    context TEXT,
    found_at TEXT NOT NULL
);

CREATE INDEX idx_string_artefacts_type ON string_artefacts(artefact_type);
CREATE INDEX idx_string_artefacts_value ON string_artefacts(value);

CREATE TABLE browser_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    tool_version TEXT NOT NULL,
    config_hash TEXT NOT NULL,
    evidence_path TEXT NOT NULL,
    evidence_sha256 TEXT,
    
    source_db_offset INTEGER NOT NULL,
    browser TEXT NOT NULL,
    url TEXT NOT NULL,
    title TEXT,
    visit_time TEXT,  -- ISO 8601
    visit_count INTEGER,
    typed_count INTEGER
);

CREATE INDEX idx_browser_history_browser ON browser_history(browser);
CREATE INDEX idx_browser_history_url ON browser_history(url);

-- Similar tables for browser_cookies, browser_downloads, entropy_regions

CREATE TABLE run_summary (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL UNIQUE,
    tool_version TEXT NOT NULL,
    config_hash TEXT NOT NULL,
    evidence_path TEXT NOT NULL,
    evidence_sha256 TEXT,
    
    start_time TEXT NOT NULL,
    end_time TEXT,
    bytes_scanned INTEGER,
    chunks_processed INTEGER,
    files_carved INTEGER,
    string_spans INTEGER,
    artefacts_extracted INTEGER,
    status TEXT  -- completed, interrupted, failed
);
```

### Implementation Architecture

```
src/metadata/
├── mod.rs          # MetadataSink trait, backend selection
├── jsonl.rs        # JSONL implementation
├── csv.rs          # CSV implementation
├── parquet.rs      # Parquet implementation
└── sqlite.rs       # NEW: SQLite implementation
```

### SQLite Sink Structure

```rust
pub struct SqliteMetadataSink {
    conn: Connection,
    batch_size: usize,
    
    // Pending records for batch insert
    pending_carved: Vec<CarvedFileRecord>,
    pending_artefacts: Vec<StringArtefactRecord>,
    pending_history: Vec<BrowserHistoryRecord>,
    // ... etc
}

impl MetadataSink for SqliteMetadataSink {
    fn record_carved_file(&mut self, record: CarvedFileRecord) -> Result<()>;
    fn record_string_artefact(&mut self, record: StringArtefactRecord) -> Result<()>;
    fn record_browser_history(&mut self, record: BrowserHistoryRecord) -> Result<()>;
    // ... etc
    fn flush(&mut self) -> Result<()>;
    fn close(self) -> Result<()>;
}
```

### Batching Strategy

- Accumulate records in memory (default batch_size: 1000)
- On batch full or flush(), execute batch INSERT
- Use prepared statements for performance
- Use transactions for atomicity

```rust
fn flush_carved_files(&mut self) -> Result<()> {
    if self.pending_carved.is_empty() {
        return Ok(());
    }
    
    let tx = self.conn.transaction()?;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO carved_files (...) VALUES (?, ?, ?, ...)"
        )?;
        for record in self.pending_carved.drain(..) {
            stmt.execute(params![
                record.run_id,
                record.file_type,
                record.offset,
                // ...
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}
```

### Thread Safety

- SQLite connection is NOT thread-safe by default
- Options:
  1. **Single writer thread** (current architecture) — metadata writer serializes all writes
  2. **Connection pool** with WAL mode — allows concurrent reads during write
  3. **Mutex-protected connection** — simple but may bottleneck

Current pipeline uses single metadata writer thread, so single connection is fine.

---

## Implementation Plan

### Phase 1: SQLite Sink Core

1. **Create `src/metadata/sqlite.rs`:**
   - `SqliteMetadataSink` struct
   - Schema creation on init
   - Implement `MetadataSink` trait

2. **Add schema creation:**
   - Create all tables with proper types
   - Create indexes for common query patterns

3. **Implement batch inserts:**
   - Prepared statements for each table
   - Transaction-wrapped batch commits

### Phase 2: Integration

4. **Update `src/metadata/mod.rs`:**
   - Add `Sqlite` variant to backend enum
   - Create SQLite sink in factory function

5. **Update `src/cli.rs`:**
   - Add `Sqlite` to `MetadataBackend` enum

6. **Update `src/main.rs`:**
   - Handle SQLite backend selection

### Phase 3: Configuration

7. **Update `config/default.yml`:**
   - Add `sqlite_batch_size` option (default 1000)
   - Add `sqlite_journal_mode` option (default WAL)

8. **Update config parsing:**
   - Parse new SQLite options

### Phase 4: Testing

9. **Create `tests/metadata_sqlite.rs`:**
   - Test schema creation
   - Test record insertion for all categories
   - Test batch flushing
   - Test querying results
   - Test concurrent reads during write (if applicable)

10. **Integration test:**
    - Run pipeline with SQLite backend
    - Verify all tables populated
    - Run sample queries

### Phase 5: Documentation

11. **Create `docs/metadata_sqlite.md`:**
    - Schema documentation
    - Example queries
    - Performance notes

12. **Update README.md:**
    - Add SQLite to metadata backend options
    - Add example usage

13. **Update docs/config.md:**
    - Document SQLite-specific options

---

## Expected Tests

- `tests/metadata_sqlite.rs`:
  - `test_sqlite_schema_creation` — verify all tables created
  - `test_sqlite_carved_file_insert` — insert and query carved files
  - `test_sqlite_string_artefact_insert` — insert and query artefacts
  - `test_sqlite_browser_history_insert` — insert and query history
  - `test_sqlite_batch_flush` — verify batching works correctly
  - `test_sqlite_close_commits` — verify close() commits pending data
  - `test_sqlite_indexes_exist` — verify indexes created

---

## Impact on Docs and README

- **README.md:** 
  - Add `--metadata-backend sqlite` to CLI options
  - Add SQLite to output metadata section
- **docs/config.md:** Document `sqlite_batch_size`, `sqlite_journal_mode`
- **docs/metadata_sqlite.md:** New file with schema and query examples

---

## Example Queries (for docs)

```sql
-- Count files by type
SELECT file_type, COUNT(*) as count, SUM(size) as total_bytes
FROM carved_files
GROUP BY file_type
ORDER BY count DESC;

-- Find all URLs from browser history
SELECT DISTINCT url, browser, visit_time
FROM browser_history
ORDER BY visit_time DESC
LIMIT 100;

-- Find email addresses and their locations
SELECT value, offset, context
FROM string_artefacts
WHERE artefact_type = 'email'
ORDER BY offset;

-- Join carved SQLite DBs with browser history
SELECT cf.output_path, bh.url, bh.title
FROM carved_files cf
JOIN browser_history bh ON cf.offset = bh.source_db_offset
WHERE cf.file_type = 'sqlite';

-- Run summary
SELECT * FROM run_summary;
```

---

## Open Questions

1. Should we also implement DuckDB in the same PR (very similar API)?
2. Should we add a `--sqlite-path` option to customize output location?
3. Should WAL mode be default (better concurrent access) or DELETE (single file)?
4. Should we add views for common query patterns?
