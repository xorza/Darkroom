# Code Review: registration

## Summary

Overall code quality is **good to excellent**. The registration module is well-architected, thoroughly tested, and mathematically correct. No correctness bugs were found across any submodule. The codebase has comprehensive test coverage with hand-computed expected values, proper edge case handling, and SIMD-vs-scalar reference tests.

The main improvement opportunities are: eliminating code duplication (LU solvers, test helpers, spatial traversal patterns), tightening internal API visibility, adding missing assertions in unsafe SIMD code, and cleaning up stale `#[allow(dead_code)]` annotations.

**Files reviewed**: 25 source files across 6 submodules + 4 top-level files.

## Findings

### Priority 1 — High Impact, Low Invasiveness

#### [F1] Stale `#[allow(dead_code)]` on `WarpParams::new()`
- **Location**: `interpolation/mod.rs:33`
- **Category**: Dead code annotation
- **Impact**: 2/5 — misleading lint signal; `WarpParams::new()` IS used in tests
- **Meaningfulness**: 3/5 — incorrect `#[allow]` hides real dead code if it appears later
- **Invasiveness**: 1/5 — remove one line
- **Description**: `WarpParams::new()` is used extensively in `interpolation/tests.rs` (lines 8, 266, 743, etc.) and `tests/warping.rs`. The `#[allow(dead_code)]` is incorrect and should be removed.

#### [F2] `#[allow(dead_code)]` on `RansacResult` fields instead of `pub(crate)`
- **Location**: `ransac/mod.rs:126-131`
- **Category**: API cleanliness
- **Impact**: 3/5 — `#[allow(dead_code)]` hides genuinely unused code; `pub(crate)` is the right tool
- **Meaningfulness**: 4/5 — already noted in NOTES-AI.md item #9
- **Invasiveness**: 1/5 — change 2 annotations to `pub(crate)` visibility
- **Description**: `iterations` and `inlier_ratio` fields on `RansacResult` are marked `#[allow(dead_code)]` but are useful diagnostics. Change to `pub(crate)` so they're available to the parent module and tests without suppressing lint warnings.

#### [F3] Missing bounds assertions in `lanczos3_kernel_fma`
- **Location**: `interpolation/warp/sse.rs:282-313`
- **Category**: Safety / assertions
- **Impact**: 4/5 — unsafe SIMD function assumes `kx + 7 < input_width` and valid `ky` range
- **Meaningfulness**: 4/5 — would catch caller bugs before memory corruption
- **Invasiveness**: 1/5 — add 2 `debug_assert!` lines
- **Description**: The function has a comment stating the precondition (`kx + 7 < input_width`) but no runtime check. Callers enforce this (line 256), but a `debug_assert!` inside the unsafe function provides defense-in-depth. Add:
  ```rust
  debug_assert!(kx + 8 <= input_width);
  debug_assert!(ky + 6 <= pixels.len() / input_width);
  ```

#### [F4] Incomplete `SipConfig` validation
- **Location**: `distortion/sip/mod.rs:85-93`
- **Category**: Consistency / validation
- **Impact**: 3/5 — zero or negative `clip_sigma` produces NaN in threshold calculation
- **Meaningfulness**: 4/5 — silent NaN propagation causes hard-to-debug downstream failures
- **Invasiveness**: 1/5 — add 2 assertions to `validate()`
- **Description**: `SipConfig::validate()` only checks `order` range. `clip_sigma` and `clip_iterations` have no validation. A negative `clip_sigma` would produce NaN in the MAD-based threshold at line 213. Add:
  ```rust
  assert!(self.clip_sigma > 0.0, "clip_sigma must be positive");
  assert!(self.clip_iterations > 0, "clip_iterations must be positive");
  ```

#### [F5] Hardcoded singular matrix threshold in 3 places
- **Location**: `distortion/sip/mod.rs:441,494`, `distortion/tps/mod.rs:312`
- **Category**: Consistency
- **Impact**: 3/5 — three independent `1e-12` thresholds with no shared constant
- **Meaningfulness**: 3/5 — if tuning is needed, three places must be found and updated
- **Invasiveness**: 1/5 — extract one `const SINGULAR_THRESHOLD: f64 = 1e-12;`
- **Description**: Both SIP (Cholesky and LU) and TPS (LU) solvers hardcode `1e-12` as the singular matrix detection threshold. Extract to a shared constant with a comment explaining the choice.

