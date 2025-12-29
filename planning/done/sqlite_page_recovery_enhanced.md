Status: Implemented
Implemented in version: 0.1.0

# SQLite Page Recovery Enhancements

## Problem statement
Page-level recovery only extracts URLs and ignores overflow payloads, titles, and timestamps.

## Scope
- Read overflow pages to reconstruct full payloads.
- Extract title and visit timestamp heuristically from recovered records.
- Share timestamp conversion helpers with standard SQLite parsing.

## Non-goals
- Full schema reconstruction.
- Guaranteed accuracy for titles/timestamps (best-effort only).

## Design notes
- Implement SQLite leaf table local/overflow payload logic.
- Parse record fields into text and integer lists.
- Choose title from non-URL text fields and detect plausible timestamps.

## Expected tests
- Recover URL, title, and timestamp from page scan.
- Recover URL when payload spans overflow pages.

## Impact on docs and README
- Clarify page recovery can include titles/timestamps.
