# Code Review: lumos

## Summary

Comprehensive code quality review across all 11 submodules (~40k+ lines). The codebase is well-structured with strong SIMD optimizations, good test coverage, and clear module boundaries. The main improvement areas are: **numerical stability** in gradient_removal, **code duplication** across SIMD implementations and test utilities, **incomplete Bayer demosaic** in raw, and several **API inconsistencies** across modules.

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

#### [F2] TPS coordinates not normalized in gradient_removal
- **Location**: `gradient_removal/mod.rs:628-652`
- **Category**: Numerical stability
- **Impact**: 4/5 --- Silent precision loss on large images
- **Meaningfulness**: 5/5 --- Standard mathematical practice per Wikipedia TPS, GDAL, ALGLIB docs
- **Invasiveness**: 1/5 --- Add 2 lines to normalize coordinates before matrix construction
- **Description**: TPS system uses raw pixel coordinates (0..6000) while polynomial path normalizes to [-1,1]. For 6000x4000 images, kernel terms reach ~4.6e8 while affine terms are ~1-6000, creating ~5 orders of magnitude scale mismatch. Documented in GDAL mailing list as causing 50m errors with >1000 points.

#### [F3] Normal equations solver squares condition number
- **Location**: `gradient_removal/mod.rs:502-526`
- **Category**: Numerical stability
- **Impact**: 3/5 --- Affects polynomial degrees 3-4
- **Meaningfulness**: 5/5 --- Established mathematical principle (Harvard AM205, BYU ACME)
- **Invasiveness**: 2/5 --- Replace with nalgebra QR decomposition for 15x15 max matrix
- **Description**: `solve_least_squares()` computes A^T*A then solves via Gaussian elimination. This squares the condition number: cond(A^T*A) = cond(A)^2. For degree-4 polynomials (15 terms), condition numbers can exceed 1e8, losing ~8 significant digits in f64.

#### [F4] Wasteful clone in AstroImage::save()
- **Location**: `astro_image/mod.rs:523-527`
- **Category**: Performance
- **Impact**: 4/5 --- Clones entire image (potentially GBs) before saving
- **Meaningfulness**: 4/5 --- Real memory waste on large images
- **Invasiveness**: 2/5 --- Change `self.clone().into()` to consume self or add borrow-based path
- **Description**: `save()` clones the entire AstroImage before converting to Image format. For 4K+ images this allocates and copies gigabytes unnecessarily.

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

#### [F7] Division correction uses mean instead of median
- **Location**: `gradient_removal/mod.rs:713`
- **Category**: Consistency / Robustness
- **Impact**: 1/5 --- Only matters with edge extrapolation
- **Meaningfulness**: 3/5 --- Inconsistent with subtraction path which uses median
- **Invasiveness**: 1/5 --- One-line change
- **Description**: Subtraction path normalizes by `compute_median(gradient)` (robust to outliers), but division path uses arithmetic mean. Mean is sensitive to extreme values from polynomial extrapolation at image edges.

#### [F8] Unnecessary clone in DefectMap::correct()
- **Location**: `calibration_masters/defect_map.rs:134`
- **Category**: Simplification
- **Impact**: 2/5 --- Minor, CfaType is small
- **Meaningfulness**: 2/5 --- Trivial to fix
- **Invasiveness**: 1/5 --- Change `.clone().unwrap()` to `.as_ref().unwrap()`
- **Description**: `cfa_type` is cloned but only used by reference. Use `.as_ref().unwrap()` instead.

### Priority 2 --- High Impact, Moderate Invasiveness

#### [F9] Drizzle kernel implementations duplicated 4x
- **Location**: `drizzle/mod.rs:241-525`
- **Category**: Generalization
- **Impact**: 3/5 --- ~280 lines of near-identical code across square/point/gaussian/lanczos
- **Meaningfulness**: 4/5 --- Real maintenance burden; adding new kernels requires copy-paste
- **Invasiveness**: 3/5 --- Extract kernel trait with `compute_weight(dx, dy) -> f32`
- **Description**: All four kernel methods follow identical loop structure: iterate input pixels, transform to output coords, compute overlap/weight, accumulate. Only the weight function differs. A `DrizzleKernel` trait would eliminate ~200 lines of duplication.

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

#### [F12] RNG code duplicated 16x across testing module
- **Location**: `testing/synthetic/` (artifacts.rs, backgrounds.rs, star_field.rs, patterns.rs, stamps.rs, transforms.rs)
- **Category**: Generalization
- **Impact**: 4/5 --- 16 separate LCG implementations with inconsistent naming (next_f32/next_rand/next_random)
- **Meaningfulness**: 4/5 --- Prevents copy-paste bugs and inconsistent normalization
- **Invasiveness**: 3/5 --- Extract shared `TestRng` struct
- **Description**: The LCG pattern `state.wrapping_mul(6364136223846793005).wrapping_add(1)` with various normalization schemes is duplicated across every test data generation file. Return types vary (f32/f64/u64) and ranges differ (0-1 vs -1 to 1).

#### [F13] DVec2/Star transform functions duplicated in testing
- **Location**: `testing/synthetic/transforms.rs:66-501`
- **Category**: Generalization
- **Impact**: 5/5 --- ~160 lines of identical logic with only position field access differing
- **Meaningfulness**: 5/5 --- Every function implemented twice (DVec2 and Star variants)
- **Invasiveness**: 4/5 --- Generic `Positioned` trait or closure-based approach
- **Description**: Functions like `translate_stars`/`translate_star_list`, `add_position_noise`/`add_star_noise`, etc. are pair-wise duplicates. The Star versions are identical logic but access `.pos` field instead of using the value directly.

