# Reduce False Positives in High-Output Carvers

Status: Implemented

## Problem Statement

SwiftBeaver generates severe false positives and excessive output volume on real-world evidence. On a 100GB test image comparison:
- bulk_extractor output: ~6.1GB
- SwiftBeaver output: ~162GB (when stopped early)

The largest SwiftBeaver carved directories were:
- bzip2: 62GB
- mp3: 34GB
- ogg: 19GB
- ole: 15GB
- 7z: 13GB
- wav: 11GB

## Scope

### In scope
- Audit and improve validation for: bzip2, mp3, ogg, ole, 7z, wav
- Add sanity limits to prevent explosive output
- Reduce default max_size for high-risk formats
- Add regression tests for false positive patterns

### Out of scope
- Changes to other carvers
- Decompression-based validation
- Per-handler metrics infrastructure

## Design Notes

### Root Causes Identified

| Carver | Problem | Solution |
|--------|---------|----------|
| 7z | No validation of offset/size header fields | CRC32 validation + sanity limits |
| WAV | RIFF size field fully trusted | fmt chunk validation |
| OGG | Page limit too high (1M), no size validation | Reduced to 100K, added page data limit |
| BZIP2 | Searches up to 1GB for footer | 10MB search limit |
| MP3 | Only 3 frames required, no consistency check | 5 frames, sample rate consistency |
| OLE | FAT sector count unbounded | 1000 sector cap |

### Key Decisions

1. **Reject early, reject often**: Invalid files should be rejected during validation, not written and then deleted
2. **Sanity limits over deep parsing**: Adding CRC checks and field limits is cheaper than full format parsing
3. **Config changes for high-risk formats**: Reduced max_size provides defense in depth

## Expected Tests

- 7z: Invalid CRC rejection, excessive offset/size rejection
- WAV: Invalid fmt chunk rejection (bad format, channels, sample rate, bits)
- OGG: Page limit exceeded, page data size exceeded
- BZIP2: Search limit exceeded (footer not found in 10MB)
- OLE: FAT sector limit exceeded
- MP3: Frame count insufficient, sample rate inconsistency

## Documentation Impact

- Created docs/carver/bzip2.md
- Created docs/carver/ogg.md
- Updated docs/carver/mp3.md (5 frames, consistency check, duration limit)
- Updated docs/carver/7z.md (CRC validation, offset/size limits)
- Updated docs/carver/wav.md (fmt validation, corrected max_size)
- Updated CHANGELOG.md
