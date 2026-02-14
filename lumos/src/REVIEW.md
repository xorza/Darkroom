# Code Review: lumos

## Summary

Comprehensive code quality review across submodules (~40k+ lines). The codebase is well-structured with strong SIMD optimizations, good test coverage, and clear module boundaries. The main improvement areas are: **code duplication** across SIMD implementations and test utilities, **incomplete Bayer demosaic** in raw, and several **API inconsistencies** across modules.

**Findings by severity:** 8 Critical/High, 20 Medium, 30+ Low/Minor (only significant findings listed below).

---

## Findings

### Priority 1 --- High Impact, Low Invasiveness

#### [F1] Incomplete Bayer demosaicing panics at runtime
- **Location**: `raw/demosaic/bayer/mod.rs:189-191`
- **Category**: Dead code / Correctness
- **Impact**: 5/5 --- Bayer sensors are >95% of cameras; any Bayer RAW file panics
- **Meaningfulness**: 5/5 --- Showstopper for most users
- **Invasiveness**: 4/5 --- Requires implementing RCD or falling back to libraw
- **Description**: `demosaic_bayer()` is `todo!()`. Either implement RCD demosaicing or add libraw fallback for Bayer sensors. X-Trans path works correctly.

#### ~~[F4] Wasteful clone in AstroImage::save()~~ --- FIXED
- Added `to_image(&self)` that borrows pixel data; `save()` no longer clones.

#### ~~[F5] Duplicate `use rayon::prelude::*` import~~ --- FIXED
- Removed duplicate import in `star_detection/threshold_mask/mod.rs`.

#### ~~[F6] Unused imports in calibration_masters~~ --- FIXED
- Removed unused `StackableImage` and `rayon::prelude::*` imports from `calibration_masters/defect_map.rs`.

#### ~~[F8] Unnecessary clone in DefectMap::correct()~~ --- FIXED
- Changed `.clone().unwrap()` to `.as_ref().unwrap()` and switched from trait method to direct field access.

### Priority 2 --- High Impact, Moderate Invasiveness

#### ~~[F9] Drizzle kernel implementations duplicated 4x~~ --- FIXED
- Extracted `accumulate()` helper and `add_image_radial()` with closure-based kernel. Gaussian and Lanczos unified; ~120 lines removed.

#### [F10] SIMD Neumaier/Kahan reduction duplicated across AVX2/SSE/NEON --- REVIEWED, NO ACTION
- ~290 lines duplicated across 3 platforms. Macro/trait dedup viable but high risk for working, tested numerical code. Platform intrinsics differ only in type names and lane widths. Keeping as-is.

#### [F11] Paired SIMD functions duplicated in star_detection threshold_mask --- REVIEWED, NO ACTION
- ~400 lines duplicated (with/without background variants × 3 platforms). Difference is one threshold computation line per SIMD group. Macro dedup viable but SIMD hot-path code benefits from explicit, auditable implementations. Keeping as-is.

#### ~~[F12] RNG code duplicated 16x across testing module~~ --- FIXED
- Extracted `TestRng` struct in `testing/mod.rs` with `next_u64()`, `next_f32()`, `next_f64()`. Replaced all 16+ inline LCG closures across `testing/synthetic/`, `star_detection/`, and `registration/` test files.

#### ~~[F13] DVec2/Star transform functions duplicated in testing~~ --- FIXED
- Added `Positioned` trait (`pos()`, `with_pos()`) implemented for `DVec2` and `Star`. Six function pairs unified via generic `_impl` functions. `add_spurious_*` kept separate (structurally different). ~100 lines removed.

#### ~~[F14] Drizzle coverage incorrectly averages across channels~~ --- FIXED
- Coverage now uses `self.weights[0]` only (channel 0). All channels share identical geometric overlap, so a single `Buffer2<f32>` coverage map is correct.

#### ~~[F15] min_coverage semantics misleading in drizzle~~ --- FIXED
- `min_coverage` is now compared against `min_coverage * max_weight`, normalizing the threshold to the 0.0–1.0 range as documented.

