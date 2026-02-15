# Code Review: registration

## Summary

Overall code quality is **good to excellent**. The registration module is well-architected, thoroughly tested, and mathematically correct. No correctness bugs were found across any submodule. The codebase has comprehensive test coverage with hand-computed expected values, proper edge case handling, and SIMD-vs-scalar reference tests.

The main improvement opportunities are: eliminating code duplication (LU solvers, test helpers, spatial traversal patterns, normalization logic), tightening internal API visibility, fixing inconsistent error handling patterns, and cleaning up stale annotations. Test code has significant boilerplate duplication that could be reduced with shared helpers and parametrized tests.

**Files reviewed**: 33 source files across 7 submodules (distortion, interpolation, ransac, spatial, triangle, tests) + 4 top-level files.

### Completed Findings

- **[F3]** `#[allow(dead_code)]` on `RansacResult` fields → targeted per-field `#[allow(dead_code)] // Used in tests`
- **[F6]** Stale `#[allow(dead_code)]` on `WarpParams::new()` → targeted annotation with comment

## Findings

### Priority 1 -- High Impact, Low Invasiveness

#### [F1] `TransformType` ordering via unsafe `as u8` cast
- **Location**: `transform.rs:274-278`
- **Category**: Safety / correctness
- **Impact**: 4/5 -- relies on implicit enum discriminant ordering; reordering variants breaks silently
- **Meaningfulness**: 5/5 -- no compiler help if variants reorder
- **Invasiveness**: 1/5 -- add `#[derive(PartialOrd, Ord)]` and use `.max()`
- **Description**: `compose()` compares transform types via `self.transform_type as u8 > other.transform_type as u8`. This depends on discriminant ordering with no compile-time guarantee. Add `#[derive(PartialOrd, Ord)]` to `TransformType` and replace with `self.transform_type.max(other.transform_type)`.

#### [F2] `estimate_and_refine` takes `transform_type` AND `config` redundantly
- **Location**: `mod.rs:273-280`
- **Category**: API cleanliness
- **Impact**: 3/5 -- parameter overrides config field of same name; confusing for maintainers
- **Meaningfulness**: 4/5 -- real API design issue that invites bugs
- **Invasiveness**: 1/5 -- remove parameter, read from config at call sites
- **Description**: `estimate_and_refine(ref_stars, target_stars, matches, transform_type, max_sigma, config)` accepts an explicit `transform_type` while `config` also has a `transform_type` field. The parameter overrides config in some calls (Auto logic), creating ambiguity. Either remove the parameter and use a temporary config copy, or remove the field from Config and always pass explicitly.

#### [F3] ~~`#[allow(dead_code)]` on `RansacResult` fields instead of `pub(crate)`~~ DONE
- **Location**: `ransac/mod.rs:126-131`
- **Category**: API cleanliness
- **Impact**: 3/5 -- `#[allow(dead_code)]` hides genuinely unused code
- **Meaningfulness**: 4/5 -- proper visibility is the right tool
- **Invasiveness**: 1/5 -- change 2 annotations to `pub(crate)` visibility
- **Description**: `iterations` and `inlier_ratio` fields on `RansacResult` are marked `#[allow(dead_code)]` but are useful diagnostics. Change to `pub(crate)` so they're available to the parent module and tests without suppressing lint warnings.

#### [F4] Incomplete `SipConfig` validation
- **Location**: `distortion/sip/mod.rs:85-93`
- **Category**: Consistency / validation
- **Impact**: 3/5 -- zero or negative `clip_sigma` produces NaN in threshold calculation
- **Meaningfulness**: 4/5 -- silent NaN propagation causes hard-to-debug downstream failures
- **Invasiveness**: 1/5 -- add 2 assertions to `validate()`
- **Description**: `SipConfig::validate()` only checks `order` range. `clip_sigma` and `clip_iterations` have no validation. A negative `clip_sigma` would produce NaN in the MAD-based threshold at line 213. Add assertions for positive values.

