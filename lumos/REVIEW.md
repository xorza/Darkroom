# Lumos Code Review

## Executive summary

Lumos has strong module boundaries, unusually good numerical test coverage, and several deliberate
memory-conscious designs. The most urgent problems are concentrated at unsafe and scientific-data
boundaries rather than in the overall architecture:

- safe public warping can reach unchecked LUT reads with non-finite projective coordinates;
- parallel Bayer interpolation creates overlapping mutable slices;
- one AVX2 dispatch can execute FMA instructions without checking FMA support;
- FITS and calibrated RAW paths can silently change scientifically meaningful pixel values.

The public API also exposes several independently mutable fields whose relationships are invariants,
and a few hot paths allocate in proportion to image size, label count, or RANSAC iterations when
bounded or reusable storage is available. The batches below are ordered by implementation priority.

Scope: all production code under `lumos/src`, the crate manifest, production callers, and the
published surface reported by `cargo-public-api 0.52.0`. Tests and benchmarks were used only to
validate intent and coverage.

## Batch 1 — Critical: close safety holes and preserve scientific signal

- [ ] **Guard every warp sampler before unchecked LUT or gather access.** `warp` is a safe,
  infallible public function (`lumos/src/stacking/registration/resample.rs:50`), while a projective
  horizon deliberately maps to infinity (`lumos/src/math/dmat3.rs:130`) and the Lanczos row path
  converts that coordinate into an unchecked scalar LUT index or AVX2 gather
  (`lumos/src/stacking/registration/interpolation/warp/mod.rs:294`,
  `lumos/src/stacking/registration/interpolation/warp/mod.rs:307`,
  `lumos/src/stacking/registration/interpolation/warp/mod.rs:323`,
  `lumos/src/stacking/registration/interpolation/warp/sse.rs:427`). A valid homography whose horizon
  crosses the output can therefore violate the unsafe functions' in-bounds contracts. Treat every
  non-finite projected coordinate as border before floor/fraction/LUT evaluation, validate raw
  public transforms and `WarpParams`, and make invalid configuration fallible. Test every
  interpolation method with a crossing horizon and NaN coefficients; run the x86 path under ASan.

- [ ] **Remove overlapping mutable references from parallel RCD interpolation.** Each Rayon row
  constructs a full `&mut [f32]` over the same allocation
  (`lumos/src/io/raw/demosaic/bayer/rcd.rs:423`,
  `lumos/src/io/raw/demosaic/bayer/rcd.rs:439`,
  `lumos/src/io/raw/demosaic/bayer/rcd.rs:515`,
  `lumos/src/io/raw/demosaic/bayer/rcd.rs:525`). Disjoint writes do not make simultaneously
  overlapping mutable slices legal. Split true row-local mutable slices and use raw-pointer reads
  only for cross-row neighbors, or keep the entire operation behind the existing
  `concurrency::UnsafeSendPtr` without manufacturing aliased slices. Validate against a sequential
  reference bit-for-bit and exercise a reduced kernel with Miri plus the parallel kernel with
  ThreadSanitizer.

- [ ] **Require FMA before dispatching the FMA-compiled arcsinh kernel.** The implementation is
  compiled with `#[target_feature(enable = "avx2,fma")]`
  (`lumos/src/image_ops/stretching/simd_avx2.rs:19`,
  `lumos/src/image_ops/stretching/simd_avx2.rs:72`), but dispatch checks only AVX2
  (`lumos/src/image_ops/stretching/mod.rs:609`). Use the already available
  `cpu_features::has_avx2_fma()` and keep the scalar fallback for AVX2-only CPUs. Validate the
  AVX2=true/FMA=false feature combination with mocked detection or a VM that masks FMA.

- [ ] **Remove frame-dependent normalization of floating-point FITS data.** When `DATAMAX` is
  absent, `normalize_fits_pixels` divides a floating-point frame by its own maximum if that maximum
  exceeds 2 (`lumos/src/io/astro_image/fits.rs:161`,
  `lumos/src/io/astro_image/fits.rs:170`,
  `lumos/src/io/astro_image/fits.rs:188`). A cosmic ray, saturation event, or exposure difference
  therefore rescales the entire frame relative to its peers before calibration and stacking.
  Preserve floating-point physical values by default, or require an explicit, frame-independent
  normalization policy. Load two otherwise identical FITS frames with different isolated maxima
  and assert that their common pixels and stacking ratio remain identical.