#### ~~[F17] `#[cfg(test)]` helper functions in star_detection production code~~ --- FIXED
- Moved `detect_stars_test()` to `detect_test_utils.rs` and `label_map_from_raw()`/`label_map_from_mask_with_connectivity()` to `labeling/test_utils.rs`. Both are `#[cfg(test)]` submodules. Updated all import sites.

#### ~~[F18] Duplicated CCL run-merging logic in star_detection~~ --- FIXED
- Extracted `merge_runs_with_prev()` helper with `RunMergeUF` trait. Sequential path passes `&mut UnionFind` directly; parallel path wraps `&AtomicUnionFind` via `AtomicUFRef` adapter.

### Priority 3 --- Moderate Impact

#### [F21] Serde error handling inconsistency in common crate
- **Location**: `common/src/serde.rs:34-60`
- **Category**: Consistency
- **Impact**: 2/5 --- Serialization panics, deserialization returns Result
- **Meaningfulness**: 2/5 --- Asymmetric behavior
- **Invasiveness**: 2/5 --- Change serialize to return Result
- **Description**: `serialize()` uses `.unwrap()` extensively while `deserialize()` returns `Result<T>` properly. Serialization failures panic instead of returning errors.

#### [F22] Empty-input handling inconsistent in math/statistics
- **Location**: `math/statistics/mod.rs` (multiple functions)
- **Category**: Consistency
- **Impact**: 3/5 --- Could cause panics in release builds
- **Meaningfulness**: 3/5 --- `debug_assert!` stripped in release
- **Invasiveness**: 2/5 --- Add explicit checks to 2 functions
- **Description**: `median_f32_mut` and `median_and_mad_f32_mut` use `debug_assert!(!data.is_empty())` while `sigma_clipped_median_mad` and `mad_f32_with_scratch` explicitly handle empty arrays. The debug_assert functions will index out of bounds in release builds if passed empty arrays.

#### ~~[F23] CfaImage::demosaic() panics on missing cfa_type~~ --- FIXED
- Changed `.unwrap()` to `.expect("CfaImage missing cfa_type: set metadata.cfa_type before calling demosaic()")`.

#### ~~[F24] HashSet reallocation in registration recover_matches loop~~ --- FIXED
- Pre-allocated 3 HashSets before loop with `with_capacity()`, reusing via `.clear()` + `.extend()` each iteration.

#### ~~[F31] Float FITS data not normalized~~ --- FIXED
- **Location**: `astro_image/fits.rs` — `normalize_fits_pixels()`
- Float FITS data (BITPIX -32/-64) passed through unchanged. DeepSkyStacker outputs [0,65535], other tools use arbitrary ranges. Downstream assumes [0,1].
- Added heuristic: compute max, normalize by dividing by max if max > 2.0. Threshold of 2.0 provides headroom for HDR overexposure while catching [0,65535] and [0,255] ranges.
- 10 tests covering: [0,1] unchanged, HDR headroom, threshold boundary, [0,65535], [0,255], negative values, Float64, UInt16 regression, all-zero, single pixel.

#### ~~[F33] Background mask fallback uses contaminated pixels~~ --- FIXED
- **Location**: `star_detection/background/tile_grid.rs` — `compute_tile_stats()`
- When a tile had fewer unmasked pixels than `min_pixels` (30% of tile), it discarded the good background pixels and fell back to sampling ALL pixels including bright stars. This biased the background estimate upward in crowded regions.
- Changed fallback condition from `values.len() < min_pixels` to `values.is_empty()`. Now uses whatever unmasked pixels are available; only falls back to all-pixels when the tile is 100% masked. Removed `min_pixels` parameter from internal APIs.
- Updated test to verify that a 95%-masked tile uses the 5% unmasked background pixels (median ≈ 0.2) instead of falling back to star-contaminated samples (median ≈ 0.9).

