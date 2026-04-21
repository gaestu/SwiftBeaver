# Review Uncommitted Changes

You are reviewing uncommitted SwiftBeaver changes before commit.

First read `AGENTS.md` and follow it strictly.

This prompt is the combined reviewer. Use the specialist review prompts when you want separate parallel review passes.

## Review Focus

Check changed files for:

1. Correctness
2. Forensic safety
3. Error handling
4. Security
5. Architecture
6. Documentation, only when behavior, CLI, schema, or docs-relevant files changed

## Critical Checks

- No writes to evidence or source images; inputs must be opened read-only.
- Output rows must include:
  - `run_id`
  - `tool_version`
  - `config_hash`
  - `evidence_path`
- No `.unwrap()` in `src/` except clearly justified safe cases.
- No hardcoded version strings; use `env!("CARGO_PKG_VERSION")`.
- Use `tracing`, not `println!` or `eprintln!`.
- Preserve Parquet schema names from `/docs/metadata_parquet.md`.
- Prevent path traversal and unchecked indexing where relevant.

## Procedure

1. Inspect uncommitted changed files.
2. Read enough local context to understand each change.
3. Pay special attention to:
   - `src/carve/` for evidence safety and output isolation
   - `src/metadata/` for provenance and schema
   - `src/parsers/` for bounds checks and malformed input handling
4. Check docs only when relevant.
5. Report only actual findings.

## Output Format

```text
### [SEVERITY] filename:line — Short title

**Domain:** {Correctness | Forensic Safety | Error Handling | Security | Architecture | Documentation}
**What:** Describe the issue clearly.
**Why it matters:** Explain the impact.
**Suggestion:** Provide a concrete fix or improvement.
```

## Verdict

End with exactly one of:

- `✅ PASS`
- `⚠️ NEEDS FIXES`
- `❌ ISSUES`

Also include:
- findings by severity
- findings by domain
- files reviewed with no issues
