# Stacking and drizzle specification versus implementation review

Date: 2026-07-21

Scope: production code implementing [`05-stacking-drizzle.md`](05-stacking-drizzle.md),
including statistical combine configuration, normalization, rejection, resident and
memory-mapped gather/reduction, quality products, drizzle frame and accumulator
contracts, every drizzle kernel, output geometry, registration/pipeline handoffs,
metadata, cancellation, and public exports. Test-only, benchmark, fixture, and
real-data-test code was not reviewed.

## Outcome

The document is a strong target specification and its §10 source audit is accurate:
all fourteen statistical-stack gaps in §10.2 and all seventeen drizzle gaps in §10.4
are present in production. The current implementation also has worthwhile
foundations: coherent image/support/confidence records for the ordinary registered
path, robust paired normalization over a common domain, correct normalization-gain
scaling in the frame noise proxy, value/index preservation through rejection,
compensated weighted means, a sound GESD implementation, one shared RAM/mmap combine
engine, exact square-drop polygon overlap, and coefficient-square quality factors.

Those foundations do not yet make Stage 5 scientifically complete. The largest risks
are concentrated at interfaces:

1. Drizzle is disconnected from detection/registration/normalization and accepts the
   opposite transform direction through the same undirected `Transform` type.
2. The drizzle grid maps centers as `scale * coordinate` into dimensions derived from
   scaled sizes, producing the exact half-output-pixel boundary error warned about in
   §4.2. It also loses all output metadata.
3. Every non-Square drizzle kernel divides an already normalized coefficient by a
   second Jacobian. Signed Lanczos is then globally thresholded as if its signed
   statistical denominator were geometric coverage and its negative science result
   is clamped to zero.
4. Neither reconstruction path accepts calibrated per-pixel variance or channel-shaped
   masks. Statistical rejection therefore uses raw heteroscedastic values, while both
   reported “variance” products are coefficient factors rather than science variance.
5. Statistical normalization can silently replace a degenerate Deming fit with gain
   `1.0`; sigma clipping deliberately keeps an isolated outlier when the surviving MAD
   is zero.
6. Drizzle accumulates all output planes in resident `f32`, has no cancellation token
   or output-memory budget, and exposes a public accumulator that can finalize a valid-
   looking zero-frame product.

The implementation has also expanded sideways before closing its central contract.
Five drizzle kernels and a public incremental accumulator exist while the reference
Square path lacks a correct grid, mapping type, variance/mask inputs, source-plane
rejection, metadata, and scalable storage. The simplest route is to establish one
end-to-end Square reference path first, then restore optional kernels only against its
invariants.

## Current production flow

```text
ordinary registered stack
  calibrated LinearImage
    -> detect/register
    -> inverse-warp once into source-sized reference grid
    -> StackFrame { warped image, coverage, confidence, pre-warp MAD stats }
    -> require coverage common to every frame for registered normalization
    -> choose lowest-average-MAD reference
    -> paired Deming gain + median offset (silent identity fallback)
    -> one global frame weight from average RGB background sigma
    -> resolve rejection method once from global frame count
    -> gather raw normalized values + frame_weight*confidence per pixel/channel
    -> raw-value rejection
    -> mean/median + sum(weight) + sum(weight^2)/sum(weight)^2
    -> coverage = fraction of frames geometrically supported before rejection
    -> StackProduct using first/reference metadata

standalone drizzle
  DrizzleFrame { source, untyped Transform, scalar frame weight, shared pixel weight }
    -> allocate ceil(input_width*scale) x ceil(input_height*scale) resident f32 planes
    -> map source center as scale*Transform(source_center)
    -> deposit Square/Turbo/Point/Gaussian/Lanczos coefficients
    -> accumulate shared signed/statistical weight and coefficient squares
    -> coverage = signed weight / global maximum
    -> fill below min_coverage*global_max; clamp Lanczos science to >= 0
    -> construct LinearImage with default metadata
```

`align_and_stack` always follows the first flow. No production pipeline constructs a
`DrizzleFrame`; the drizzle entry points are an independent public island.

## Contract coverage

| Specification area | Production status | Main evidence |
|---|---|---|
| Coherent image/variance/masks/maps/normalization/provenance record | Partial | ordinary frames carry image/support/confidence; drizzle carries image/transform/two scalar weights |
| Direction-specific complete mappings | Incorrect | one `Transform` means reference→source in registration and source→common in drizzle |
| Boundary-derived output grid and WCS | Incorrect | fixed scaled input size and center multiplication; default output metadata |
| Preflight transform/sample/unit compatibility | Partial | dimensions and weights checked; transform, pixels, filter, and units are not |
| Correct Square overlap coefficient | Implemented for linear/projective maps | overlap divided by mapped drop area once |
| Correct non-Square coefficient semantics | Incorrect | normalized coefficients receive a second Jacobian division |
| Independent coverage, validity, statistical weight, and coefficient maps | Missing | drizzle derives coverage from accumulated statistical weight |
| Input variance and general propagated output variance | Missing | only unit-variance coefficient factors are emitted |
| Per-channel DQ/user/artifact masks | Missing | one shared optional nonnegative pixel-weight plane |
| Drizzle median/blot/derivative rejection | Missing | one final deposition pass only |
| Flux versus surface-brightness units | Missing | no unit mode or `s²`/`s⁴` conversion |
| Registered paired normalization | Partial | good common-domain Deming fit, but all-frame overlap and silent fallback |
| Heteroscedastic rejection | Missing | rejection accepts values only |
| Exact documented rejection variants | Incorrect | zero-MAD, Winsorized, LinearFit, and Percentile semantics drift |
| Local small-N policy | Missing | one method is selected from global frame count |
| Survivor/rejection/`N_eff`/variance diagnostics | Missing | coverage, weight, and coefficient factor only |
| Derived metadata and provenance | Missing | first/reference metadata copied; drizzle resets metadata |
| Drizzle tiling, cancellation, and stable precision | Missing | resident `f32` output planes and no cancel token |

## Batch 1 — establish one coherent reconstruction contract

