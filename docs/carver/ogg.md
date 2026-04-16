# OGG Carver

## Overview

The OGG carver extracts OGG container files (audio, video) from evidence images by walking the page structure with CRC32 validation, codec identification, and serial number consistency checks.

## Detection

### Header Pattern
- **Magic bytes**: `4F 67 67 53` ("OggS")
- **Version**: Must be 0

## Validation

The carver performs the following validation:

1. **CRC32 verification**: Every page's CRC32 is verified using the Ogg-specific CRC-32/MPEG-2 (polynomial 0x04C11DB7, no initial value, no final XOR). A first-page CRC failure rejects the entire hit. Subsequent CRC failures terminate the stream at the last valid page.
2. **Codec identification**: The first page must be a BOS (beginning-of-stream) page containing a recognized codec signature:
   - `\x01vorbis` — Vorbis audio
   - `OpusHead` — Opus audio
   - `\x7fFLAC` — FLAC lossless audio
   - `\x80theora` — Theora video
   - `Speex   ` — Speex audio (note trailing spaces)
3. **Serial number consistency**: All pages must share the same serial number as the BOS page. A mismatch terminates the stream.
4. **Minimum page count**: At least 2 valid pages are required before output is committed.
5. **Page signature**: Each page must start with "OggS"
6. **Version check**: Only version 0 is supported
7. **Page data size**: Maximum 65,025 bytes per page (255 segments × 255 bytes)
8. **Page count limit**: Maximum 100,000 pages per stream

### Pre-validation

Before any file I/O, the carver reads the entire first page from evidence and verifies:
- Valid OggS signature and version
- BOS flag is set
- CRC32 matches
- Known codec signature present

This rejects most false positives without creating output files.

## Page Structure

Each OGG page has:
- 27-byte fixed header
- Segment table (0-255 bytes)
- Page data (sum of segment lengths)

The carver reads pages sequentially until:
- End-of-stream flag (bit 0x04) is set in header_type
- CRC32 mismatch on a subsequent page (stream truncated)
- Serial number mismatch (stream truncated)
- Page limit (100,000) is exceeded
- max_size is reached

## Configuration

| Setting | Default | Description |
|---------|---------|-------------|
| `max_size` | 100 MB | Maximum carved file size |
| `min_size` | 28 bytes | Minimum file size (one empty page) |

## False Positive Mitigation

The OGG carver includes several protections against false positives:

1. **CRC32 verification**: First-page CRC must be valid; eliminates random data matching the OggS pattern
2. **Codec identification**: BOS page must contain a known codec header
3. **Serial number consistency**: Prevents concatenation of unrelated streams
4. **Minimum page count** (2 pages): Rejects trivially short matches
5. **Page limit** (100,000 pages): Prevents infinite loops on malformed data
6. **Page data size limit** (65,025 bytes): Rejects pages exceeding OGG format maximum
7. **Reduced max_size** (100 MB): Limits maximum output from any single hit

## Supported Codecs

| Codec | Signature | Type |
|-------|-----------|------|
| Vorbis | `\x01vorbis` | Audio |
| Opus | `OpusHead` | Audio |
| FLAC | `\x7fFLAC` | Lossless audio |
| Theora | `\x80theora` | Video |
| Speex | `Speex   ` | Audio |

## Limitations

- No granule position validation
- Does not handle multiplexed (chained) streams separately
- May extract partial streams if end-of-stream flag is not found (marked as truncated)

## File Extensions

| Extension | Typical Content |
|-----------|-----------------|
| `.ogg` | Vorbis audio |
| `.oga` | General audio |
| `.ogv` | Theora video |