#### [F5] Hardcoded singular matrix threshold in 3 places
- **Location**: `distortion/sip/mod.rs:441,494`, `distortion/tps/mod.rs:312`
- **Category**: Consistency
- **Impact**: 3/5 -- three independent `1e-12` thresholds with no shared constant
- **Meaningfulness**: 3/5 -- if tuning is needed, three places must be found and updated
- **Invasiveness**: 1/5 -- extract one `const SINGULAR_THRESHOLD: f64 = 1e-12;`
- **Description**: Both SIP (Cholesky and LU) and TPS (LU) solvers hardcode `1e-12` as the singular matrix detection threshold. Extract to a shared constant with a comment explaining the choice.

#### [F6] ~~Stale `#[allow(dead_code)]` on `WarpParams::new()`~~ DONE
- **Location**: `interpolation/mod.rs:33`
- **Category**: Dead code annotation
- **Impact**: 2/5 -- misleading lint signal; `WarpParams::new()` IS used in tests
- **Meaningfulness**: 3/5 -- incorrect `#[allow]` hides real dead code if it appears later
- **Invasiveness**: 1/5 -- remove one line
- **Description**: `WarpParams::new()` is used extensively in `interpolation/tests.rs` and `tests/warping.rs`. The `#[allow(dead_code)]` is incorrect and should be removed.

#### [F7] SIP error handling mixes `assert!` and `Option` in same function
- **Location**: `distortion/sip/mod.rs:147-159`
- **Category**: Consistency
- **Impact**: 3/5 -- same function panics on some bad input, returns None on other bad input
- **Meaningfulness**: 4/5 -- inconsistent contract makes caller expectations unclear
- **Invasiveness**: 1/5 -- convert asserts to early-return None or vice versa
- **Description**: `fit_from_transform` uses `assert_eq!` for ref/target length mismatch (line 147-150) but returns `None` for insufficient points (line 157-159). Both are caller-error conditions. Pick one pattern: either panic on all precondition violations (per project convention) or return `None` for all.

#### [F8] Quality score formula undocumented
- **Location**: `result.rs:152-159`
- **Category**: Documentation / maintainability
- **Impact**: 3/5 -- magic constants with no justification
- **Meaningfulness**: 4/5 -- no user can understand or adjust quality scoring
- **Invasiveness**: 1/5 -- add doc comment
- **Description**: The quality score formula `exp(-rms/2.0) * min(inliers/20.0, 1.0)` has unexplained constants: why divide RMS by 2.0? Why 20 inliers for saturation? Why 4 inlier minimum? Add a doc comment explaining the formula, its calibration, and what scores mean in practice.

### Priority 2 -- High Impact, Moderate Invasiveness

#### [F9] Duplicate LU solvers in SIP and TPS
- **Location**: `distortion/sip/mod.rs:471-527`, `distortion/tps/mod.rs:282-341`
- **Category**: Generalization
- **Impact**: 4/5 -- ~90% identical Gaussian elimination with partial pivoting
- **Meaningfulness**: 5/5 -- bug fixes must be applied twice; testing burden doubled
- **Invasiveness**: 3/5 -- different backing stores (flat array vs Vec<Vec<f64>>)
- **Description**: Both modules implement LU decomposition with partial pivoting. The SIP version works on flat `[f64]` arrays with `ArrayVec` return, while TPS uses `Vec<Vec<f64>>` with heap allocation. Same pivoting logic, same threshold, same back-substitution. Extract a shared solver, or have TPS call the SIP solver after converting its matrix to flat layout.