- [ ] **P0 — Integrate drizzle as an alternative Stage 5 sink for calibrated source-grid frames instead of a standalone API after the registered-stack pipeline.**

  **Contract.** Drizzle must consume original calibrated samples exactly once, paired
  with normalization, masks, complete forward geometry, registration diagnostics, and
  provenance (`05-stacking-drizzle.md:38` and `05-stacking-drizzle.md:103`).

  **Evidence.** `AlignStackConfig` exposes only a statistical `stack` configuration at
  `../pipeline/config.rs:20`. The pipeline always warps successful registrations at
  `../pipeline/align.rs:130` and sends those resampled frames to `stack_images` at
  `../pipeline/align.rs:175`. Drizzle has separate path/in-memory entry points at
  `../drizzle/stack.rs:53` and `../drizzle/stack.rs:100`; no production caller bridges
  registration results into them.

  **Impact.** The normal end-to-end workflow cannot select drizzle without external
  orchestration. A caller is encouraged either to lose the pipeline's normalization,
  rejection, cancellation, and registration context or to feed already warped pixels
  into drizzle and resample twice.

  **Change.** Add a reconstruction choice to the alignment pipeline and retain each
  calibrated source image until it can be consumed by either ordinary inverse warping
  or direct drizzle deposition. Build a private `ReconstructionFrame` containing the
  source image/storage handle, normalization, masks/variance, direction-specific maps,
  source statistics, registration solution, and input identity. Do not route drizzle
  through `StackFrame::registered`.

  **Validation.** Run the same registered dataset through both reconstruction choices.
  Instrument source reads and mapping calls to prove drizzle consumes calibrated source
  samples once, preserves input order and diagnostics, and never calls the ordinary
  warp. Verify RAM and streaming pipeline outcomes agree.

- [ ] **P0 — Replace the shared `Transform` field with direction-specific complete forward/inverse mapping types, including SIP.**

  **Contract.** Registration's inverse warp is common/reference→source, while drizzle
  requires source→common; direction must be encoded in types and nonlinear distortion
  must not be dropped (`05-stacking-drizzle.md:71`).

  **Evidence.** `Transform::apply` is explicitly reference→target at
  `../registration/transform.rs:243`. `DrizzleFrame` stores that same type but documents
  it as input→common at `../drizzle/accumulator.rs:22`. Registration bundles SIP only in
  the inverse-direction `WarpTransform` at `../registration/transform.rs:347`, and
  `RegistrationResult::warp_transform` exposes that bundle at
  `../registration/result/mod.rs:279`; drizzle accepts only the base `Transform`.

  **Impact.** Passing a registration result's base transform directly produces a
  reversed drizzle mapping. Inverting only the matrix fixes direction for linear models
  but silently loses SIP, so wide-field inputs can be deposited with a spatially varying
  astrometric error.

  **Change.** Introduce named `CommonToSourceMap` and `SourceToCommonMap` types. A
  registration solution should own or derive both directions for the complete mapping.
  For SIP, implement a bounded numerical inverse with convergence/domain diagnostics or
  fit and validate a separate forward polynomial. Make `DrizzleFrame::new` require the
  forward type so the inverse warp cannot compile at that boundary.

  **Validation.** For translations, rotations, affine maps, safe homographies, and SIP
  fields, assert `forward(inverse(p))` and `inverse(forward(p))` over the complete image
  domain. Include a compile-fail/API test that a common→source map cannot be passed to
  drizzle and a synthetic distortion case where omitting SIP fails astrometry.

- [ ] **P0 — Build drizzle output geometry from mapped pixel boundaries and derive its metadata from that same grid.**

  **Contract.** Output mode, bounds, crop offset, dimensions, center mapping, WCS, pixel
  area, and units must arise from mapped source boundaries (`05-stacking-drizzle.md:756`).

  **Evidence.** `DrizzleAccumulator::new` fixes output dimensions to
  `ceil(input_dimension * scale)` at `../drizzle/accumulator.rs:69`. Turbo maps a source
  center to `scale * transformed_center` at `../drizzle/accumulator.rs:283`; Square uses
  the same multiplication for corners at `../drizzle/accumulator.rs:369`. No grid origin
  or output mode exists in `DrizzleConfig` at `../drizzle/config.rs:30`. Finalization uses
  `LinearImage::from_planar_channels` at `../drizzle/accumulator.rs:635`, whose constructor
  installs default metadata in `../../io/image/linear.rs`.

  **Impact.** At `scale=2`, identity maps source center zero to output center zero instead
  of `0.5`; half of the first scaled source footprint lies outside the output. Rotated or
  translated frames are clipped to the first input's nominal rectangle, mosaics cannot be
  represented, and the resulting pixels have no trustworthy WCS or physical scale.

  **Change.** Add a typed `OutputGrid` with Reference, Intersection, Union, and Explicit
  modes. Map every valid source boundary through the complete forward mapping, adaptively
  subdividing nonlinear edges, and use
  `o = scale * (common - bounds_min) - 0.5`. Carry the grid origin and output-to-sky
  transform into derived metadata; never call a default-metadata image constructor for a
  science product.

  **Validation.** Hand-check identity at scales 1, 1.5, 2, and 3, including both outer
  boundaries and the expected center-0 coordinate. Verify translated, rotated, union,
  intersection, and nonlinear-edge grids neither clip nor add an unexplained shift, and
  round-trip output WCS pixel centers to sky coordinates.

