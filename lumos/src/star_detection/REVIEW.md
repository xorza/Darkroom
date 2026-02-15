# Code Review: star_detection

## Summary

The `star_detection` module is a well-architected, high-performance astronomical source detection pipeline. Code quality is generally excellent: algorithms are correct, SIMD implementations are well-tested, and the pipeline structure follows industry standards (SExtractor/DAOFIND). The deblend submodule is particularly clean.

The main opportunities for improvement fall into four categories:
1. **SIMD code duplication** — AVX2/SSE/NEON variants are near-identical copies; sorting networks exist in 4 places; paired with/without-background functions double every SIMD module
2. **Test infrastructure duplication** — Two separate star field generators, duplicated test helpers across 6+ files, duplicated matching/comparison logic
3. **Silent error masking** — `unwrap_or(0)` on logic errors, `debug_assert` where `assert` is needed, silent fallbacks on empty components
4. **API hygiene** — `#[cfg(test)]` helpers in production code, too-many-parameter functions, inconsistent visibility

No algorithmic bugs were found. One behavioral inconsistency (Euclidean vs Chebyshev distance in deblend) affects detection results.

### Completed Findings

- **[F36]** Cleaned up `#[allow(dead_code)]` in `buffer_pool.rs`, `gaussian_fit/mod.rs`, `moffat_fit/mod.rs`, `tests/synthetic/star_field.rs` — removed blanket suppressions, added targeted per-field/per-method annotations
- **[F1]** Changed Chebyshev to Chebyshev distance (with comment) in multi_threshold deblend — matches local_maxima's metric
- **[F2]** Changed `unwrap_or(0)` → `.expect("label out of range in label_map")` in labeling
- **[F3]** Changed `unwrap_or(...)` → `.expect()` in deblend peak-finding (`ComponentData::find_peak`, `find_region_peak`)
- **[F4]** Removed wasted initial `batch_compute_chi2` in L-M optimizer (replaced with `f64::MAX`)
- **[F6]** Added `threshold` parameter to `Star::is_saturated()`, updated all callers, added threshold-varies-behavior tests
- **[F9]** Renamed `total_pixels` → `pixel_end` in threshold_mask (mod.rs, sse.rs, neon.rs, bench.rs)
- **[F10]** Changed `debug_assert!` → `assert!` for buffer dimension checks in `LabelMap::from_pool` and `from_buffer`
- **[F11]** Cleaned dead code: fixed detection_file.rs doc, removed unused imports (detection_file tests, detector/mod.rs ArrayVec), fixed stale comments (labeling bench, median_filter tests), removed duplicate benchmark (local_maxima bench)
- **[F12]** Added `#[derive(Debug)]` to `StripResult`, `UnionFind`, `AtomicUFRef`, `SendPtr`; manual Debug impl for `AtomicUnionFind`

---

## Findings

### Priority 1 — High Impact, Low Invasiveness

#### [F1] Euclidean vs Chebyshev distance inconsistency in deblend separation check
- **Location**: `deblend/local_maxima/mod.rs:216-218` vs `deblend/multi_threshold/mod.rs:693-695`
- **Category**: Consistency / Correctness
- **Impact**: 4/5 — Same `min_separation` config produces different results depending on algorithm
- **Meaningfulness**: 5/5 — Behavioral difference for diagonal close pairs (e.g., at (0,0)+(3,3) with min_sep=4: Euclidean keeps both, Chebyshev merges them)
- **Invasiveness**: 1/5 — Change multi_threshold's Chebyshev check to Euclidean (one line)
- **Description**: `local_maxima` uses squared Euclidean distance (`dx*dx + dy*dy >= min_sep_sq`), while `multi_threshold` uses Chebyshev (`dx < min_sep && dy < min_sep`). Already documented in NOTES-AI.md as weakness W5.