#### [F10] Duplicate test helper functions across test modules
- **Location**: `tests/robustness.rs` vs `tests/transform_types.rs` (`apply_affine`, `apply_homography`, FWHM constants)
- **Category**: Generalization
- **Impact**: 3/5 -- identical implementations, maintenance sync risk
- **Meaningfulness**: 4/5 -- real duplication that will drift
- **Invasiveness**: 3/5 -- extract to shared test helpers module
- **Description**: `apply_affine` and `apply_homography` are copied verbatim between test files. FWHM constants (`FWHM_TIGHT`, `FWHM_NORMAL`, `FWHM_SUBPIXEL`, `FWHM_LOOSE`) are duplicated between `robustness.rs` (lines 23-26) and `transform_types.rs` (lines 20-24). The `max_error` computation pattern (iterate pairs, apply transform, compute distance, track max) is repeated 5+ times. Extract into `tests/helpers.rs`.

#### [F11] `VoteMatrix::iter_nonzero()` returns `Vec` instead of iterator
- **Location**: `triangle/voting.rs:76-89`
- **Category**: Data flow / simplification
- **Impact**: 3/5 -- allocates intermediate Vec, then immediately filtered and collected again
- **Meaningfulness**: 3/5 -- double allocation in matching phase
- **Invasiveness**: 3/5 -- change return type and update `resolve_matches` caller
- **Description**: `iter_nonzero()` collects all non-zero entries into a `Vec<(usize, usize, usize)>`, then `resolve_matches` immediately `.into_iter().filter().map().collect()` into another Vec. Return an `impl Iterator` or add `iter_nonzero_filtered(min_votes)` that combines both steps.

#### [F12] Duplicate dimension extraction logic in spatial module
- **Location**: `spatial/mod.rs:81-92,164-165,228-229,282-283`
- **Category**: Simplification / generalization
- **Impact**: 3/5 -- same `if split_dim == 0 { p.x } else { p.y }` in 4 places
- **Meaningfulness**: 3/5 -- maintenance burden, changes could introduce inconsistency
- **Invasiveness**: 2/5 -- extract one `#[inline] fn dim_value(p: DVec2, dim: usize) -> f64`
- **Description**: The split dimension extraction pattern appears in `k_nearest_range`, `nearest_one_range`, `radius_indices_range`, and the build comparator. Extract to a shared helper. Similarly, the near/far subtree selection (lines 169-173, 232-236) is duplicated.

#### [F13] `local_optimization()` takes 8 parameters
- **Location**: `ransac/mod.rs:170-183`
- **Category**: API cleanliness
- **Impact**: 3/5 -- hard to read, easy to swap arguments
- **Meaningfulness**: 4/5 -- real maintainability issue
- **Invasiveness**: 3/5 -- introduce a context struct or builder
- **Description**: The method signature spans 12 lines with 8 parameters: `ref_points`, `target_points`, `initial_transform`, `initial_inliers`, `scorer`, `inlier_buf`, `point_buf_ref`, `point_buf_target`. Group the buffer parameters into a reusable `RansacBuffers` struct, or pass the point slices as a `(&[DVec2], &[DVec2])` tuple.

#### [F14] Duplicate point normalization in affine and homography estimation
- **Location**: `ransac/transforms.rs:176-177,272-273`
- **Category**: Generalization
- **Impact**: 3/5 -- same normalization pattern in both estimators
- **Meaningfulness**: 3/5 -- normalization code duplicated with identical logic
- **Invasiveness**: 2/5 -- extract shared `normalize_points()` helper
- **Description**: Both `estimate_affine()` and `estimate_homography()` compute Hartley normalization with identical centering and scaling logic. Extract into a shared `fn normalize_points(points: &[DVec2]) -> (Vec<DVec2>, DVec2, f64)` helper.

#### [F15] Config boilerplate repeated 18+ times in robustness tests
- **Location**: `tests/robustness.rs` (lines 45, 88, 128, 168, 218, 265, 309, 350, 388, 424, 454, 490, 530, 561, 598, 671, 702, 748, 800, 850, 889, 939, 992)
- **Category**: Generalization / test quality
- **Impact**: 3/5 -- massive boilerplate obscures test intent
- **Meaningfulness**: 4/5 -- real maintenance burden; easy to miss a field when copying
- **Invasiveness**: 2/5 -- create helper functions like `config_for_translation()`, `config_for_similarity()`, etc.
- **Description**: Nearly identical `Config { transform_type, min_stars: 6, min_matches: 4, ..Default::default() }` blocks appear 18+ times. Extract per-transform-type config constructors into `tests/helpers.rs`.