- [ ] **P0 — Perform complete, overflow-safe preflight validation before allocating or mutating either reconstruction path.**

  **Contract.** Stage 5 must reject incompatible layouts/units/filters, non-finite
  science or variance, invalid transforms, empty/overflowing grids, and budget violations
  before accumulation (`05-stacking-drizzle.md:148`).

  **Evidence.** Drizzle validates only dimensions, frame weight, and the optional weight
  map at `../drizzle/accumulator.rs:112`; although `Transform::is_valid` exists at
  `../registration/transform.rs:326`, it is never called. No drizzle error represents a
  bad transform or non-finite science sample in `../drizzle/error.rs:32`.
  `DrizzleAccumulator::new` casts scaled `f32` dimensions to `usize` and allocates full
  planes immediately at `../drizzle/accumulator.rs:69`. Statistical stacking validates
  dimensions at `../combine/cache/mod.rs:513` and finite pixels at
  `../combine/cache/mod.rs:522`, but simply chooses the
  first frame's metadata; filter, image type, binning, calibration state, and units are
  not compared.

  **Impact.** A NaN source value contaminates every drizzle output it touches. A singular,
  non-finite, or horizon-crossing homography can produce NaN coordinates and accumulators
  rather than a typed failure. Extreme scale can request an enormous allocation. Same-size
  but scientifically incompatible exposures can be silently combined under the first
  frame's metadata.

  **Change.** Preflight the complete frame set and resolved grid in checked `f64`/integer
  arithmetic. Validate source samples/masks/variance, mapping finiteness/invertibility and
  projective denominator over every used footprint, metadata compatibility, positive
  exposure/normalization/weights, output dimensions, and the selected RAM/disk budget.
  Accumulation should start only after this returns a validated plan.

  **Validation.** Assert exact typed errors and no accumulator mutation for each invalid
  field, a transform horizon crossing an edge, NaN/Inf samples, mismatched filters/binning,
  overflowed dimensions, and an over-budget output. Include a valid negative science
  sample to prove validation does not confuse sign with invalidity.

## Batch 2 — correct drizzle coefficient and quality mathematics

- [ ] **P0 — Remove the second Jacobian division from Turbo, Point, Gaussian, and Lanczos coefficients.**

  **Contract.** A mapped Square coefficient is overlap divided by mapped drop area once;
  Point has unit coefficient, and normalized alternative kernels must not receive an
  additional local-area division (`05-stacking-drizzle.md:788` and
  `05-stacking-drizzle.md:891`).

  **Evidence.** Square correctly forms `overlap / abs_jaco` at
  `../drizzle/accumulator.rs:396`. Turbo first normalizes overlap by nominal drop area and
  then divides the effective weight by `local_jacobian` at
  `../drizzle/accumulator.rs:303`. Point deposits `weight * pixel_weight / jaco` at
  `../drizzle/accumulator.rs:442`; Gaussian and Lanczos normalize their tap sum and then
  divide it by `jaco` at `../drizzle/accumulator.rs:516`.

  **Impact.** Under affine scale, homography, or varying distortion, identical source
  samples acquire model-dependent relative statistical weights. Kernels disagree about
  the same frame, and a local magnification changes the normalized science estimator even
  though each non-Square coefficient set already sums to its declared amount.

  **Change.** Define each kernel's coefficient independently of statistical weight and
  unit conversion. Square uses mapped overlap/mapped area exactly once; Turbo uses its
  declared axis-aligned normalized footprint; Point uses one; positive radial kernels use
  their declared full support normalization. Apply science-unit conversion separately and
  never hide it in `W`.

  **Validation.** For unit input weight under translation and uniform affine scales 0.5,
  1, and 2, hand-sum every interior input pixel's output coefficients. Cross-check Square
  against the pinned STScI implementation and prove every other declared normalized kernel
  is not divided by scale squared a second time.

- [ ] **P0 — Separate geometric support/context from statistical weight and signed kernel denominators.**

  **Contract.** Validity, geometric support, statistical weight, actual estimator
  coefficient, and coverage have distinct meanings (`05-stacking-drizzle.md:128` and
  `05-stacking-drizzle.md:958`).

  **Evidence.** One shared `weight` plane accumulates every signed/statistical
  `pixel_weight` at `../drizzle/accumulator.rs:553`. Finalization finds its global maximum,
  derives the validity threshold from that maximum, and labels `weight/max_weight` as
  coverage at `../drizzle/accumulator.rs:571`. `StackProduct` then exposes only coverage,
  weight, and a coefficient factor at `../product.rs:50`.

  **Impact.** Increasing one frame's statistical weight or one central pixel changes the
  validity threshold everywhere. Geometric holes are indistinguishable from low exposure
  quality. Signed Lanczos lobes can cancel the denominator, create negative “coverage,” or
  invalidate a supported pixel, while a large weight can make unrelated edges fail.

  **Change.** Accumulate contributor count/context and nonnegative absolute/geometric
  support independently of signed science coefficients and statistical `W`. Configure
  `min_coverage` against an explicit local expected-support policy. Expose each plane with
  a name and channel shape matching its meaning.

  **Validation.** Hold geometry fixed while multiplying one frame's statistical weight by
  100 and assert contributor/context/coverage are unchanged. Test signed kernels whose tap
  sum is small but whose geometric support is complete, and an extreme central weight that
  must not change edge validity.

- [ ] **P0 — Preserve signed science and full-support kernel normalization at crop boundaries.**

  **Contract.** Signed calibrated/Lanczos results remain signed, near-zero denominators are
  invalid, and Gaussian/Lanczos taps outside a requested crop are not silently
  renormalized away (`05-stacking-drizzle.md:927` and
  `05-stacking-drizzle.md:947`).

  **Evidence.** The radial path computes `total_weight` using only in-bounds output taps at
  `../drizzle/accumulator.rs:505`, so each cropped kernel is renormalized at the edge.
  Finalization marks Lanczos for special clamping at `../drizzle/accumulator.rs:564` and
  applies `value.max(0)` at `../drizzle/accumulator.rs:598`.

  **Impact.** Edge sources brighten relative to their retained kernel support. Lanczos
  loses ringing symmetry and legitimate negative calibrated/background values, biasing
  photometry and background statistics. A clamped image is no longer the declared linear
  reconstruction.

  **Change.** Normalize against the full phase-dependent kernel support, deposit only the
  in-grid subset without renormalizing, and report lost support separately. For signed
  kernels, retain signed values and denominators, use coefficient squares for variance,
  and reject a near-zero signed denominator with an explicit invalid flag.

  **Validation.** Compare an impulse at the center and each boundary phase with hand-summed
  full kernel taps. Assert crop loss appears only in support, negative inputs/results remain
  exact, positive/negative Lanczos lobes are preserved, and a zero-crossing denominator is
  invalid rather than filled or clamped.

