
# Review Error Handling

You are reviewing SwiftBeaver changes for **error handling only**.

First read `AGENTS.md` and follow it strictly.

## Focus

Check for:
- `.unwrap()` in `src/`
- unjustified `.expect()`
- missing or weak `Result` propagation
- missing error context
- poor distinction between library errors and CLI errors
- misuse of `anyhow` in library code
- absence of structured error types where needed
- swallowed errors or silent fallback behavior

## Rules

- Prefer `thiserror` for library error types.
- Use `anyhow` only in CLI or binary code.
- Prefer meaningful error context.
- Avoid panics in normal failure paths.

## Output

Review only the changed files and the relevant surrounding context.

Report only actual findings. If there are no findings, output exactly:

`✅ PASS`

Use this format for each finding:

```text
### [SEVERITY] filename:line — Short title

**Domain:** Error Handling
**What:** Describe the issue clearly.
**Why it matters:** Explain the reliability or debugging impact.
**Suggestion:** Provide a concrete fix or improvement.
```
