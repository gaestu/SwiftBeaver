# BZIP2 Carver

## Overview

The BZIP2 carver extracts bzip2-compressed files from evidence images by detecting the stream header and searching for the end marker.

## Detection

### Header Pattern
- **Magic bytes**: `42 5A 68` ("BZh")
- **Version byte**: ASCII '1' through '9' (block size multiplier)

### Footer Pattern
- **End-of-stream marker**: `17 72 45 38 50 90` (6 bytes)

## Validation

The carver performs the following validation:

1. **Header validation**: Magic bytes must be `BZh` followed by a valid version byte
2. **Footer search**: Scans for the end-of-stream marker
3. **Search limit**: Stops searching if footer not found within 10 MB (prevents false positives)

## Configuration

| Setting | Default | Description |
|---------|---------|-------------|
| `max_size` | 100 MB | Maximum carved file size |
| `min_size` | 14 bytes | Minimum file size (header + footer) |

## False Positive Mitigation

The BZIP2 carver includes a **search limit** to prevent false positives from magic bytes appearing in random data:

- If the footer is not found within the first 10 MB of searching, the hit is rejected
- This prevents the carver from scanning gigabytes of data on false header matches

## Limitations

- No CRC verification (bzip2 CRCs are computed per-block and require decompression)
- No block structure validation (relies on footer presence only)
- Does not extract individual blocks from corrupted streams

## Extended Format Support

The carver recognizes bzip2 streams with block sizes 100 KB ('1') through 900 KB ('9').

## Related Formats

- **gzip** (`.gz`) - Different compression algorithm, separate carver
- **xz** (`.xz`) - LZMA-based compression, separate carver
