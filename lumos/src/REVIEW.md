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

#### [F5] Duplicate `use rayon::prelude::*` import
- **Location**: `star_detection/threshold_mask/mod.rs:10,24`
- **Category**: Dead code
- **Impact**: 1/5 --- No runtime effect
- **Meaningfulness**: 3/5 --- Indicates copy-paste
- **Invasiveness**: 1/5 --- Delete one line
- **Description**: Rayon prelude imported twice in the same file.

#### [F6] Unused imports in calibration_masters
- **Location**: `calibration_masters/defect_map.rs:31,35`
- **Category**: Dead code
- **Impact**: 2/5 --- No runtime effect but misleading
- **Meaningfulness**: 3/5 --- Indicates stale code
- **Invasiveness**: 1/5 --- Delete two lines
- **Description**: `use crate::stacking::cache::StackableImage` and `use rayon::prelude::*` are never used in the module.

#### [F8] Unnecessary clone in DefectMap::correct()
- **Location**: `calibration_masters/defect_map.rs:134`
- **Category**: Simplification
- **Impact**: 2/5 --- Minor, CfaType is small
- **Meaningfulness**: 2/5 --- Trivial to fix
- **Invasiveness**: 1/5 --- Change `.clone().unwrap()` to `.as_ref().unwrap()`
- **Description**: `cfa_type` is cloned but only used by reference. Use `.as_ref().unwrap()` instead.

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

#### [F13] DVec2/Star transform functions duplicated in testing
- **Location**: `testing/synthetic/transforms.rs:66-501`
- **Category**: Generalization
- **Impact**: 5/5 --- ~160 lines of identical logic with only position field access differing
- **Meaningfulness**: 5/5 --- Every function implemented twice (DVec2 and Star variants)
- **Invasiveness**: 4/5 --- Generic `Positioned` trait or closure-based approach
- **Description**: Functions like `translate_stars`/`translate_star_list`, `add_position_noise`/`add_star_noise`, etc. are pair-wise duplicates. The Star versions are identical logic but access `.pos` field instead of using the value directly.

#### ~~[F14] Drizzle coverage incorrectly averages across channels~~ --- FIXED
- Coverage now uses `self.weights[0]` only (channel 0). All channels share identical geometric overlap, so a single `Buffer2<f32>` coverage map is correct.

#### ~~[F15] min_coverage semantics misleading in drizzle~~ --- FIXED
- `min_coverage` is now compared against `min_coverage * max_weight`, normalizing the threshold to the 0.0–1.0 range as documented.

#### [F17] `#[cfg(test)]` helper functions in star_detection production code
- **Location**: `star_detection/detector/stages/detect.rs:348`, `star_detection/labeling/mod.rs:24`
- **Category**: API cleanliness
- **Impact**: 4/5 --- Violates CLAUDE.md rule against `#[cfg(test)]` in production modules
- **Meaningfulness**: 5/5 --- Project rule violation
- **Invasiveness**: 1/5 --- Move helpers to test modules
- **Description**: Functions like `detect_stars_test()` and `label_map_from_raw()` are defined in production files with `#[cfg(test)]`. Per project rules, these should be in test module files.

#### [F18] Duplicated CCL run-merging logic in star_detection
- **Location**: `star_detection/labeling/mod.rs:336-359` vs `513-533`
- **Category**: Generalization
- **Impact**: 4/5 --- 25-line identical block in sequential and parallel paths
- **Meaningfulness**: 5/5 --- Bug fixes must be applied twice
- **Invasiveness**: 2/5 --- Extract `merge_runs_with_prev()` helper
- **Description**: The run-merging logic for connected component labeling is identically duplicated between the sequential and parallel code paths.

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

#### [F23] CfaImage::demosaic() panics on missing cfa_type
- **Location**: `astro_image/cfa.rs:91-135`
- **Category**: Error handling
- **Impact**: 3/5 --- Unhelpful "called unwrap() on None" panic
- **Meaningfulness**: 4/5 --- Real error path; cfa_type is Option
- **Invasiveness**: 2/5 --- Return Result or require at construction
- **Description**: `demosaic()` calls `.unwrap()` on `metadata.cfa_type` which is `Option<CfaType>`. If a CfaImage is created without setting cfa_type, it panics with a generic message instead of a descriptive error.