#### [F2] `unwrap_or(0)` masks logic errors in labeling
- **Location**: `labeling/mod.rs:490`, `labeling/mod.rs:721`
- **Category**: Consistency / Correctness
- **Impact**: 3/5 — Silent data corruption instead of crash on label out-of-range
- **Meaningfulness**: 5/5 — Per project rules: "Crash on logic errors. Do not silently swallow them."
- **Invasiveness**: 1/5 — Replace with `.expect("label out of range")`
- **Description**: `label_map.get(run.label as usize).copied().unwrap_or(0)` silently maps out-of-bounds labels to background. A label outside the map range indicates a union-find logic error. Similarly, `flatten_labels` silently skips out-of-range labels.

#### [F3] Silent fallback on empty components in deblend peak-finding
- **Location**: `deblend/mod.rs:90-101`, `deblend/multi_threshold/mod.rs:1062-1075`
- **Category**: API cleanliness / Correctness
- **Impact**: 3/5 — Returns phantom peak at (0,0) with value 0.0 instead of crashing
- **Meaningfulness**: 4/5 — A component with area>0 but no matching pixels is a logic error
- **Invasiveness**: 1/5 — Change `unwrap_or(...)` to `.unwrap()` or `.expect()`
- **Description**: `ComponentData::find_peak` and `find_region_peak` use fallback values for empty iterators. All callers guarantee non-empty components via area checks, so empty iteration is a logic error.

#### [F4] Wasted `batch_compute_chi2` on first L-M iteration
- **Location**: `centroid/lm_optimizer.rs:142-157`
- **Category**: Simplification / Performance
- **Impact**: 3/5 — Eliminates one full pass over stamp data on every star fit
- **Meaningfulness**: 4/5 — Real performance waste; result is immediately overwritten
- **Invasiveness**: 1/5 — Remove one line and the `if iter == 0` block
- **Description**: Line 142 calls `batch_compute_chi2` to initialize `prev_chi2`, but the first iteration of the loop (line 155) overwrites it with `current_chi2` from `batch_build_normal_equations`. The initial call is wasted work.

#### [F5] `#[cfg(test)]` helper functions in production modules
- **Location**: `detector/stages/detect.rs:343-355`, `background/mod.rs:237-251`, `labeling/mod.rs:23-36`
- **Category**: API cleanliness
- **Impact**: 4/5 — Violates project rule "Never use `#[cfg(test)]` on functions in production code"
- **Meaningfulness**: 5/5 — Direct contradiction of CLAUDE.md coding standard
- **Invasiveness**: 1/5 — Move functions to their respective `tests.rs` modules
- **Description**: Three modules define `pub(crate)` test helper functions guarded by `#[cfg(test)]` in production source files.

#### [F6] Hardcoded saturation threshold in `Star::is_saturated()`
- **Location**: `star.rs:38-40`
- **Category**: Consistency
- **Impact**: 3/5 — Saturation level varies by camera/bit-depth; 0.95 is arbitrary
- **Meaningfulness**: 4/5 — All other filter methods take threshold parameters; this one doesn't
- **Invasiveness**: 1/5 — Add parameter: `is_saturated(threshold: f32) -> bool`
- **Description**: `is_saturated()` hardcodes `self.peak > 0.95` while `is_cosmic_ray()` and `is_round()` accept threshold parameters. Config has thresholds for sharpness, roundness, eccentricity, SNR, and FWHM deviation, but not saturation.

#### [F7] Duplicate compaction logic in star filtering
- **Location**: `detector/stages/filter.rs:157-170` and `filter.rs:194-207`
- **Category**: Generalization
- **Impact**: 3/5 — 10 lines of identical code duplicated verbatim
- **Meaningfulness**: 4/5 — Adding a new filter requires updating both compaction blocks
- **Invasiveness**: 1/5 — Extract `compact_by_mask(stars, kept) -> usize`
- **Description**: `remove_duplicate_stars` and `remove_duplicate_stars_simple` end with identical 10-line compaction blocks (count removed, in-place swap, truncate).

