# Implement GitHub Issue

You are implementing a GitHub issue for **SwiftBeaver**.

First read `AGENTS.md` and follow it strictly.

## Workflow

1. Read the GitHub issue carefully and extract:
   - problem statement
   - requirements
   - acceptance criteria
   - constraints

2. Inspect the relevant code paths and identify:
   - files to change
   - existing patterns to follow
   - tests that need updates

3. If the issue is non-trivial, create a short implementation plan before editing code.

4. Implement conservatively:
   - preserve backward compatibility unless the issue explicitly requires otherwise
   - follow existing abstractions and module boundaries
   - avoid unnecessary dependencies
   - keep changes focused

5. Add or update deterministic tests:
   - regression tests for bug fixes
   - integration tests for CLI or workflow changes
   - schema tests for metadata changes

6. Update docs only if user-facing behavior, CLI usage, or schema documentation changed.

7. Run specialist review passes after implementation:
   - always run:
     - `prompts/review_correctness.md`
     - `prompts/review_security.md`
     - `prompts/review_error_handling.md`
     - `prompts/review_architecture.md`
   - run `prompts/review_documentation.md` only if docs, CLI usage, or schema behavior changed
   - run `prompts/review_issue_completeness.md` for issue-driven work
   - if review subagents are available, run independent review passes in parallel
   - if any review finds real issues, fix them and re-run only the failed review passes

8. Before finishing, run:

   ```bash
   cargo fmt
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test
   ```

9. Return:
   - summary of changes
   - files modified
   - tests added or updated
   - docs updated, if any
   - review passes run
   - remaining risks or follow-ups
   - a proposed commit message

## Extra SwiftBeaver Checks

- Never write to evidence or source images.
- Ensure all output rows preserve provenance fields.
- Do not hardcode version strings.
- Keep Parquet schema field names aligned with `/docs/metadata_parquet.md`.
