# OGG Carver

## Overview

The OGG carver extracts OGG container files (audio, video) from evidence images by walking the page structure until the end-of-stream flag is found.

## Detection

### Header Pattern
- **Magic bytes**: `4F 67 67 53` ("OggS")
- **Version**: Must be 0

## Validation

The carver performs the following validation:

1. **Page signature**: Each page must start with "OggS"
2. **Version check**: Only version 0 is supported
3. **Page data size**: Maximum 65,025 bytes per page (255 segments × 255 bytes)
4. **Page count limit**: Maximum 100,000 pages per stream

## Page Structure

Each OGG page has:
- 27-byte fixed header
- Segment table (0-255 bytes)
- Page data (sum of segment lengths)

The carver reads pages sequentially until:
- End-of-stream flag (bit 0x04) is set in header_type
- Page limit (100,000) is exceeded
- max_size is reached

## Configuration

| Setting | Default | Description |
|---------|---------|-------------|
| `max_size` | 500 MB | Maximum carved file size |
| `min_size` | 28 bytes | Minimum file size (one empty page) |

## False Positive Mitigation

The OGG carver includes several protections against false positives:

1. **Page limit** (100,000 pages): Prevents infinite loops on malformed data
2. **Page data size limit** (65,025 bytes): Rejects pages exceeding OGG format maximum
3. **Reduced max_size** (500 MB): Limits maximum output from any single hit

## Supported Codecs

The OGG container can hold various codecs. Common ones include:
- **Vorbis** - Audio
- **FLAC** - Lossless audio
- **Opus** - Audio
- **Theora** - Video

The carver extracts the container regardless of codec.

## Limitations

- No CRC verification (4-byte CRC present in pages but not validated)
- No granule position validation
- Does not handle multiplexed (chained) streams separately
- May extract partial streams if end-of-stream flag is not found

## File Extensions

| Extension | Typical Content |
|-----------|-----------------|
| `.ogg` | Vorbis audio |
| `.oga` | General audio |
| `.ogv` | Theora video |
