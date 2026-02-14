# Code Review: lumos

## Summary

Comprehensive code quality review across submodules (~40k+ lines). The codebase is well-structured with strong SIMD optimizations, good test coverage, and clear module boundaries. The main improvement areas are: **code duplication** across SIMD implementations and test utilities, **incomplete Bayer demosaic** in raw, and several **API inconsistencies** across modules.

**Findings by severity:** 8 Critical/High, 18 Medium, 30+ Low/Minor (only significant findings listed below).

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

#### [F10] SIMD Neumaier/Kahan reduction duplicated across AVX2/SSE/NEON
- **Location**: `math/sum/avx2.rs:33-51`, `math/sum/sse.rs:34-52`, `math/sum/neon.rs:34-50`
- **Category**: Generalization
- **Impact**: 4/5 --- ~90 lines duplicated across 3 files for compensated summation
- **Meaningfulness**: 4/5 --- Numerical code; any bug fix must be replicated 3x
- **Invasiveness**: 3/5 --- Extract macro or generic helper
- **Description**: Horizontal SIMD lane reduction with Neumaier compensation is identically implemented in AVX2, SSE, and NEON files. The `weighted_mean_f32` function is also nearly identical across all three (~75 additional duplicated lines).

#### [F11] Paired SIMD functions duplicated in star_detection threshold_mask
- **Location**: `star_detection/threshold_mask/` (SSE, NEON, scalar)
- **Category**: Generalization
- **Impact**: 4/5 --- ~300 lines of near-identical code (process_words vs process_words_filtered)
- **Meaningfulness**: 5/5 --- Duplicated across 3 platform implementations
- **Invasiveness**: 3/5 --- Unify with `Option<&[f32]>` for background parameter
- **Description**: Each platform has two nearly identical functions: one with background subtraction and one without. Unifying via an optional background parameter would eliminate half the SIMD code.

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

### 1. SIMD Code Duplication
The most pervasive issue: SIMD implementations (AVX2/SSE/NEON) duplicate logic across platform files. Affects: `math/sum/`, `star_detection/threshold_mask/`, `star_detection/background/simd/`, `star_detection/centroid/gaussian_fit/`. Consider macros or generic helpers for common patterns like Neumaier reduction, weighted accumulation, and paired with/without-background variants.

### 2. Test Utility Duplication
~~LCG RNG duplication~~ (FIXED — `TestRng` in `testing/mod.rs`). ~~DVec2/Star transform duplication~~ (FIXED — `Positioned` trait in `transforms.rs`). Remaining: `star_detection/tests/` helper functions (`to_gray_image`, `match_stars`, `mask_to_gray`) redefined in multiple test files instead of importing from `tests/common/`.

### 3. `#[allow(clippy::too_many_arguments)]` Proliferation
~~Drizzle kernel methods~~ (FIXED — refactored to `add_image_radial` with fewer params; only `compute_square_overlap` retains annotation). Remaining: `star_detection/convolution/` (matched_filter), `raw/demosaic/xtrans/` (XTransImage constructors), `testing/synthetic/stamps.rs` (stamp generators). Consider parameter structs for the most egregious cases.

### 4. Inconsistent `#[allow(dead_code)]` Usage
Some annotations mark truly unused code, others mark actively-used runtime-dispatched code, and some mark test-only code in production modules. Three different patterns for the same concept across `star_detection`, `raw`, and `common`.
