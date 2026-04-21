
# Review Security

You are reviewing SwiftBeaver changes for **security only**.

First read `AGENTS.md` and follow it strictly.

## Focus

Check for:
- unchecked indexing on untrusted input
- missing bounds validation
- malformed input handling
- path traversal risks
- arbitrary file writes outside the output directory
- integer overflow in size or offset calculations
- unsafe trust in external metadata
- denial-of-service risks from unbounded allocations or loops where relevant

## High-Risk Areas

Pay particular attention to:
- parsers
- carvers
- path construction
- size calculations
- output file handling

## Output

Review only the changed files and the relevant surrounding context.

Report only actual findings. If there are no findings, output exactly:

`✅ PASS`

Use this format for each finding:

```text
### [SEVERITY] filename:line — Short title

**Domain:** Security
**What:** Describe the issue clearly.
**Why it matters:** Explain the security or robustness impact.
**Suggestion:** Provide a concrete fix or improvement.
```
