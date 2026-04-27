
# Review Architecture

You are reviewing SwiftBeaver changes for **architecture and maintainability only**.

First read `AGENTS.md` and follow it strictly.

## Focus

Check for:
- fit with existing module and trait patterns
- unnecessary abstractions
- parallel or duplicate mechanisms
- poor separation of concerns
- dependency bloat
- dead code
- confusing naming
- over-large functions doing multiple jobs
- style inconsistency with the surrounding codebase

## Rules

- Prefer extending existing structures over inventing parallel ones.
- Prefer focused, reviewable changes.
- Flag unnecessary complexity.

## Output

Review only the changed files and the relevant surrounding context.

Report only actual findings. If there are no findings, output exactly:

`✅ PASS`

Use this format for each finding:

```text
### [SEVERITY] filename:line — Short title

**Domain:** Architecture
**What:** Describe the issue clearly.
**Why it matters:** Explain the maintenance or design impact.
**Suggestion:** Provide a concrete fix or improvement.
```
