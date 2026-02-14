# Code Review: star_detection

## Summary

The `star_detection` module is a well-architected, high-performance astronomical source detection pipeline. Code quality is generally excellent: algorithms are correct, SIMD implementations are well-tested, and the pipeline structure follows industry standards (SExtractor/DAOFIND). The deblend submodule is particularly clean.

The main opportunities for improvement fall into three categories:
1. **Cross-cutting code duplication** — paired SIMD functions (with/without background), duplicated test utilities, and repeated dispatch logic
2. **API hygiene** — `#[cfg(test)]` helpers in production code, leaked internals, too-many-parameter functions
3. **Consistency** — mixed `#[allow(dead_code)]` usage, inconsistent inline attributes, magic numbers

No algorithmic bugs were found. All unsafe code is correctly justified.

---

## Findings

### Priority 1 — High Impact, Low Invasiveness

#### [F1] `#[cfg(test)]` helper functions in production modules
- **Location**: `detector/stages/detect.rs:343-355`, `background/mod.rs:237-251`, `labeling/mod.rs:23-36`
- **Category**: API cleanliness
- **Impact**: 4/5 — Violates project rule "Never use `#[cfg(test)]` on functions in production code"
- **Meaningfulness**: 5/5 — Direct contradiction of CLAUDE.md coding standard
- **Invasiveness**: 1/5 — Move functions to their respective `tests.rs` modules
- **Description**: Three modules define `pub(crate)` test helper functions guarded by `#[cfg(test)]` in production source files: `detect_stars_test()`, `estimate_background_test()`, and `label_map_from_raw()`/`label_map_from_mask_with_connectivity()`. These should be moved to the test modules that use them.

#### [F2] Misleading `#[allow(dead_code)]` on actually-used functions
- **Location**: `background/simd/mod.rs:20-21,46-47,70`, `gaussian_fit/mod.rs:30`, `centroid/mod.rs:515`
- **Category**: Dead code / Consistency
- **Impact**: 3/5 — Misleads maintainers about code liveness; comments say "kept for future use" but functions are actively dispatched
- **Meaningfulness**: 4/5 — Incorrect annotations cause confusion and suppress legitimate warnings
- **Invasiveness**: 1/5 — Remove attributes, verify with `cargo check`
- **Description**: Several functions in background SIMD dispatch and centroid have `#[allow(dead_code)]` despite being actively used in production paths. `sum_and_sum_sq_scalar` is dispatched on non-SIMD platforms, not "kept for testing." Similarly, `GaussianFitResult` and `StarMetrics` fields are all used.

#### [F3] Hardcoded saturation threshold in `Star::is_saturated()`
- **Location**: `star.rs:42`
- **Category**: API cleanliness
- **Impact**: 3/5 — Saturation level varies by camera/bit-depth; 0.95 is arbitrary
- **Meaningfulness**: 4/5 — All other filter methods take threshold parameters; this one doesn't
- **Invasiveness**: 1/5 — Add parameter: `is_saturated(threshold: f32) -> bool`
- **Description**: `is_saturated()` hardcodes `self.peak > 0.95` while `is_cosmic_ray()` and `is_round()` accept threshold parameters. The API is inconsistent. Config already has filtering thresholds for other metrics but not saturation level.

#### [F4] Misleading parameter name `max_deviation` in FWHM outlier filter
- **Location**: `detector/stages/filter.rs:81`
- **Category**: Consistency / Naming
- **Impact**: 3/5 — Name suggests absolute deviation; it's actually a MAD multiplier (sigma factor)
- **Meaningfulness**: 4/5 — Misleading names cause incorrect usage and make tuning harder
- **Invasiveness**: 1/5 — Rename to `fwhm_mad_factor` or `fwhm_outlier_sigma`
- **Description**: `filter_fwhm_outliers(stars, max_deviation)` uses the parameter as `median + max_deviation * MAD`, making it a sigma-like multiplier, not a maximum absolute deviation. The name should reflect this (e.g., `mad_multiplier` or `outlier_sigma`).

