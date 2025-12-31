# Production Readiness

Status: WIP

## Problem Statement

fastcarve is functionally complete but lacks production-grade infrastructure for CI/CD, robustness, and operational reliability. This document tracks the remaining work to make the tool production-ready.

## Scope

### In Scope
- CI/CD infrastructure
- Error handling improvements
- Testing gaps
- Observability and diagnostics
- Robustness features
- Security considerations
- Documentation completeness

### Out of Scope
- New file type support
- New carving features
- Performance optimizations (separate feature)

---

## 1. CI/CD Infrastructure

**Status:** ✅ Done

- [x] GitHub Actions workflow created (`.github/workflows/ci.yml`)
- [x] Test job (with/without EWF)
- [x] Lint job (fmt + clippy)
- [x] Release build job
- [x] GPU feature build verification
- [x] Documentation build

**Remaining:**
- [ ] Add release workflow for tagged versions
- [ ] Add code coverage reporting (cargo-llvm-cov)

---

## 2. Error Handling Improvements

**Status:** Not Started

**Issues:**
- `src/evidence.rs:116` - `.unwrap()` in non-Unix `read_at` on lock could panic
- Channel errors could be more informative

**Tasks:**
- [ ] Audit all `.unwrap()` calls in non-test code paths
- [ ] Replace with proper error propagation or `.expect()` with context
- [ ] Improve channel error messages with context

**Files to review:**
- `src/evidence.rs`
- `src/pipeline/mod.rs`
- `src/pipeline/workers.rs`

---

## 3. Testing Gaps

**Status:** Not Started

**Current state:**
- 52 unit tests + 2 integration tests (all passing)
- No negative/adversarial tests
- No stress tests
- No benchmark tests
- GPU tests are conditional

**Tasks:**
- [ ] Add malformed input tests
  - Corrupt/truncated JPEG, PNG, GIF headers
  - Invalid SQLite page sizes
  - Malformed ZIP EOCD
  - Oversized file claims
- [ ] Add boundary condition tests
  - Files spanning chunk boundaries
  - Files at exact chunk size limits
  - Empty evidence files
- [ ] Add stress tests (optional, CI-excluded)
  - Large synthetic images (1GB+)
  - High hit density (many small files)
- [ ] Add benchmark suite
  - `benches/` directory with cargo bench targets
  - Throughput measurement (MB/s)
  - Per-file-type carving speed

**Proposed test structure:**
```
tests/
├── integration_basic.rs      # existing
├── metadata_parquet.rs       # existing
├── malformed_inputs.rs       # NEW
├── boundary_conditions.rs    # NEW
└── ewf_integration.rs        # NEW (see golden_ewf.md)
```

---

## 4. Observability & Diagnostics

**Status:** Not Started

**Current state:**
- Basic tracing logging
- No progress reporting
- No metrics

**Tasks:**
- [ ] Add progress callback/trait for long-running scans
  - Bytes processed / total
  - Files carved count
  - ETA estimation
- [ ] Add optional JSON structured logging
  - `--log-format json` flag
- [ ] Consider metrics hooks (optional)
  - Processing throughput
  - Error rates by category

---

## 5. Robustness Features

**Status:** Not Started

**Current state:**
- No graceful shutdown
- No resume capability
- No checkpointing

**Tasks:**
- [ ] Add signal handling (Ctrl+C)
  - Graceful worker shutdown
  - Flush metadata before exit
  - Report partial progress
- [ ] Consider checkpoint/resume (future)
  - Save scan position on interrupt
  - Resume from checkpoint file
  - Lower priority - enterprise feature

---

## 6. Security Considerations

**Status:** Not Started

**Current state:**
- No input validation on carved filenames
- No resource limits
- Output directory permissions not checked

**Tasks:**
- [ ] Input path sanitization
  - Validate output paths don't escape output directory
  - Sanitize carved file names from evidence content
- [ ] Resource limits
  - Optional max memory usage
  - Max open file descriptors
  - Max output file count
- [ ] Permission checks
  - Verify output directory is writable
  - Warn on world-writable output

---

## 7. Documentation Completeness

**Status:** Partially Done

**Issues:**
- `docs/architecture.md` header says "Phase 1" but content is Phase 2+
- No CHANGELOG.md
- No CONTRIBUTING.md
- No doc examples in public API

**Tasks:**
- [ ] Fix architecture.md phase reference
- [ ] Create CHANGELOG.md with release history
- [ ] Create CONTRIBUTING.md with development setup
- [ ] Add doc examples to key public types:
  - `CarvedFile`
  - `EvidenceSource` trait
  - `MetadataSink` trait
  - `SignatureScanner` trait

---

## 8. Build & Distribution

**Status:** Not Started

**Current state:**
- Version 0.1.0
- No release binaries
- No container support

**Tasks:**
- [ ] Create release workflow
  - Build on tag push
  - Create GitHub release
  - Upload Linux binary
- [ ] Consider container image (lower priority)
  - Dockerfile with libewf
  - Pre-built forensic workstation image

---

## Priority Order

| Priority | Item | Effort | Impact |
|----------|------|--------|--------|
| 1 | Fix architecture.md | 5 min | Low |
| 2 | Add CHANGELOG.md | 30 min | Medium |
| 3 | Graceful shutdown | 2-4 hrs | High |
| 4 | Malformed input tests | 4-6 hrs | High |
| 5 | Golden EWF test (see golden_ewf.md) | 2-4 hrs | Medium |
| 6 | Error handling audit | 2-4 hrs | Medium |
| 7 | Progress reporting | 2-4 hrs | Medium |
| 8 | Release workflow | 2-4 hrs | Medium |
| 9 | Input sanitization | 2-4 hrs | Medium |
| 10 | Benchmark suite | 4-8 hrs | Low |

---

## Completion Criteria

This feature is complete when:
- [ ] CI runs green on all PRs
- [ ] All high-priority items implemented
- [ ] Tests cover malformed input scenarios
- [ ] Graceful shutdown works on Ctrl+C
- [ ] Documentation is accurate and complete
- [ ] Version bumped to 0.2.0 or 1.0.0

---

## Notes

- Keep changes backward compatible
- Each sub-feature can be a separate PR
- Tests should not require external network access
- GPU tests remain conditional (skip without hardware)
