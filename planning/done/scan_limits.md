Status: Implemented
Implemented in version: 0.1.0

# Scan Limits (max bytes / chunks)

Short description: Allow stopping a run after scanning a limited amount of data.

## Problem statement
Large images can take a long time to scan; developers need a fast way to sample a subset.

## Scope
- Add `--max-bytes` and `--max-chunks` CLI options.
- Stop the reader early when limits are reached.

## Non-goals
- Exact byte-for-byte limits across overlaps.
- Partial chunk processing beyond the limit.

## Design notes
- Enforce limits in the reader loop and allow a truncated final chunk.

## Expected tests
- CLI parsing test for new flags.

## Impact on docs and README
- Mention new flags in README.
