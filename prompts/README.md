# Prompt Set

Use `AGENTS.md` as the base instruction set for every SwiftBeaver task.

Then add exactly one task prompt when useful:

- `prompts/implement_issue.md` for implementing a GitHub issue end to end
- `prompts/review_uncommitted.md` for reviewing local uncommitted changes
- `prompts/review_correctness.md` for a correctness-only review
- `prompts/review_security.md` for a security-only review
- `prompts/review_error_handling.md` for an error-handling-only review
- `prompts/review_architecture.md` for an architecture-only review
- `prompts/review_documentation.md` for a documentation-only review
- `prompts/review_issue_completeness.md` for checking whether an issue was fully implemented

Composition rules:

- `AGENTS.md` defines repository invariants and global rules.
- Task prompts add workflow and output expectations.
- Review prompts should be used as specialist reviewer briefs.
- Do not duplicate global rules from `AGENTS.md` into every prompt unless needed for clarity.
