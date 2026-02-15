# Code Review: lumos/src

## Summary

Comprehensive review of all 10 submodules (~189 files, ~88k lines). The codebase is
generally well-structured with clear separation of concerns, good SIMD optimization
patterns, and solid test coverage. The main themes across modules are:

1. **Inconsistent error handling** — mix of `assert!`/`unwrap` for user-facing errors vs
   `Result` returns, sometimes within the same module
2. **API surface leakage** — internal types, `#[allow(dead_code)]` on public items,
   scratch buffers in function signatures
3. **Minor duplication** — gradient generation, Neumaier reduction, margin checks
4. **Magic constants** — numerical thresholds without documented rationale

No critical bugs found. Most findings are maintainability improvements.

### Completed Findings

- **[F1]** Added `# Panics` docs to all panicking public functions (stack, register, validate, fit_from_transform, add_image)
- **[F2]** Cleaned up `#[allow(dead_code)]`: removed blanket suppressions, added targeted per-item annotations with comments
- **[F3]** Fixed float equality in `sgarea()`: `dx == 0.0` → `dx.abs() < SGAREA_DX_MIN`
- **[F4]** Cleaned up stale comments/TODOs (drizzle "Allow dead code", median filter "todo swap?")
- **[F5]** Removed redundant `to_image()`; `save()` now uses `self.clone().into()`
- **[F8]** Changed `debug_assert!` → `assert!` for w≈0 in `DMat3::transform_point`
- **[F15]** Deduplicated gradient logic: `background_map.rs` reuses `patterns.rs` gradients
- **[F17]** Extracted named constants in drizzle: `JACOBIAN_MIN`, `KERNEL_WEIGHT_MIN`, `SINC_ZERO_THRESHOLD`, `SGAREA_DX_MIN`
- **[F20]** Consolidated duplicate `make_cfa()`/`constant_cfa()` into `testing/mod.rs`
- **[F22]** Extracted `FWHM_MIN`/`FWHM_MAX` constants in star detection
- **[F23]** Extracted `COLLINEARITY_THRESHOLD` constant in RANSAC
- **[F24]** Consolidated noise generation: canonical `add_gaussian_noise` in `patterns.rs`
- **[F6]** Renamed `hot_map` → `defect_map` in defect map tests
- **[F7]** Added `.expect()` messages to non-obvious unwraps in `defect_map.rs`
- **[F14]** Documented CFA `None` → mono fallback in `cfa_color_at()`
- **[F19]** Fixed drizzle dimension mismatch: `Error::ImageLoad` → `Error::DimensionMismatch`
- **[F21]** Documented magic constant 24 in X-Trans same-color median
- **[F28]** Investigated — `w > 0.0` is NOT redundant (prevents division by zero when `min_coverage=0.0`)
- **[F30]** Fixed ProgressCallback doc signature to match `StackingProgress` struct API
- **[F12]** Replaced ambiguous `Option<(f32, f32)>` return in `sigma_clip_iteration` with `ClipResult` enum (Converged/Clipped/TooFew)
- **[F25]** Added `assert!` in `Transform::from_matrix()` to prevent `TransformType::Auto` from being stored
- **[F11]** `precise_wide_field()` now builds on `Self::wide_field()` instead of `Self::default()`
- **[F13]** `save()` now returns `Result<(), ImageLoadError>` with `Save` variant wrapping `imaginarium::Error`
- **[F16]** Documented `num_stars` as maximum attempt count in `generate_globular_cluster()`
- **[F29]** SKIPPED — deblend param ordering already consistent: `(data, pixels, labels, ...config...)`
- **[F32]** Fixed `sort_with_indices` NaN handling: `partial_cmp().unwrap()` → `total_cmp()`
- **[F26]** Removed redundant SIP clone: eliminated `sip_correction` field from `RegistrationResult`, derived from `sip_fit` in `warp_transform()`
- **[F31]** Changed `warp_image` from `pub` to `pub(crate)`

---

## Findings

### Priority 1 — High Impact, Low Invasiveness

#### [F1] ~~Inconsistent panic-vs-Result for input validation (stacking, registration)~~ DONE
- **Location**: `stacking/stack.rs:128-135`, `registration/config.rs:112`,
  `registration/mod.rs:220`