### Priority 3 -- Moderate Impact

#### [F16] TPS module blanket `#[allow(dead_code)]` suppression
- **Location**: `distortion/tps/mod.rs:1-2`
- **Category**: Dead code
- **Impact**: 3/5 -- 445-line module with blanket suppression; new dead code accumulates undetected
- **Meaningfulness**: 3/5 -- significant implementation on unintegrated code
- **Invasiveness**: 2/5 -- move to per-item `#[allow]` or feature flag
- **Description**: The entire TPS module has `#![allow(dead_code)]`. This masks any new dead code within TPS. Move the annotation to specific public items, or put TPS behind a feature flag.

#### [F17] TPS `compute_residuals` unnecessarily denormalizes then re-normalizes
- **Location**: `distortion/tps/mod.rs:241-251`
- **Category**: Data flow / simplification
- **Impact**: 3/5 -- O(n) unnecessary math per residual computation
- **Meaningfulness**: 3/5 -- conceptually confusing round-trip
- **Invasiveness**: 2/5 -- evaluate directly in normalized space, denormalize only the result
- **Description**: `compute_residuals` stores normalized control points, denormalizes to pixel space (line 247), then `transform()` re-normalizes internally (line 173). Direct evaluation in normalized space would eliminate 2n denormalization operations and simplify the data flow.

#### [F18] SIP monomial basis evaluation duplicated 3 times
- **Location**: `distortion/sip/mod.rs:199-202,349-352,436-437`
- **Category**: Generalization
- **Impact**: 2/5 -- same loop with identical logic
- **Meaningfulness**: 3/5 -- changes to basis evaluation must be applied in 3 places
- **Invasiveness**: 2/5 -- extract helper `fn evaluate_basis(u, v, terms) -> ArrayVec<f64, N>`
- **Description**: The monomial basis evaluation loop (iterate terms, compute `monomial(u, v, p, q)`) appears in residual computation, correction, and normal equations. Similarly, the coordinate normalization `(point - ref_pt) / scale` appears in 3 places (lines 194-195, 344-345, 432-433). Extract small helpers.

#### [F19] `RngWrapper` enum dispatch overhead in RANSAC hot loop
- **Location**: `ransac/mod.rs:74-116`
- **Category**: Simplification / performance
- **Impact**: 3/5 -- runtime match on every RNG call across thousands of iterations
- **Meaningfulness**: 3/5 -- measurable overhead in tight loop
- **Invasiveness**: 3/5 -- would require generic parameter or always-seeded approach
- **Description**: `RngWrapper` wraps `ChaCha8Rng` or `ThreadRng` in an enum, forcing a match on every RNG call. Alternative: always use `ChaCha8Rng`, seeding from `thread_rng()` when no user seed is provided. This eliminates the enum entirely and makes RANSAC deterministic-by-default.

#### [F20] Hardcoded progressive sampling strategy in RANSAC
- **Location**: `ransac/mod.rs:437-487`
- **Category**: API cleanliness / simplification
- **Impact**: 2/5 -- magic numbers (3 phases, 25%/50%/full pools) with no configuration
- **Meaningfulness**: 3/5 -- makes tuning impossible without code changes
- **Invasiveness**: 3/5 -- either document the design or make configurable
- **Description**: The sampling phases (0-33%, 33-66%, 66-100%) and pool selection (25%, 50%, full) are hardcoded with magic numbers in `estimate()`. Line 468: `let phase = iteration * 3 / max_iter;`. Either document the rationale with references or extract the strategy into a configurable component.