### Priority 2 — High Impact, Moderate Invasiveness

#### [F6] Duplicate LU solvers in SIP and TPS
- **Location**: `distortion/sip/mod.rs:471-527`, `distortion/tps/mod.rs:282-341`
- **Category**: Generalization
- **Impact**: 4/5 — ~90% identical Gaussian elimination with partial pivoting
- **Meaningfulness**: 5/5 — bug fixes must be applied twice; testing burden doubled
- **Invasiveness**: 3/5 — different backing stores (flat array vs Vec<Vec<f64>>)
- **Description**: Both modules implement LU decomposition with partial pivoting using the same algorithm. The SIP version works on flat `[f64]` arrays with `ArrayVec` return, while TPS uses `Vec<Vec<f64>>` with heap allocation. Same pivoting logic, same threshold, same back-substitution. Extract a shared solver, or at minimum have TPS call the SIP solver after converting its matrix to flat layout.

#### [F7] Duplicate test helper functions across test modules
- **Location**: `tests/robustness.rs:1028-1040` vs `tests/transform_types.rs:402-414` (`apply_affine`); `tests/robustness.rs:1129-1142` vs `tests/transform_types.rs:566-579` (`apply_homography`)
- **Category**: Generalization
- **Impact**: 3/5 — identical implementations, maintenance sync risk
- **Meaningfulness**: 4/5 — real duplication that will drift
- **Invasiveness**: 3/5 — extract to shared test helpers module
- **Description**: `apply_affine` and `apply_homography` are copied verbatim between test files. Additionally, FWHM constants (`FWHM_TIGHT`, `FWHM_NORMAL`, `FWHM_SUBPIXEL`, `FWHM_LOOSE`) are duplicated between `robustness.rs` and `transform_types.rs`. The `max_error` computation pattern (iterate pairs, apply transform, compute distance, track max) is repeated 5+ times across test files. Extract these into a shared `test_helpers` module under `tests/`.

#### [F8] `VoteMatrix::iter_nonzero()` returns `Vec` instead of iterator
- **Location**: `triangle/voting.rs:76-89`
- **Category**: Data flow / simplification
- **Impact**: 3/5 — allocates intermediate Vec, then immediately filtered and collected again
- **Meaningfulness**: 3/5 — double allocation in hot path (matching phase)
- **Invasiveness**: 3/5 — change return type and update `resolve_matches` caller
- **Description**: `iter_nonzero()` collects all non-zero entries into a `Vec<(usize, usize, usize)>`, then `resolve_matches` (line 170-180) immediately `.into_iter().filter().map().collect()` into another Vec. This creates two allocations where one suffices. Options: (a) return an iterator using `impl Iterator`, (b) add a `iter_nonzero_filtered(min_votes)` method that combines both steps, or (c) have `resolve_matches` take `&VoteMatrix` and iterate directly.

#### [F9] Duplicate dimension extraction logic in spatial module
- **Location**: `spatial/mod.rs:81-92,164-165,228-229,282-283`
- **Category**: Simplification / generalization
- **Impact**: 3/5 — same `if split_dim == 0 { p.x } else { p.y }` in 4 places
- **Meaningfulness**: 3/5 — maintenance burden, changes could introduce inconsistency
- **Invasiveness**: 2/5 — extract one `#[inline] fn dim_value(p: DVec2, dim: usize) -> f64`
- **Description**: The split dimension extraction pattern appears in `k_nearest_range`, `nearest_one_range`, `radius_indices_range`, and the build comparator. Extract to:
  ```rust
  #[inline]
  fn dim_value(p: DVec2, dim: usize) -> f64 {
      if dim == 0 { p.x } else { p.y }
  }
  ```
  Similarly, the near/far subtree selection tuple construction (lines 169-173, 232-236) is duplicated and could share a helper.

### Priority 3 — Moderate Impact