- [ ] **Preserve signed and above-unity calibrated samples through demosaicing.** Calibration
  intentionally keeps the below-black noise tail (`lumos/src/io/raw/mod.rs:356`) and dark
  subtraction explicitly allows negatives (`lumos/src/io/astro_image/cfa.rs:178`), but the f32
  Bayer path clamps synthesized channels to `[0, 1]`
  (`lumos/src/io/raw/demosaic/bayer/rcd.rs:270`,
  `lumos/src/io/raw/demosaic/bayer/rcd.rs:493`) and X-Trans clamps them nonnegative
  (`lumos/src/io/raw/demosaic/xtrans/markesteijn_steps.rs:653`). Make clamping a decode policy:
  retain display-oriented clamping for direct RAW loading, but disable it for calibrated
  `CfaImage` demosaicing. Cross-check both demosaicers with analytical mosaics containing negative
  and greater-than-one samples and an end-to-end dark-subtraction neutrality test.

## Batch 2 — High: repair algorithmic correctness

- [ ] **Stop converting FITS nulls into valid zero-valued samples.** `physical_f32` maps FITS
  `BLANK` and non-finite nulls to NaN (`lumos/src/io/astro_image/fits.rs:48`), after which
  normalization silently rewrites them to zero (`lumos/src/io/astro_image/fits.rs:154`,
  `lumos/src/io/astro_image/fits.rs:176`). Those invented dark pixels participate in means,
  rejection, and stacking. Until `AstroImage` carries a validity plane, fail the load with a useful
  null summary; longer term, propagate validity into the existing coverage/weight pipeline. Verify
  exact small-N mean and median results when one contributor is null.

- [ ] **Remove the invalid multi-threshold deblend early exit.** The tree stops after four
  consecutive levels without a new node
  (`lumos/src/stacking/star_detection/deblend/multi_threshold/mod.rs:436`,
  `lumos/src/stacking/star_detection/deblend/multi_threshold/mod.rs:541`), even though blended peaks
  normally stay connected through low thresholds and split only after the threshold crosses their
  saddle. Process the configured ladder fully unless a mathematically terminal state is reached.
  Add a two-Gaussian fixture whose saddle is crossed after level four and compare it with a
  full-ladder reference.

- [ ] **Use the auto-estimated FWHM for final measurement.** Detection receives
  `fwhm_result.fwhm` (`lumos/src/stacking/star_detection/detector/mod.rs:145`,
  `lumos/src/stacking/star_detection/detector/mod.rs:157`), but measurement still receives the
  configured seed (`lumos/src/stacking/star_detection/detector/mod.rs:177`). That seed controls
  stamp radius and centroid weighting (`lumos/src/stacking/star_detection/centroid/mod.rs:84`), so
  automatic estimation currently affects candidate detection but not the precision stage. Pass the
  effective estimate with an explicit fallback and validate narrow and broad synthetic PSFs far
  from the configured seed.

- [ ] **Use a stable least-squares solve for background extraction and report rank failure.**
  `solve_ls` forms `AᵀA`, squares the condition number, applies LU, and silently replaces failure
  with zero coefficients (`lumos/src/image_ops/background_extraction/mod.rs:230`). Subtract can then
  falsely succeed as a no-op, while Divide silently returns when the zero surface has non-positive
  mean (`lumos/src/image_ops/background_extraction/mod.rs:141`,
  `lumos/src/image_ops/background_extraction/mod.rs:149`). Solve `A c ≈ z` directly with
  pivoted QR or SVD and propagate an `OpError`, optionally lowering the degree explicitly when rank
  is insufficient. Validate constant surfaces, extreme aspect ratios, nearly collinear grids, and
  rank-deficient samples.

