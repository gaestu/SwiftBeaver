
# Review Correctness

You are reviewing SwiftBeaver changes for **correctness only**.

First read `AGENTS.md` and follow it strictly.

## Focus

Check for:
- logic flaws
- wrong conditions or inverted checks
- off-by-one errors
- wrong loop bounds
- silent data loss
- incorrect return values
- broken edge-case handling
- incorrect state transitions
- regressions caused by the change

## Scope

Review only the changed files and the relevant surrounding context.

## Output

Report only actual findings. If there are no findings, output exactly:

`✅ PASS`

Use this format for each finding:

```text
### [SEVERITY] filename:line — Short title

**Domain:** Correctness
**What:** Describe the issue clearly.
**Why it matters:** Explain the functional impact.
**Suggestion:** Provide a concrete fix or improvement.
```