- **Category**: Consistency
- **Impact**: 3/5 — Different failure modes for similar errors confuse callers
- **Meaningfulness**: 3/5 — Empty paths return `Err`, wrong weight count panics
- **Invasiveness**: 2/5 — Convert asserts to `Err` variants or document panic contracts
- **Description**: `stack()` returns `Err(Error::NoPaths)` for empty paths but
  `assert_eq!` panics for wrong weight count. `register()` panics via
  `config.validate()`. Either all user-facing validation should return `Result`, or
  the panic-based APIs need clear `# Panics` documentation.

#### [F2] ~~`#[allow(dead_code)]` on public API items~~ DONE
- **Location**: `registration/ransac/mod.rs:126-130`,
  `registration/interpolation/mod.rs:33`, `star_detection/buffer_pool.rs:38-52`
- **Category**: Dead code / API cleanliness
- **Impact**: 3/5 — Unclear what's part of the real API vs. leftover
- **Meaningfulness**: 3/5 — Suppressed warnings hide genuine dead code
- **Invasiveness**: 1/5 — Remove unused items or remove the allow attribute
- **Description**: Several public struct fields and methods carry
  `#[allow(dead_code)]`. Either these are part of the API (remove the allow) or
  they're unused (remove the code). `RansacResult.iterations` and `.inlier_ratio`
  are documented as "diagnostics" but never read outside tests.

#### [F3] ~~Float equality in drizzle `sgarea()`~~ DONE
- **Location**: `drizzle/mod.rs:750`
- **Category**: Correctness
- **Impact**: 3/5 — Exact `== 0.0` check on computed float can miss near-zero cases
- **Meaningfulness**: 4/5 — Could produce wrong overlap areas for near-vertical segments
- **Invasiveness**: 1/5 — Change `dx == 0.0` to `dx.abs() < 1e-14`
- **Description**: `sgarea()` checks `if dx == 0.0 { return 0.0; }` for vertical
  segment detection. After floating-point arithmetic, `dx` may be very small but
  non-zero, causing the function to proceed with a near-zero denominator.

#### [F4] ~~Stale `// Allow dead code` and TODO comments~~ DONE
- **Location**: `drizzle/mod.rs:24`, `star_detection/median_filter/mod.rs:25`,
  `raw/README.md:35`
- **Category**: Dead code / Documentation
- **Impact**: 2/5 — Misleading or stale information
- **Meaningfulness**: 3/5 — Comments that contradict reality erode trust
- **Invasiveness**: 1/5 — Delete or update one-liners
- **Description**: Drizzle has "Allow dead code for now - new module" but is fully
  implemented. Median filter has `// todo swap?` with no context. Raw README
  references "TODO: DCB" but uses RCD demosaicing.

#### [F5] ~~Redundant `to_image()` alongside `From<AstroImage> for Image`~~ DONE
- **Location**: `astro_image/mod.rs:538-561` vs `639-657`
- **Category**: Duplication
- **Impact**: 2/5 — Two implementations of identical logic can diverge
- **Meaningfulness**: 2/5 — Maintenance hazard
- **Invasiveness**: 1/5 — Remove `to_image()`, use `.into()` instead
- **Description**: Both `to_image()` and the `From` impl perform identical
  grayscale/RGB conversion and image construction. Only one should exist.

#### [F6] ~~Inconsistent variable naming `hot_map` in defect map tests~~ DONE
- **Location**: `calibration_masters/defect_map.rs:392-704`
- **Category**: Consistency
- **Impact**: 2/5 — Module was renamed from "hot pixel" to "defect" but tests lag
- **Meaningfulness**: 3/5 — Confusing when reading test code
- **Invasiveness**: 1/5 — Simple rename of test variables
- **Description**: Test variables use `hot_map` for `DefectMap` instances. The struct
  handles both hot AND cold pixels. Rename to `defect_map` for consistency.

#### [F7] ~~Missing `.expect()` messages on non-obvious unwraps~~ DONE
- **Location**: `calibration_masters/defect_map.rs:131`,
  `testing/real_data/pipeline_bench.rs:32,79,123,175`
- **Category**: Error handling
- **Impact**: 2/5 — Bare `.unwrap()` gives no context on panic
- **Meaningfulness**: 2/5 — Per CLAUDE.md, non-obvious cases need messages
- **Invasiveness**: 1/5 — Add `.expect("reason")` to each
- **Description**: Several `.unwrap()` calls on `Option`/`Result` that are not
  obviously infallible lack `.expect()` messages explaining the invariant.

