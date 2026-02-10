# SQLite WAL + Page Fragment Carving (Carve-Only)

**Status:** Completed  
**Priority:** High  
**Effort:** Medium/High  
**Scope Type:** Carving only (no parsing/extraction logic)

---

## Problem Statement

SwiftBeaver currently carves full SQLite databases using the `SQLite format 3\0` header. This misses two important forensic cases:

1. **WAL sidecar data** (`*.sqlite-wal`) containing recent transactions not checkpointed into the main DB.
2. **Orphaned SQLite pages/fragments** that survive deletion/corruption after the main DB header is gone.

The project is intentionally carve-focused now, so we should recover these artifacts as files and defer interpretation to external tools.

---

## Goals

1. Add a **WAL file carver**.
2. Add a **SQLite page fragment carver** (raw page-size chunks with SQLite page structure validation).
3. Keep implementation strictly carve-only:
   - no browser-history/cookie/download parsing in pipeline workers
   - no record-level interpretation in this feature
4. Add deterministic **golden-image fixtures** for WAL and page-fragment scenarios.
5. Add automated tests covering correctness and false-positive boundaries.

---

## Non-Goals

- Reconstructing full SQLite DBs from fragments.
- Parsing WAL frames into rows.
- Parsing carved pages into URLs/timestamps.
- Cross-page logical correlation.
- Full filesystem-aware unallocated scanner (separate feature).

---

## Feature A: WAL Carver

### A1. Detection

- Add new file type: `sqlite_wal`.
- Signature: WAL header magic at offset 0:
  - `0x377f0682` or `0x377f0683` (big-endian)
- Secondary validation:
  - plausible page size field (`>= 512`, power-of-two or SQLite-supported value)
  - valid WAL header salt/checksum layout length

### A2. Carving Strategy

- Start at signature offset.
- Parse WAL header to get page size.
- Walk frames using frame layout:
  - frame header (24 bytes)
  - page payload (`page_size`)
- Stop when:
  - frame would exceed evidence length,
  - page number is invalid (0),
  - or checksum/frame-structure sanity fails repeatedly.
- Emit carved file as `.sqlite-wal`.

### A3. Output & Metadata

- Use existing carved file metadata path (`carved_files.*`), with:
  - `file_type = "sqlite_wal"`
  - `extension = "sqlite-wal"`
- No browser metadata emission.

### A4. Code Touchpoints

- `config/default.yml` (new `sqlite_wal` file type entry)
- `src/carve/mod.rs` (registry wiring)
- `src/carve/sqlite_wal.rs` (new handler)
- `src/util.rs` (registry build)

---

## Feature B: SQLite Page Fragment Carver

### B1. Detection

Raw single-byte signatures are noisy; use a validation-first approach.

- Candidate markers:
  - table leaf page: `0x0D`
  - index leaf page: `0x0A` (optional phase 2)
  - interior pages optional later (`0x05`, `0x02`)
- Validate page header structure against one or more target page sizes:
  - `cell_count` sane
  - `cell_content_area` bounds valid
  - cell pointers within page bounds

### B2. Page-Size Policy

Attempt page sizes in order: `[4096, 1024, 2048, 8192, 16384, 32768, 65536, 512]`.

Pick first size passing structural checks; if none pass, reject candidate.

### B3. Carving Strategy

- Carve exactly one validated page-sized chunk per accepted hit.
- File type: `sqlite_page`.
- Extension: `sqlite-page`.
- Optional phase 2:
  - contiguous multi-page run carving if adjacent pages also validate.

### B4. False-Positive Controls

- Reject if `cell_count == 0` unless explicitly enabled.
- Reject if pointer table overlaps cell content area.
- Reject if freeblock chain loops/out-of-bounds.
- Cap per-chunk page hits to prevent output explosion.

### B5. Code Touchpoints

- `config/default.yml` (new `sqlite_page` file type entry)
- `src/carve/sqlite_page.rs` (new handler)
- `src/carve/mod.rs` and `src/util.rs` (registry wiring)
- `src/cli.rs` / `src/config.rs` (optional tuning flags, if needed)

---

## Golden Image Additions

Add deterministic samples under `tests/golden_image/samples/databases/`:

1. `browser_history_wal.sqlite` (base DB)
2. `browser_history_wal.sqlite-wal` (contains uncheckpointed frames)
3. `browser_history_deleted.sqlite` (deleted-row case to leave raw pages)
4. `sqlite_orphan_page.bin` (optional synthetic isolated page bytes for direct page-carver tests)

### Fixture Generation

- Extend `tests/golden_image/samples/generate_missing.sh` with fixed timestamps/content.
- Avoid dynamic wall-clock values (`date`) in fixture content.
- Ensure WAL sidecar is explicitly preserved after creation.

