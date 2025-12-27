Status: Implemented
Implemented in version: 0.1.0

# UTF-8 String Scanning

Short description: Extend string scanning to detect printable UTF-8 strings (multi-byte).

## Problem statement
String extraction only handled ASCII and UTF-16, missing common UTF-8 text content.

## Scope
- Add UTF-8 span detection for printable characters.
- Preserve existing ASCII/UTF-16 behavior and hint flags.
- Label UTF-8 spans as `utf-8` in artefact outputs.

## Non-goals
- Language-aware normalization or Unicode category filtering.
- Deduplication of overlapping ASCII/UTF-8 spans.

## Design notes
- Scan valid UTF-8 runs and require at least one multi-byte codepoint to emit a span.
- Reuse ASCII hint flags for URL/email/phone detection.
- Append UTF-8 spans in CPU and GPU string scanners.

## Expected tests
- Unit test for UTF-8 span detection and flags.
- Artefact encoding test to confirm `utf-8` label.

## Impact on docs and README
- Note that `--scan-strings` includes ASCII/UTF-8 scanning.