#### [F10] TPS module blanket `#[allow(dead_code)]` suppression
- **Location**: `distortion/tps/mod.rs:1-2`
- **Category**: Dead code
- **Impact**: 3/5 — 445-line module marked WIP with blanket suppression; new dead code accumulates undetected
- **Meaningfulness**: 3/5 — significant implementation effort (140 lines of tests) on unintegrated code
- **Invasiveness**: 2/5 — either integrate TPS into pipeline or move to `#[allow(dead_code)]` on specific items
- **Description**: The entire TPS module has `#![allow(dead_code)]` at the module level. This is appropriate for WIP code, but the blanket suppression means any new dead code within TPS is invisible to the linter. Consider: (a) removing the blanket allow and adding `#[allow(dead_code)]` on specific public items, or (b) putting TPS behind a feature flag so it doesn't pollute the default build.

#### [F11] `RngWrapper` enum dispatch overhead
- **Location**: `ransac/mod.rs:74-116`
- **Category**: Simplification
- **Impact**: 3/5 — runtime match dispatch on every RNG call across thousands of RANSAC iterations
- **Meaningfulness**: 3/5 — measurable overhead in tight loop (though small per-call)
- **Invasiveness**: 3/5 — would require generic parameter on `ransac_loop` or trait object
- **Description**: `RngWrapper` wraps `ChaCha8Rng` (seeded) or `ThreadRng` (random) in an enum, forcing a match on every `try_next_u32/u64/fill_bytes` call. For RANSAC with thousands of iterations each calling RNG multiple times, this adds branch overhead. Alternative: make the RANSAC loop generic over `R: TryRng`, or use a single `ChaCha8Rng` always (seed from `thread_rng` when no user seed provided).

#### [F12] Lanczos3 warp: scalar fast and slow paths duplicate 6x6 loop
- **Location**: `interpolation/warp/mod.rs:281-351`
- **Category**: Generalization
- **Impact**: 3/5 — ~60 lines of duplicated kernel logic
- **Meaningfulness**: 3/5 — same algorithm with different pixel access patterns
- **Invasiveness**: 3/5 — paths have performance-critical differences (unchecked vs bounds-checked)
- **Description**: The scalar fast path (lines 281-313) and slow path (lines 315-344) both implement the 6x6 Lanczos3 accumulation loop with identical structure. They differ only in how pixels are sampled: `get_unchecked` vs bounds-checked `sample_pixel`. Could be unified with a generic sample function or closure, though this risks preventing LLVM from optimizing the unchecked path.

#### [F13] `vote_for_correspondences` takes too many parameters
- **Location**: `triangle/voting.rs:111-118`
- **Category**: API cleanliness
- **Impact**: 2/5 — 6 parameters including `n_ref` and `n_target` which are derivable
- **Meaningfulness**: 3/5 — `n_ref`/`n_target` are the original point counts, not triangle counts; passing them explicitly is error-prone
- **Invasiveness**: 2/5 — change signature and single call site in `matching.rs:76-83`
- **Description**: The function takes `n_ref` and `n_target` explicitly, but these are always `ref_positions.len()` and `target_positions.len()` from the caller. These could be passed as part of a context struct or derived from the triangle data.

#### [F14] TPS `control_points()` returns normalized coordinates
- **Location**: `distortion/tps/mod.rs:39-52,232-233`
- **Category**: API cleanliness
- **Impact**: 3/5 — public method returns coordinates in `[-1, 1]` range, not pixel space
- **Meaningfulness**: 3/5 — violates principle of least surprise
- **Invasiveness**: 2/5 — add denormalization in getter or document clearly
- **Description**: After fitting, `ThinPlateSpline` stores control points in normalized coordinates. The public `control_points()` getter returns these normalized values without documenting the coordinate system. Meanwhile, `compute_residuals()` expects pixel-space target points and internally denormalizes (line 247), creating a confusing API. Either store in pixel space, denormalize in the getter, or add explicit documentation.

#### [F15] Duplicate `load_*_calibrated_lights` functions in real_data tests
- **Location**: `tests/real_data.rs:38-56,59-85,247-271`
- **Category**: Generalization
- **Impact**: 2/5 — three near-identical directory enumeration functions
- **Meaningfulness**: 3/5 — real duplication with slightly different return types
- **Invasiveness**: 2/5 — consolidate into `load_n_calibrated_lights(n: usize) -> Vec<AstroImage>`
- **Description**: `load_first_calibrated_light`, `load_two_calibrated_lights`, and `load_all_calibrated_lights` duplicate the directory listing, filtering, and loading logic. They differ only in the count check and return type (single vs tuple vs Vec).

### Priority 4 — Low Priority