#### [F24] HashSet reallocation in registration recover_matches loop
- **Location**: `registration/mod.rs:374-381`
- **Category**: Data flow / Performance
- **Impact**: 3/5 --- 3 HashSets allocated per iteration (max 5 iterations)
- **Meaningfulness**: 3/5 --- Real but only matters for large star counts
- **Invasiveness**: 2/5 --- Pre-allocate before loop and clear per iteration
- **Description**: The recovery loop creates 3 fresh HashSets each iteration. Pre-allocating and clearing would avoid 15 allocations for typical 5-iteration runs.

#### ~~[F25] Drizzle add_image_* methods take redundant parameters~~ --- MOSTLY FIXED
- Kernel methods refactored: `add_image_radial()` takes `(&AstroImage, &Transform, weight, scale, radius, kernel_fn)` — input dims read from the image. `add_image_turbo/point` similarly simplified. Only `compute_square_overlap` retains `#[allow(clippy::too_many_arguments)]` (8 coordinate values, inherent to the function).

### Priority 4 --- Low Priority

#### [F26] Misleading `#[allow(dead_code)]` in star_detection background SIMD
- **Location**: `star_detection/background/simd/mod.rs:20-21,46-47,70`
- **Category**: Dead code annotation
- **Impact**: 1/5 | **Meaningfulness**: 4/5 | **Invasiveness**: 1/5
- **Description**: Functions like `sum_and_sum_sq_scalar` are actively dispatched at runtime but marked `#[allow(dead_code)]`. Remove the misleading annotations.

#### [F27] Unused artifact functions in testing
- **Location**: `testing/synthetic/artifacts.rs:58-257`
- **Category**: Dead code
- **Impact**: 1/5 | **Meaningfulness**: 2/5 | **Invasiveness**: 1/5
- **Description**: `add_hot_pixels`, `add_dead_pixels`, `add_bad_columns`, `add_bad_rows`, `add_linear_trail`, `add_bayer_pattern`, `generate_random_hot_pixels` are defined but never exported or used (~190 lines). Only `add_cosmic_rays` is exported.

#### [F30] `scalar` module unnecessarily public in math/sum
- **Location**: `math/sum/mod.rs`
- **Category**: API cleanliness
- **Impact**: 2/5 | **Meaningfulness**: 2/5 | **Invasiveness**: 2/5
- **Description**: `pub mod scalar` exposes implementation detail. External code should use dispatch functions, not call scalar paths directly.

---

## Cross-Cutting Patterns

### 1. SIMD Code Duplication
The most pervasive issue: SIMD implementations (AVX2/SSE/NEON) duplicate logic across platform files. Affects: `math/sum/`, `star_detection/threshold_mask/`, `star_detection/background/simd/`, `star_detection/centroid/gaussian_fit/`. Consider macros or generic helpers for common patterns like Neumaier reduction, weighted accumulation, and paired with/without-background variants.

### 2. Test Utility Duplication
~~LCG RNG duplication~~ (FIXED — `TestRng` in `testing/mod.rs`). Remaining: duplicate DVec2/Star transform functions in `testing/synthetic/transforms.rs` (F13), and `star_detection/tests/` helper functions (`to_gray_image`, `match_stars`, `mask_to_gray`) redefined in multiple test files instead of importing from `tests/common/`.

### 3. `#[allow(clippy::too_many_arguments)]` Proliferation
~~Drizzle kernel methods~~ (FIXED — refactored to `add_image_radial` with fewer params; only `compute_square_overlap` retains annotation). Remaining: `star_detection/convolution/` (matched_filter), `raw/demosaic/xtrans/` (XTransImage constructors), `testing/synthetic/stamps.rs` (stamp generators). Consider parameter structs for the most egregious cases.

### 4. Inconsistent `#[allow(dead_code)]` Usage
Some annotations mark truly unused code, others mark actively-used runtime-dispatched code, and some mark test-only code in production modules. Three different patterns for the same concept across `star_detection`, `raw`, and `common`.