#### [F8] Duplicated centroid computation in fwhm.rs vs measure.rs
- **Location**: `detector/stages/fwhm.rs:92-104` vs `detector/stages/measure.rs:18-30`
- **Category**: Generalization
- **Impact**: 3/5 — Identical `par_iter().filter_map(measure_star).collect()` pattern
- **Meaningfulness**: 4/5 — Textbook DRY violation; fwhm.rs should call measure::measure()
- **Invasiveness**: 1/5 — Replace fwhm's private `compute_centroids` with `measure::measure()`
- **Description**: Both functions do `regions.par_iter().filter_map(|r| measure_star(...)).collect()` with identical logic. The fwhm module maintains its own private copy.

#### [F9] Misleading parameter name `total_pixels` in threshold_mask
- **Location**: `threshold_mask/mod.rs:39,66,104,176`, `threshold_mask/sse.rs:16,76`, `threshold_mask/neon.rs:15,79`
- **Category**: Consistency / Naming
- **Impact**: 3/5 — Name suggests total image pixel count; it's actually `row_end_index`
- **Meaningfulness**: 3/5 — Callers pass `row_pixel_start + width`, not total pixels
- **Invasiveness**: 1/5 — Rename to `pixel_end` or `end_index` across all functions
- **Description**: The parameter `total_pixels` is the exclusive upper bound for valid indices on the current row, not the total pixel count. Bench code coincidentally passes the actual total because `pixel_offset=0`.

#### [F10] `debug_assert!` should be `assert!` for buffer dimension checks
- **Location**: `threshold_mask/mod.rs:255-260,295-299`, `buffer_pool.rs:67-71,83-87,99-103`
- **Category**: Consistency / Correctness
- **Impact**: 3/5 — Mismatched buffers cause UB in release builds via SIMD raw pointer arithmetic
- **Meaningfulness**: 3/5 — Per CLAUDE.md: "crash on logic errors"
- **Invasiveness**: 1/5 — Change `debug_assert!` to `assert!`
- **Description**: Buffer dimension checks use `debug_assert!` which is stripped in release builds. SIMD functions use raw pointer arithmetic that would produce UB on dimension mismatches.

#### [F11] Dead code and stale documentation
- **Location**: `detection_file.rs:1` (doc says "save and load", only save exists), `detection_file.rs:25-29` (unused test imports), `labeling/bench.rs:93,116` (stale "100k threshold" comments), `median_filter/tests.rs:739` (stale ROWS_PER_CHUNK reference), `deblend/local_maxima/bench.rs:104-123` (duplicate benchmark with same params), `detector/mod.rs:17` (unused ArrayVec import), `labeling/tests.rs:10` (unused import)
- **Category**: Dead code
- **Impact**: 2/5 — Misleading documentation and unused code
- **Meaningfulness**: 3/5 — Easy cleanup, reduces noise
- **Invasiveness**: 1/5 — Delete or fix affected lines
- **Description**: Multiple small dead code and stale documentation issues across the module.

#### [F12] Missing `#[derive(Debug)]` on structs
- **Location**: `labeling/mod.rs:624,735,416,311` (UnionFind, AtomicUnionFind, StripResult, AtomicUFRef), `mask_dilation/mod.rs:16-17` (SendPtr)
- **Category**: Consistency
- **Impact**: 1/5 — Per project rules: "Always add `#[derive(Debug)]` to structs"
- **Meaningfulness**: 2/5 — Makes debugging harder
- **Invasiveness**: 1/5 — Add derive attributes
- **Description**: Five structs lack `#[derive(Debug)]` contrary to project rules. All can trivially derive it.

---

### Priority 2 — High Impact, Moderate Invasiveness

#### [F13] Massive SIMD duplication in convolution (AVX2/SSE4.1/NEON + scalar = 4x)
- **Location**: `convolution/simd/sse.rs` (~400 lines), `convolution/simd/neon.rs` (~200 lines), `convolution/simd/mod.rs:80-239` (3x dispatch)
- **Category**: Generalization
- **Impact**: 5/5 — Three operations x four backends = 12 implementations of 3 algorithms; ~600 lines of near-identical code
- **Meaningfulness**: 5/5 — Bug in one variant won't be caught in others; any optimization must be applied 4x
- **Invasiveness**: 4/5 — Requires SIMD abstraction trait or macro system
- **Description**: Row, column, and 2D-row convolution each exist in AVX2, SSE4.1, NEON, and scalar variants with identical algorithmic structure. The dispatch pattern (check AVX2+FMA -> SSE4.1 -> NEON -> scalar) is also copy-pasted 3 times. Additionally, SSE4.1 functions are marked `#[target_feature(enable = "sse4.1")]` but only use SSE2 intrinsics.