### Manifest

- Regenerate via `tests/golden_image/generate.sh`.
- Commit updated `tests/golden_image/manifest.json`.
- Regenerate/commit `tests/golden_image/golden.E01` if repository policy expects E01 parity.

---

## Test Plan

### Unit Tests

1. WAL header validation accepts valid magic values, rejects invalid.
2. WAL frame walker stops safely on malformed/truncated frames.
3. Page-header validator accepts valid SQLite page structures.
4. Page-header validator rejects random/noisy buffers.

### Carver Integration Tests

1. `tests/carver_sqlite_wal.rs`
   - verifies WAL samples are carved at manifest offsets
   - verifies hashes and sizes match manifest
2. `tests/carver_sqlite_page.rs`
   - verifies page fragment samples are carved
   - verifies low false-positive behavior against noise sample

### Golden Tests

Extend existing golden tests to assert:

1. `sqlite_wal` carved count >= expected fixture count.
2. `sqlite_page` carved count >= expected fixture count (if feature enabled in config).
3. Existing carve categories unaffected.
4. Browser metadata files remain empty in carve-only mode.

---

## Rollout Plan

### Phase 1 (WAL)

1. Add `sqlite_wal` file type + handler.
2. Add focused unit tests.
3. Add golden WAL fixtures + carver test.

### Phase 2 (Page Fragment)

1. Add `sqlite_page` handler with structural validation.
2. Add per-chunk cap and heuristics tuning.
3. Add synthetic + golden fixtures and tests.

### Phase 3 (Hardening)

1. Benchmark throughput/false positives on large images.
2. Add config tuning knobs if needed (including optional per-chunk cap for `sqlite_page` single-byte marker hits).
3. Document carve-only workflow for external parser handoff.

### Implementation Status (Current)

- **Completed**:
  - Phase 1 WAL handler + wiring + tests
  - Phase 2 SQLite page handler + wiring + synthetic/golden fixtures + tests
  - Per-chunk `sqlite_page` hit cap and tuning knobs
  - WAL rolling checksum validation and configurable consecutive-failure stop threshold
  - Phase 3 throughput/false-positive benchmarking on large image sample (`/home/urias/coding/images/Image_local/test_local_win11.E01` with `.E02` segment)
  - Default `sqlite_page_max_hits_per_chunk` tuning from `4096` to `2048` based on benchmark tradeoffs
  - Dedicated carve-only handoff documentation for external parsers (`docs/sqlite_carve_handoff.md`)
- **Remaining**:
  - None for this carve-only scope

### Phase 3 Benchmark Snapshot

Command profile:

- `--types sqlite_page,sqlite_wal`
- `--chunk-size-mib 128`
- `--max-chunks 32` (~4 GiB sampled)
- `--progress-interval-secs 0`

Results (same image window):

| `sqlite_page_max_hits_per_chunk` | `hits_found` | `files_carved` | `sqlite_page` carved | `sqlite_wal` carved | elapsed |
|---|---:|---:|---:|---:|---:|
| 4096 | 131091 | 15 | 9 | 6 | 28.26s |
| 2048 | 65555 | 10 | 4 | 6 | 16.46s |
| 1024 | 32787 | 9 | 3 | 6 | 13.92s |
| 512 | 16403 | 6 | 0 | 6 | 11.71s |
| 256 | 8211 | 6 | 0 | 6 | 12.28s |

Tuning decision:

- `2048` was selected as the new default to cut scanner pressure substantially while still recovering `sqlite_page` outputs in this large-image sample.

---

## Risks and Mitigations

1. **False positives for page carving**
   - Mitigation: strict structural checks, hit caps, size heuristics.
2. **Over-carving from noisy WAL-like data**
   - Mitigation: frame-level sanity and stop thresholds.
3. **Performance overhead**
   - Current state: `sqlite_page` starts from single-byte marker candidates (`0x0D`, `0x0A`) which can produce high scanner hit volumes on large evidence.
   - Mitigation: cheap prechecks before deep validation; bounded candidate scans; optional hit-capping in hardening phase.
4. **WAL checksum threshold semantics**
   - Current state: checksum failure threshold controls **when carving stops**; it does not remove already traversed mismatching frame bytes from the carved output.
   - Mitigation: document semantics clearly; set threshold to `0` for strict stop-on-first-mismatch behavior.

---

## Acceptance Criteria

1. SwiftBeaver carves valid WAL files into `carved/sqlite_wal/`.
2. SwiftBeaver carves validated SQLite pages/fragments into `carved/sqlite_page/`.
3. No in-pipeline row parsing is introduced.
4. Golden fixtures include WAL + fragment cases and are deterministic.
5. `cargo test golden` and new carver tests pass.
