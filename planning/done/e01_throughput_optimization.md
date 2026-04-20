Status: Implemented

# E01 Throughput Optimization

## Problem Statement

SwiftBeaver exhibited throughput collapse on large E01 scans due to O(patterns × chunk_size) signature scanning, oversized chunk defaults (512 MiB), and small channel capacities that created back-pressure under high hit volume.

## Scope

- In scope: Scanner algorithm, chunk sizing, channel capacity, pipeline metrics
- Out of scope: Per-type carve queues, GPU scanner changes, string scanning optimization

## Design Notes

- Replaced per-pattern memchr loop with Aho-Corasick multi-pattern automaton (single pass, O(N))
- Reduced default chunk size from 512 MiB to 64 MiB for better pipeline parallelism
- Increased channel capacity multiplier from 2× to 4× workers
- Added scan_time_ms and carve_time_ms instrumentation to PipelineStats and ProgressSnapshot

## Expected Tests

- Multi-pattern dense benchmark added to benches/throughput.rs
- Integration test verifying pipeline metrics are populated
- Existing scanner unit tests verify Aho-Corasick produces correct matches

## Documentation Impact

- Planning document created (this file)
