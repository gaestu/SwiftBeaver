Status: Implemented
Implemented in version: 0.1.0

# Chromium Variant Schema Support (Edge/Brave)

## Problem statement
Chromium-based browsers (Edge/Brave) share SQLite schemas but include variants such as `downloads_url_chains` and optional columns that can break rigid queries.

## Scope
- Support downloads URL chains when present.
- Make cookies/history/downloads queries resilient to missing optional columns.
- Document that Chromium-based outputs may be labeled `chrome`.

## Non-goals
- Accurate browser branding without source path hints.
- Decrypting cookie values.

## Design notes
- Use PRAGMA table_info to pick available columns and fall back to NULL.
- Join `downloads_url_chains` on `id` with `chain_index = 0`.

## Expected tests
- Downloads URL resolution from `downloads_url_chains`.

## Impact on docs and README
- Add Chromium schema note to metadata docs and README.
