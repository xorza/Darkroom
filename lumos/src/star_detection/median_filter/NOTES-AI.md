# Median Filter Module - Implementation Notes

## Overview

3x3 median filter for removing Bayer/CFA pattern artifacts before star detection.
Applied in the prepare stage for CFA sensor images where alternating rows have
different sensitivities. SIMD-accelerated on x86_64 (AVX2, SSE4.1) and aarch64 (NEON).

## File Map

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | 344 | Public API, scalar sorting networks (median3-9), edge handling |
| `simd/mod.rs` | 569 | SIMD dispatch, scalar fallback, median9_scalar, tests |
| `simd/sse.rs` | 463 | AVX2 (8-wide) and SSE4.1 (4-wide) median9 via min/max sorting network |
| `simd/neon.rs` | ~200 | ARM NEON (4-wide) median9 |
| `tests.rs` | ~300 | Full-image integration tests, edge cases, SIMD vs scalar |

## Algorithm

### Sorting Network Approach
Both scalar and SIMD use a 25-comparator sorting network for median-of-9.
This is a full sort that places the median at position 4.

**Scalar** (simd/mod.rs:112-148): Uses conditional swaps on 9 `f32` variables.
The network structure is:
1. Sort three groups of 3 elements (pairs within each row)
2. Cross-compare columns (sort the three minimums, medians, maximums)
3. Final comparisons to place median at v4

**SIMD** (simd/sse.rs): Identical network using `_mm256_min_ps`/`_mm256_max_ps` (AVX2)
or `_mm_min_ps`/`_mm_max_ps` (SSE4.1). Each min/max pair replaces one conditional swap.
Processes 8 (AVX2) or 4 (SSE4.1) independent median computations in parallel.

### Two Different Sorting Networks
The codebase has two median9 implementations with different comparator counts:
1. **mod.rs `median9`** (lines 267-342): 21-comparator partial sort (column-row structured)
   - Sorts three 3-element columns, then cross-compares, then resolves middle
2. **simd/mod.rs `median9_scalar`** (lines 112-148): 25-comparator full sort
   - Used in SIMD scalar fallback

Both produce correct results. The 21-comparator version is theoretically optimal for
finding just the median (no need to fully sort), while the 25-comparator version fully
sorts all 9 elements. The 25-comparator version is used in the SIMD path because the
same network is vectorized into min/max operations.

### Edge Handling
- **Interior rows** (y > 0 and y < height-1): full 9-element neighborhood, SIMD fast path
- **Edge rows** (y=0, y=height-1): scalar, variable-size neighborhood (4-6 elements)
- **Left/right edge pixels** on interior rows: 6-element neighborhood (median6)
- **Small images** (width < 3 or height < 3): copied unchanged (mod.rs:24-28)

### Parallelism
- Row-parallel via `par_chunks_mut(width)` (mod.rs:30-42)
- Each row independently processed (no vertical dependencies in output)
- SIMD within each row for interior pixels

## Comparison with Industry Standards

### vs SExtractor
SExtractor does not apply a pre-detection median filter for CFA artifacts.
It assumes pre-debayered input. The 3x3 median filter is applied to the
**tile grid** for background estimation (BACK_FILTERSIZE), not to the image itself.

### vs PixInsight
PixInsight's `hotPixelFilterRadius` applies a similar 3x3 median filter to remove
hot/cold pixel artifacts before star detection. The approach is equivalent.

### vs photutils
photutils does not include a CFA-specific pre-filter. It assumes clean input from
the astropy/ccdproc pipeline.

### General Practice
Pre-detection median filtering for CFA sensors is standard in astrophotography
stacking software (Siril, DeepSkyStacker). The 3x3 kernel is the minimum effective
size for removing the 2x2 Bayer pattern. Larger kernels (5x5) would over-smooth
and merge close star pairs.

## Strengths

1. **SIMD acceleration**: Vectorized sorting network processes 8 (AVX2) or 4 (SSE4.1/NEON)
   median computations in parallel per iteration
2. **No heap allocation**: All sorting done in registers or stack variables
3. **CFA-only application**: Only applied when `cfa_image` is true in prepare stage,
   avoiding unnecessary filtering of already-debayered images
4. **Correct edge handling**: Variable-neighborhood median preserves edge pixels
   rather than zero-padding (which would darken edges)
5. **Comprehensive SIMD testing**: Scalar vs SIMD comparison tests across multiple
   widths, patterns, and edge cases

## Issues

### P3: Two Inconsistent Sorting Networks
- `mod.rs:median9` uses 21 comparators (partial sort, finds median only)
- `simd/mod.rs:median9_scalar` uses 25 comparators (full sort)
- Both are correct, but the inconsistency may cause confusion
- The 21-comparator version could replace the 25-comparator scalar fallback for
  marginal performance gain in the non-SIMD path

### P3: No 5x5 Option
- Fixed 3x3 kernel. No option for 5x5 median for extremely noisy images or
  images with larger CFA patterns (X-Trans 6x6, though X-Trans is handled
  separately via the demosaic pipeline)
- 3x3 is sufficient for standard Bayer and is the industry standard

## Performance

AVX2 processes 8 pixels per iteration with 25 min/max operations (2 per comparator
swap = 50 intrinsics). For a 4K row (3840 interior pixels), this is ~480 iterations
with scalar remainder handling.

The sorting network approach is optimal for SIMD because:
- No branches (min/max are branchless)
- No data-dependent control flow
- All operations are register-to-register (no memory access between comparisons)
- Perfectly maps to SIMD min/max instructions
