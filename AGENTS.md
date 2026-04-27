# AGENTS.md

Guidelines for AI coding agents working on **SwiftBeaver**, a forensic-grade file and artefact carver written in Rust.

When requirements are unclear, prefer **conservative, backward-compatible changes**.

Use this file as the base instruction set. Task-specific workflows live under `prompts/`.

## Core Invariants

- **Never modify evidence.** Input images, devices, and evidence files must be opened read-only.
- **Provenance is mandatory.** Every output row must include:
  - `run_id`
  - `tool_version`
  - `config_hash`
  - `evidence_path`
- **Reproducibility is required.** The same input and config must produce the same output.
- **Output isolation is required.** Carved files may only be written to the designated output directory.
- **No path traversal.** Sanitize filenames and validate output paths.

## Project Overview

- Language: Rust (edition 2024)
- Standard checks:
  - `cargo fmt`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test`
- CI: `.github/workflows/ci.yml`

## Repository Layout

- `src/` — core library and binary code
- `tests/` — integration and unit tests
- `benches/` — performance benchmarks
- `config/` — default configuration
- `docs/` — user and developer documentation

## Code Rules

- Do not hardcode version strings. Use `env!("CARGO_PKG_VERSION")`.
- Do not use `.unwrap()` in `src/` unless clearly justified and safe.
- Prefer structured errors with `thiserror`; use `anyhow` only in CLI or binary code.
- Use `tracing` for logging, not `println!` or `eprintln!`.
- Follow existing module and trait patterns. Avoid parallel abstractions and unnecessary dependencies.
- Keep changes focused. Do not mix unrelated refactors with feature or bug-fix work.

## Parser and Carver Safety

- Validate bounds on all untrusted input.
- Handle malformed, truncated, or corrupt data safely.
- Avoid unchecked indexing where failure is possible.
- Guard size calculations against overflow.
- Never write back to evidence or mutate source data in place.

## Metadata and Parquet

- Field names must match `/docs/metadata_parquet.md`.
- Keep one Parquet file per category per run.
- Required fields on every row:
  - `run_id`
  - `tool_version`
  - `config_hash`
  - `evidence_path`
  - `evidence_sha256` when available

## Documentation and Tests

- Update docs when behavior, schema, or CLI usage changes.
- Add deterministic tests for new behavior and bug fixes.
- Use `tempfile` for temporary test directories when needed.
- Schema changes require corresponding documentation updates.

## Before Finishing

Run:

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```