- [ ] **P0 — Add per-channel variance, masks, DQ/context, and actual variance accumulation to drizzle.**

  **Contract.** Drizzle must combine detector/calibration/user/artifact masks before
  deposition and accumulate `Vnum += c²v`, with channel shape matching the estimator
  (`05-stacking-drizzle.md:820` and `05-stacking-drizzle.md:958`).

  **Evidence.** `DrizzleFrame` contains only source, transform, scalar frame weight, and one
  shared pixel-weight plane at `../drizzle/accumulator.rs:22`. The accumulator stores
  channel science numerators but only shared `weight` and `weight_sq` at
  `../drizzle/accumulator.rs:47`. Finalization labels `weight_sq/weight²` a linear-variance
  factor at `../drizzle/accumulator.rs:609`; no input variance is ever read.

  **Impact.** A defect affecting one RGB channel must suppress all three or contaminate one.
  Cosmic rays, trails, saturation, and calibration DQ cannot be audited after deposition.
  The output factor is correct only for common unit input variance and cannot support
  quantitative error bars.

  **Change.** Extend the coherent reconstruction frame with per-channel variance and
  validity/DQ planes plus an optional explicit joint mask. For every actual coefficient
  accumulate channel-shaped `Vnum`, `W`, `C2`, contributor/context, and documented DQ
  combination. Emit `Vnum/W²` as variance and keep `C2/W²` separately as the linear
  variance factor.

  **Validation.** Hand-compute a two-frame, two-channel example with unequal variances and
  one channel-specific bad pixel. Assert exact science, `W`, `Vnum/W²`, coefficient factor,
  context, and DQ. Monte Carlo independent noise must match the predicted diagonal
  variance.

- [ ] **P1 — Make surface-brightness versus flux-per-output-pixel units explicit and propagate their scale factors into variance and weights.**

  **Contract.** Surface brightness remains numerically constant with output scale; flux
  per input pixel requires `s²` science and `s⁴` variance conversion before weights are
  derived (`05-stacking-drizzle.md:859`).

  **Evidence.** `DrizzleConfig` has scale, pixfrac, kernel, fill, and coverage only at
  `../drizzle/config.rs:30`. `accumulate` multiplies the raw source value directly by its
  coefficient at `../drizzle/accumulator.rs:548`; neither an output-unit mode nor an `s²`
  conversion exists. The generated image metadata is defaulted as noted above.

  **Impact.** Current output behaves like a surface-brightness image, so total numeric DN
  grows by `scale²`, but nothing tells callers that interpretation. Applying an external
  flux correction later would leave variance and inverse-variance weights inconsistent.

  **Change.** Add a required science-unit convention to the reconstruction plan. Convert
  source science and variance into the chosen output units before computing statistical
  weight, derive local area conversion from WCS where necessary, and record the convention
  and output pixel area in metadata/provenance.

  **Validation.** At several scales, assert a constant field preserves surface brightness;
  assert an isolated integrated-flux impulse preserves total output flux only in flux mode;
  and assert flux-mode variance scales by `s⁴` and inverse variance by `1/s⁴`.

## Batch 3 — make statistical normalization and weighting physically complete

- [ ] **P0 — Carry calibrated per-pixel, per-channel variance through normalization and compute the general output variance.**

  **Contract.** Normalization multiplies variance by `gain²`; a weighted mean reports
  `sum(q²v)/sum(q)²`, not merely a coefficient factor (`05-stacking-drizzle.md:256` and
  `05-stacking-drizzle.md:299`).

  **Evidence.** `StackFrame` carries image, coverage, confidence, and one `FrameStats`
  record but no variance at `../combine/stack.rs:37`. Noise weighting averages all channel
  MAD sigmas into one frame scalar and squares it at `../combine/stack.rs:251`.
  `CombinedSample` accumulates only `sum(weight)` and `sum(weight²)` at
  `../combine/cache/mod.rs:73`; `StackProduct.linear_variance` is explicitly only
  `sum(w²)/sum(w)²` at `../product.rs:60`.

  **Impact.** Bright-source Poisson noise, flat/calibration uncertainty, channel response,
  and spatial detector noise never enter weights or uncertainty. RGB channels share an
  unjustified scalar frame weight. Manual weights cannot produce a modeled output variance,
  and the current factor is easy to mistake for science variance.

  **Change.** Add optional channel-shaped variance to `StackFrame`/stored frames, resample it
  with squared interpolation coefficients, apply normalization gain squared, and gather it
  with value/frame identity. Derive per-channel inverse-variance weights when available and
  accumulate `Vnum` from the actual survivors. Keep proxy-background mode explicit and mark
  its products approximate.

  **Validation.** Reproduce the specification's `[10,20]`, variances `[1,4]` result: mean
  `12`, variance `0.8`, and `N_eff=25/17`. Show a normalization gain of two multiplies
  variance by four, a channel-specific variance changes only its channel, and arbitrary
  manual weights use the general formula.

- [ ] **P0 — Standardize rejection residuals with sample and model variance instead of rejecting raw normalized values.**

  **Contract.** Heteroscedastic samples must be compared as residual divided by the square
  root of sample plus model variance (`05-stacking-drizzle.md:443`).

  **Evidence.** Every rejection method accepts only `&mut [f32]` and scratch, beginning with
  sigma clipping at `../combine/rejection.rs:77`; no variance or sample record reaches the
  rejection API. The combine gather packs values and effective weights separately at
  `../combine/cache/mod.rs:764`, then calls the reducer at
  `../combine/cache/mod.rs:795`.

  **Impact.** A noisy frame is rejected more often than a precise frame for the same
  statistically plausible excursion. Thresholds have no calibrated meaning once confidence,
  normalization gain, or source Poisson variance differs among samples.

  **Change.** Gather a named sample record retaining value, normalized variance, weight,
  frame ID, and flags. Fit centers/models in science units but run rejection on standardized
  residuals, including model uncertainty or documenting its approximation. Apply the final
  survivor mask to original values and weights.

  **Validation.** Construct equal standardized residuals from unequal raw deviations and
  assert identical keep/reject decisions. Verify asymmetric thresholds, boundary inclusion,
  frame/weight identity after sorting, and an injected high-variance good sample versus a
  low-variance outlier.

