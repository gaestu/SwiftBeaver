Status: Implemented

# EML End-of-Message Detection

## Problem Statement

The EML carver produced severely oversized output because its `\nFrom ` (mbox) boundary detection fails on most real-world email fragments found in forensic images. On test images, 82% of EML files hit the 50 MiB max_size cap, producing 5.3 GB of mostly-binary garbage.

## Scope

- In scope: Multi-strategy end detection, stricter header validation, reduced max_size, post-carve binary content validation
- Out of scope: Recursive MIME parsing, encrypted email body handling, email address syntax validation

## Design Notes

Three end-detection strategies applied in priority order:
1. **MIME final boundary** (`--boundary--`): Extracted from Content-Type header
2. **Mbox boundary** (`\nFrom `): Existing strategy, retained
3. **Binary content transition**: 512-byte sliding window, >30% binary indicator bytes triggers end

Additional changes:
- `Received:` added to header markers (7 total)
- Minimum required headers raised from 2 to 3
- max_size reduced from 50 MiB to 10 MiB
- Post-carve validation rejects files with >30% binary indicators when no structural boundary found

## Expected Tests

- Binary transition detection
- MIME boundary detection (quoted/unquoted, case-insensitive)
- Post-carve binary rejection
- Base64 attachment preservation
- Mbox boundary detection
- Regression test for old max_size behavior

## Documentation Impact

- Created `docs/carver/eml.md`
- Updated `docs/carver/README.md` EML row

Closes #12
