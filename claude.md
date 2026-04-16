# agents.md

Guidelines for AI Coding Agents Working on **SwiftBeaver**

SwiftBeaver is a high-speed, forensic-grade file and artefact carver written in Rust. This document defines how AI agents must work on this repository: the coding procedure, review process, code standards, and planning workflow.

**Follow this document strictly.** If something is unclear, prefer conservative changes and keep behaviour backward compatible.

---

## 1. Project Overview

| Aspect | Details |
|--------|---------|
| Language | Rust (edition 2024) |
| Build | `cargo build`, `cargo test`, `cargo fmt`, `cargo clippy` |
| CI | GitHub Actions (`.github/workflows/ci.yml`) |
| License | Apache-2.0 |

### Directory Structure

```
src/           Core library and binary code
tests/         Integration and unit tests
benches/       Performance benchmarks
config/        Default configuration (default.yml)
docs/          User and developer documentation
planning/
  features/    Planned/in-progress feature specs
  done/        Completed feature specs
  wip/         Work-in-progress drafts
```

### Key Forensic Principles

1. **Never modify evidence** — input images/devices are read-only
2. **Provenance is mandatory** — every output row includes `run_id`, `tool_version`, `config_hash`, `evidence_path`
3. **Reproducibility** — same input + config = same output

---

## 2. Core Rules

### 2.1 No Hardcoded Versions

**Never write literal version strings in code.** Use:

```rust
// ✅ Correct
env!("CARGO_PKG_VERSION")

// ❌ Wrong — creates version drift
"0.3.0"
```

This applies to:
- Source code
- Documentation referencing specific versions (use "current version" or omit)
- Planning documents (use "Implemented" without version numbers)

### 2.2 Minimal, Focused Changes

- One feature or bug fix per task
- Do not mix unrelated refactors with functional changes
- Extend existing traits/modules rather than inventing parallel abstractions

### 2.3 Explicit Over Magic

- No hidden side-effects
- Clear, explicit function parameters
- Configuration over convention

---

## 3. Coding Procedure

Follow this pipeline for every non-trivial task. Skip to Phase 2 for simple, well-defined changes.

### Phase 1: Research (main agent)

1. Use `search_subagent` or `runSubagent` (Explore) to gather codebase context
2. Identify affected files, existing patterns, and test coverage
3. If requirements are ambiguous → ask the user via `vscode_askQuestions`
4. **Do NOT proceed until all open questions are resolved**

### Phase 1b: Implementation Plan (issue-driven work only)

If the task originates from a GitHub Issue:

1. Write a structured implementation plan:
   - Affected files and planned changes
   - New files to create
   - Key design decisions and trade-offs
2. Post the plan as a comment on the GitHub Issue
3. This creates a traceable link between plan and implementation

### Phase 2: Implementation

Launch a `runSubagent` with a detailed prompt containing:
- Full task description
- All research findings from Phase 1 (file paths, code snippets, patterns)
- Explicit constraints (forensic rules, error handling rules, etc.)

The coding subagent implements the change and returns a summary of all modified/created files.

### Phase 3: Review (6 mandatory reviewers + 1 conditional)

Launch **all 6 core reviewers in parallel** as subagents. Each receives:
- The task description
- The full content of every changed/created file

| Reviewer | Focus | Pass Criteria |
|----------|-------|---------------|
| **Correctness** | Logic flaws, bugs, edge cases, off-by-one errors, data integrity | No logic issues found |
| **Forensic Safety** | Evidence read-only, provenance fields present, reproducible outputs, no evidence mutation | All forensic rules satisfied |
| **Error Handling** | No `.unwrap()` in lib code, proper `Result` propagation, meaningful error context | Robust error handling confirmed |
| **Security** | Buffer bounds, path traversal prevention, input validation, no arbitrary file writes outside output dir | No security concerns |
| **Architecture** | Fits existing patterns, no unnecessary dependencies, code quality, style consistency, no bloat | Clean architecture |
| **Documentation** | README reflects changes, `/docs/` updated for behavioral changes, examples valid | Docs in sync with code |

Each reviewer returns: **"✅ PASS"** or **"❌ ISSUES: [numbered list]"**

#### Conditional Reviewer (issue-driven work only)

| Reviewer | Focus | Pass Criteria |
|----------|-------|---------------|
| **Issue Completeness** | Every requirement/acceptance criterion from the Issue is addressed, implementation matches plan | All requirements met |

Receives: GitHub Issue body, implementation plan from Phase 1b, summary of changes.

### Phase 4: Decision

```
IF all reviewers pass:
    → Proceed to Phase 5

ELSE:
    → Fix the identified issues
    → Re-run ONLY the failed reviewers (not all)
    → Max 3 fix iterations before escalating to user
```

### Phase 5: Finalize

1. **Format code:**
   ```bash
   cargo fmt
   ```

2. **Run linter:**
   ```bash
   cargo clippy --all-targets --all-features -- -D warnings
   ```

3. **Run tests:**
   ```bash
   cargo test
   ```