- [ ] **P0 — Turn degenerate photometric fits into typed outcomes instead of silently substituting gain `1.0`.**

  **Contract.** Normalization must validate gain, inlier count, residual structure, and
  extrapolation; a failed fit must fail or take a reported fallback
  (`05-stacking-drizzle.md:194`).

  **Evidence.** The initial gain becomes `1.0` when frame MAD is tiny at
  `../combine/normalization/mod.rs:590`. `deming_gain` returns `1.0` when there are fewer
  than two inliers, covariance is nonpositive, or the solved gain is invalid at
  `../combine/normalization/mod.rs:628`. The returned `FrameNorm` stores only gain/offset,
  with no status or diagnostics at `../combine/normalization/mod.rs:14`.

  **Impact.** A blank, anticorrelated, clipped, or poorly overlapping frame can be combined
  under a plausible-looking identity gain without an error, warning, or provenance marker.
  Its offset is still derived as though that fallback were a measured photometric relation.

  **Change.** Return a `NormalizationFit` containing coefficients, inlier count, residual
  scale/structure, fit domain, uncertainties, and an explicit status. Configure whether a
  failed frame is dropped, the job fails, or a named proxy fallback is accepted; never make
  identity indistinguishable from a successful fit.

  **Validation.** Exercise constant frames, negative/zero covariance, insufficient inliers,
  invalid gain, low overlap, and a valid known gain/offset. Assert exact typed outcomes and
  that accepted fallbacks appear in product provenance.

- [ ] **P1 — Replace all-frame common-domain normalization and lowest-MAD reference selection with an overlap-aware photometric plan.**

  **Contract.** The reference should balance exposure, saturation, photometric stability,
  PSF, and overlap; mosaics require overlap-graph/background solutions rather than a domain
  common to every image (`05-stacking-drizzle.md:194`).

  **Evidence.** Registered normalization intersects coverage/confidence from every frame in
  `build_common_domain` at `../combine/normalization/mod.rs:300` and errors if that
  intersection is empty at `../combine/normalization/mod.rs:333`. Reference
  selection chooses the smallest average channel MAD at
  `../combine/normalization/mod.rs:182`. Global fits are first measured against frame zero at
  `../combine/normalization/mod.rs:410` and later rerooted to the selected MAD reference.

  **Impact.** Valid disjoint mosaic tiles cannot be normalized. A clouded or low-transparency
  frame can win because it has low MAD. A single scalar sky offset can erase or mismatch real
  extended structure across a mosaic.

  **Change.** Build a pairwise overlap graph, choose an anchor using declared quality and
  overlap criteria, solve photometric gains and background differences across connected
  components, and support constrained low-frequency surfaces only from validated sky
  regions. Preserve the graph, fits, and excluded domains in provenance.

  **Validation.** Recover known gains/offsets in a chain-overlap mosaic with no all-frame
  intersection; reject disconnected components or report them explicitly. Include a dark
  cloudy low-MAD frame that must not become the anchor and extended emission that must not be
  fitted away.

## Batch 4 — make rejection names, parameters, and small-N behavior exact

- [ ] **P0 — Add a propagated noise/quantization floor to sigma clipping and make the fast path prove the same zero-scale semantics.**

  **Contract.** `MAD=0` must not mean “no outliers,” and a fast path is legal only when it
  proves the exact algorithm cannot reject (`05-stacking-drizzle.md:468`).

  **Evidence.** Iterative sigma clipping exits without rejection when converted MAD is below
  `f32::EPSILON` at `../combine/rejection.rs:124`. For `N>=10`, the pre-screen trims one
  minimum and maximum and returns “no outliers possible” when the remaining variance is
  effectively zero at `../combine/rejection.rs:160`. Thus many equal values plus one cosmic
  ray deliberately takes the no-rejection path.

  **Impact.** Quantized bias/dark/flat pixels and smooth backgrounds can retain an obvious
  isolated transient. The failure is most likely precisely where robust scale estimation
  needs a sensor/quantization floor.

  **Change.** Feed normalized per-sample variance or a declared quantization/noise floor into
  sigma rejection. When robust scatter is zero, compare against that floor. Redefine the
  range pre-screen as a conservative proof against the same final thresholds, including
  asymmetric and inclusive-bound behavior.

  **Validation.** Hand-check repeated equal values plus low/high outliers, exact threshold
  boundaries, sub-quantization differences, asymmetric thresholds, iterative masking, and
  fast/full path equivalence.

- [ ] **P1 — Either implement the documented outer-loop Winsorized rejection exactly or rename the current one-pass estimator.**

  **Contract.** Winsorized clipping must state its exact variant, finite inner/outer loop,
  starting scale, correction, zero-scale behavior, and whether original or clamped samples
  are finally combined (`05-stacking-drizzle.md:514`).

  **Evidence.** `robust_estimate` begins with `1.134 * RMS-about-median`, uses that corrected
  scale in every clamp iteration at `../combine/rejection.rs:271`, then `reject` performs one
  final compaction at `../combine/rejection.rs:318`. There is no outer reject/re-estimate
  loop. Yet the enum says Winsorized “replace[s] outliers” at
  `../combine/rejection.rs:867`, while the final mean receives original survivors.

  **Impact.** The algorithm and parameters do not match the Siril/PixInsight claim or the
  public name. Operators cannot reproduce threshold behavior, and future “fixes” can change
  serialized semantics without an honest algorithm boundary.

  **Change.** Prefer implementing the §3.7 inner scale loop and outer original-sample
  rejection loop exactly, including a configured outer cap/minimum survivors. Otherwise
  rename the variant to describe the one-pass robust-scale rejection and correct all API/UI
  text; do not claim source equivalence.

  **Validation.** Use a hand-computed vector to verify initial sample deviation, each 1.5σ
  clamp, `1.134` update, convergence, outer rejection, re-estimation, asymmetric bounds, and
  final use of original—not clamped—values.