#### [F14] Sorting network duplicated 4 times in median_filter
- **Location**: `median_filter/mod.rs:267-343` (median9), `median_filter/simd/mod.rs:102-149` (median9_scalar), `median_filter/simd/sse.rs:152-202,209-257` (AVX2+SSE4.1), `median_filter/simd/neon.rs:80-132` (NEON)
- **Category**: Generalization
- **Impact**: 4/5 — 25-comparator network exists in 4 identical copies; row-processing loop in 3 copies
- **Meaningfulness**: 4/5 — If network needs correction, all four must be updated
- **Invasiveness**: 3/5 — A macro generating the network from a swap operation would unify all four
- **Description**: The same 25-comparator sorting network for 9-element median is implemented four times. The row-processing loop wrapping it exists in three near-identical copies (AVX2, SSE4.1, NEON). The file `sse.rs` is misnamed -- it contains both SSE4.1 and AVX2 code.

#### [F15] Paired SIMD function duplication in threshold_mask
- **Location**: `threshold_mask/sse.rs`, `threshold_mask/neon.rs`, `threshold_mask/mod.rs:31-236`
- **Category**: Generalization
- **Impact**: 4/5 — 8 near-identical functions across 3 files; each pair differs by one line
- **Meaningfulness**: 5/5 — Each file has with-background and without-background variants differing only in `bg[i] +`
- **Invasiveness**: 3/5 — Macro or `Option<&[f32]>` for background would halve the code
- **Description**: `process_words`/`process_words_filtered` exist in scalar, SSE, and NEON variants. The only difference is whether a background array is loaded and added. The dispatch logic is also duplicated. The SIMD functions also inline scalar remainder code that duplicates the standalone scalar functions.

#### [F16] Duplicated Cephes exp() constants and simd_int_pow across AVX2/NEON
- **Location**: `centroid/gaussian_fit/simd_avx2.rs:26-40` vs `simd_neon.rs:18-33`, `centroid/moffat_fit/simd_avx2.rs:10-42` vs `simd_neon.rs:10-41`
- **Category**: Generalization
- **Impact**: 3/5 — Identical polynomial coefficients and power function duplicated across platforms
- **Meaningfulness**: 3/5 — Easy to diverge; constants should be shared
- **Invasiveness**: 2/5 — Extract constants to shared parent module; intrinsic logic must remain per-platform
- **Description**: Cephes exp() polynomial coefficients (9 constants) are duplicated between AVX2 and NEON. The `simd_int_pow`/`simd_fast_pow_neg` functions share identical match structure. `hsum` helper is defined differently (module-level vs inline) between gaussian and moffat AVX2.

#### [F17] Two separate star field generation systems in tests
- **Location**: `tests/synthetic/star_field.rs` vs `crate::testing::synthetic`
- **Category**: Consistency / Simplification
- **Impact**: 4/5 — Two `generate_star_field` functions, two star structs, two config structs
- **Meaningfulness**: 4/5 — Simpler system is a strict subset of the richer one; ~120 lines eliminable
- **Invasiveness**: 3/5 — Update callers in mod.rs, debug_steps.rs, subpixel_accuracy.rs
- **Description**: `tests/synthetic/star_field.rs` has a simple star generation system (returns `Vec<f32>`, `SyntheticStar`), while `crate::testing::synthetic` has a richer one (Moffat, nebula, cosmic rays, returns `Buffer2` + `GroundTruthStar`). All pipeline and stage tests use the richer system; only 3 older tests use the simpler one.