- [ ] **Accept the final RANSAC refit only when it preserves the robust solution.** Hypotheses and
  local optimization are checked for plausibility and score improvement
  (`lumos/src/stacking/registration/ransac/mod.rs:343`,
  `lumos/src/stacking/registration/ransac/mod.rs:373`), but the final all-inlier least-squares refit
  is returned unconditionally and its rescore is discarded
  (`lumos/src/stacking/registration/ransac/mod.rs:403`,
  `lumos/src/stacking/registration/ransac/mod.rs:421`). Retain the saved model unless the refit is
  finite, valid, physically plausible, sufficiently supported, and non-worse by the same scorer.
  Test a refit pulled outside configured rotation/scale bounds and one whose inlier set shrinks.

## Batch 3 — High: make public invariants enforceable

- [ ] **Make transform model and matrix representation a single invariant.** `Transform.matrix` and
  `transform_type` are independently public (`lumos/src/stacking/registration/transform.rs:70`),
  and `from_matrix` publicly accepts arbitrary pairs while exposing the otherwise private `DMat3`
  type (`lumos/src/stacking/registration/transform.rs:207`); `cargo-public-api` confirms that private
  type leaks into the published signature. `WarpTransform::is_linear` then trusts only the tag
  (`lumos/src/stacking/registration/transform.rs:361`), so a projective matrix tagged Affine takes
  incorrect constant-step warping. In addition, `is_valid` checks only the upper-left 2×2
  determinant even for homographies (`lumos/src/stacking/registration/transform.rs:303`). Make raw
  fields and construction internal, expose model-specific constructors, validate affine bottom
  rows, and use the full 3×3 determinant for homographies.

- [ ] **Remove invalid states from `ImageDimensions`.** The public type derives `Default`, yielding
  `0×0×0`, and exposes fields that can be changed to zero sizes or unsupported channel counts
  (`lumos/src/io/astro_image/mod.rs:70`), while `new` asserts positive `{1, 3}` dimensions
  (`lumos/src/io/astro_image/mod.rs:78`). Public image constructors trust the value when sizing and
  selecting L versus RGB storage (`lumos/src/io/astro_image/mod.rs:278`,
  `lumos/src/io/astro_image/mod.rs:309`). Remove the invalid default and use an
  invariant-preserving representation, preferably a channel-layout enum and checked dimensions;
  cover multiplication overflow as well as zero/unsupported shapes.

- [ ] **Enforce CFA-pattern coherence during calibration.** `CfaImage` exposes its plane and
  metadata publicly (`lumos/src/io/astro_image/cfa.rs:52`), so a missing or inconsistent pattern is
  constructible. Flat division then prefers the flat's pattern without checking that it matches the
  light (`lumos/src/io/astro_image/cfa.rs:207`,
  `lumos/src/io/astro_image/cfa.rs:228`), potentially applying R/G/B normalization factors to the
  wrong photosites. Add invariant-enforcing construction and reject mismatched or missing patterns
  before mutation. Validate every Bayer pairing, differing X-Trans matrices, and Mono/color
  mismatches.

- [ ] **Validate all registration inputs and limits as finite at the public boundary.** `register`
  checks counts but not star positions or FWHM (`lumos/src/stacking/registration/mod.rs:104`), so a
  NaN position reaches `partial_cmp(...).unwrap()` during triangle construction
  (`lumos/src/stacking/registration/triangle/geometry.rs:37`). `max_rms_error` and
  `max_rotation` also accept NaN because their validators only compare with zero
  (`lumos/src/stacking/registration/config.rs:354`,
  `lumos/src/stacking/registration/ransac/mod.rs:128`), and SIP does not validate its optional
  reference point (`lumos/src/stacking/registration/distortion/sip/mod.rs:79`). Return typed
  validation errors rather than panicking or silently disabling quality gates.

- [ ] **Reject or define zero `pixfrac` before drizzle arithmetic.** `DrizzleConfig::validate`
  accepts the documented inclusive range `0..=1`
  (`lumos/src/stacking/drizzle/config.rs:103`), but Turbo divides by
  `drop_size²` (`lumos/src/stacking/drizzle/accumulator.rs:264`) and Gaussian derives an inverse
  variance from zero sigma (`lumos/src/stacking/drizzle/accumulator.rs:229`). Require
  `0 < pixfrac <= 1`, or explicitly map zero to Point semantics without entering those kernels.
  Add exact boundary tests for every drizzle kernel and assert finite output, coverage, weight, and
  variance planes.