#### ~~[F32] Per-CFA-channel flat normalization missing~~ --- FIXED
- **Location**: `astro_image/cfa.rs` — `divide_by_normalized()`
- Single global mean across all CFA pixels caused color shift with non-white flat light sources (LED panels, twilight flats).
- Refactored into `divide_by_normalized_mono()` (unchanged single-mean path) and `divide_by_normalized_cfa()` (per-R/G/B means). CFA path computes independent per-color sums/counts, then normalizes each pixel by its own color's mean. Row-parallel via rayon.
- 6 tests: non-white flat (uniform light unchanged), vignetting + color, with bias, mono regression, color shift correction (key test demonstrating the fix).

#### ~~[F25] Drizzle add_image_* methods take redundant parameters~~ --- MOSTLY FIXED
- Kernel methods refactored: `add_image_radial()` takes `(&AstroImage, &Transform, weight, scale, radius, kernel_fn)` — input dims read from the image. `add_image_turbo/point` similarly simplified. Only `compute_square_overlap` retains `#[allow(clippy::too_many_arguments)]` (8 coordinate values, inherent to the function).

### Priority 4 --- Low Priority

#### [F26] `#[allow(dead_code)]` in star_detection background SIMD --- NOT A BUG
- Functions `sum_and_sum_sq_simd` and `sum_abs_deviations_simd` are genuinely unused in production (the pipeline uses scalar `math/statistics` instead). Annotations are correct per project rules: kept intentionally for future use with explanatory comments.

#### ~~[F27] Unused artifact functions in testing~~ --- FIXED
- Removed `add_hot_pixels`, `add_dead_pixels`, `add_bad_columns`, `add_bad_rows`, `BadPixelMode`, `add_linear_trail`, `generate_random_hot_pixels` and their tests (~190 lines). Kept `add_cosmic_rays`, `add_bayer_pattern`, `BayerPattern` (used by star_field.rs).

#### ~~[F30] `scalar` module unnecessarily public in math/sum~~ --- FIXED
- Changed `pub mod scalar` to `pub(super) mod scalar` in `math/sum/mod.rs`.

---

## Cross-Cutting Patterns

### 1. SIMD Code Duplication --- REVIEWED, NO ACTION
Reviewed all ~690 lines of duplication across AVX2/SSE/NEON (F10: `math/sum/` ~290 lines, F11: `threshold_mask/` ~400 lines). Macro-based dedup is viable but high risk for low reward: the code is correct, tested, and rarely changes. Platform-specific intrinsics make generic abstractions complex. Keeping as-is.

### 2. Test Utility Duplication --- FIXED
~~LCG RNG duplication~~ (FIXED — `TestRng` in `testing/mod.rs`). ~~DVec2/Star transform duplication~~ (FIXED — `Positioned` trait in `transforms.rs`). ~~Image conversion helpers~~ (FIXED — removed duplicate `to_gray_image`, `to_gray_stretched`, `mask_to_gray` from `synthetic/`, now imported from `common/output/image_writer.rs`). `match_stars` in `subpixel_accuracy.rs` is NOT a duplicate (different signature and purpose from `comparison.rs` version).

### 3. `#[allow(clippy::too_many_arguments)]` --- REVIEWED, NO ACTION
~~Drizzle kernel methods~~ (FIXED). Reviewed all 37 remaining occurrences. All are justified: SIMD dispatch functions (identical signatures across platform variants), hot-path pixel operations (struct wrapping adds indirection), constructors with heterogeneous parameters (XTransImage), test-only generators, and sorting networks (`median9_scalar` where params ARE the data). No refactoring needed.

### 4. `#[allow(dead_code)]` --- REVIEWED, NO ACTION
Reviewed all 24 occurrences. All annotations are correct and well-justified: public API methods available for downstream use (`Aabb`), struct fields used by tests (`GaussianFitResult`, `MoffatFitResult`), SIMD fallbacks kept for testing, diagnostic fields (`iterations`, `inlier_ratio`), WIP modules with documentation (`tps`), and `#[cfg_attr(test, allow(dead_code))]` for production-only code paths. No changes needed.