#### [F18] Duplicated test helpers across deblend, centroid, and integration tests
- **Location**: `deblend/tests.rs`, `deblend/local_maxima/tests.rs`, `deblend/multi_threshold/tests.rs` (3x `make_test_component`), `centroid/tests.rs`, `centroid/bench.rs`, `centroid/gaussian_fit/tests.rs`, `centroid/gaussian_fit/bench.rs`, `centroid/moffat_fit/tests.rs`, `centroid/moffat_fit/bench.rs` (6x `make_*_star`/`make_*_stamp`), `tests/synthetic/subpixel_accuracy.rs:17` (duplicate `match_stars`)
- **Category**: Generalization
- **Impact**: 3/5 — 10+ duplicated helper functions across test files
- **Meaningfulness**: 4/5 — Changes to synthetic star generation must be applied in many places
- **Invasiveness**: 2/5 — Extract to shared test_utils modules
- **Description**: Test helper functions for star/stamp generation and star matching are copy-pasted across many test and benchmark files instead of being shared from `test_utils.rs` or `tests/common/`.

#### [F19] Double filtering and Mutex-based parallelism in detect.rs
- **Location**: `detector/stages/detect.rs:162,174,330` (double max_area), `detect.rs:246-337` (Mutex pattern)
- **Category**: Simplification
- **Impact**: 3/5 — Confusing two-step area filtering; Mutex where fold/reduce suffices
- **Meaningfulness**: 3/5 — `collect_component_data` clamps to `max_area+1` then `extract_candidates` filters `<= max_area`; Rayon fold/reduce would be more idiomatic
- **Invasiveness**: 3/5 — Either filter in one place or the other; replace Mutex with fold/reduce
- **Description**: Components with `area > max_area` are clamped to `max_area + 1` in `collect_component_data`, then immediately filtered out by `extract_candidates`. The sentinel value is confusing. Separately, the parallel aggregation uses a `Mutex<Vec>` for what is conceptually a parallel fold/reduce.

#### [F20] Manual field-by-field copy between QualityFilterStats and Diagnostics
- **Location**: `detector/mod.rs:200-206`
- **Category**: Generalization
- **Impact**: 3/5 — Seven lines of fragile field-by-field copying
- **Meaningfulness**: 3/5 — Adding a rejection category requires updating both structs and the copy block
- **Invasiveness**: 2/5 — Embed QualityFilterStats in Diagnostics or add From impl
- **Description**: Six fields are manually copied with different naming prefixes (`rejected_` vs none). A `From` impl, embedding, or unified naming would eliminate the brittle copy block.

#### [F21] Unsafe raw pointer patterns for Rayon — three different approaches
- **Location**: `labeling/mod.rs:451-465` (usize cast), `mask_dilation/mod.rs:14-19` (SendPtr), xtrans `UnsafeSendPtr`
- **Category**: Simplification / Data flow
- **Impact**: 3/5 — Three different patterns for the same problem
- **Meaningfulness**: 4/5 — Inconsistent and fragile; mask_dilation's SendPtr lacks `.get()` (Edition 2024 issue)
- **Invasiveness**: 2/5 — Standardize on `UnsafeSendPtr<T>` from common module
- **Description**: Three modules solve "pass mutable pointer through Rayon closure" differently. The generic `UnsafeSendPtr<T>` from xtrans demosaic should be shared. The mask_dilation `SendPtr` accesses the field via `.0` which will break under Edition 2024 closure capture rules.

#### [F22] Mixed atomic orderings in parallel union-find
- **Location**: `labeling/mod.rs:750,756,768,790-795`
- **Category**: Consistency / Correctness
- **Impact**: 3/5 — `SeqCst` in make_set, `Relaxed` in find, `AcqRel` in union
- **Meaningfulness**: 3/5 — Works on x86 (TSO) but `Relaxed` reads may miss stores on ARM
- **Invasiveness**: 1/5 — Standardize orderings
- **Description**: `make_set` uses `SeqCst` (stronger than needed), `find` uses `Relaxed` (may be too weak on ARM), `union` uses `AcqRel`. Should use uniform `AcqRel`/`Acquire` or document why mixed orderings are safe.

---

### Priority 3 — Moderate Impact