## Batch 4 — Medium: reduce peak allocations and repeated work

- [ ] **Replace dense per-worker component accumulators with sparse touched-label state.**
  Component extraction allocates one `num_labels` result vector plus another `num_labels` vector
  for every Rayon worker (`lumos/src/stacking/star_detection/detector/stages/detect.rs:275`,
  `lumos/src/stacking/star_detection/detector/stages/detect.rs:289`,
  `lumos/src/stacking/star_detection/detector/stages/detect.rs:298`). On salt noise, labels can
  approach pixel count, making peak memory `O(labels × threads)`. Accumulate only labels touched by
  each row chunk, or derive metadata while labels are finalized. Benchmark peak RSS and time on
  salt noise and representative sparse/crowded fields.

- [ ] **Make background-mesh scratch genuinely bounded and reusable.** The unmasked stride computes
  `sqrt(floor(tile_pixels / 1024))`, so tiles just above 1024 samples exceed the stated cap
  (`lumos/src/background_mesh/tile_stats.rs:102`,
  `lumos/src/background_mesh/tile_stats.rs:113`); the masked path first collects every unmasked
  pixel and only then subsamples (`lumos/src/background_mesh/tile_stats.rs:36`), while Rayon workers
  reserve two full `tile_size²` vectors (`lumos/src/background_mesh/mod.rs:159`). Compute the stride
  from the non-truncated ratio, sample masked tiles directly, and size reusable worker scratch to
  the actual cap. Also retain median-filter and spline scratch currently reallocated by each
  `compute` (`lumos/src/background_mesh/mod.rs:206`,
  `lumos/src/background_mesh/mod.rs:253`), which iterative refinement calls repeatedly
  (`lumos/src/stacking/star_detection/background/mod.rs:71`). Assert the sample bound and benchmark
  repeated 256-pixel-tile refinement with allocation counts.

- [ ] **Remove per-hypothesis heap allocation from minimal affine/homography RANSAC.** Every estimate
  creates normalized point vectors (`lumos/src/stacking/registration/ransac/transforms.rs:171`,
  `lumos/src/stacking/registration/ransac/transforms.rs:270`), and homography also allocates a design
  vector and dynamic matrix (`lumos/src/stacking/registration/ransac/transforms.rs:281`). Minimal
  samples have fixed sizes, so use stack-backed arrays/`SMatrix` or estimator-owned scratch for the
  hypothesis loop, retaining dynamic least squares only for all-inlier refits. Measure allocations
  per 2,000 hypotheses and end-to-end registration latency for each model.

- [ ] **Eliminate full-frame staging allocations during RAW decode.** Both full decode and the
  dimensions probe copy the complete file with `fs::read` before `libraw_open_buffer`
  (`lumos/src/io/raw/mod.rs:668`, `lumos/src/io/raw/mod.rs:881`); calibration decode then normalizes
  the entire sensor into f32 before allocating and copying the active area
  (`lumos/src/io/raw/mod.rs:364`, `lumos/src/io/raw/mod.rs:414`). Use `libraw_open_file` where path
  handling is sound, otherwise retain a `memmap2` mapping, and add a row kernel that normalizes and
  applies channel corrections directly into the active-area destination. Verify bit-exact output
  and measure anonymous peak RSS plus `raw_dimensions` latency on a representative large RAW.

- [ ] **Prepare a normalized master flat once per calibration set.** Mono and per-color means are
  recomputed over the full flat for every light (`lumos/src/io/astro_image/cfa.rs:252`,
  `lumos/src/io/astro_image/cfa.rs:295`,
  `lumos/src/io/astro_image/cfa.rs:329`), even though `CalibrationMasters::calibrate` reuses the same
  flat/bias pair (`lumos/src/stacking/calibration_masters/mod.rs:354`). Build an internal prepared
  flat after master construction and defect detection so each light performs only the division
  pass. Validate bit-identical calibration and benchmark a 30-light set for total time and peak
  memory.