#### [F5] Duplicate `use rayon::prelude::*` import
- **Location**: `threshold_mask/mod.rs:10,24`
- **Category**: Dead code
- **Impact**: 1/5 — Harmless but sloppy
- **Meaningfulness**: 3/5 — Easy cleanup
- **Invasiveness**: 1/5 — Delete one line
- **Description**: `threshold_mask/mod.rs` imports `rayon::prelude::*` twice. Already noted in NOTES-AI.md open issues.

#### [F6] README documents non-existent `create_adaptive_threshold_mask()`
- **Location**: `threshold_mask/README.md:38-44`
- **Category**: Dead code / Documentation
- **Impact**: 3/5 — Misleading documentation for contributors
- **Meaningfulness**: 4/5 — Documents a function that doesn't exist
- **Invasiveness**: 1/5 — Remove section from README
- **Description**: The threshold_mask README describes `create_adaptive_threshold_mask()` with performance numbers, but this function does not exist in the codebase.

#### [F7] Unused `BackgroundRefinement::iterations()` method
- **Location**: `config.rs:152-158`
- **Category**: Dead code
- **Impact**: 2/5 — Adds API surface with no callers
- **Meaningfulness**: 3/5 — All callers use pattern matching directly
- **Invasiveness**: 1/5 — Delete method
- **Description**: `BackgroundRefinement::iterations()` returns `usize` but is never called anywhere. All consumers pattern-match the enum directly.

---

### Priority 2 — High Impact, Moderate Invasiveness

#### [F8] Paired SIMD function duplication in threshold_mask
- **Location**: `threshold_mask/sse.rs` (66 + 53 lines), `threshold_mask/neon.rs` (69 + 58 lines), `threshold_mask/mod.rs:98-236`
- **Category**: Generalization / Simplification
- **Impact**: 4/5 — ~300 lines of near-identical code across 3 files; maintenance burden doubles for any change
- **Meaningfulness**: 5/5 — Each SSE/NEON file has two functions differing only in whether background is subtracted
- **Invasiveness**: 3/5 — Requires refactoring SIMD functions to share inner logic or use const generics
- **Description**: `process_words_sse`/`process_words_filtered_sse`, `process_words_neon`/`process_words_filtered_neon`, and their scalar counterparts are ~90% identical. The only difference is one loads and adds a background vector. The dispatch logic in `mod.rs` is also duplicated (lines 98-168 vs 172-236). A parameterized approach (Option<&[f32]> for background, or const generic bool) would halve the code.

#### [F9] Duplicated CCL run-merging logic between sequential and parallel paths
- **Location**: `labeling/mod.rs:336-359` vs `labeling/mod.rs:513-533`
- **Category**: Generalization
- **Impact**: 4/5 — Identical 25-line block duplicated; bug in one won't be caught in the other
- **Meaningfulness**: 5/5 — Exact same algorithm, different contexts
- **Invasiveness**: 2/5 — Extract shared `merge_runs_with_prev()` helper
- **Description**: The sequential `label_mask_sequential()` and parallel `label_strip()` both contain identical run-merging loops that scan previous-row runs and union labels. This should be a shared helper function.

#### [F10] Duplicated Cephes exp() polynomial across AVX2 and NEON
- **Location**: `centroid/gaussian_fit/simd_avx2.rs` vs `centroid/gaussian_fit/simd_neon.rs` (~100 lines each)
- **Category**: Generalization
- **Impact**: 4/5 — 200+ lines of identical numerical coefficients and algorithm; divergence risk
- **Meaningfulness**: 4/5 — Same polynomial, same coefficients, only intrinsic types differ
- **Invasiveness**: 3/5 — Requires abstracting over SIMD lane type or sharing coefficient tables
- **Description**: Both AVX2 and NEON implementations of the fast Cephes exp() approximation use identical coefficients and identical algorithm structure. The coefficients and evaluation order could be shared via a common module, with only the SIMD intrinsic calls platform-specific.

#### [F11] Too many parameters in `convolution::matched_filter` (8 params)
- **Location**: `convolution/mod.rs:49-56`
- **Category**: API cleanliness
- **Impact**: 3/5 — 8 parameters with clippy suppression; hard to call correctly
- **Meaningfulness**: 4/5 — Three scratch buffers could be a reusable context struct
- **Invasiveness**: 3/5 — Introduce `PsfParams { fwhm, axis_ratio, angle }` and use buffer pool
- **Description**: `matched_filter(pixels, background, fwhm, axis_ratio, angle, output, subtraction_scratch, temp)` takes 8 parameters. The PSF parameters (fwhm, axis_ratio, angle) and scratch buffers (output, subtraction_scratch, temp) are natural groupings that would simplify the API.