#### [F21] Robustness test validates transform on outlier stars
- **Location**: `tests/robustness.rs:179-190`
- **Category**: Test quality
- **Impact**: 3/5 -- test doesn't verify what it claims
- **Meaningfulness**: 4/5 -- validates alignment on the 20% spurious stars it intentionally added
- **Invasiveness**: 1/5 -- filter to original stars only before computing max_error
- **Description**: `test_outlier_rejection_20_percent_spurious()` computes `max_error` across ALL reference/target stars including the 20% spurious ones. The test should validate transform quality only on the original 80% that were genuine correspondences.

#### [F22] Lanczos scalar fast and slow paths duplicate 6x6 loop
- **Location**: `interpolation/warp/mod.rs:281-351`
- **Category**: Generalization
- **Impact**: 3/5 -- ~60 lines of duplicated kernel logic
- **Meaningfulness**: 3/5 -- same algorithm with different pixel access patterns
- **Invasiveness**: 3/5 -- paths have performance-critical differences (unchecked vs bounds-checked)
- **Description**: The scalar fast path (lines 281-313) and slow path (lines 315-344) both implement the 6x6 Lanczos3 accumulation loop with identical structure, differing only in pixel sampling: `get_unchecked` vs bounds-checked. Could be unified with a generic sample function, though this risks preventing LLVM from optimizing the unchecked path.

#### [F23] `panic!()` instead of `unreachable!()` for Auto transform type
- **Location**: `ransac/transforms.rs:47-49`
- **Category**: Consistency
- **Impact**: 2/5 -- panic message doesn't explain precondition; callers must know to resolve Auto
- **Meaningfulness**: 3/5 -- `unreachable!()` better communicates intent
- **Invasiveness**: 1/5 -- change one line
- **Description**: `estimate_transform` panics for `TransformType::Auto` with a message "Auto must be resolved..." This is correct but should use `unreachable!("Auto must be resolved before calling estimate_transform")` to communicate that this is a logic error, not a runtime possibility.

#### [F24] Three nearly identical rotation tests could be parametrized
- **Location**: `tests/synthetic/warping.rs:735-872`
- **Category**: Generalization / test quality
- **Impact**: 2/5 -- 137 lines of triplicated test logic
- **Meaningfulness**: 3/5 -- same test at 45, 90, -45 degrees with identical structure
- **Invasiveness**: 2/5 -- extract helper, loop over angle array
- **Description**: `test_large_rotation_45_degrees()`, `test_large_rotation_90_degrees()`, and `test_large_rotation_negative_45_degrees()` all generate stars, apply rotation + translation, register, and verify. Consolidate into a single parametrized test or a helper + 3 one-liner tests.

### Priority 4 -- Low Priority

#### [F25] `vote_for_correspondences` takes redundant `n_ref` / `n_target`
- **Location**: `triangle/voting.rs:111-118`
- **Category**: API cleanliness
- **Impact**: 2/5 -- 6 parameters including 2 derivable from input
- **Meaningfulness**: 2/5 -- `n_ref`/`n_target` are original point counts (not triangle counts), so not trivially derivable
- **Invasiveness**: 2/5 -- change signature and single call site
- **Description**: The function takes `n_ref` and `n_target` explicitly, but these are always `ref_positions.len()` and `target_positions.len()` from the caller.

#### [F26] `BoundedMaxHeap::Large` allocates `capacity + 1`
- **Location**: `spatial/mod.rs:347-351`
- **Category**: Data flow
- **Impact**: 1/5 -- wastes 8-16 bytes per large heap
- **Meaningfulness**: 2/5 -- suggests off-by-one thinking, though not a bug
- **Invasiveness**: 1/5 -- change to `Vec::with_capacity(capacity)`
- **Description**: The `Large` variant allocates `capacity + 1` elements but only uses up to `capacity`. The `+1` appears unnecessary.

