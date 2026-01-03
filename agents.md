# agents.md

Guidelines for AI Coding Agents Working on `SwiftBeaver`

This document defines how AI coding agents should work on this repository: how to structure code, keep tests and documentation in sync, and maintain planning artefacts.  
**Follow this document strictly.** If something is unclear, prefer conservative changes and keep behaviour backward compatible.

---

## 1. General Behaviour

1. **Respect existing architecture and abstractions.**  
   - Do not introduce new global patterns or frameworks without strong reasons.
   - Prefer extending existing traits, modules, and data structures over inventing parallel ones.

2. **Keep changes minimal and focused.**  
   - One feature / bug fix per pull request or commit set.
   - Avoid unrelated refactors mixed into functional changes.

3. **Preserve forensic properties.**  
   - Never modify evidence data.
   - Never write to input images or devices.
   - Maintain run-level provenance (run_id, tool_version, config_hash, evidence identifiers).

4. **Prefer explicitness over magic.**  
   - Avoid hidden side-effects.
   - Use clear, explicit configuration and function parameters.

---

## 2. Project Structure Conventions

The repository follows a structured layout. Agents **must not** arbitrarily change this layout:

- Core Rust project:
  - `src/` — core library and binary code.
  - `tests/` — integration and performance tests.
  - `examples/` — small runnable examples.

- Documentation:
  - `docs/` — **all user-facing and developer-facing documentation**.
    - Keep this directory up to date whenever you add or change features.
    - Include architecture notes, format specs, and usage guides.

- Planning:
  - `planning/features/` — **new / planned features**.
    - Each feature should have its own markdown file describing:
      - Problem statement
      - Scope and out-of-scope
      - Design notes
      - Open questions (if any)
  - `planning/done/` — **completed features**.
    - When a feature is implemented, tested, and documented:
      - Move its planning document from `planning/features/` to `planning/done/`.
      - Optionally add a short “implemented in version X.Y.Z” note at the top.

- Top-level:
  - `README.md` — **must always contain up-to-date instructions on how to use the project**.
  - `LICENSE` — license information.
  - `agents.md` — this file.

Agents **must** respect these directories and update them where required.

---

## 3. Workflow for Implementing a Feature or Fix

For each feature or bugfix, follow this workflow:

1. **Check planning files.**
   - If this is a new feature:
     - Create a new planning document under `planning/features/` (if it does not exist).
   - If a planning document already exists:
     - Read it and follow the spec.

2. **Design before coding.**
   - Validate that the requested change fits the existing architecture.
   - If design changes are needed:
     - Update the relevant `planning/features/` document with a short “Design update” section.

3. **Implement code changes.**
   - Modify or extend the relevant module(s) only.
   - Keep functions small and focused.
   - Preserve public APIs unless the planning explicitly allows changes.

4. **Add or update tests.**
   - For **every non-trivial change**, add or update tests in `tests/` or appropriate unit test modules under `src/`.
   - Ensure tests cover:
     - Expected success cases.
     - Typical error conditions.
   - Tests must be deterministic and not rely on external network access.

5. **Update documentation.**
   - Update or add relevant documentation under `docs/`.
   - If user-facing behaviour or CLI changes:
     - Update `README.md` to reflect new usage, options, or outputs.

6. **Mark feature as done (when complete).**
   - When the feature is fully implemented, tested, and documented:
     - Move its document from `planning/features/` to `planning/done/`.
     - Add a short note to that file describing:
       - Implementation status
       - Version or commit hash (if known)
     - Ensure `README.md` and `docs/` reflect the final behaviour.

---

## 4. Testing Requirements

Agents must treat tests as **mandatory**, not optional.

1. **Always run tests logically.**
   - After implementing or modifying functionality:
     - Assume `cargo test` will be executed and must pass.
   - If adding dependencies or features, consider whether additional tests are required.

2. **Add tests for new behaviour.**
   - Any new core module or important path must have:
     - Unit tests where feasible.
     - Integration tests if it affects cross-module behaviour (e.g. pipeline, metadata outputs).

3. **Keep tests in sync with behaviour.**
   - If a change modifies outputs (e.g. Parquet schemas, JSON structure, CLI flags):
     - Update tests accordingly.
   - Do *not* comment out failing tests; instead, fix the underlying cause or update expectations if behaviour is intentionally changed.

4. **Specific to Parquet and metadata (example):**
   - When touching metadata sinks (JSONL, CSV, Parquet, SQLite, DuckDB):
     - Add tests that:
       - Write sample records.
       - Flush/close the sink.
       - Confirm required files/tables exist.
       - Optionally read them back and verify fields.

---