#### [F12] Unsafe raw pointer arithmetic in parallel CCL label writing
- **Location**: `labeling/mod.rs:451-465`
- **Category**: Simplification / Data flow
- **Impact**: 3/5 — Converts `*mut u32` → `usize` → `*mut u32` for Rayon closure capture
- **Meaningfulness**: 4/5 — Unnecessary pointer laundering; same pattern as mask_dilation's `SendPtr`
- **Invasiveness**: 3/5 — Replace with `UnsafeSendPtr` wrapper (already used in xtrans demosaic) or scoped parallelism
- **Description**: The parallel label-writing phase converts a mutable pointer to `usize` then back to `*mut u32` to satisfy Rayon's `Send` requirements. The project already has `UnsafeSendPtr` in the demosaic module. Reuse it here for consistency and to avoid bare pointer casts.

#### [F13] Duplicated test utilities across test files
- **Location**: `tests/synthetic/mod.rs:24-30`, `tests/synthetic/debug_steps.rs:18-35`, `tests/synthetic/subpixel_accuracy.rs:17-65`
- **Category**: Generalization
- **Impact**: 3/5 — `to_gray_image()`, `to_gray_stretched()`, `mask_to_gray()`, `match_stars()` duplicated 2-3 times
- **Meaningfulness**: 4/5 — Changes to one copy won't propagate to others
- **Invasiveness**: 2/5 — Import from `tests/common/` instead of redefining
- **Description**: Several test utility functions are redefined in multiple test files instead of importing from the `tests/common/` module where canonical implementations exist. `match_stars()` in `subpixel_accuracy.rs` duplicates `comparison.rs::match_stars()`.

---

### Priority 3 — Moderate Impact

#### [F14] Complex NEON bit extraction without helper
- **Location**: `threshold_mask/neon.rs:41-45,104-108`
- **Category**: Simplification
- **Impact**: 3/5 — 5-line manual bit extraction with confusing reinterpret casts, duplicated twice
- **Meaningfulness**: 3/5 — Obfuscates intent; includes a no-op `vreinterpretq_u32_f32(vreinterpretq_f32_u32(cmp))`
- **Invasiveness**: 2/5 — Extract `extract_movemask_neon()` helper
- **Description**: NEON lacks `movemask`, so threshold_mask manually extracts 4 comparison bits with verbose lane operations. The code includes a `vreinterpretq_u32_f32(vreinterpretq_f32_u32(cmp))` double-cast that is a no-op. Extract to a named helper and remove the unnecessary cast.

#### [F15] `debug_assert!` should be `assert!` for buffer dimension checks
- **Location**: `median_filter/mod.rs` (dimension checks), `threshold_mask/mod.rs:256-261,296-300`
- **Category**: Consistency
- **Impact**: 3/5 — Mismatched buffers cause UB in release builds
- **Meaningfulness**: 3/5 — Per CLAUDE.md: "crash on logic errors"
- **Invasiveness**: 1/5 — Change `debug_assert!` to `assert!`
- **Description**: Buffer dimension checks use `debug_assert!` which is stripped in release builds. Since dimension mismatches are logic errors (not performance-sensitive checks), they should use `assert!` per project guidelines.

#### [F16] Inconsistent inline attributes across SIMD dispatch
- **Location**: `threshold_mask/mod.rs:97-98,171`, `threshold_mask/sse.rs`, `threshold_mask/neon.rs`
- **Category**: Consistency
- **Impact**: 2/5 — Some dispatchers use `#[cfg_attr(not(test), inline)]`, some use `#[inline]`, SIMD functions have none
- **Meaningfulness**: 3/5 — Inconsistency suggests accidental rather than deliberate choices
- **Invasiveness**: 1/5 — Standardize to `#[inline]` for all thin dispatchers
- **Description**: The inline attribute strategy is inconsistent: `process_words` uses `#[cfg_attr(not(test), inline)]`, `process_words_filtered` uses `#[inline]`, and SSE/NEON functions have no inline hint. Pick a consistent strategy.

