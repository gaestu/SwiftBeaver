# Hash Calculation & Deduplication

**Status:** WIP  
**Priority:** High  
**Effort:** Medium  

---

## Problem Statement

Currently, `SwiftBeaver` extracts files from evidence images but does not compute cryptographic hashes for carved files. Forensic workflows require:

1. **Integrity verification** — Hashes prove a carved file hasn't been modified post-extraction.
2. **Deduplication** — Large images often contain duplicate files; identifying them saves analysis time.
3. **Known-file matching** — Comparing against hash sets (NSRL, malware databases, organization-specific lists) to filter or flag files.

Without hashes, analysts must run separate tools post-carving, breaking the forensic workflow.

---

## Scope

### In Scope

1. **Compute hashes for every carved file:**
   - MD5 (legacy compatibility, widely used in forensics)
   - SHA1 (legacy, still common)
   - SHA256 (modern standard)

2. **Record hashes in metadata outputs:**
   - Add `md5`, `sha1`, `sha256` fields to `carved_files` records
   - Support all backends (JSONL, CSV, Parquet)

3. **Deduplication tracking:**
   - Track unique files by SHA256
   - Add `duplicate_of` field pointing to first occurrence offset
   - Add `is_duplicate` boolean field

4. **CLI options:**
   - `--hash-algorithms md5,sha1,sha256` — select which hashes to compute (default: sha256)
   - `--dedupe` — enable deduplication tracking
   - `--skip-duplicates` — don't write duplicate files to disk (only record metadata)

5. **Config file support:**
   - `hash_algorithms: [sha256]`
   - `enable_deduplication: false`
   - `skip_duplicate_files: false`

### Out of Scope (Future Work)

- Hash set matching (known-good/known-bad lists) — separate feature
- Fuzzy hashing (ssdeep, TLSH) — separate feature
- Partial/streaming hash for very large files — may revisit if needed

---

## Design Notes

### Hash Computation Location

Hashes should be computed **during carve validation**, not as a post-process:

```
CarveResult {
    file_type: String,
    offset: u64,
    size: u64,
    output_path: PathBuf,
    md5: Option<String>,
    sha1: Option<String>,
    sha256: Option<String>,
    is_duplicate: bool,
    duplicate_of: Option<u64>,  // offset of first occurrence
}
```

### Deduplication Strategy

1. Maintain a `HashMap<[u8; 32], u64>` mapping SHA256 → first occurrence offset
2. After computing SHA256, check if hash exists:
   - If new: insert into map, `is_duplicate = false`
   - If exists: `is_duplicate = true`, `duplicate_of = first_offset`
3. If `--skip-duplicates`, don't write file to disk but still emit metadata record

### Performance Considerations

- Hash computation is CPU-bound; leverage existing worker threads
- For large files, consider computing hash during the read pass (streaming)
- MD5/SHA1/SHA256 can be computed in parallel on different cores
- Use `sha2` crate (already in deps), add `md-5` and `sha1` crates

### Thread Safety for Deduplication

- Dedup map must be shared across carve workers
- Use `Arc<RwLock<HashMap<...>>>` or `DashMap` for concurrent access
- Read-heavy workload suggests `RwLock` is appropriate

---

## Implementation Plan

### Phase 1: Core Hash Infrastructure

1. **Add dependencies to Cargo.toml:**
   ```toml
   md-5 = "0.10"
   sha1 = "0.10"
   ```

2. **Create `src/hash.rs` module:**
   - `HashConfig` struct with enabled algorithms
   - `compute_hashes(data: &[u8], config: &HashConfig) -> FileHashes`
   - `FileHashes` struct with optional md5/sha1/sha256 strings

3. **Update `CarvedFileRecord` in metadata:**
   - Add `md5: Option<String>`
   - Add `sha1: Option<String>`  
   - Add `sha256: Option<String>`
   - Add `is_duplicate: bool`
   - Add `duplicate_of_offset: Option<u64>`

### Phase 2: Integration with Carve Pipeline

4. **Update carve handlers:**
   - After successful validation, compute hashes on carved data
   - Return hashes in carve result

5. **Update `CarveRegistry`:**
   - Accept `HashConfig` parameter
   - Pass config to individual carvers

6. **Update metadata sinks:**
   - JSONL: add hash fields to JSON output
   - CSV: add hash columns
   - Parquet: add hash columns to schema

### Phase 3: Deduplication

7. **Create deduplication tracker:**
   - `DedupTracker` struct with thread-safe hash map
   - `check_and_register(sha256: &str, offset: u64) -> DedupResult`

8. **Integrate with pipeline:**
   - Pass `DedupTracker` to carve workers
   - Update file writing logic to respect `skip_duplicates`

### Phase 4: CLI & Config

9. **Update `cli.rs`:**
   - Add `--hash-algorithms` option
   - Add `--dedupe` flag
   - Add `--skip-duplicates` flag

10. **Update `config.rs`:**
    - Add hash/dedup configuration fields
    - Update `default.yml`

### Phase 5: Testing & Documentation

11. **Add tests:**
    - Unit tests for hash computation
    - Integration test for deduplication
    - Test metadata output includes hashes

12. **Update documentation:**
    - Update README.md with new options
    - Update docs/metadata_*.md with new fields
    - Update docs/config.md with new settings

---

## Expected Tests

- `tests/hash_computation.rs` — verify correct hash output for known inputs
- `tests/deduplication.rs` — verify duplicate detection and skip behavior
- Integration tests verifying metadata contains hash fields
- Performance benchmark comparing with/without hashing

---

## Impact on Docs and README

- **README.md:** Add hash options to CLI reference, mention deduplication capability
- **docs/config.md:** Document `hash_algorithms`, `enable_deduplication`, `skip_duplicate_files`
- **docs/metadata_jsonl.md:** Add hash fields to carved_files schema
- **docs/metadata_csv.md:** Add hash columns
- **docs/metadata_parquet.md:** Add hash columns to schema

---

## Open Questions

1. Should we support hash computation during streaming (before full file is carved) for very large files?
2. Should deduplication be based on SHA256 only, or allow user to choose?
3. For `--skip-duplicates`, should we create a symlink to the original instead of nothing?
