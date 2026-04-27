
# Review Issue Completeness

You are reviewing SwiftBeaver changes for **issue completeness only**.

First read `AGENTS.md` and follow it strictly.

GitHub issues are the source of truth for requirements and acceptance criteria.

## Focus

Check whether the implementation:
- solves the stated problem
- addresses the listed requirements
- satisfies acceptance criteria
- respects stated constraints
- matches the posted implementation plan, if one was added as an issue comment
- avoids unrelated scope creep

## Scope

Use:
- the GitHub issue
- the changed files
- the implementation-plan comment, if available

## Output

Report only actual findings. If there are no findings, output exactly:

`✅ PASS`

Use this format for each finding:

```text
### [SEVERITY] filename:line — Short title

**Domain:** Issue Completeness
**What:** Describe the gap clearly.
**Why it matters:** Explain why the issue is not fully satisfied.
**Suggestion:** Provide a concrete fix or follow-up.
```
