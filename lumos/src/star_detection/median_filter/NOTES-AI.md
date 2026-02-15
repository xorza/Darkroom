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
   - Sorts three 3-element rows, then sorts columns, then sorts {v[3],v[4],v[5]}
2. **simd/mod.rs `median9_scalar`** (lines 112-148): 25-comparator full sort
   - Used in SIMD scalar fallback

**BUG (dormant):** The 21-comparator `median9` in `mod.rs` is **incorrect**. After
forming a Young tableau (sort rows, sort columns), the median of all 9 elements is the
median of the anti-diagonal {v[2], v[4], v[6]}. But step 3 sorts {v[3], v[4], v[5]}
(the middle row), which is already sorted and thus a no-op. Counter-example: input
`[0.1, 0.2, 0.9, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]` produces v[4]=0.4, true median=0.5.

**Impact: NONE in production.** The `median9` in `mod.rs` is reachable only through
`median_of_n` with exactly 9 elements, called from `median_at_edge`. Edge pixels have
at most 6 neighbors (top/bottom rows: 2 rows × 3 columns = 6), never 9. All interior
pixels use the correct 25-comparator `median9_scalar` from `simd/mod.rs`.

The 25-comparator network is the proven optimal S(9)=25 full sorting network (Codish,
Cruz-Filipe, Frank, and Schneider-Kamp 2014). It is used in all SIMD paths and the
scalar fallback. It is correct.

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

### P3: Dormant Bug in `mod.rs:median9`
- `mod.rs:median9` uses 21 comparators and is **incorrect** (wrong anti-diagonal sort)
- `simd/mod.rs:median9_scalar` uses 25 comparators and is correct
- The buggy function is unreachable in production (edge pixels have at most 6 neighbors)
- Fix: either replace step 3 to sort {v[2],v[4],v[6]} instead of {v[3],v[4],v[5]},
  or remove the `median9` branch from `median_of_n` entirely (dead code elimination)

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
