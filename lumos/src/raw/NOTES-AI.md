# raw Module - Code Review vs Industry Standards

## Overview
Loads RAW camera files via libraw FFI, detects sensor type, normalizes u16 to f32 [0,1], dispatches to demosaic: Mono (extract), Bayer (todo!()), X-Trans (custom Markesteijn 1-pass), Unknown (libraw fallback). Also provides load_raw_cfa() for un-demosaiced CFA data.

## What It Does Well
- RAII guards for libraw resources (LibrawGuard, ProcessedImageGuard)
- Arena-based memory for X-Trans demosaic (~920MB in one block, explicit buffer lifetime docs)
- Drops libraw early to reduce peak memory before demosaic
- 3 SIMD normalization paths (SSE4.1, SSE2, NEON) + scalar
- X-Trans Markesteijn: precomputed ColorInterpLookup, interior fast path, summed area table, sliding YPbPr cache
- 2.1x speedup vs libraw reference with comparable quality
- 67 tests covering all paths
- Quality benchmarks against libraw reference

## Issues Found

### Critical: Bayer Demosaic Not Implemented
- File: bayer/mod.rs:157
- `todo!("Bayer DCB demosaicing not yet implemented")`
- Any Bayer camera RAW file causes panic (>95% of all cameras)
- Options: (1) Quick: fall back to libraw builtin, (2) Medium: bilinear baseline, (3) Best: RCD (recommended for astro) or AMaZE

### Medium: Missing Per-Channel Black Level (cblack[])
- File: mod.rs:335-340
- Uses only scalar `color.black`, ignores `cblack[0..3]` per-channel and `cblack[4..5]` spatial pattern
- Sony cameras have cblack offsets, X-Trans has 6x6 correction pattern
- Causes subtle color bias, especially in shadows and calibration frames

### Medium: No White Balance Before Demosaic
- Standard dcraw/libraw pipeline: subtract_black -> white_balance -> demosaic
- Implementation skips white balance entirely (no cam_mul/pre_mul references)
- Defensible for astro (handled later in pipeline) but degrades demosaic quality at color edges
- Should be documented as intentional design decision

### Low: Normalization Does Not Clamp to [0, 1]
- File: normalize.rs:6
- Floor clamped at 0.0 but no ceiling clamp at 1.0
- Test explicitly validates values > 1.0 are preserved
- Standard practice (libraw, RawTherapee) clips at white point
- Unclamped values > 1.0 can degrade X-Trans demosaic at saturated star cores

### Low: load_raw_cfa Panics on Unknown Sensor Type
- File: mod.rs:535
- Uses `unimplemented!()` but unknown sensor is expected (exotic cameras)
- Per project rules, should return Result::Err for expected failures

### Low: alloc_uninit_vec Soundness Concern
- File: mod.rs:32-35
- Used in 5 places, suppresses clippy::uninit_vec
- Valid optimization for large buffers but high UB risk if any write path is incomplete
- Every call site has SAFETY comment but fragile pattern

### Low: Missing AVX2 Path in Normalization
- Processes 4 elements at a time (128-bit)
- AVX2 could process 8 (project has AVX2 infra elsewhere)

### Info: YPbPr BT.2020 for Derivatives is Correct
- Matches reference implementation for 1-pass Markesteijn
- BT.2020 vs BT.709 makes negligible difference (relative metric)