- [ ] **P1 — Rename fixed-tail trimming and rank-linear clipping so their serialized parameters describe their actual mathematics.**

  **Contract.** Fixed-count trimming is not Siril percentile-deviation clipping, and a rank
  fit is not a temporal/reference/spatial-gradient fit (`05-stacking-drizzle.md:544` and
  `05-stacking-drizzle.md:565`).

  **Evidence.** `PercentileClipConfig::surviving_range` removes
  `floor(percent * N / 100)` positions from each sorted tail at
  `../combine/rejection.rs:548`. `LinearFitClipConfig` is documented as fitting a reference
  and handling sky gradients at `../combine/rejection.rs:365`, but its later passes sort
  values, fit against sorted index, and call mean absolute residual “sigma” at
  `../combine/rejection.rs:400`. With `max_iterations=1`, only the seed MAD pass runs, so no
  linear fit occurs.

  **Impact.** UI/config names imply different estimators and calibrated thresholds. Users can
  select LinearFit for a sky gradient it cannot model, while a nominal one-iteration fit
  never executes its defining operation.

  **Change.** Rename Percentile to `TrimmedFraction` and LinearFit to `RankLinearFit`; rename
  rank thresholds from `sigma_*` unless a consistency conversion is applied. Define the seed
  pass separately from the number of rank-fit iterations and remove claims about spatial sky
  gradients.

  **Validation.** Assert exact floor counts, ties, asymmetric tails, frame-ID preservation,
  and minimum survivors for trimming. For rank fit, hand-compute slope/intercept/mean absolute
  residual and prove one requested fit iteration actually performs one rank fit.

- [ ] **P1 — Resolve fallback from the local valid count and fix `SmallN` so an explicit median is not treated as a rejecting method.**

  **Contract.** Small-N behavior is a local per-output decision because borders and masks can
  have far fewer samples than the global frame count (`05-stacking-drizzle.md:627`).

  **Evidence.** `run_stacking_weighted` resolves one method from total frame count before any
  pixels are gathered at `../combine/stack.rs:485`. `SmallN::resolve` defines every method
  except `Mean(None)` as “does rejection” at `../combine/config.rs:60`; consequently a custom
  `Median` plus `Mean(None)` fallback can unexpectedly become a mean below the threshold.

  **Impact.** A 30-frame stack can run a large-N estimator on only two supported edge samples.
  Configuration shape, rather than local evidence, controls the estimator; an explicit median
  is not stable under all valid `SmallN` settings.

  **Change.** Resolve the estimator after per-pixel validity gathering. Represent local count
  bands explicitly and treat `Median` and `Mean(None)` as non-rejecting terminal estimators.
  Validate fallback graphs so they cannot recurse or silently replace a method that needs no
  fallback.

  **Validation.** In one large global stack, construct pixels with 0, 1, 2, 4, 10, and 30 valid
  samples and assert the exact configured local estimator and flags. Sweep every primary/
  fallback pair and prove explicit Median remains Median.

- [ ] **P2 — Make median weighting and even-sample center conventions explicit API choices.**

  **Contract.** Weighted median is a distinct estimator; otherwise requested weights should
  be rejected or produce an explicit caller-visible outcome (`05-stacking-drizzle.md:415`).

  **Evidence.** `warn_if_weights_ignored` emits tracing only when median meets non-equal
  weighting at `../combine/stack.rs:306`. `run_stacking_weighted` then calls the median path
  with no frame weights at `../combine/stack.rs:501`. The final estimator uses
  `median_f32_mut`, which averages both middle values, at `../combine/stack.rs:503`; rejection
  centers instead select the upper-middle value directly at `../combine/rejection.rs:121` or
  through `median_f32_fast` at `../../math/statistics/mod.rs:65`.

  **Impact.** A successfully returned product can ignore a deliberate manual/noise weighting
  configuration. Logs are not a stable API or provenance channel. For even local counts, the
  rejection boundary is centered on a different statistic from the final estimator, which can
  create asymmetric keep/reject behavior that is neither configured nor reported.

  **Change.** Either reject non-equal weighting with unweighted Median at configuration
  validation, or introduce an explicitly named WeightedMedian estimator with exact tie and
  cumulative-weight semantics. Choose and name the even-sample center convention separately for
  robust-scale estimation and final combination; if upper-middle is retained as a hot-path
  approximation, make that approximation explicit. Record the resolved estimator in the product.

  **Validation.** Assert unweighted Median plus Equal succeeds; non-equal weights either fail
  with a typed configuration error or change the result according to hand-computed weighted-
  median semantics. Include small-N fallback into Median and an even sample where average-middle
  and upper-middle centers produce different rejection masks.

## Batch 5 — reject drizzle artifacts and expose quantitative outcomes

- [ ] **P1 — Implement the source-plane median/blot/derivative artifact-mask workflow before final drizzle deposition.**

  **Contract.** Drizzle does not reject cosmic rays itself; incomplete masks require separate
  model drizzles, a robust common model, blotting to each source grid, derivative-aware two-pass
  thresholds and growth, then one final deposition from the original calibrated inputs
  (`05-stacking-drizzle.md:983`).

  **Evidence.** `DrizzleConfig` has no rejection/model/blot parameters at
  `../drizzle/config.rs:30`, and `DrizzleFrame` has no artifact mask at
  `../drizzle/accumulator.rs:22`. `drizzle_images` creates one accumulator, deposits each frame
  once, and immediately finalizes at `../drizzle/stack.rs:124`; the disk entry point follows the
  same one-pass structure at `../drizzle/stack.rs:79`.

  **Impact.** Unless callers have already produced a perfect scalar zero-weight map externally,
  source cosmic rays, satellite/aircraft trails, saturation blooms, and residual defects are
  deposited into multiple output pixels. Direct output-grid clipping cannot reconstruct which
  source sample caused the correlated footprint.

  **Change.** After Batch 1/2 provide complete mappings, variance, and masks, add the documented
  model pass: per-frame smooth drizzles at `pixfrac=1`, robust common median/model, exact blot to
  each source plane with consistent units, derivative map, two noise-aware thresholds, neighbor
  growth, and optional connected line/trail growth. Merge the result into the source DQ mask and
  run final drizzle exactly once from original inputs.

  **Validation.** Inject isolated cosmic rays, a saturated bloom, a thin trail, undersampled
  shifted stellar cores, and diffuse structure. Assert exact source-mask pixels, growth behavior,
  preservation of real cores/edges under the derivative term, and absence of every masked source
  coefficient from numerator, variance, weight, context, and DQ-good count.

