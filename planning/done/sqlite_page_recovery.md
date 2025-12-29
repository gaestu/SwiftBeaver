Status: Implemented
Implemented in version: 0.1.0

# SQLite Page Recovery

## Problem statement
Corrupted SQLite databases can fail to open, leaving browser history data unrecovered.

## Scope
- Add a page-level scanner to recover URL-like strings from leaf table pages.
- Emit recovered URLs as `BrowserHistoryRecord` entries when enabled.
- Gate recovery behind a config/CLI flag.

## Non-goals
- Full schema recovery or page overflow handling.
- Table-aware reconstruction of titles and visit times.

## Design notes
- Parse SQLite page size and leaf table pages (0x0D).
- Decode record payloads and extract text fields.
- Use existing URL normalization logic for consistency.

## Expected tests
- Create a SQLite DB and verify URL recovery from pages.

## Impact on docs and README
- Document the new config/CLI flag.
- Note the `browser=sqlite_page` and `visit_source=page_scan` values.