#### [F17] buffer_pool u32 single-buffer `Option` pattern
- **Location**: `buffer_pool.rs:89-103`
- **Category**: API cleanliness
- **Impact**: 2/5 — If caller acquires two u32 buffers, second allocation is leaked on release
- **Meaningfulness**: 3/5 — Silent memory leak if invariant violated
- **Invasiveness**: 2/5 — Change to `Vec<Buffer2<u32>>` like f32 and bit pools
- **Description**: The u32 buffer pool uses `Option<Buffer2<u32>>` (single buffer) while f32 and bit pools use `Vec`. If a caller accidentally holds two u32 buffers simultaneously, the second release overwrites the first, leaking memory. Using `Vec` would be consistent and safer.

#### [F18] `#[allow(dead_code)]` on public methods of `BufferPool`
- **Location**: `buffer_pool.rs:38,45,52`
- **Category**: Dead code
- **Impact**: 2/5 — Public methods with dead_code suppression suggest unused API surface
- **Meaningfulness**: 3/5 — Either remove unused methods or remove the allow attribute
- **Invasiveness**: 1/5 — Check usage, remove attribute or method
- **Description**: `width()`, `height()`, and `matches_dimensions()` are public methods with `#[allow(dead_code)]`. If they're genuinely unused, remove them. If used, remove the attribute.

#### [F19] `GaussianFitConfig` type alias hides intent
- **Location**: `centroid/gaussian_fit/mod.rs:26`
- **Category**: API cleanliness
- **Impact**: 2/5 — `type GaussianFitConfig = LMConfig` makes it unclear what config fields are for
- **Meaningfulness**: 3/5 — Readers seeing `GaussianFitConfig::default()` don't know it's an LM optimizer config
- **Invasiveness**: 2/5 — Change to newtype wrapper or keep alias with documentation
- **Description**: `GaussianFitConfig` is a bare type alias for `LMConfig`. Users can't tell from the type that it controls convergence thresholds and iteration limits for Levenberg-Marquardt optimization. A newtype or at minimum a doc comment on the alias would clarify intent.

#### [F20] Redundant result validation after LM constrain()
- **Location**: `centroid/gaussian_fit/mod.rs:242-276`, `centroid/moffat_fit/mod.rs:535-543`
- **Category**: Simplification
- **Impact**: 2/5 — Post-fit validation repeats bounds already enforced by `constrain()`
- **Meaningfulness**: 3/5 — Defense-in-depth is reasonable, but creates confusion about where constraints are enforced
- **Invasiveness**: 2/5 — Remove redundant checks, add comment to `constrain()` documenting guarantees
- **Description**: Both Gaussian and Moffat fit result validation checks position and sigma/alpha bounds that are already enforced by the `constrain()` method called on every LM iteration. Either document `constrain()` as the single source of truth and remove redundant checks, or keep them with a comment explaining they're intentional defense-in-depth.

---

### Priority 4 — Low Priority

#### [F21] Magic numbers without named constants
- **Location**: `labeling/mod.rs:254` (65000), `labeling/mod.rs:404` (64 rows/strip), `detector/stages/filter.rs:94` (100 for spatial hash threshold), `convolution/mod.rs:86` (0.01 axis ratio threshold)
- **Category**: Consistency
- **Impact**: 2/5 — Undocumented thresholds are hard to tune
- **Meaningfulness**: 2/5 — Values are reasonable but arbitrary
- **Invasiveness**: 1/5 — Extract to named constants with comments
- **Description**: Several performance-tuning thresholds are inline magic numbers. The 65000-pixel parallel CCL threshold, 64-row strip minimum, 100-star spatial hash crossover, and 0.01 axis ratio "circular" threshold should be named constants with derivation comments.

#### [F22] Inconsistent test tolerances across modules
- **Location**: `convolution/tests.rs` (1e-4, 1e-5, 1e-6), `background/tests.rs` (0.01), `convolution/simd/tests.rs` (1e-4)
- **Category**: Consistency
- **Impact**: 2/5 — Different tolerances for similar operations; some overly loose
- **Meaningfulness**: 2/5 — Loose tolerances may hide precision regressions
- **Invasiveness**: 2/5 — Audit and standardize
- **Description**: Test tolerances vary widely across modules. Background tests use 0.01 (coarse), convolution tests mix 1e-4 through 1e-6. For uniform/deterministic data, tolerances should be tighter (1e-6). For noisy data, document why looser tolerance is needed.