#### [F8] ~~`DMat3::transform_point` uses `debug_assert!` for w≈0 check~~ DONE
- **Location**: `math/dmat3.rs:138-141`
- **Category**: Robustness
- **Impact**: 2/5 — Silent NaN/infinity in release builds
- **Meaningfulness**: 3/5 — Point-at-infinity division produces garbage silently
- **Invasiveness**: 1/5 — Change to runtime `assert!` or return `Option<DVec2>`
- **Description**: The function divides by the homogeneous coordinate `w`. In debug
  builds, a `debug_assert!` catches `w ≈ 0`, but release builds divide by near-zero
  silently, producing infinity or NaN values that propagate through the pipeline.

---

### Priority 2 — High Impact, Moderate Invasiveness

#### [F9] Scratch buffers in public function signatures
- **Location**: `star_detection/convolution/mod.rs:49-58` (8 params),
  `raw/demosaic/xtrans/mod.rs:27-75` (10 params)
- **Category**: API cleanliness
- **Impact**: 3/5 — Implementation details leak into API
- **Meaningfulness**: 3/5 — Hard to use correctly, easy to pass wrong buffers
- **Invasiveness**: 3/5 — Requires grouping into config/buffer structs
- **Description**: `matched_filter()` takes 3 scratch buffers as separate parameters.
  `process_xtrans()` takes 10 parameters. Grouping into structs
  (`ConvolutionBuffers`, `XTransConfig`) would improve usability.

#### [F10] Neumaier/Kahan reduction logic duplicated across 4 SIMD files
- **Location**: `math/sum/scalar.rs`, `math/sum/avx2.rs`, `math/sum/sse.rs`,
  `math/sum/neon.rs`
- **Category**: Generalization
- **Impact**: 4/5 — ~150 LOC of identical algorithm with different intrinsic names
- **Meaningfulness**: 4/5 — Real maintenance burden across 4 files
- **Invasiveness**: 3/5 — Requires macro or trait-based generation
- **Description**: The Kahan reduction and Neumaier compensation logic appears
  identically in every SIMD backend. A macro `define_reduce_kahan!(T, N)` or a
  generic function parameterized by lane count would eliminate the duplication.
  **Previously reviewed and deferred** — low risk for working numerical code.

#### [F11] ~~Config preset composition is fragile (registration)~~ DONE
- **Location**: `registration/config.rs:245-256`
- **Category**: Consistency / Hidden complexity
- **Impact**: 3/5 — Silent divergence when defaults change
- **Meaningfulness**: 3/5 — `precise_wide_field()` claims to combine presets but
  uses `..Self::default()` instead
- **Invasiveness**: 3/5 — Refactor to call `Self::precise()` then override
- **Description**: `precise_wide_field()` documents itself as combining `precise()`
  and `wide_field()` but actually starts from `Default` and overrides individual
  fields. If any preset gains new fields, this silently diverges.

#### [F12] ~~`sigma_clip_iteration` return semantics are ambiguous~~ DONE
- **Location**: `math/statistics/mod.rs:112-160`
- **Category**: Code clarity
- **Impact**: 2/5 — `None` means both "continue iterating" and "too few items"
- **Meaningfulness**: 3/5 — Could cause unnecessary iterations or subtle bugs
- **Invasiveness**: 2/5 — Change early return for `len < 3` to return `Some`
- **Description**: The function returns `None` to mean "not converged, keep going"
  and `Some(result)` for convergence. But it also returns `None` when `len < 3`,
  which causes the caller's loop to keep iterating even though no further progress
  is possible.

#### [F13] ~~Inconsistent error wrapping — `save()` leaks `imaginarium::Error`~~ DONE
- **Location**: `astro_image/mod.rs` (save method), `astro_image/error.rs`
- **Category**: API consistency
- **Impact**: 2/5 — External error type in public API
- **Meaningfulness**: 2/5 — Breaks error type consistency
- **Invasiveness**: 2/5 — Add wrapping variant to `ImageLoadError`
- **Description**: `from_file()` returns `Result<_, ImageLoadError>` but `save()`
  returns `Result<_, imaginarium::Error>`. The external type leaks into the public
  API. Wrap it in an `ImageLoadError::Save` variant.