#### [F27] `k_nearest` allocates heap before empty check
- **Location**: `spatial/mod.rs:127-138`
- **Category**: Data flow
- **Impact**: 1/5 -- minor inefficiency for empty tree case
- **Meaningfulness**: 2/5 -- easy optimization
- **Invasiveness**: 1/5 -- reorder two lines
- **Description**: The empty tree check happens after `BoundedMaxHeap::new(k)` is created. Move the check before heap creation.

#### [F28] `ThinPlateSpline::control_points()` returns normalized coordinates
- **Location**: `distortion/tps/mod.rs:39-52,232-233`
- **Category**: API cleanliness
- **Impact**: 3/5 -- public getter returns `[-1,1]` range, not pixel space
- **Meaningfulness**: 3/5 -- violates principle of least surprise
- **Invasiveness**: 2/5 -- add denormalization in getter or document clearly
- **Description**: After fitting, `control_points()` returns normalized coordinates while `compute_residuals()` expects pixel-space target points and internally denormalizes. Either store in pixel space, denormalize in the getter, or add explicit documentation.

#### [F29] `recover_matches` creates new `HashSet`s every iteration
- **Location**: `mod.rs:374-381`
- **Category**: Data flow
- **Impact**: 2/5 -- allocates two HashSets per iteration (up to 5 iterations)
- **Meaningfulness**: 2/5 -- small cost relative to transform estimation
- **Invasiveness**: 2/5 -- could reuse with `.clear()`
- **Description**: Each iteration of the recovery loop creates fresh `matched_target` and `matched_ref` `HashSet`s. For typical counts (50-200), this is cheap, but reusing preallocated sets would be cleaner.

#### [F30] `form_triangles_kdtree` exposes `k_neighbors` parameter
- **Location**: `triangle/matching.rs:19`
- **Category**: API cleanliness
- **Impact**: 2/5 -- public function exposes implementation detail
- **Meaningfulness**: 2/5 -- callers must guess appropriate k value
- **Invasiveness**: 1/5 -- make `pub(crate)` since `match_triangles` is the intended API
- **Description**: `form_triangles_kdtree(positions, k_neighbors)` is public but `k_neighbors` is an implementation detail. The high-level API `match_triangles` computes k internally. Consider `pub(crate)`.

## Cross-Cutting Patterns

### Pattern 1: Stale `#[allow(dead_code)]` annotations
Found in 3 locations (F3, F6, F16). These annotations mask real issues. Replace with proper visibility (`pub(crate)`) or remove when code IS used. Reserve `#[allow(dead_code)]` for intentionally unused code with a comment explaining why.

### Pattern 2: Duplicate algorithms across submodules
The LU solver duplication (F9) is the most significant, but the pattern appears in: test helpers (F10), spatial traversal (F12), point normalization (F14), SIP basis evaluation (F18), and warp kernel paths (F22). When two implementations of the same algorithm exist, they inevitably drift.

### Pattern 3: Inconsistent error handling patterns
The codebase mixes `panic!`/`assert!`, `Option<>` returns, and `Result<>` within similar contexts. SIP uses both in one function (F7). RANSAC uses `panic!` where `unreachable!` fits better (F23). Config validation is thorough (16 assertions) but SipConfig validates only one field (F4). Establish a clear convention: panic on logic errors (per project rules), return `None`/`Result` only for expected failures.

### Pattern 4: Test boilerplate duplication
The test suite has excellent coverage but significant duplication: config construction (F15, 18+ instances), helper functions copied between files (F10), and parametrizable test series (F24, rotation tests; robustness extreme-scale tests). A shared `tests/helpers.rs` with config constructors, transform application helpers, and metric computation would cut hundreds of lines.

### Pattern 5: Intermediate Vec allocations in hot paths
Several functions allocate temporary Vecs that are immediately consumed: `iter_nonzero()` (F11), `recover_matches` HashSets (F29), and `estimate_similarity` centered vectors (ransac). These are typically not performance bottlenecks, but for consistency, hot-path functions should prefer iterators or reusable buffers.