#### [F23] Per-row heap allocations in background interpolation hot path
- **Location**: `background/mod.rs:166-190`, `background/tile_grid.rs:343`
- **Category**: Performance
- **Impact**: 3/5 — Five Vec allocations per row in parallel iterator; `solve_natural_spline_d2` allocates scratch Vec per call
- **Meaningfulness**: 3/5 — Tile counts are small, so Vecs are small, but thousands of rows means thousands of allocations
- **Invasiveness**: 2/5 — Use SmallVec or stack arrays for small tile counts; precompute `centers_x` on TileGrid
- **Description**: `interpolate_row` allocates 5 Vecs per row (node_bg, node_noise, centers_x, d2x_bg, d2x_noise). The comment says "SmallVec-style approach" but uses plain Vec. Also `centers_x` is recomputed for every row despite depending only on grid geometry.

#### [F24] Median filter tests check vague properties instead of exact values
- **Location**: `median_filter/tests.rs:40-54,178-202,256-266,294-302`
- **Category**: Test quality
- **Impact**: 3/5 — Tests give false confidence without catching regressions
- **Meaningfulness**: 4/5 — Per CLAUDE.md: "Do NOT write tests that only check `result < 10` or `remaining > 0`"
- **Invasiveness**: 3/5 — Need to compute and assert exact expected medians
- **Description**: `test_preserves_edges` checks `output[0] < output[99]`. `test_large_image_parallel` checks `is_finite()`. `test_non_square_image` checks only `.len()`. `test_bayer_pattern_removal` checks mean within 0.15 of 0.5. All violate project testing rules.

#### [F25] Duplicated pipeline test wrappers and background boilerplate in tests
- **Location**: `tests/synthetic/pipeline_tests/standard_tests.rs:17-73` vs `challenging_tests.rs:20-92`, all `stage_tests/` files (5x `const TILE_SIZE`, 15x background estimation pattern)
- **Category**: Generalization
- **Impact**: 3/5 — ~90% shared logic between run_pipeline_test and run_challenging_test; TILE_SIZE=64 in 5 files
- **Meaningfulness**: 3/5 — Easy to miss updating one copy
- **Invasiveness**: 2/5 — Merge into parameterized helper; centralize TILE_SIZE; extract `estimate_bg_default()`
- **Description**: Two near-identical pipeline test wrappers differ only in output prefix and pass-criteria handling. All stage tests repeat the same background estimation boilerplate and TILE_SIZE constant.

#### [F26] Dead functions and blanket `#[allow(dead_code)]` in test output utilities
- **Location**: `tests/common/output/comparison.rs:96,123,202`, `tests/common/output/image_writer.rs:127,206,239`, `tests/common/output/mod.rs:2`
- **Category**: Dead code
- **Impact**: 3/5 — Six unused functions; blanket allow suppresses all future dead code warnings
- **Meaningfulness**: 3/5 — Removing ~80 lines and the blanket allow restores useful compiler warnings
- **Invasiveness**: 2/5 — Delete functions, remove `#![allow(dead_code)]`
- **Description**: `create_ground_truth_image`, `create_detection_image`, `draw_centroid_path`, `gray_to_rgb`, `side_by_side`, `side_by_side_rgb` are never called. The module-level `#![allow(dead_code)]` was added to suppress their warnings but also hides future dead code.

#### [F27] Hard-coded magic numbers in FWHM estimation
- **Location**: `detector/stages/fwhm.rs:85` (4.0), `fwhm.rs:131` (0.5..20.0), `fwhm.rs:157` (3.0, 0.1)
- **Category**: Consistency
- **Impact**: 2/5 — Undocumented thresholds that may need to match values elsewhere
- **Meaningfulness**: 3/5 — Should be named constants at minimum
- **Invasiveness**: 2/5 — Extract to constants with comments explaining derivation
- **Description**: Default FWHM (4.0), valid FWHM range (0.5..20.0), MAD multiplier (3.0), and MAD floor (0.1) are inline magic numbers. These could diverge from similar values in centroid code.