#### [F23] Benchmark naming mismatch: "6k" is actually 4k
- **Location**: `labeling/bench.rs:75`
- **Category**: Consistency
- **Impact**: 1/5 — Misleading benchmark name
- **Meaningfulness**: 2/5 — Could confuse performance analysis
- **Invasiveness**: 1/5 — Rename function
- **Description**: `bench_label_map_from_buffer_6k_globular` creates a 4096x4096 image, not 6144x6144.

#### [F24] `SmallVec<[usize; 4]>` in spatial hash without overflow handling
- **Location**: `detector/stages/filter.rs:115-116`
- **Category**: API cleanliness
- **Impact**: 2/5 — If >4 stars fall in one cell, SmallVec spills to heap (performance, not correctness issue)
- **Meaningfulness**: 2/5 — Unlikely in practice; SmallVec handles overflow gracefully
- **Invasiveness**: 1/5 — Document the inline capacity choice
- **Description**: The spatial hash uses `SmallVec<[usize; 4]>` per cell. With 4 inline slots, cells with >4 stars spill to the heap. This is handled correctly by SmallVec but the capacity choice of 4 should be documented.

#### [F25] Convolution scalar column loop is cache-hostile
- **Location**: `convolution/simd/mod.rs:180-190`
- **Category**: Data flow
- **Impact**: 2/5 — Iterates x-then-y on row-major data (scalar fallback only)
- **Meaningfulness**: 2/5 — Only affects non-SIMD platforms
- **Invasiveness**: 2/5 — Swap loop order
- **Description**: The scalar column convolution fallback iterates `x` in the outer loop and `y` in the inner loop, which is cache-hostile for row-major `Buffer2` layout. The SIMD versions iterate correctly (y outer, x inner). The scalar fallback should match.

---

## Cross-Cutting Patterns

### 1. Paired SIMD functions (with/without background variant)
**Modules affected**: threshold_mask, convolution (matched_filter vs gaussian_convolve)
**Pattern**: Nearly identical SIMD functions duplicated for "with background subtraction" and "without background subtraction" variants. Each platform (SSE, NEON, scalar) multiplies the duplication.
**Recommendation**: Use `Option<&[f32]>` for the background parameter or const generic `<const HAS_BG: bool>` to share the implementation.

### 2. `UnsafeSendPtr` / raw pointer patterns for Rayon
**Modules affected**: mask_dilation (`SendPtr`), labeling (raw `usize` cast), xtrans demosaic (`UnsafeSendPtr`)
**Pattern**: Three different approaches to the same problem (passing mutable pointers through Rayon closures).
**Recommendation**: Standardize on a single `UnsafeSendPtr` wrapper in `common/` and use it everywhere. Document the thread-safety invariant once.

### 3. `#[allow(clippy::too_many_arguments)]` suppression
**Modules affected**: convolution/mod.rs, centroid/moffat_fit, centroid/gaussian_fit, background/simd, threshold_mask/simd
**Pattern**: Functions with 7-9 parameters suppress clippy. Often the parameters fall into natural groups (PSF params, buffer params, threshold params).
**Recommendation**: Introduce small parameter structs where groupings are clear. Not every instance needs fixing — only where the API is public or called from multiple sites.

### 4. Test utility duplication
**Modules affected**: tests/synthetic/, tests/common/
**Pattern**: Helper functions like `to_gray_image()`, `match_stars()`, `mask_to_gray()` are redefined in 2-3 test files instead of imported from `tests/common/`.
**Recommendation**: Consolidate all test utilities into `tests/common/` and import from there.

### 5. Inconsistent `#[allow(dead_code)]` usage
**Modules affected**: background/simd, buffer_pool, gaussian_fit, centroid
**Pattern**: Some `#[allow(dead_code)]` annotations are on genuinely dead code, some are on actively used code, and some are on test-only code compiled in all builds.
**Recommendation**: Audit all `#[allow(dead_code)]` annotations. Remove those on used code. For test-only code, move to test modules.