#### [F14] Drizzle coverage incorrectly averages across channels
- **Location**: `drizzle/mod.rs:541-546`
- **Category**: Data flow / Correctness
- **Impact**: 3/5 --- Coverage is spatial, not per-channel
- **Meaningfulness**: 4/5 --- All channels undergo same geometric transform
- **Invasiveness**: 1/5 --- Use single weight per spatial pixel
- **Description**: Coverage is computed as average weight across R/G/B channels, but all channels share the same geometric transform and overlap. Weights should be identical per-channel. Use `Vec<f32>` of length W*H (not W*H*C) to reduce memory by 1/C.

#### [F15] min_coverage semantics misleading in drizzle
- **Location**: `drizzle/mod.rs:68-70, 553`
- **Category**: API clarity
- **Impact**: 3/5 --- Users will misunderstand the parameter
- **Meaningfulness**: 4/5 --- Name suggests 0.0-1.0 but comparison is against raw weight
- **Invasiveness**: 2/5 --- Normalize or rename parameter
- **Description**: Documentation says `min_coverage` is "0.0-1.0 normalized" but the comparison is `weight >= min_coverage` against raw accumulated weight (~10.0 for 10 frames). Either rename to `min_weight` or normalize before comparison.

#### [F16] TPS regularization scaling is arbitrary
- **Location**: `gradient_removal/mod.rs:618`
- **Category**: Numerical stability / API design
- **Impact**: 2/5 --- Functional but not portable across image sizes
- **Meaningfulness**: 4/5 --- MATLAB tpaps uses data-dependent scaling
- **Invasiveness**: 2/5 --- Compute `mean(diag(K))` after K construction (3 lines)
- **Description**: `smoothing^2 * 1000.0` has no relation to data scale, image size, or sample count. MATLAB's approach: `lambda = ((1-p)/p) * mean(diag(K))` adapts to kernel magnitude automatically.

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

#### [F19] O(n*W*H) TPS evaluation doesn't scale
- **Location**: `gradient_removal/mod.rs:673-688`
- **Category**: Performance
- **Impact**: 3/5 --- 6.1 billion kernel evaluations for 6000x4000 with 256 samples
- **Meaningfulness**: 4/5 --- PixInsight uses coarse-grid evaluation for this reason
- **Invasiveness**: 4/5 --- Requires coarse-grid evaluation + bilinear interpolation layer
- **Description**: For each pixel, evaluates sum over all n sample kernel functions. Standard solution: evaluate TPS on coarse grid (32x32), bilinearly interpolate to full resolution (~1000x speedup per literature).

#### [F20] Sample box too small (5x5) in gradient_removal
- **Location**: `gradient_removal/mod.rs:330`
- **Category**: Robustness
- **Impact**: 2/5 --- High variance from 25 pixels
- **Meaningfulness**: 4/5 --- photutils uses box_size=50; PixInsight recommends larger samples
- **Invasiveness**: 2/5 --- Make `sample_radius` configurable, default to 5+
- **Description**: Local median computed over 5x5 box (25 pixels) at each sample point. Too small for robust background estimation with read noise. Industry standard is 11x11+ pixels minimum.

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

#### [F25] Drizzle add_image_* methods take redundant parameters
- **Location**: `drizzle/mod.rs:241-250, 311-320, 349-358, 441-450`
- **Category**: API cleanliness
- **Impact**: 3/5 --- 8-10 parameters per method, most derivable from self
- **Meaningfulness**: 4/5 --- Obscures core algorithm
- **Invasiveness**: 2/5 --- Store input dims on accumulator, extract config fields locally
- **Description**: All four kernel methods receive `input_width`, `input_height`, `scale`, `drop_size` as parameters despite being available on `self.config` or computable from the first image. Reducing to 3 parameters (pixels, transform, weight) would clarify the API.

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

#### [F28] Doc example uses wrong module path in gradient_removal
- **Location**: `gradient_removal/mod.rs:35`
- **Category**: Documentation
- **Impact**: 1/5 | **Meaningfulness**: 2/5 | **Invasiveness**: 1/5
- **Description**: Shows `lumos::stacking::gradient_removal` but actual path is `lumos::gradient_removal`.

#### [F29] Unreachable default in polynomial_terms()
- **Location**: `gradient_removal/mod.rs:474`
- **Category**: Code quality
- **Impact**: 1/5 | **Meaningfulness**: 2/5 | **Invasiveness**: 1/5
- **Description**: `_ => 6` is unreachable because degree is validated to 1-4. Replace with `unreachable!()`.

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
The `testing/synthetic/` module has 16 independent LCG RNG implementations and duplicate DVec2/Star transform functions. The `star_detection/tests/` has helper functions (`to_gray_image`, `match_stars`, `mask_to_gray`) redefined in multiple test files instead of importing from `tests/common/`.

### 3. Numerical Stability in gradient_removal
Three related findings (F2, F3, F16) all stem from the same root: gradient_removal was implemented with simple numerical approaches that work for easy cases but degrade for large images or high polynomial degrees. The TPS path needs coordinate normalization + data-dependent regularization; the polynomial path needs QR/SVD instead of normal equations.

### 4. `#[allow(clippy::too_many_arguments)]` Proliferation
Functions with 7-10 parameters appear in: `drizzle/mod.rs` (kernel methods), `star_detection/convolution/` (matched_filter), `raw/demosaic/xtrans/` (XTransImage constructors), `testing/synthetic/stamps.rs` (stamp generators). Consider parameter structs for the most egregious cases.

### 5. Inconsistent `#[allow(dead_code)]` Usage
Some annotations mark truly unused code, others mark actively-used runtime-dispatched code, and some mark test-only code in production modules. Three different patterns for the same concept across `star_detection`, `raw`, and `common`.