#### [F14] ~~Silent fallback when CFA type is `None` in defect correction~~ DONE
- **Location**: `calibration_masters/defect_map.rs:146-152`
- **Category**: Error handling
- **Impact**: 2/5 — Treats missing CFA as monochrome silently
- **Meaningfulness**: 3/5 — Could hide metadata bugs
- **Invasiveness**: 1/5 — Add assertion or document behavior
- **Description**: `cfa_color_at()` returns 0 for `None` CFA type, silently treating
  the image as monochrome. If metadata is accidentally missing, this produces wrong
  defect correction without any warning.

#### [F15] ~~Gradient/vignette logic duplicated 3 times in testing module~~ DONE
- **Location**: `testing/synthetic/patterns.rs:14-76`,
  `testing/synthetic/backgrounds.rs:28-80`,
  `testing/synthetic/background_map.rs:21-103`
- **Category**: Generalization
- **Impact**: 3/5 — Identical math in 3 places
- **Meaningfulness**: 4/5 — Real maintenance burden
- **Invasiveness**: 3/5 — Extract shared gradient primitives
- **Description**: Horizontal/vertical/radial gradient generation is implemented
  three times: as in-place modifications, as additive operations, and as
  `BackgroundEstimate` constructors. All share the same underlying math.

#### [F16] ~~`generate_globular_cluster()` returns fewer stars than `num_stars`~~ DONE
- **Location**: `testing/synthetic/star_field.rs:502-573`
- **Category**: API contract violation
- **Impact**: 3/5 — Parameter name and behavior don't match
- **Meaningfulness**: 3/5 — Misleading API
- **Invasiveness**: 2/5 — Fix loop or rename parameter
- **Description**: The function iterates `num_stars` times but `continue`s on
  out-of-bounds positions, returning fewer stars than requested. Either retry
  failed iterations, rename to `max_stars`, or document the behavior.

---

### Priority 3 — Moderate Impact

#### [F17] ~~Inconsistent epsilon/tolerance constants (drizzle)~~ DONE
- **Location**: `drizzle/mod.rs:248,333,423,481,523,727,750`
- **Category**: Consistency / Maintainability
- **Impact**: 2/5 — Seven different threshold values scattered through one module
- **Meaningfulness**: 3/5 — Hard to audit numerical stability
- **Invasiveness**: 2/5 — Extract named constants
- **Description**: Drizzle uses `f32::EPSILON`, `1e-30`, `1e-6`, `1e-10`, and
  `== 0.0` for various near-zero checks. Named constants with semantic meaning
  (`JACOBIAN_THRESHOLD`, `WEIGHT_EPSILON`) would improve clarity.

#### [F18] `abs_deviation_inplace` computed twice in sigma clipping
- **Location**: `math/statistics/mod.rs:128-141`
- **Category**: Data flow / Efficiency
- **Impact**: 3/5 — Redundant work in hot path
- **Meaningfulness**: 4/5 — Documented as intentional but still wasteful
- **Invasiveness**: 2/5 — Restructure to preserve index correspondence
- **Description**: After `median_f32_fast` destroys array order, deviations must be
  recomputed from scratch. This doubles the work. Tracking indices during the
  partial sort could eliminate the second pass.

#### [F19] Error type mismatch in drizzle dimension check
- **Location**: `drizzle/mod.rs:935-944`
- **Category**: Correctness
- **Impact**: 2/5 — Dimension mismatch reported as `Error::ImageLoad`
- **Meaningfulness**: 2/5 — Confusing error messages
- **Invasiveness**: 1/5 — Use or add a validation error variant
- **Description**: `drizzle_stack()` reports a dimension mismatch as an image load
  error. The image loaded fine; the problem is post-load validation. Use a
  dedicated error variant.

#### [F20] ~~Duplicate `make_cfa()` / `constant_cfa()` test helpers~~ DONE
- **Location**: `calibration_masters/defect_map.rs:382-390` vs
  `calibration_masters/tests.rs:7-15`
- **Category**: Duplication
- **Impact**: 2/5 — Two identical test helpers
- **Meaningfulness**: 2/5 — Consolidation improves clarity
- **Invasiveness**: 1/5 — Use one, delete the other
- **Description**: Both files define a function that creates a constant-value
  `CfaImage` for testing. Consolidate into a single helper in `tests.rs`.

