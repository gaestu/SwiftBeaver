# Carver Quality Improvements

**Status: Implemented**
**Implemented in commit:** (current working changes)

Short description: Reduce false positives, improve phone number validation, and handle GPU span overflow gracefully.

## Problem Statement

Testing against a 4.87 GB EWF image revealed several quality and accuracy issues:

1. **GPU String Scanner Overflow**: 4 warnings where span counts exceeded the 250K capacity (up to 5M spans), causing potential data loss.

2. **Phone Number False Positives**: 0% E164 normalization success rate. Numbers like `7676766773`, `5767676767` are clearly false positives with low digit entropy.

3. **Very Small Carved Files**: 669 files under 100 bytes were carved, including 292 JPEGs and 249 GIFs that are likely header-only fragments.

4. **ASF/WMV Issues**: 3 carve errors with "asf object truncated" and potential over-carving (37GB for 24 WMV files).

## Scope

### In Scope
- Improve phone number plausibility checks to reduce false positives
- Add graceful handling for GPU span overflow (fallback or adaptive capacity)
- Increase minimum file size thresholds for image formats
- Add CLI/config options for controlling min file sizes
- Improve run_summary with truncation/overflow statistics

### Out of Scope
- Changing core carving algorithms
- Modifying parquet schemas (only additive changes)
- GPU kernel modifications
- New file type handlers

## Design Notes

### 1. Enhanced Phone Validation

Update `is_plausible_phone()` in `src/strings/mod.rs`:

```rust
fn is_plausible_phone(value: &str) -> bool {
    let digits: Vec<char> = value.chars().filter(|c| c.is_ascii_digit()).collect();
    let len = digits.len();
    
    // Length validation
    if len < 10 || len > 15 {
        return false;
    }
    
    // Entropy check: require at least 4 unique digits
    let unique: std::collections::HashSet<_> = digits.iter().collect();
    if unique.len() < 4 {
        return false;
    }
    
    // Reject obvious patterns (all same digit already covered)
    // Could add: sequential detection, area code validation
    true
}
```

### 2. GPU Span Overflow Handling

Options (implement at least one):

**Option A - CPU Fallback on Overflow**:
When overflow detected, re-scan the chunk with CPU scanner to capture all spans.

**Option B - Adaptive Capacity**:
Add `--gpu-max-string-spans` CLI flag. If not set, auto-scale based on chunk_size.

**Option C - Overflow Tracking**:
Track overflow events in run_summary. Add `spans_lost_to_overflow` counter.

Recommended: Option C (tracking) + Option A (fallback).

### 3. Minimum File Size Improvements

Add to `config/default.yml`:
```yaml
file_types:
  jpeg:
    min_size: 500  # Currently likely lower
  gif:
    min_size: 100
  bmp:
    min_size: 200
  png:
    min_size: 100
```

Add CLI override: `--min-carved-size <bytes>` that applies globally.

### 4. Run Summary Enhancements

Add fields to run_summary:
- `string_spans_overflow_count`: Number of chunks with span overflow
- `string_spans_lost_estimate`: Estimated spans lost due to overflow
- `files_truncated`: Count of files truncated due to max_size
- `files_below_min_size`: Count of files rejected for being too small

## Implementation Plan

1. **Phone validation** (src/strings/mod.rs):
   - Update `is_plausible_phone()` with entropy check
   - Add tests for edge cases

2. **GPU overflow handling** (src/strings/opencl.rs, src/strings/cuda.rs):
   - Add overflow counter to scanner state
   - Implement CPU fallback when overflow detected
   - Expose overflow stats to pipeline

3. **Min file size** (config + CLI):
   - Update default.yml with higher minimums
   - Add `--min-carved-size` to CLI
   - Thread through to carve handlers

4. **Run summary** (src/metadata/*.rs):
   - Add new fields to summary struct
   - Update parquet/jsonl/csv writers
   - Update docs

5. **Tests**:
   - Phone validation unit tests
   - Overflow fallback integration test
   - Min size filtering test

## Expected Tests

- `src/strings/mod.rs`: Unit tests for improved `is_plausible_phone()`
- Integration test with high-density string data to trigger overflow
- Config test verifying min_size settings are respected

## Impact on Docs and README

- Update `docs/config.md` with new min_size recommendations
- Add note about `--min-carved-size` flag to README
- Document phone number validation criteria in `docs/metadata_parquet.md`