#### [F16] `SoftClampAccum` field names too terse
- **Location**: `interpolation/warp/mod.rs:22-33`
- **Category**: Consistency
- **Impact**: 2/5 — `sp`, `sn`, `wp`, `wn` are documented but non-obvious
- **Meaningfulness**: 2/5 — clarity improvement for maintainers
- **Invasiveness**: 2/5 — rename 4 fields and all uses
- **Description**: Fields could be `sum_positive`, `sum_negative`, `weight_positive`, `weight_negative` for self-documentation. Current names are acceptable for a performance-critical internal struct with doc comments, but longer names would help during debugging.

#### [F17] `BoundedMaxHeap::Large` allocates `capacity + 1`
- **Location**: `spatial/mod.rs:347-351`
- **Category**: Data flow
- **Impact**: 1/5 — wastes 8-16 bytes per large heap
- **Meaningfulness**: 2/5 — suggests off-by-one thinking, though not a bug
- **Invasiveness**: 1/5 — change to `Vec::with_capacity(capacity)`
- **Description**: The `Large` variant allocates `capacity + 1` elements but only uses up to `capacity`. The `+1` appears unnecessary.

#### [F18] `k_nearest` allocates heap before empty check
- **Location**: `spatial/mod.rs:127-138`
- **Category**: Data flow
- **Impact**: 1/5 — minor inefficiency for empty tree case
- **Meaningfulness**: 2/5 — easy optimization
- **Invasiveness**: 1/5 — reorder two lines
- **Description**: The empty tree check happens after `BoundedMaxHeap::new(k)` is created. Move the `if self.indices.is_empty() || k == 0` check before heap creation.

#### [F19] `recover_matches` creates new `HashSet`s every iteration
- **Location**: `mod.rs:374-381`
- **Category**: Data flow
- **Impact**: 2/5 — allocates two `HashSet`s per iteration (up to 5 iterations)
- **Meaningfulness**: 2/5 — small cost relative to transform estimation
- **Invasiveness**: 2/5 — could reuse with `.clear()` or use `Vec<bool>` flags
- **Description**: Each iteration of the recovery loop creates fresh `matched_target` and `matched_ref` `HashSet`s. For typical match counts (50-200), this is cheap, but reusing preallocated sets would be cleaner.

#### [F20] `form_triangles_kdtree` exposes `k_neighbors` parameter
- **Location**: `triangle/matching.rs:19`
- **Category**: API cleanliness
- **Impact**: 2/5 — public function exposes implementation detail
- **Meaningfulness**: 2/5 — callers must guess appropriate k value
- **Invasiveness**: 2/5 — could make `pub(crate)` since `match_triangles` is the intended API
- **Description**: `form_triangles_kdtree(positions, k_neighbors)` is public but `k_neighbors` is an implementation detail. The high-level API `match_triangles` computes k internally (line 60). Consider making `form_triangles_kdtree` `pub(crate)`.

## Cross-Cutting Patterns

### Pattern 1: Stale `#[allow(dead_code)]` annotations
Found in 3 locations (F1, F2, F10). These annotations mask real issues and should be replaced with proper visibility (`pub(crate)`) or removed when the code IS used. The project convention should be: use `#[allow(dead_code)]` only for intentionally unused code with a comment explaining why.

### Pattern 2: Duplicate algorithms across submodules
The LU solver duplication (F6) is the most significant instance, but the pattern appears in test helpers (F7), spatial traversal (F9), and warp kernel paths (F12). When two implementations of the same algorithm exist, they inevitably drift. Priority should be given to F6 (LU solver) since it affects correctness-critical code.

### Pattern 3: Intermediate Vec allocations in hot paths
Several functions allocate temporary Vecs that are immediately consumed: `iter_nonzero()` (F8), `recover_matches` HashSets (F19), and `estimate_similarity` centered vectors (ransac). These are typically not performance bottlenecks (matching and RANSAC dominate runtime), but for consistency, hot-path functions should prefer iterators or reusable buffers.

### Pattern 4: Inconsistent validation depth
`Config::validate()` in `config.rs` is thorough (16 assertions). `SipConfig::validate()` only checks `order` (F4). `TriangleParams` has no validation at all. Validation should be consistent across configuration types, at minimum checking for obviously invalid values that would cause NaN or panics.