4. **If all pass:** Present summary + proposed commit message to user

5. **If tests fail:** Fix and re-run (max 3 attempts before escalating)

---

## 4. Code Style & Quality

### 4.1 Rust Conventions

- Idiomatic Rust: ownership, lifetimes, proper error handling
- Use `clippy`-friendly patterns
- Run `cargo fmt` before finishing — **CI will reject unformatted code**

### 4.2 Error Handling

```rust
// ✅ Library code: propagate errors
pub fn parse_header(data: &[u8]) -> Result<Header, CarveError> {
    let magic = data.get(0..4).ok_or(CarveError::TooShort)?;
    // ...
}

// ❌ Never in library code
data.get(0..4).unwrap()
```

- Use `thiserror` for structured error types
- Use `anyhow` only in binary/CLI code
- Add context to errors: `.context("parsing JPEG header")?`

### 4.3 Logging

- Use `tracing` (not `println!` or `eprintln!`)
- Appropriate levels: `error!`, `warn!`, `info!`, `debug!`, `trace!`

### 4.4 Configuration

- All tunables (chunk sizes, thresholds, limits) must be configurable or have documented defaults
- No magic numbers in code without explanation

---

## 5. Testing Requirements

Tests are **mandatory**, not optional.

### 5.1 When to Add Tests

| Change Type | Required Tests |
|-------------|----------------|
| New carver/parser | Unit tests + integration test with sample data |
| Bug fix | Regression test that would have caught the bug |
| New CLI flag | Integration test exercising the flag |
| Schema change | Test that verifies new schema fields |

### 5.2 Test Standards

- Tests must be **deterministic** — no network access, no timing dependencies
- Use `tempfile` for temporary directories
- Clean up test artifacts

### 5.3 Running Tests

```bash
# All tests
cargo test

# Specific test
cargo test test_name

# With output
cargo test -- --nocapture
```

---

## 6. Documentation & Planning

### 6.1 `/docs/` Directory

Use for:
- Architecture documents
- Format specifications (Parquet schemas, record models)
- How-to guides

**Update docs when behaviour changes.**

### 6.2 `README.md`

Must always be **usable for a new user**:
- CLI flags documented must actually exist
- Example commands must be valid
- Keep it succinct — detailed docs go in `/docs/`

### 6.3 Planning Workflow

```
planning/features/<name>.md   # New/in-progress features
        ↓ (when complete)
planning/done/<name>.md       # Completed features
```

#### Feature Document Template

```markdown
# Feature Name

## Problem Statement
What problem does this solve?

## Scope
- In scope: ...
- Out of scope: ...

## Design Notes
Key decisions and trade-offs.

## Expected Tests
What tests will be added?

## Documentation Impact
What docs/README changes needed?
```

#### Marking Complete

When a feature is done:
1. Move file from `planning/features/` to `planning/done/`
2. Add at top: `Status: Implemented`
3. Do NOT add version numbers (see Rule 2.1)

---

## 7. Forensic Safety Checklist

Reviewers must verify these for **every change**:

### Evidence Integrity
- [ ] No writes to input evidence (images, devices, files)
- [ ] Input sources opened read-only
- [ ] No in-place modifications of source data

### Provenance
- [ ] All output rows include: `run_id`, `tool_version`, `config_hash`, `evidence_path`
- [ ] `evidence_sha256` included when available
- [ ] Timestamps are UTC

### Reproducibility
- [ ] Same input + same config = same output
- [ ] No non-deterministic behaviour (random, time-based seeds)
- [ ] Processing order does not affect results

### Output Safety
- [ ] All carved files go to designated output directory only
- [ ] No path traversal vulnerabilities
- [ ] Filenames are sanitized

---

## 8. Parquet & Metadata

### 8.1 Schema Stability

- Document schemas in `/docs/metadata_parquet.md`
- Field names in code must match docs
- Schema changes require doc updates + migration notes

### 8.2 File Organization

- One Parquet file per category per run
- Categories: `carved_files`, `string_artefacts`, `browser_history`, `browser_cookies`, `browser_downloads`, `run_summary`, `entropy_regions`

### 8.3 Required Fields

Every row must include:
```
run_id: String
tool_version: String
config_hash: String
evidence_path: String
evidence_sha256: Option<String>
```

---

## 9. Quick Reference

### Github interaction

```
For interacting with github use if possible 
1. github mcp
2. gh (github cli)

Repo Owner is: gaestu
Project Name is: SwiftBeaver
Repo Link is: github.com/gaestu/SwiftBeaver
```

### Before Starting Work
```
1. Read this document
2. Check planning/features/ for existing spec
3. Gather context with search_subagent
4. Ask questions if requirements unclear
```

### Before Finishing Work
```
1. All 6 reviewers passed
2. cargo fmt (mandatory)
3. cargo clippy passes
4. cargo test passes
5. Docs updated if needed
6. Planning doc moved to done/ if feature complete
```

### Commit Message Format
```
<type>: <short description>

<body explaining what and why>

Closes #<issue> (if applicable)
```

Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`