- [ ] **P1 — Emit validity, geometric count, survivor count, rejection counts, `N_eff`, and modeled variance with estimator-matching channel shape.**

  **Contract.** A statistical stack should distinguish support before rejection, survivors,
  actual weight sum, effective count, variance, and low/high rejection diagnostics
  (`05-stacking-drizzle.md:662`).

  **Evidence.** Combine coverage counts frames whose coverage exceeds a fixed threshold and
  divides by total frames at `../combine/cache/mod.rs:645`; it is computed independently of
  confidence and rejection. `CombinedSample` retains only value, weight sum, and coefficient
  factor at `../combine/cache/mod.rs:62`. `StackProduct` exposes only image, coverage, weight,
  and optional coefficient factor at `../product.rs:50`.

  **Impact.** A pixel supported by ten frames but surviving one rejection looks geometrically
  well covered. There is no way to distinguish uncovered fill zero from valid science zero
  without interpreting coverage indirectly, audit low versus high artifacts, or judge whether
  one weight dominates.

  **Change.** Expand the product with explicit validity, geometric contributor count/fraction,
  survivor count, low/high rejection counts, absolute `W`, `N_eff`, and modeled variance;
  optionally add empirical scatter/reduced chi-square. Use shared or per-channel maps according
  to the estimator's actual survivor policy.

  **Validation.** Hand-construct pixels covering every combination of unsupported, masked,
  rejected-low, rejected-high, and accepted samples. Assert every map exactly, including a
  valid numeric zero and an invalid fill value that must remain distinguishable.

- [ ] **P1 — Derive output metadata and retain normalization, registration, rejection, and input provenance instead of copying one frame.**

  **Contract.** Output metadata/provenance must describe ordered inputs, units, coefficients,
  transforms, output grid, estimator/fallback, kernel, and deterministic mode
  (`05-stacking-drizzle.md:1177`).

  **Evidence.** The combine cache describes and stores metadata from the first frame at
  `../combine/cache/mod.rs:115` and copies it into the product at
  `../combine/cache/mod.rs:582`. The alignment pipeline later replaces it with reference
  metadata at `../pipeline/align.rs:181`. `AlignmentSummary` stores only reference index,
  registered count, and dropped indices at `../pipeline/result.rs:13`; `StackProduct` has no
  provenance field. Drizzle resets metadata entirely as shown in Batch 1.

  **Impact.** Exposure time, pixel scale, WCS, units, normalization coefficients, individual
  transform/rejection outcomes, and software/config identity cannot be reconstructed from the
  result. Copied source exposure and dimensions can actively misdescribe a coadd.

  **Change.** Build an immutable `ReconstructionProvenance` during the validated plan and carry
  it through both RAM and streaming paths. Derive aggregate exposure and output geometry fields;
  retain ordered per-frame accepted/dropped status, normalization fit, complete mappings,
  resolved local/global policy, kernel/unit/covariance semantics, and reduction mode.

  **Validation.** Compare RAM and streaming provenance byte-for-byte for the same deterministic
  run. Verify aggregate exposure, WCS, units, all frame identities/outcomes, transform/SIP,
  normalization, and resolved estimator parameters against the input plan.

- [ ] **P2 — Characterize interpolation/drizzle covariance and the effective coadd PSF instead of presenting diagonal quality as complete uncertainty.**

  **Contract.** Shared source coefficients create off-diagonal covariance, and differing input
  PSFs require an effective-PSF or explicitly separate measurement product
  (`05-stacking-drizzle.md:366` and `05-stacking-drizzle.md:1056`).

  **Evidence.** Ordinary warp reports only per-pixel coverage and white-noise confidence at
  `../registration/resample/mod.rs:19`. Drizzle retains only coefficient squares for each
  output independently at `../drizzle/accumulator.rs:55`. Neither `StackProduct` nor
  `AlignmentSummary` contains covariance/correlation or PSF information.

  **Impact.** Aperture and smoothed measurements that sum diagonal variances underestimate
  noise. Users cannot tell whether a sharper-looking stack reflects sampling, a different
  effective PSF, or unmodeled correlation.

  **Change.** At minimum record kernel/interpolation settings and a reproducible correlation
  model/factor appropriate to the actual transforms and masks. Prefer a compact covariance
  stencil or validated white-noise propagation for quantitative products, and propagate a
  coadd PSF diagnostic separately from inverse-variance weight.

  **Validation.** Drizzle independent white-noise realizations through exact production maps;
  compare diagonal variance, adjacent covariance, and blank-sky aperture variance at several
  sizes with the stored model. Measure effective PSF on synthetic stars across the field.

## Batch 6 — simplify the public surface and make the implementation scalable