## Batch 5 — Low-risk simplification and API cleanup

- [ ] **Build each matched-filter kernel once.** `matched_filter` constructs a Gaussian kernel only
  to compute its norm (`lumos/src/stacking/star_detection/convolution/mod.rs:95`), then calls a
  convolution function that constructs the same kernel again
  (`lumos/src/stacking/star_detection/convolution/mod.rs:108`,
  `lumos/src/stacking/star_detection/convolution/mod.rs:139`). Pass the prepared circular or
  elliptical kernel into the backend and derive normalization from that same value. Benchmark both
  shapes and keep a scalar-output equivalence test.

- [ ] **Collapse `RegistrationResult`'s parallel mutable state.** `matched_stars`, `residuals`,
  `rms_error`, `max_error`, and `num_inliers` are independently public
  (`lumos/src/stacking/registration/result.rs:139`), and public `new` accepts mismatched match and
  residual lengths (`lumos/src/stacking/registration/result.rs:172`). Store one coherent
  match-with-residual collection, derive diagnostics when constructing internally, and expose only
  state that callers can mutate without breaking relationships.

- [ ] **Delete the unintegrated TPS subsystem unless integration is scheduled now.** TPS is
  explicitly blanket-allowed as dead production code
  (`lumos/src/stacking/registration/distortion/tps/mod.rs:1`) but is still compiled through
  `lumos/src/stacking/registration/distortion/mod.rs:30`. Removing the unused 459-line
  implementation and its separate 1,048-line test suite eliminates an alternate solver and
  distortion model that no production path can select.

- [ ] **Clean up direct dependencies.** `serde_json` is declared as a normal dependency but has no
  Lumos source use (`lumos/Cargo.toml:30`). `tracing-subscriber` is also normal
  (`lumos/Cargo.toml:31`) but is referenced only by the test-support module and example
  (`lumos/src/testing/mod.rs:167`, `lumos/examples/full_pipeline.rs:23`). Remove `serde_json` and
  move `tracing-subscriber` to dev-dependencies, then confirm the simplified normal dependency tree
  with `cargo tree -p lumos --edges normal --depth 1`.

- [ ] **Short-circuit exact no-op image operations after validation.** Denoise with zero strength
  still allocates image-sized scratch (`lumos/src/image_ops/denoise/mod.rs:122`), while zero-amount
  HDR and zero-strength LocalContrast still build and remap intensity planes
  (`lumos/src/image_ops/hdr/mod.rs:62`,
  `lumos/src/image_ops/local_contrast/mod.rs:72`). Return immediately after format and configuration
  validation; assert byte-identical images and benchmark allocation count on a representative
  master.

## Open questions

- [ ] **Define the unit contract for `NoiseModel`.** It documents gain as electrons per ADU
  (`lumos/src/stacking/star_detection/config.rs:75`), but the detector operates on normalized pixels
  and directly evaluates `signal / gain` and `read_noise² / gain²`
  (`lumos/src/stacking/star_detection/centroid/mod.rs:197`). Either carry the ADU/full-scale
  conversion through decode metadata, or rename the field and document electrons per normalized
  unit. The decision should be locked by hand-computed SNR and inverse-variance tests.

- [ ] **Choose whether calibrated RAW output is sensor-linear or camera-white-balanced.** Direct
  RAW loading applies camera white balance before demosaic (`lumos/src/io/raw/mod.rs:439`), while
  calibration loading intentionally omits it and `CfaImage::demosaic` has no stored WB coefficients
  (`lumos/src/io/raw/mod.rs:932`, `lumos/src/io/astro_image/cfa.rs:125`). If both public workflows
  should produce comparable color, WB metadata must survive CFA loading and be applied at one
  defined post-calibration stage.

- [ ] **Define coverage for negative-lobe interpolation kernels.** Bicubic/Lanczos coverage is a
  signed in-bounds kernel-weight fraction clamped to `[0, 1]`
  (`lumos/src/stacking/registration/interpolation/mod.rs:413`). Decide whether downstream stacking
  needs signed weight fraction, absolute support, or geometric coverage, then validate monotonic
  border behavior against that definition.
