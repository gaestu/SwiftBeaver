Status: Implemented
Implemented in version: 0.1.0

# Entropy Region Detection

Short description: Detect high-entropy regions and emit metadata.

## Problem statement
High-entropy regions can indicate compressed or encrypted data; surfacing them helps analysts triage evidence.

## Scope
- Add optional entropy detection with configurable window size and threshold.
- Emit entropy region metadata in JSONL/CSV/Parquet.
- Update docs/README and add tests.

## Non-goals
- GPU acceleration for entropy detection.
- Advanced region clustering or per-type tagging.

## Design notes
- Use fixed window scanning and merge adjacent windows above threshold.
- Record max entropy per region.

## Expected tests
- Unit tests for entropy detection.
- Metadata sink tests for entropy output.

## Impact on docs and README
- Document new outputs and CLI/config options.