- [ ] **P1 — Replace resident `f32` drizzle accumulation with budgeted deterministic tiles and cancellable stable sums.**

  **Contract.** Long accumulations and polygon geometry should use `f64`/stable summation;
  drizzle must tile when output planes dominate memory and poll cancellation during deposition
  and finalization (`05-stacking-drizzle.md:1139`, `05-stacking-drizzle.md:1150`, and
  `05-stacking-drizzle.md:1170`).

  **Evidence.** The accumulator allocates channel numerator, weight, and weight-square as full
  resident `Buffer2<f32>` planes at `../drizzle/accumulator.rs:47`. Every source pixel updates
  those planes directly in `f32` at `../drizzle/accumulator.rs:548`. `DrizzleConfig` has no
  storage budget at `../drizzle/config.rs:30`, and neither public entry point accepts a
  cancellation token at `../drizzle/stack.rs:53` or `../drizzle/stack.rs:100`.

  **Impact.** A 3× RGB drizzle requires large full-frame arrays before variance, masks,
  support, and context are added. Small contributions can be lost in long/high-dynamic-range
  stacks, cancellation cannot stop deposition, and the otherwise memory-tiered pipeline cannot
  safely adopt drizzle.

  **Change.** Plan output tiles from the shared memory budget, map each source footprint to
  touched tiles with kernel halos, and accumulate numerator/`W`/`Vnum`/`C2` in `f64` or
  compensated sums under a defined frame order. Support RAM and mmap tile sinks and poll a
  `CancelToken` during validation, loading, frame/tile deposition, rejection/blotting, and
  finalization.

  **Validation.** Enforce peak-memory budgets on large synthetic grids; cancel in every phase;
  compare all-resident, RAM-tiled, and mmap-tiled outputs to an `f64` reference under the
  documented exact/tolerance policy and across thread counts.

- [ ] **P1 — Make the incremental accumulator internal or make its lifecycle enforce non-empty validated plans.**

  **Contract.** Empty reconstruction is an error and no partial product may escape as complete
  (`05-stacking-drizzle.md:148` and `05-stacking-drizzle.md:1170`).

  **Evidence.** `DrizzleAccumulator` is publicly re-exported at `../../lib.rs:97`. Callers can
  construct it at `../drizzle/accumulator.rs:69` and immediately call public `finalize` at
  `../drizzle/accumulator.rs:560`; `frames_added` is incremented but never checked. The higher-
  level entry points correctly reject empty vectors at `../drizzle/stack.rs:58` and
  `../drizzle/stack.rs:105`, so the public accumulator bypasses their contract.

  **Impact.** Public API permits a default-metadata, all-fill, zero-weight image presented as a
  successful drizzle. It also freezes a low-level lifecycle that cannot infer a union grid,
  preflight a complete frame set, or guarantee transactional cancellation.

  **Change.** Prefer making the accumulator private behind a validated reconstruction plan and
  the two safe entry points. If incremental deposition is a real requirement, require an
  explicit grid/metadata plan, make `finalize` return `Result`, reject zero frames, and mark a
  cancelled/failed accumulator unusable.

  **Validation.** Prove every public construction path rejects zero frames, invalid planning, and
  finalize-after-failure/cancellation. Ensure no partially accumulated image can be obtained from
  a failed lifecycle.

- [ ] **P2 — Reduce the kernel surface to the validated Square reference plus demonstrated needs before optimizing alternatives.**

  **Contract.** Exact Square is the scientific reference; Turbo is allowed only after bounded
  error comparison, and optional kernels must have exact normalization/photometric semantics
  (`05-stacking-drizzle.md:895` and `05-stacking-drizzle.md:1210`).

  **Evidence.** The public enum already contains Square, Turbo, Point, Gaussian, and Lanczos at
  `../drizzle/config.rs:5`, while the default is the approximate Turbo kernel at
  `../drizzle/config.rs:54`. Separate Turbo, Point, and shared radial implementations occupy
  `../drizzle/accumulator.rs:256`, `../drizzle/accumulator.rs:412`, and
  `../drizzle/accumulator.rs:454`, despite the coefficient, edge, coverage, unit, and variance
  defects above. Point also ignores pixfrac although validation rejects zero at
  `../drizzle/config.rs:103`.

  **Impact.** Multiple incorrect policies multiply the work needed to repair grid, masks,
  variance, tiling, and metadata, while the default avoids the only geometry treated as the
  correctness reference. Complexity is being maintained without an end-to-end validated
  scientific path.

  **Change.** Make Square the default and the sole first implementation of the complete contract.
  Retain Point only if interlacing is a demonstrated workflow and allow its natural `pixfrac=0`.
  Delete or feature-gate Turbo/Gaussian/Lanczos until each has a written selection bound,
  full-support normalization, signed/variance behavior, photometric validation, and measured need.

  **Validation.** Establish the full Square reference suite first. For every restored kernel,
  compare flux/surface-brightness behavior, support, variance, edges, rotation/shear/distortion,
  PSF, and covariance against Square and its stated accuracy/performance criterion.

## Recommended implementation order

1. Close the immediate false-science paths: correct mapping direction, half-pixel grid,
   transform/sample validation, second Jacobian, signed clamping, and zero-MAD rejection.
2. Introduce the coherent reconstruction frame, complete mapping pair, output grid, units,
   variance, and masks; these types are prerequisites for both reconstruction paths.
3. Integrate exact Square drizzle into the alignment pipeline without pre-warping and make
   its output metadata/provenance correct.
4. Make statistical weighting/rejection variance-aware and normalization failures explicit;
   then resolve small-N and exact rejection semantics.
5. Add complete quality/diagnostic products, tiled stable drizzle accumulation, cancellation,
   and source-plane median/blot/derivative artifact masks.
6. Restore optional drizzle kernels only after each passes the Square reference contract.

## Open questions

- [ ] **Choose the first supported drizzle output mode.** Reference-footprint is the smallest
  coherent first delivery; Union/Intersection/Explicit should be implemented immediately only if
  current callers need mosaics or caller-provided WCS.

- [ ] **Choose the science-unit default.** Confirm whether ordinary Lumos calibrated pixels are
  formally surface brightness/count rate per area or integrated flux per detector pixel. The
  choice controls `s²`, variance, inverse weight, metadata, and flux tests.

- [ ] **Choose the normalization failure policy.** Decide whether an invalid per-frame fit drops
  that frame, fails the entire job, or permits a specifically named proxy fallback. Identity must
  never remain an unreported success.

- [ ] **Decide which optional drizzle kernels have demonstrated production value.** Keep only
  kernels with representative datasets and a quantitative accuracy/performance reason; otherwise
  delete them until the reference path is complete.