#### [F28] Background module tests 330 lines of `sigma_clipped_median_mad` from math module
- **Location**: `background/tests.rs:1050-1377`
- **Category**: Consistency
- **Impact**: 2/5 — Tests belong with the function they test
- **Meaningfulness**: 2/5 — Misplaced tests confuse ownership
- **Invasiveness**: 2/5 — Move to math module's test file
- **Description**: ~330 lines of `background/tests.rs` test `sigma_clipped_median_mad` which is defined in the `math` module, not `background`. These tests should live alongside the function definition.

#### [F29] Duplicated naive verification pattern in mask_dilation tests
- **Location**: `mask_dilation/tests.rs:582-604,661-683,802-828`
- **Category**: Generalization
- **Impact**: 2/5 — Same ~20-line naive dilation verification loop copy-pasted 3 times
- **Meaningfulness**: 3/5 — Extract `assert_dilated_matches_naive(mask, dilated, radius)`
- **Invasiveness**: 1/5 — Simple extraction
- **Description**: Three tests contain identical nested loops that compare dilated output against naive O(n*r^2) reference. A shared helper would eliminate ~40 lines of duplication.

#### [F30] `StampData` is a 4-element tuple; should be a named struct
- **Location**: `centroid/mod.rs:113-118`
- **Category**: API cleanliness
- **Impact**: 2/5 — Callers destructure with positional names; intent unclear without context
- **Meaningfulness**: 3/5 — Named fields (`.x`, `.y`, `.z`, `.peak`) would be clearer at every call site
- **Invasiveness**: 2/5 — Define struct, update extract_stamp and callers
- **Description**: `type StampData = (ArrayVec<f32, N>, ArrayVec<f32, N>, ArrayVec<f32, N>, f32)` is opaque. Both gaussian_fit and moffat_fit destructure it identically.

#### [F31] `convolve_cols_parallel` name is misleading (not actually parallel)
- **Location**: `convolution/mod.rs:246-264`
- **Category**: Consistency / Naming
- **Impact**: 2/5 — Name implies parallelism but delegates to single-threaded SIMD column processing
- **Meaningfulness**: 2/5 — Asymmetric with `convolve_rows_parallel` which genuinely parallelizes
- **Invasiveness**: 1/5 — Rename to `convolve_cols` or add comment
- **Description**: `convolve_rows_parallel` uses rayon for actual row-level parallelism. `convolve_cols_parallel` just calls the SIMD function with no parallelism. The naming suggests symmetry that doesn't exist.

---

### Priority 4 — Low Priority

#### [F32] Magic numbers without named constants
- **Location**: `labeling/mod.rs:254` (65000), `labeling/mod.rs:404` (64 rows/strip), `detector/stages/filter.rs:94` (100), `convolution/mod.rs:86` (0.01)
- **Category**: Consistency
- **Impact**: 2/5 — Undocumented thresholds are hard to tune
- **Meaningfulness**: 2/5 — Values are reasonable but arbitrary
- **Invasiveness**: 1/5 — Extract to named constants
- **Description**: Several performance-tuning thresholds are inline magic numbers.

#### [F33] Inconsistent test tolerances across modules
- **Location**: `convolution/tests.rs` (1e-4..1e-6), `background/tests.rs` (0.01)
- **Category**: Consistency
- **Impact**: 2/5 — Loose tolerances may hide precision regressions
- **Meaningfulness**: 2/5 — Different tolerances for similar operations
- **Invasiveness**: 2/5 — Audit and standardize
- **Description**: Test tolerances vary widely. For uniform/deterministic data, tolerances should be tighter.

#### [F34] Benchmark naming mismatch and stale comments
- **Location**: `labeling/bench.rs:75` ("6k" is 4k), `labeling/bench.rs:93` ("100k" threshold is 65k)
- **Category**: Consistency
- **Impact**: 1/5 — Misleading names/comments
- **Meaningfulness**: 2/5 — Could confuse performance analysis
- **Invasiveness**: 1/5 — Rename and fix comments
- **Description**: `bench_label_map_from_buffer_6k_globular` creates 4096x4096 (4k). Comments reference old 100k threshold when actual is 65k.

