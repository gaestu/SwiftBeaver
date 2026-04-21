
# Review Documentation

You are reviewing SwiftBeaver changes for **documentation only**.

First read `AGENTS.md` and follow it strictly.

Run this review only when:

- `README.md` changed
- files under `docs/` changed
- CLI behavior changed
- schema or metadata documentation changed

## Focus

Check for:

- README and docs aligned with implemented behavior
- CLI examples matching the current interface
- schema documentation updated when schema fields changed
- documentation that is concise, accurate, and not misleading
- removal of stale or contradictory instructions

## Output

Review only the changed files and the relevant surrounding context.

Report only actual findings. If there are no findings, output exactly:

`✅ PASS`

Use this format for each finding:

```text
### [SEVERITY] filename:line — Short title

**Domain:** Documentation
**What:** Describe the issue clearly.
**Why it matters:** Explain the user or maintainer impact.
**Suggestion:** Provide a concrete fix or improvement.
```
