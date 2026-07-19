# Lumos Code Review

Updated 2026-07-19 against the current Lumos source.

## Current status

The review now tracks implementation status instead of preserving the original snapshot:

- 12 findings completed;
- 13 concrete findings open;
- 1 allocation finding partially resolved;
- 2 proposals deferred until there is a product decision or measured failure.

The remaining highest-priority work is algorithmic correctness in deblending, FWHM propagation,
background fitting, and final RANSAC refits. Lower-priority work is mostly invariant enforcement
and repeated whole-frame work.

Scope: production code under `lumos/src`, `lumos/Cargo.toml`, production callers, and the published
surface. Paths and symbol names are used instead of brittle line numbers.

## Completed

- [x] **Guard warp samplers against non-finite projective coordinates.**
  `stacking::registration::interpolation::warp` now classifies non-finite source positions as
  border before floor, LUT indexing, or SIMD gather. Horizon and NaN-coefficient tests cover every
  interpolation method. Remaining transform-construction concerns are tracked separately under
  public invariants.

- [x] **Require FMA before dispatching the FMA-compiled arcsinh kernel.**
  `image_ops::stretching` dispatches through `cpu_features::has_avx2_fma()` and retains the scalar
  fallback for AVX2-only CPUs.

- [x] **Bound component-extraction scratch without replacing it with slower sparse maps.**
  `stacking::star_detection::detector::stages::detect` uses direct dense indexing with a concurrency
  cap that bounds scratch to one additional label plane. If that plane exceeds the budget, it
  scans directly into the final accumulator. Representative, crowded, and low-threshold benches
  showed this was faster than the proposed sparse touched-label implementation.

- [x] **Remove per-hypothesis heap allocation from minimal affine and homography RANSAC.**
  Minimal estimators use lazy normalization and fixed-size matrices; dynamic storage remains only
  for non-minimal all-inlier refits.

- [x] **Eliminate full-frame RAW file and active-area staging allocations.**
  Unix decode uses `libraw_open_file`, the dimensions probe no longer reads the full file, and
  calibration normalization writes the active area directly into its destination. The non-Unix
  buffer fallback remains intentional.

- [x] **Build each matched-filter kernel once.**
  `stacking::star_detection::convolution` now builds the selected circular or elliptical kernel
  once, uses it for convolution, and returns its noise normalization from the same operation. The
  1 MP benchmark improved from 489 to 471 µs for the circular path and from 2.16 to 1.89 ms for the
  elliptical path.

- [x] **Clean up direct dependencies.**
  Unused `serde_json` was removed and test/example-only `tracing-subscriber` moved to
  dev-dependencies.

- [x] **Short-circuit exact no-op image operations after validation.**
  Zero-strength denoise/local contrast and zero-amount HDR return after configuration and format
  validation without allocating image-sized scratch or intensity planes. Identity tests assert
  bit-exact output.

- [x] **Remove overlapping mutable references from parallel RCD interpolation.**
  `io::raw::demosaic::bayer::rcd` now keeps cross-row channel access behind
  `concurrency::UnsafeSendPtr` without constructing full-allocation mutable slices in Rayon jobs.
  The access phases read only completed R/B sites and write disjoint destination sites; a
  single-thread/four-thread cross-check asserts bit-identical output, and the reduced case passes
  Miri under tree borrows.

- [x] **Preserve physical floating-point FITS values.**
  `io::astro_image::fits` no longer derives a scale from a frame maximum and ignores `DATAMAX` for
  float sample types because it is descriptive metadata, not a guaranteed sensor full-scale.
  Synthetic FITS round-trips cover negative and above-unity values.

- [x] **Preserve signed and above-unity calibrated samples through demosaicing.**
  Bayer and X-Trans demosaicing now use an explicit output-range policy: direct Bayer decode keeps
  its `[0, 1]` clamp, direct X-Trans keeps its nonnegative floor, and calibrated `CfaImage`
  demosaicing preserves its linear range. End-to-end tests cover negative and above-unity mosaics
  through both algorithms.

- [x] **Reject FITS nulls instead of inventing zero-valued samples.**
  Integer `BLANK` and floating non-finite samples are rejected with their total count and first
  linear index until the image model carries a validity plane.

Completed follow-up work not present in the original review: `DetectorPool` threads reusable
detectors through parallel frame processing without thread-local state. On the 16 × 1 MP,
8-thread benchmark, reuse reduced the median from 50.06 ms to 41.91 ms.

## Batch 1 — Algorithmic correctness

- [ ] **Remove the invalid multi-threshold deblend early exit.**
  `stacking::star_detection::deblend::multi_threshold` stops after four levels without a split,
  although connected peaks can split only after a later threshold crosses their saddle. Process
  the configured ladder fully unless a mathematically terminal state is reached, and compare a
  late-splitting two-Gaussian fixture with a full-ladder reference.

- [ ] **Use the auto-estimated FWHM for final measurement.**
  Detection receives the effective estimate, but measurement still receives
  `config.fwhm.expected`. That seed controls stamp radius and centroid weighting. Pass the
  estimated value with its explicit fallback and validate narrow and broad synthetic PSFs far from
  the configured seed.

