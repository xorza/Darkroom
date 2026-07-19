# Lumos Code Review

Updated 2026-07-19 against the current Lumos source.

## Current status

The review now tracks implementation status instead of preserving the original snapshot:

- 24 findings completed;
- 2 concrete findings open;
- 2 proposals deferred until there is a product decision or measured failure.

The remaining concrete work is API policy and state-coherence decisions.

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

- [x] **Process the full multi-threshold deblend ladder.**
  `stacking::star_detection::deblend::multi_threshold` no longer treats four levels without a
  split as terminal because a later threshold can still cross a connected pair's saddle. A
  two-Gaussian regression stays connected through that old exit window and separates at the
  expected late ladder level.

- [x] **Use the effective FWHM for final star measurement.**
  The same manual, estimated, or fallback FWHM now drives matched-filter detection, measurement
  stamp radius, and centroid weighting. Narrow and broad synthetic PSFs cross-check auto mode
  against a fixed run using the returned estimate.

- [x] **Fit background surfaces with rank-checked SVD.**
  `image_ops::background_extraction` solves the original design matrix instead of normal equations
  and LU. Rank-deficient sample geometry returns `OpError::RankDeficient` without rewriting the
  failed channel instead of silently substituting a zero surface.

- [x] **Accept the final RANSAC refit only when it preserves the robust solution.**
  The all-inlier least-squares candidate is now returned only when it remains finite, valid,
  physically plausible, sufficiently supported, and non-worse under the same MAGSAC scorer.
  Otherwise RANSAC retains the saved robust model and its complete inlier set. A deterministic
  edge-inlier regression covers the concrete case where a translation refit shifts by 0.6 pixels
  and degrades the robust score.

- [x] **Enforce CFA-pattern coherence during calibration.**
  `CalibrationMasters::calibrate` validates the light and every stored master before changing
  pixels or the calibrated flag. Missing metadata and mismatched Mono, Bayer, Bayer-phase, and
  X-Trans patterns return typed `CalibrationError` variants; the lower-level flat-division helper
  is no longer public.

- [x] **Validate registration inputs and limits as finite at the public boundary.**
  `register` rejects non-finite positions and FWHM values with catalog-and-index-aware error
  variants before triangle construction. Configuration validation now rejects non-finite RMS and
  rotation limits, scale-range bounds, and SIP reference points.

- [x] **Reject zero `pixfrac` before drizzle arithmetic.**
  `DrizzleConfig::validate` now requires `0 < pixfrac <= 1`, so every kernel rejects zero before
  output allocation or drop arithmetic. Tests cover all five kernels and assert finite image,
  coverage, weight, and variance planes at the valid upper boundary.

Completed follow-up work not present in the original review: `DetectorPool` threads reusable
detectors through parallel frame processing without thread-local state. On the 16 × 1 MP,
8-thread benchmark, reuse reduced the median from 50.06 ms to 41.91 ms.

## Batch 1 — Enforce public invariants

- [x] **Make transform model and matrix representation a single invariant.**
  The matrix and concrete model are now private, public construction is model-specific, and raw
  matrix construction is internal and rejects projective bottom rows for affine-or-simpler models.
  Public callers receive immutable row-major coefficients without access to `DMat3`, and
  homography validity uses the full 3 × 3 determinant.

- [x] **Remove invalid states from `ImageDimensions`.**
  Size and channel count are private, `Default` is gone, and construction validates non-zero
  dimensions, supported channel counts, and checked pixel/sample products. Immutable accessors
  expose the validated dimensions.

## Batch 2 — Remaining repeated work and allocations

- [x] **Bound masked background-mesh sampling.**
  At the supported 256 × 256 tile maximum, a sparse mask grew 32 retained value scratches from
  77,824 bytes to 8,388,608 bytes and added 9,707,520 bytes of RSS. The masked path now counts
  unmasked bits first and directly reads the same evenly spaced ordinals selected by the old
  collect-then-subsample path. The same measurement retained 131,072 bytes after the masked pass
  with a 172,032-byte RSS delta, while exact-output and per-job capacity tests lock the bound. The
  existing 6K sparse-mask benchmark improved from a 168.59 ms median to 8.30 ms.

- [x] **Prepare a normalized master flat once per calibration set.**
  Cold defects are detected from the raw flat, then `CalibrationMasters` consumes it into a
  flat-dark/bias-subtracted, per-colour normalized, clamped divisor. The versioned bundle cache
  persists that prepared representation, so cache hits repeat neither preparation nor defect
  detection. Hand-computed Mono/Bayer/X-Trans tests and a cache round-trip assert bit-exact
  calibration. The 3 MP Bayer apply benchmark improved from a 3.20 ms median to 2.33 ms
  (27.3% faster); the representative 30-light benchmark completes in 72.33 ms.

## Batch 3 — API decisions with concrete inconsistencies

- [x] **Collapse `RegistrationResult`'s parallel mutable state.**
  `StarMatch` now carries its final residual, while `RegistrationResult` keeps transform, SIP,
  matches, and elapsed time private. Construction is internal, public views are immutable, and RMS,
  maximum error, inlier count, and quality are derived from the single match collection.

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
