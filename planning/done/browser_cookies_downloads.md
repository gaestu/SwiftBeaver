Status: Implemented
Implemented in version: 0.1.0

# Browser Cookies & Downloads Parsing

## Problem statement
Only browser history is parsed today; cookie and download records are missing.

## Scope
- Parse cookies and downloads from common Chromium/Firefox schemas.
- Emit new metadata outputs for cookies and downloads (JSONL/CSV/Parquet).

## Non-goals
- Decryption of protected cookie values.
- Full coverage of all schema variants.

## Design notes
- Add `BrowserCookieRecord` and `BrowserDownloadRecord` types.
- Extend metadata sinks with new categories and schemas.
- Parse typical tables (`cookies`, `moz_cookies`, `downloads`, `moz_downloads`).

## Expected tests
- SQLite parser tests for cookies and downloads.
- Metadata sink tests for cookies/downloads output files.

## Impact on docs and README
- Document new metadata outputs in `docs/metadata_*` and README.
