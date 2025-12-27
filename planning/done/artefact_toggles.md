Status: Implemented
Implemented in version: 0.1.0

# Per-Artefact Scan Toggles

Short description: Add per-artefact toggles for URL/email/phone extraction.

## Problem statement
Users need to enable or disable specific artefact types without editing code.

## Scope
- Add config fields to enable/disable URL, email, and phone extraction.
- Add CLI overrides for each artefact type.
- Apply settings during string artefact extraction.
- Update docs and README.

## Non-goals
- Adding additional artefact types.
- Changing string scanning heuristics beyond toggles.

## Design notes
- Keep defaults enabled to preserve existing behavior.
- CLI flags allow explicit enable/disable.

## Expected tests
- CLI parsing test for new flags.
- Artefact extraction respects config toggles.

## Impact on docs and README
- Document new config fields and CLI overrides.