#### [F21] Magic constant `24` in X-Trans same-color median
- **Location**: `calibration_masters/defect_map.rs:370`
- **Category**: Documentation
- **Impact**: 2/5 — Undocumented magic number
- **Meaningfulness**: 2/5 — No derivation for why 24 neighbors
- **Invasiveness**: 1/5 — Add comment
- **Description**: `candidates.len().min(24)` limits X-Trans same-color neighbors
  to 24. The comment says "avoid directional bias" but doesn't explain why 24
  specifically (e.g., ~4 per cardinal/diagonal direction in 6x6 pattern).

#### [F22] ~~FWHM range `0.5..20.0` hardcoded in star detection~~ DONE
- **Location**: `star_detection/detector/stages/fwhm.rs:131`
- **Category**: Consistency / Magic numbers
- **Impact**: 2/5 — Limits aren't configurable or documented
- **Meaningfulness**: 2/5 — Could reject valid stars on unusual setups
- **Invasiveness**: 1/5 — Extract to named constants
- **Description**: The FWHM estimation filter uses hardcoded `(0.5..20.0)` range.
  Extract to `FWHM_MIN_PHYSICAL` / `FWHM_MAX_REASONABLE` constants with
  documentation explaining the astronomical rationale.

#### [F23] ~~Collinearity check uses absolute threshold (RANSAC)~~ DONE
- **Location**: `registration/ransac/mod.rs:600-604`
- **Category**: Numerical stability
- **Impact**: 2/5 — Threshold `1.0` is coordinate-scale-dependent
- **Meaningfulness**: 2/5 — May fail for very small or very large coordinate ranges
- **Invasiveness**: 2/5 — Scale threshold by vector magnitudes
- **Description**: Cross product `> 1.0` for collinearity detection is an absolute
  threshold. For small coordinate ranges (sub-pixel), nearly all triangles would
  appear collinear. Scale by vector magnitudes for robustness.

#### [F24] ~~Inconsistent noise generation in testing module~~ DONE
- **Location**: `testing/synthetic/star_field.rs:397-402` vs
  `testing/synthetic/patterns.rs:120-136`
- **Category**: Duplication
- **Impact**: 2/5 — Same logic in two places
- **Meaningfulness**: 2/5 — Maintenance hazard
- **Invasiveness**: 2/5 — Reuse `patterns::add_noise()`
- **Description**: `star_field.rs` implements its own `add_gaussian_noise()` instead
  of reusing the identical function from `patterns.rs`.

#### [F25] ~~`TransformType::Auto` panics in `degrees_of_freedom()` and `Display`~~ DONE
- **Location**: `registration/transform.rs:55-56,121-122`
- **Category**: Robustness
- **Impact**: 2/5 — Panic in unexpected places if Auto leaks through
- **Meaningfulness**: 2/5 — Should be caught at construction time
- **Invasiveness**: 1/5 — Add validation in Transform constructors
- **Description**: If a `Transform` is somehow created with `Auto` type,
  `degrees_of_freedom()` and Display both panic. Add validation in constructors
  to prevent `Auto` from being stored.

#### [F26] ~~Redundant clone in SIP result creation (registration)~~ DONE
- **Location**: `registration/mod.rs:368-369`
- **Category**: Data flow / Performance
- **Impact**: 2/5 — Polynomial stored twice in memory
- **Meaningfulness**: 2/5 — Unnecessary allocation
- **Invasiveness**: 1/5 — Reconstruct via `as_ref()` in `warp_transform()`
- **Description**: SIP polynomial is cloned into `sip_correction` and also stored
  inside `sip_fit`. Store once and reconstruct the reference lazily.

---

### Priority 4 — Low Priority

#### [F27] `PixelSource` enum branching overhead in X-Trans inner loop
- **Location**: `raw/demosaic/xtrans/mod.rs:147-155`
- **Category**: Performance
- **Impact**: 2/5 — Branch per pixel in hot path
- **Meaningfulness**: 2/5 — May be offset by avoiding f32→u16→f32 roundtrip
- **Invasiveness**: 3/5 — Would require splitting into two functions
- **Description**: `read_normalized()` branches on `PixelSource::U16` vs `F32` for
  every pixel during demosaic. The branch predictor handles this well, but
  monomorphized functions would eliminate it entirely.