## 5. Documentation Guidelines

### 5.1 `/docs` directory

- Use `/docs` for:
  - Architecture documents (e.g. pipeline diagrams, module descriptions).
  - Format specifications (e.g. Parquet schemas, record models).
  - How-to guides for developers (e.g. adding a new file-type handler, adding a new artefact parser).
- Whenever behaviour, format, or architecture changes:
  - Update the relevant document under `/docs`.
- If a new major component is added (e.g. a GPU scanner, a new artefact module):
  - Create a new document under `/docs` and link it from any relevant overview.

### 5.2 `README.md`

`README.md` must always stay **usable for a new user**. Agents must ensure that, after changes:

1. **Basic usage stays correct.**
   - Verify that:
     - CLI flags documented in the README actually exist and behave as described.
     - Example commands are still valid.
     - References to config files, output directories, and formats are up to date.

2. **New features affecting the user are documented.**
   - If you add a new flag, mode, or output format:
     - Add a short description and one example invocation in `README.md`.
   - Keep the README **succinct**; detailed explanations belong in `/docs`.

3. **Cross-reference `docs`.**
   - When appropriate, mention relevant documents in `/docs` for deeper detail.

---

## 6. Planning Files: Features vs Done

Agents should use `planning/features` and `planning/done` to keep track of work.

### 6.1 New features (`planning/features/`)

- When a new feature is proposed or started, create a new file:
  - `planning/features/<short-name>.md`
- This file should contain:
  - **Title and short description**
  - **Problem statement**
  - **Scope**
  - **Non-goals**
  - **Design notes**
  - **Expected tests**
  - **Impact on docs and README**

### 6.2 Completed features (`planning/done/`)

- When the feature is fully implemented:
  - Move the file to `planning/done/<short-name>.md`.
  - Add a section at the top:
    - `Status: Implemented`
    - `Implemented in version: <version>` or `Implemented in commit: <hash>` (if known).
- Do not delete or overwrite planning documents; they serve as historical records.

---

## 7. Code Style & Quality

1. **Follow Rust conventions.**
   - Idiomatic Rust: ownership, lifetimes, error handling with `Result` and error enums.
   - Use `clippy`-friendly patterns where possible.

2. **Run `cargo fmt` before finishing work.**
   - **Always** run `cargo fmt` after making code changes to ensure consistent formatting.
   - CI enforces formatting via `cargo fmt --check` — code that is not formatted will fail the pipeline.
   - This is **mandatory** before committing or finishing any task.

3. **Error handling.**
   - Do not panic in library code for expected error conditions.
   - Use structured error types (e.g. `thiserror`) and propagate with `Result`.

4. **Logging.**
   - Use the existing logging infrastructure (`tracing`).
   - Do not print directly to stdout/stderr except where explicitly intended.

5. **Configuration.**
   - All tunables (chunk sizes, overlaps, row group sizes, etc.) should either:
     - Be part of configuration, or
     - Have clearly documented defaults.

6. **Public APIs & formats are contracts.**
   - If you change a public API, CLI, or on-disk format (e.g. Parquet schemas), treat it as a contract change:
     - Update `/docs`.
     - Update `README.md` if user-facing.
     - Update tests.
     - Update any relevant planning document with a note.

---

## 8. Specific Notes for Parquet and Metadata

When extending or touching Parquet and metadata-related code:

1. **Keep schemas stable and documented.**
   - For each Parquet category, ensure there is a corresponding schema description in `/docs`.
   - Fields in code must match names and types in docs.

2. **One file per category per run.**
   - Do not merge multiple categories into a single Parquet unless explicitly requested.
   - Do not split a category across multiple files unless the design is updated in `/docs` and planning.

3. **Provenance fields are mandatory.**
   - `run_id`, `tool_version`, `config_hash`, and `evidence_path` must be present in all rows.
   - If `evidence_sha256` is known, include it as specified.

4. **Testing:**
   - Always add tests that:
     - Produce at least one row in each modified category.
     - Verify that Parquet files are created and contain data.

---

## 9. Summary for Agents

When you work on this repository, always:

1. **Read `agents.md` and relevant `/docs` before changing code.**
2. **Update code, tests, planning, and docs together.**
3. **Run `cargo fmt` before finishing work** — CI will reject unformatted code.
4. **Ensure `README.md` stays correct and usable after your changes.**
5. **Preserve the forensic nature and reproducibility of the tool.**
6. **Keep changes well-scoped and traceable to a feature document in `planning/features` or `planning/done`.**

If in doubt, prefer:

- Smaller, focused changes.
- Clear documentation in `/docs`.
- Additional tests that validate the intended behaviour.