#### [F35] Scalar column convolution fallback is cache-hostile
- **Location**: `convolution/simd/mod.rs:180-190`
- **Category**: Data flow
- **Impact**: 2/5 — Iterates x-then-y on row-major data
- **Meaningfulness**: 2/5 — Only affects non-SIMD platforms
- **Invasiveness**: 2/5 — Swap loop order to match SIMD versions
- **Description**: Scalar column convolution iterates `for x in 0..width { for y in 0..height }`, which is column-major traversal on row-major data. SIMD versions iterate correctly (y outer, x inner).

#### [F36] ~~Inconsistent `#[allow(dead_code)]` and visibility across modules~~ DONE
- **Location**: `buffer_pool.rs:38-55` (allow on used methods), `background/simd/mod.rs:20-21` (allow on dispatched functions), `median_filter/mod.rs:126` (pub unnecessarily), `deblend/local_maxima/mod.rs:53` vs `multi_threshold/mod.rs:370` (pub vs pub(crate))
- **Category**: Consistency
- **Impact**: 2/5 — Incorrect annotations confuse maintainers
- **Meaningfulness**: 2/5 — Audit and fix
- **Invasiveness**: 1/5 — Remove incorrect allows; tighten visibility
- **Description**: Several `#[allow(dead_code)]` attributes are on actually-used functions. Some functions are `pub` when `pub(crate)` suffices. The two deblend algorithms have inconsistent visibility.

---

## Cross-Cutting Patterns

### 1. SIMD code duplication (AVX2/SSE/NEON variants)
**Modules**: convolution, threshold_mask, median_filter, centroid (gaussian_fit, moffat_fit), background
**Pattern**: Near-identical SIMD implementations exist for each platform, differing only in intrinsic names. The convolution module has 12 implementations of 3 algorithms. The median filter's sorting network exists in 4 places.
**Recommendation**: Consider macros for the most mechanical duplications (sorting networks, dispatch patterns, accumulator reduce). Accept that some duplication is inherent to SIMD. Prioritize the highest-duplication modules (convolution, median_filter).

### 2. Test utility duplication
**Modules**: tests/synthetic/, deblend/*/tests.rs, centroid/*/tests.rs, centroid/*/bench.rs
**Pattern**: Star generation helpers, matching functions, and pipeline wrappers are copy-pasted across 10+ files. Two entirely separate star field generation systems coexist.
**Recommendation**: Consolidate into `test_utils.rs` modules at appropriate levels. Retire the simpler star field generator in favor of the richer `crate::testing::synthetic` system.

### 3. Silent error masking vs crash-on-logic-error
**Modules**: labeling (unwrap_or), deblend (fallback peaks), buffer_pool (debug_assert on dimensions)
**Pattern**: Several places use `unwrap_or` or `debug_assert` where the project's error-handling philosophy calls for crashing on logic errors.
**Recommendation**: Replace `unwrap_or(default)` with `.expect()` or `.unwrap()` where the "empty" case represents a logic error. Upgrade `debug_assert` to `assert` for dimension checks that guard unsafe pointer arithmetic.

### 4. `UnsafeSendPtr` / raw pointer patterns for Rayon
**Modules**: mask_dilation (concrete `SendPtr`), labeling (raw `usize` cast), xtrans demosaic (generic `UnsafeSendPtr<T>`)
**Pattern**: Three different approaches to passing mutable pointers through Rayon closures.
**Recommendation**: Move the generic `UnsafeSendPtr<T>` to `common/`, use it everywhere. This also fixes the Edition 2024 closure capture issue in mask_dilation.

### 5. Paired with/without-background SIMD functions
**Modules**: threshold_mask, convolution (matched_filter vs gaussian_convolve)
**Pattern**: Nearly identical SIMD functions duplicated for "with background subtraction" and "without" variants. Each platform multiplies the duplication.
**Recommendation**: Use `Option<&[f32]>` or const generic `<const HAS_BG: bool>` to share implementations.