- [ ] **Use a stable least-squares solve for background extraction and report rank failure.**
  `image_ops::background_extraction::solve_ls` forms normal equations, solves with LU, and silently
  substitutes zero coefficients on failure. Solve the original system with pivoted QR or SVD and
  propagate `OpError`; do not let subtract/divide silently succeed as a no-op.

- [ ] **Accept the final RANSAC refit only when it preserves the robust solution.**
  Hypotheses and local optimization are checked for plausibility and score improvement, but the
  final all-inlier least-squares refit is returned unconditionally and its rescore is discarded.
  Retain the saved model unless the refit is finite, valid, physically plausible, sufficiently
  supported, and non-worse under the same scorer.

## Batch 2 — Enforce public invariants

- [ ] **Make transform model and matrix representation a single invariant.**
  `Transform.matrix` and `transform_type` remain independently public, and `from_matrix` accepts
  arbitrary pairs while exposing the internal `DMat3`. `WarpTransform::is_linear` trusts the tag,
  and homography validity still uses only the upper-left 2 × 2 determinant. Make raw construction
  internal, expose model-specific constructors, validate affine bottom rows, and use the full
  determinant for homographies.

- [ ] **Remove invalid states from `ImageDimensions`.**
  The public type derives `Default` and exposes mutable size/channel fields, so zero dimensions,
  unsupported channel counts, and unchecked sample-count overflow remain constructible. Replace
  this with an invariant-preserving representation and checked dimension arithmetic.

- [ ] **Enforce CFA-pattern coherence during calibration.**
  `CfaImage` exposes its plane and metadata, and flat division can select the flat pattern without
  verifying that it matches the light. Reject missing or mismatched Mono, Bayer, and X-Trans
  patterns before mutation.

- [ ] **Validate registration inputs and limits as finite at the public boundary.**
  `register` does not validate star positions or FWHM before triangle construction. NaN
  `max_rms_error`, `max_rotation`, and SIP reference points can also bypass their current
  comparisons. Return typed validation errors before sorting or fitting.

- [ ] **Reject or define zero `pixfrac` before drizzle arithmetic.**
  `DrizzleConfig::validate` accepts zero although Turbo divides by `drop_size²` and Gaussian derives
  inverse variance from zero sigma. Require `0 < pixfrac <= 1`, or map zero explicitly to Point
  semantics before entering those kernels. Test every kernel at the boundary and assert finite
  image, coverage, weight, and variance planes.

## Batch 3 — Remaining repeated work and allocations

- [ ] **Partially resolved — bound masked background-mesh sampling.**
  `MeshWorkspace` now retains tile, median-filter, spline, and per-job scratch, and the unmasked
  stride keeps at most `MAX_TILE_SAMPLES`. The masked branch still collects every unmasked pixel
  before subsampling to 1,024, so its vector can grow to and retain a full tile. Measure masked
  refinement allocation/RSS first; replace it with deterministic direct sampling only if the
  retained capacity is material.

- [ ] **Prepare a normalized master flat once per calibration set.**
  Mono and per-colour means are recomputed over the complete flat for every light even though
  `CalibrationMasters` reuses the same flat/bias pair. Build an internal prepared flat after master
  construction and defect detection so each light performs only division. Validate bit-identical
  calibration and benchmark a representative 30-light set.

## Batch 4 — API decisions with concrete inconsistencies

- [ ] **Collapse `RegistrationResult`'s parallel mutable state.**
  Matches, residuals, RMS, maximum error, and inlier count remain independently public, and `new`
  accepts mismatched match/residual lengths. Store coherent match-with-residual records, derive
  diagnostics internally, and expose only state callers can mutate independently.

- [ ] **Define the unit contract for `NoiseModel`.**
  The type documents electrons per ADU, while detection operates on normalized pixels and directly
  evaluates `signal / gain` and `read_noise² / gain²`. Carry the ADU/full-scale conversion through
  metadata or rename the field to normalized-signal units, then lock the decision with
  hand-computed SNR and inverse-variance tests.

- [ ] **Choose one white-balance policy for calibrated and direct RAW output.**
  Direct RAW loading applies camera white balance before demosaic. Calibration loading omits it,
  and `CfaImage::demosaic` does not retain the coefficients. If the workflows should produce
  comparable colour, preserve WB metadata and apply it at one defined post-calibration stage.

## Deferred pending evidence or product direction

- **TPS removal.** The unused subsystem is compiled but cannot be selected by production
  registration. Deleting it is a maintenance choice, not a demonstrated correctness or runtime
  defect. Remove it only after deciding that integration is not planned.

- **Negative-lobe coverage semantics.** Bicubic/Lanczos currently define coverage as signed
  in-bounds kernel weight clamped to `[0, 1]`; tests cover finite bounds, interior/exterior values,
  and flat-field edge renormalization. Do not replace this with absolute or geometric support until
  downstream stacking requirements identify the intended scientific meaning or a failing border
  case.
