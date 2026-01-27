# Lumos Astrophotography Stacking - Implementation Specification

> **Status**: In Progress  
> **Last Updated**: 2026-01-27  
> **Reference**: See `../PLAN.md` for detailed algorithm descriptions

---

## Overview

This specification defines remaining implementation tasks for the lumos astrophotography
stacking library. Claude Code will work through each task sequentially, implementing
with tests and running verification commands per project conventions.

**Crate**: `lumos`  
**Language**: Rust  
**Verification**: `cargo nextest run && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings`

---

## Phase 1: Local Normalization [IN PROGRESS]

### 1.1 Research & Design
- [x] Research PixInsight-style local normalization algorithms
- [x] Document approach in `src/stacking/NOTES-AI.md`
- [ ] Design API that fits existing `NormalizationMethod` enum

### 1.2 Implementation
- [ ] Implement tile-based local statistics computation (128x128 tiles)
- [ ] Implement smooth interpolation between tile statistics
- [ ] Add `LocalNormalization` variant to `NormalizationMethod`
- [ ] Write unit tests for local normalization
- [ ] Run verification commands

---

## Phase 2: GPU Acceleration [PARTIAL - 30%]

> **Rule**: Each GPU implementation must show measurable improvement or be documented as not beneficial

### 2.1 GPU FFT for Phase Correlation
- [ ] Research wgpu FFT implementations or libraries (rustfft alternatives)
- [ ] Document findings in `src/gpu/NOTES-AI.md`
- [ ] Implement GPU FFT wrapper if viable library exists
- [ ] Integrate with phase correlation registration
- [ ] Benchmark against CPU rustfft implementation
- [ ] If <10% improvement: document and remove, mark "[SKIPPED]"

### 2.2 GPU Sigma Clipping
- [ ] Research parallel reduction strategies for sigma clipping
- [ ] Design GPU kernel for per-pixel stack statistics
- [ ] Implement GPU sigma clipping in wgpu compute shader
- [ ] Write integration tests
- [ ] Benchmark against CPU implementation
- [ ] If <10% improvement: document and remove, mark "[SKIPPED]"

### 2.3 GPU Star Detection
- [ ] Design GPU kernel for threshold detection
- [ ] Implement parallel connected component labeling (or alternative)
- [ ] Benchmark against SIMD CPU implementation
- [ ] If <10% improvement: document and remove, mark "[SKIPPED]"

### 2.4 Batch Processing Pipeline
- [ ] Implement overlapped compute/transfer for frame processing
- [ ] Add async GPU operations with proper synchronization
- [ ] Benchmark end-to-end pipeline throughput

---

## Phase 3: Advanced Features [NOT STARTED]

### 3.1 Comet/Asteroid Stacking Mode
- [ ] Research dual-stack approach algorithms
- [ ] Design API for comet position input (t1, t2 positions)
- [ ] Implement frame-specific offset based on timestamp interpolation
- [ ] Implement composite output (stars from one stack, comet from other)
- [ ] Write integration tests with synthetic comet data
- [ ] Run verification commands

### 3.2 Multi-Session Integration
- [ ] Design per-session quality assessment workflow
- [ ] Implement session-aware local normalization
- [ ] Implement session-weighted integration
- [ ] Add gradient removal post-stack (optional)
- [ ] Write integration tests
- [ ] Run verification commands

### 3.3 Distortion Correction Extensions
- [ ] Implement radial distortion models (barrel/pincushion)
- [ ] Implement tangential distortion correction
- [ ] Add field curvature correction
- [ ] Write unit tests for each model
- [ ] Run verification commands

### 3.4 Astrometric Solution
- [ ] Research Gaia/UCAC4 catalog formats and access
- [ ] Implement catalog star matching algorithm
- [ ] Compute plate solution (WCS coordinates)
- [ ] Write integration tests
- [ ] Run verification commands

---

## Phase 4: Quality & Polish [PARTIAL]

### 4.1 Real-time Preview
- [ ] Design live stacking API (incremental updates)
- [ ] Implement streaming accumulator for real-time display
- [ ] Add quality metrics streaming
- [ ] Write integration tests

### 4.2 Code Quality
- [ ] Review and update all NOTES-AI.md files
- [ ] Ensure public API is consistent and well-documented
- [ ] Remove any deprecated or unused code
- [ ] Final clippy and fmt pass

---

## Verification Commands

After EACH task completion, run:
```bash
cargo nextest run -p lumos && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings
```

For benchmarks:
```bash
cargo bench -p lumos --features bench --bench <name> | tee benches/<name>_results.txt
```

---

## Progress Tracking

| Phase | Tasks | Complete | Status |
|-------|-------|----------|--------|
| Local Normalization | 5 | 0 | Not Started |
| GPU Acceleration | 12 | 0 | Partial (warping done) |
| Advanced Features | 14 | 0 | Not Started |
| Quality & Polish | 4 | 0 | Not Started |
| **Total** | **35** | **0** | **In Progress** |

---

## Notes

- All implementations must follow `CLAUDE.md` coding rules
- Use `.unwrap()` for infallible operations, `.expect()` with message for non-obvious cases
- Add `#[derive(Debug)]` to all structs
- Update relevant `NOTES-AI.md` after completing each section
- Benchmark results go to `benches/` with analysis in `bench-analysis.md`
