# Pipeline scaling and refactor plan

Short description: reduce memory usage in large scans, enforce max_files strictly,
decouple SQLite parsing from carving, and simplify pipeline/carve registry internals.

## Problem statement
- The pipeline eagerly allocates all chunks, which scales linearly with evidence size.
- `max_files` is best-effort under concurrency and can overshoot.
- SQLite parsing runs on carve workers, which can stall carving throughput.
- The pipeline orchestration function is large and hard to test in isolation.
- `build_carve_registry` is long and repetitive, making new handlers error-prone.

## Scope
- Introduce a streaming chunk iterator and a cheap chunk_count helper.
- Add a strict max_files limiter shared across pipeline + carve workers.
- Move SQLite artifact extraction to its own worker stage.
- Split the pipeline orchestration into smaller helpers or a runner struct.
- Refactor carve registry creation to reduce boilerplate while keeping behavior.

## Non-goals
- Change output formats, metadata schemas, or CLI flags.
- Modify scanner/carver algorithms or GPU backends.
- Rework evidence I/O or checkpoint formats.

## Design notes

### 1) Streaming chunk iteration
- Add `ChunkIter` in `src/chunk.rs` implementing `Iterator<Item = ScanChunk>`.
- Add `chunk_count(total_len, chunk_size) -> u64` for logging.
- Keep `build_chunks` for tests or compatibility, but move pipeline to iterator.
- Make sure `overlap` is applied the same as today.

### 2) Strict max_files limiter
- Introduce a `CarveLimiter` struct (new module or in `pipeline/mod.rs`) with:
  - `limit: Option<u64>`
  - `reserved: AtomicU64`
  - `carved: AtomicU64`
  - `try_reserve() -> bool` (uses CAS to keep `carved + reserved <= limit`)
  - `commit()` on successful carve (decrement reserved, increment carved)
  - `release()` on `Ok(None)` or `Err` (decrement reserved)
  - `should_stop()` for pipeline early-stop checks.
- Pass `Arc<CarveLimiter>` to carve workers and pipeline.
- Replace `files_carved` counter with `CarveLimiter::carved` to keep stats consistent.
- Ensure the pipeline stops when `should_stop()` becomes true.

### 3) SQLite artifact stage
- Add `SqliteJob { path, run_id, rel_path, enable_page_recovery }`.
- Create `spawn_sqlite_workers` that:
  - Reads `SqliteJob` from a channel.
  - Runs history/cookie/download extraction.
  - Sends `MetadataEvent` into `meta_tx`.
  - Updates `sqlite_errors` counter.
- In carve workers, send a `SqliteJob` instead of parsing inline.
- If SQLite scanning is disabled, do not spawn workers or channel.

### 4) Pipeline struct split
- Create `PipelineRunner` (internal) with methods like:
  - `validate_checkpoint`
  - `setup_channels`
  - `spawn_workers`
  - `scan_loop`
  - `finalize`
- Keep the public `run_pipeline*` APIs unchanged.
- Preserve logging and progress reporting behavior.

### 5) Carve registry refactor
- Replace the long `match` with a small builder table or helper macro.
- Keep special-case logic (mp4/mov quicktime, footer handlers) explicit.
- Preserve existing validators and defaults.

## Implementation plan
1. Add `ChunkIter` + `chunk_count` in `src/chunk.rs`; add unit tests that:
   - Match the existing `build_chunks` outputs.
   - Validate count and overlap edge cases.
2. Update `src/pipeline/mod.rs` to:
   - Use `chunk_count` for logging.
   - Iterate with `ChunkIter` to avoid `Vec<ScanChunk>`.
3. Implement `CarveLimiter` (new file or in `pipeline/mod.rs`), with unit tests.
4. Thread `Arc<CarveLimiter>` into:
   - `run_pipeline_inner` early-stop checks.
   - `spawn_carve_workers` to reserve/commit/release.
5. Add `SqliteJob` channel and `spawn_sqlite_workers` in `src/pipeline/workers.rs`.
6. Update `run_pipeline_inner` to:
   - Start SQLite workers when enabled.
   - Close the SQLite channel during shutdown.
   - Join SQLite worker threads in `finalize`.
7. Refactor pipeline orchestration into `PipelineRunner` helpers, preserving behavior.
8. Refactor `build_carve_registry` into a table-driven or macro approach.
9. Run `cargo fmt` and update any tests/doc references.

## Expected tests
- `src/chunk.rs` tests for `ChunkIter` and `chunk_count`.
- Unit tests for `CarveLimiter` (reservation, release, commit, should_stop).
- Integration test ensuring `max_files` is not exceeded under concurrency.
- SQLite worker test with a small SQLite fixture to verify metadata events.

## Impact on docs and README
- Update `docs/` pipeline/architecture notes to reflect:
  - Streaming chunk iteration.
  - SQLite parsing stage.
  - Strict max_files semantics.
- If max_files behavior is now strict, add a brief README note.

## Open questions
- Should `max_files` be a strict cap (never write more than N) or just strict in
  metadata counts?
- Is early-stop acceptable when `carved + reserved == limit`, or should the scan
  continue until all in-flight reservations resolve?