#### [F28] ~~Redundant condition in drizzle `finalize()`~~ DONE
- **Location**: `drizzle/mod.rs:642`
- **Category**: Simplification
- **Impact**: 2/5 — `w > 0.0` is implied by `w >= weight_threshold` when threshold >= 0
- **Meaningfulness**: 3/5 — Unnecessary branch
- **Invasiveness**: 1/5 — Remove `&& w > 0.0`
- **Description**: `if w >= weight_threshold && w > 0.0` — the second condition is
  redundant since `weight_threshold = min_coverage * max_weight >= 0.0`.

#### [F29] ~~Inconsistent parameter ordering in deblend functions~~ SKIPPED
- **Location**: `star_detection/deblend/local_maxima/mod.rs:53-59` vs
  `deblend/multi_threshold/mod.rs`
- **Category**: Consistency
- **Impact**: 1/5 — Minor API friction
- **Meaningfulness**: 2/5 — Confusing when switching between deblend modes
- **Invasiveness**: 1/5 — Reorder parameters
- **Description**: Deblend functions place threshold parameters in different
  positions. Standardize: `(data, pixels, labels, ...config...)`.

#### [F30] ProgressCallback documentation shows wrong callback signature
- **Location**: `stacking/stack.rs:107-109`
- **Category**: Documentation
- **Impact**: 2/5 — Doc example shows `|stage, current, total|` but type takes
  `StackingProgress` struct
- **Meaningfulness**: 3/5 — Confuses users trying to use the API
- **Invasiveness**: 1/5 — Update the doc example
- **Description**: Fix the example to use the correct signature.

#### [F31] ~~`warp_image` is `pub` but only called internally~~ DONE
- **Location**: `registration/interpolation/mod.rs:300-320`
- **Category**: API cleanliness
- **Impact**: 1/5 — Over-exposed internal function
- **Meaningfulness**: 2/5 — Clutters public API
- **Invasiveness**: 1/5 — Change to `pub(crate)`
- **Description**: The public API is `warp()`. The internal `warp_image()` should be
  `pub(crate)` to avoid exposing implementation details.

#### [F32] `sort_with_indices` panics on NaN (stacking rejection)
- **Location**: `stacking/rejection.rs:813`
- **Category**: Robustness
- **Impact**: 2/5 — NaN unlikely but possible after normalization edge cases
- **Meaningfulness**: 2/5 — Panic in production pipeline
- **Invasiveness**: 1/5 — Use `.unwrap_or(Ordering::Equal)` or pre-validate
- **Description**: `partial_cmp().unwrap()` panics if any value is NaN. While
  unlikely in practice, a normalization edge case (0/0) could produce NaN.
  Either handle gracefully or add a finite-values precondition.

---

## Cross-Cutting Patterns

### Error handling inconsistency
The codebase mixes three error patterns without clear rules:
- `assert!` / `panic!` for input validation (stacking weights, registration config)
- `Result<T, E>` for I/O and expected failures
- `debug_assert!` for invariants that disappear in release

**Recommendation**: Adopt a clear rule: public API validation returns `Result`.
Internal invariants use `assert!` (not `debug_assert!` for division-by-zero
cases). Add `# Panics` docs to all panicking public functions.

### `#[allow(dead_code)]` proliferation
Found across registration, star_detection, and math modules. Each should be
resolved: either the code is used (remove the allow) or it's dead (remove the
code). Keeping `#[allow(dead_code)]` on public items is an anti-pattern.

### Magic numerical constants
Multiple modules use bare floating-point constants for thresholds (`1e-10`,
`1e-6`, `1e-30`, `1.0`, `24`, `5.0`). Extract to named constants with doc
comments explaining the choice. This applies especially to drizzle (7 different
epsilons), star_detection (FWHM range), and RANSAC (collinearity threshold).

### Scratch buffer management
Several modules pass scratch buffers as function parameters (convolution,
drizzle, statistics). While this avoids allocation, it creates unwieldy APIs
with 8-10 parameters. Consider grouping into workspace structs that can be
reused across calls.

### Test helper duplication
Test utilities (`make_cfa`, `approx_eq`, noise generation) are reimplemented in
multiple modules. Consider expanding `testing/` with shared helpers and
importing them where needed.
