# Star-detection specification versus implementation review

Date: 2026-07-21

Scope: production code implementing
[`03-star-detection.md`](03-star-detection.md), including the shared background
mesh, detection-plane construction, background estimation, matched filtering,
thresholding, connected-component labeling, deblending, centroid/PSF measurement,
quality filtering, and the handoff to registration. Test-only, benchmark, fixture,
and memory-probe code was not reviewed.

## Outcome

The specification is substantially stronger than the implementation. Section 15 is
an unusually accurate self-audit: its central findings about the derived measurement
plane, absent masks and variance, stationary-noise threshold, biased flux, fixed
saturation level, pre-deblend area rejection, raw-value deblending, weak fit
acceptance, misleading DAOFIND names, and flux-only catalog are all confirmed by the
production code.

The implementation is not yet a correct realization of the normative algorithm in
sections 1–14. The largest risks are not tuning problems:

1. Detection and measurement share one inverse-MAD RGB combination that is median
   filtered for ordinary demosaiced RAW images, so the returned centroids, widths,
   fluxes, peaks, and SNR are properties of a synthetic detection plane.
2. The API cannot receive permanent masks, propagated variance, saturation metadata,
   or cancellation, and it cannot return runtime failures. Invalid pixels therefore
   participate in every scientific stage.
3. Background refinement deliberately reintroduces all pixels when a tile becomes
   fully masked; interpolated RMS has no finite/positive invariant; and the matched
   statistic is normalized only for stationary independent noise.
4. Both deblenders operate on raw derived-plane values and end in geometric Voronoi
   ownership. The multithreshold tree compares background-contaminated sums from
   different isophotal levels, while measurement ignores even that ownership.
5. Profile-fitting infrastructure is optimized across AVX2 and NEON before the model
   supplies pixel integration, trustworthy convergence, residual gates, or position
   covariance. The caller even enables a position-only early exit and then consumes
   the incompletely converged shape parameters as FWHM.
6. Registration receives only a flux-sorted `Vec<Star>` and takes its first `N`
   elements. There is no uncertainty ranking or spatial uniformization, so the module
   does not yet perform the “registration-catalog construction” in the document title.

One statement in the document's current-implementation audit is stale: background
interpolation no longer extrapolates before the first tile center; both axes clamp the
spline parameter. That sentence should be removed while retaining the real RMS
positivity problem.

The reusable whole-frame buffers, signed background subtraction before convolution,
stationary-noise kernel-energy normalization, packed threshold masks, RLE CCL,
separable convolution, robust tile statistics, and scalar model/Jacobian definitions
are useful foundations. They should be retained, but the data contract and statistical
domains need correction before presets or performance kernels are retuned.

## Current production data flow

```text
LinearImage
  -> copy mono OR globally inverse-MAD-weight RGB
  -> if metadata.cfa_type is present: nonlinear 3x3 median filter
  -> tiled empirical background + RMS
  -> optional source-mask refinement
  -> fixed/bootstrapped FWHM
  -> background-subtracted Gaussian convolution
  -> one high-threshold bit mask using center-pixel RMS
  -> 4/8-connected components
  -> discard parent area > max_area
  -> raw-value local-maxima OR raw-value multithreshold deblend
  -> independently measure a full square stamp around every seed
  -> fixed saturation/SNR/shape cuts
  -> flux sort
  -> broad-FWHM-only rejection
  -> fixed-radius brightest-first duplicate removal
  -> Vec<Star>
  -> registration takes the first max_stars entries
```

The same derived buffer is used on both sides of candidate detection. A `Region`
contains only a bounding box, peak, raw peak value, and area; no segmentation owner
map reaches the measurement stage.

## Contract coverage

| Specification area | Production status | Main evidence |
|---|---|---|
| Immutable measurement planes separate from detection products | Incorrect | prepared plane is passed to both detection and measurement |
| Permanent mask, variance/RMS, saturation, provenance input | Missing | `detect` accepts only `&LinearImage` |
| Runtime validation, typed failure, and cancellation | Missing | detection returns `DetectionResult` directly |
| Robust tiled background | Partly implemented | clipping/mode/spline exist; invalid tiles and positive RMS do not |
| Variance-aware matched statistic | Incomplete | correct only for locally stationary independent noise |
| Hysteresis footprints and topology | Missing | one threshold creates one mask |
| Deterministic row-major labeling | Sequential only | parallel provisional IDs depend on atomic allocation order |
| Large-island deblending | Incorrect | `max_area` is applied before either deblender |
| Local-maxima deblending | Incomplete | strict raw-pixel maxima, raw fractional prominence, Voronoi ownership |
| SExtractor-compatible multithreshold deblending | Incorrect | raw isophotal sums, fixed 8-connectivity, first-eight truncation, Voronoi ownership |
| Ownership-aware crowded measurement | Missing | each seed receives the complete square stamp |
| Windowed centroid fallback | Incomplete | rectified signal, missing factor two, no convergence result |
| Pixel-integrated PSF fit and acceptance | Incomplete | center-sampled restricted models and weak gates |
| Position/flux covariance and source flags | Missing | `Star` has only nine scalar fields plus position |
| Metadata-driven saturation | Missing | fixed `0.95` derived-plane threshold |
| Registration-specific selection | Missing | final flux order is consumed directly by registration |
| Diagnostic capture | Incomplete | funnel counts exist; masks, invalid meshes, fit status, flags, timings do not |

## Batch 1 — restore the scientific data contract

- [ ] **P0 — Separate the temporary detection product from immutable measurement data.**

  **Contract.** Sections 1, 3, and 13 require nonlinear filtering to remain a
  detection-only operation and require centroid, saturation, PSF, and flux to be
  measured from original calibrated planes (`03-star-detection.md:27`,
  `03-star-detection.md:156`, and `03-star-detection.md:1126`).

  **Evidence.** `prepare` globally combines RGB with inverse-MAD weights and median
  filters whenever `metadata.cfa_type` remains present at
  `../star_detection/detector/stages/prepare.rs:20` and
  `../star_detection/detector/stages/prepare.rs:36`. That metadata intentionally
  travels through RAW demosaic: the new planar image receives the source metadata at
  `../../io/astro_image/cfa.rs:160`. `StarDetector::detect` passes the resulting
  `grayscale_image` to background, detection, FWHM bootstrap, and measurement at
  `../star_detection/detector/mod.rs:125`, `../star_detection/detector/mod.rs:155`,
  and `../star_detection/detector/mod.rs:176`. The preparation comment also calls
  inverse-variance weighting optimal for an “unknown (flat) source SED” at
  `../star_detection/detector/stages/prepare.rs:47`; a fixed inverse-variance linear
  combination is optimal only for the particular assumed response vector, not an
  unknown source color.

  **Impact.** Ordinary demosaiced RAW frames are measured after a nonlinear median,
  which changes pixel phase, PSF width, peaks, saturation plateaus, and noise
  covariance. RGB flux is a frame-global noise-weighted synthetic quantity whose
  color response changes with channel noise. Registration sees plausible numbers but
  cannot interpret or compare their physical domain reliably.

  **Change.** Introduce a detection work product containing one or more explicitly
  defined detection planes and keep the calibrated source planes alive separately.
  Candidate generation may use artifact-suppressed or color-combined products, but
  measurement must receive original planes, permanent masks, and variance. Record the
  detection combination and assumed source response. For unknown colors, implement
  the documented per-band union or chi-square discovery policy rather than presenting
  one flat-SED linear combination as color-agnostic.

  **Validation.** Inject red-only, green-only, blue-only, and neutral pixel-integrated
  stars with unequal channel noise and dense subpixel phases. Detection-plane changes
  may change completeness as documented, but accepted centroids and per-band/source
  measurements must remain those of the immutable source samples. Assert that merely
  retaining CFA provenance after demosaic does not median-filter measurement data.

- [ ] **P0 — Replace `detect(&LinearImage) -> DetectionResult` with a validated, cancellable input/result contract.**

  **Contract.** Section 2 requires image, permanent bit mask, uncertainty, saturation,
  and coordinate metadata with exact shape/finite validation; section 14 requires
  typed invalid-input/background errors and cancellation
  (`03-star-detection.md:58`, `03-star-detection.md:76`,
  `03-star-detection.md:1150`, and `03-star-detection.md:1189`).

  **Evidence.** The public call accepts only `&LinearImage` and cannot fail at
  `../star_detection/detector/mod.rs:115`. The only error type is configuration-time,
  and configuration validation occurs only in `from_config` at
  `../star_detection/detector/mod.rs:96`. No production stage accepts a permanent mask
  or `CancelToken`. The RAM pipeline checks cancellation before detection and only
  interprets it after all detector jobs finish at `../pipeline/align.rs:47` and
  `../pipeline/align.rs:74`.

  **Impact.** Non-finite samples can poison medians, splines, fits, and sorting; bad
  pixels and saturation enter convolution and measurement; a full-resolution frame
  cannot be interrupted inside its expensive passes; and image-specific invalid
  configuration is represented as an empty catalog or panic. Because no runtime
  `Result` exists, pooled buffers also have no structured early-return discipline for
  the error/cancellation paths the specification requires.

  **Change.** Add a `DetectionInput`/`MeasurementInput` bundle and a typed
  `StarDetectionError`, then make detection return `Result<DetectionResult, _>` and
  accept a `CancelToken`. Validate nonzero dimensions, channel shapes, finite usable
  samples, masks, variance/RMS, saturation level, and image-dependent radii/margins
  before allocating work. Use scoped/RAII pool leases so every return path restores
  acquired planes automatically. Poll cancellation at the documented stage and block
  boundaries.

  **Validation.** Table-test every shape/non-finite/missing-metadata failure and assert
  the exact typed error without mutation. Cancel between background passes,
  convolution blocks, component batches, and fit groups; assert bounded latency and
  identical pool inventory before and after each forced exit. A valid blank image must
  still return `Ok` with an empty catalog.

- [ ] **P0 — Define one non-double-counting variance convention and compute unbiased flux/SNR.**

  **Contract.** Sections 4 and 11 require an explicit empirical-versus-physics variance
  convention, signed or model-based flux, background-estimation variance, covariance
  where relevant, and `SNR = flux / sqrt(flux_variance)`
  (`03-star-detection.md:219` and `03-star-detection.md:879`).

  **Evidence.** `compute_star` clips every residual to zero, integrates the complete
  square stamp, and uses that square's pixel count at
  `../star_detection/centroid/mod.rs:707`,
  `../star_detection/centroid/mod.rs:743`, and
  `../star_detection/centroid/mod.rs:838`. Gaussian/Moffat amplitude and residual
  diagnostics are not used for production flux; `measure_star` always calls the same
  moment-based `compute_star` after fitting at
  `../star_detection/centroid/mod.rs:463`. `NoiseModel::variance_normalized` adds
  explicit read-noise variance on top of the empirical tile RMS at
  `../star_detection/config.rs:97`, even though that RMS already measures blank-sky
  read noise.

  **Impact.** Rectification gives every noise sample positive expectation, so flux and
  SNR rise with stamp area even for blank sky. The denominator describes a square
  aperture while the numerator is neither a signed aperture nor fitted total flux.
  Enabling sensor parameters can lower SNR by counting read noise twice, making a
  supposedly more informed configuration less correct.

  **Change.** Give flux an explicit definition: signed owned-footprint aperture with
  aperture/background covariance, or preferably integrated PSF amplitude with the
  full fitted covariance. Store `flux_variance` and derive SNR from it. Split noise
  configuration into mutually exclusive empirical-total and physics-built semantics;
  empirical RMS may receive only variance terms it does not already contain. Do not
  use a geometric square's nominal pixel count when pixels are weighted, masked, or
  owned by neighbors.

  **Validation.** On blank Gaussian noise, the mean signed flux must be statistically
  zero for every stamp radius. Hand-compute aperture/model flux variance for tiny
  fixtures, then sweep gain/read-noise modes to prove that the same physical variance
  is counted once. Repeated injected-star realizations must give standardized flux
  residuals with mean near zero and RMS near one.

- [ ] **P0 — Return a raw flagged catalog and build a separate registration catalog from uncertainty and spatial coverage.**

  **Contract.** Sections 2.3, 11, and 12 require source flags, position covariance,
  flux variance, fit/background diagnostics, PSF axes/orientation, segmentation and
  mask context, followed by a deterministic registration-specific eligibility,
  quality, and uniformization pass (`03-star-detection.md:113`,
  `03-star-detection.md:877`, and `03-star-detection.md:1006`).

  **Evidence.** `Star` exposes only position, flux, scalar FWHM/eccentricity/SNR/peak,
  sharpness, and two roundness values at `../star_detection/star.rs:6`.
  `DetectionResult` returns only the already filtered stars and aggregate diagnostics
  at `../star_detection/detector/mod.rs:25`; individual rejected sources and reasons
  are discarded. Filtering sorts solely by flux at
  `../star_detection/detector/stages/filter.rs:58`. Registration assumes that order
  and takes the first `max_stars` at `../registration/mod.rs:138`.

  **Impact.** Downstream code cannot tell a converged fit from a moment fallback,
  isolated from blended, or precise from weakly constrained. Bright central stars can
  dominate triangle construction while field edges remain underrepresented. Rejection
  tuning cannot be audited from saved output because rejected measurements and flags
  no longer exist.

  **Change.** Add a rich `DetectedSource` raw catalog with stable source ID, flags,
  position covariance, flux/variance definition, local background, fit status and
  residual, axes/orientation, ownership/mask coverage, neighbor distance, and detection
  scale. Preserve rejected entries. Implement the section 12 selector as a distinct
  function that ranks eligible sources by positional uncertainty and fit quality,
  spatially uniformizes to `K`, then emits the compact registration view.

  **Validation.** Hand-build catalogs with exact flags, covariance ties, and empty or
  crowded spatial cells and assert the exact output sequence. Demonstrate on synthetic
  transformations that uncertainty-ranked uniform selection is no worse than the
  current flux prefix and improves edge-constrained affine/projective recovery.

## Batch 2 — make background and detection statistics valid

- [ ] **P0 — Represent invalid background tiles and enforce finite positive RMS through interpolation.**

  **Contract.** Section 5 requires a minimum usable fraction, invalid-node repair,
  never sampling fully masked source pixels, and interpolation of `log(sigma)` or
  variance with a positive floor (`03-star-detection.md:271`,
  `03-star-detection.md:336`, and `03-star-detection.md:349`).

  **Evidence.** A fully masked tile deliberately falls back to sampling all pixels at
  `../../background_mesh/tile_stats.rs:13` and
  `../../background_mesh/tile_stats.rs:36`. No validity/count field exists in
  `TileStats`; empty data become numeric zeros at
  `../../background_mesh/tile_stats.rs:53`. Large unmasked tiles use a regular 2-D
  stride at `../../background_mesh/tile_stats.rs:99`. The grid directly median-filters
  scalar sigma and skips the entire filter if either grid dimension is below three at
  `../../background_mesh/mod.rs:175`. Star-detection interpolation splines sigma itself
  with no postcondition at `../star_detection/background/mod.rs:187`, while thresholding
  silently converts any zero or negative result to `1e-6` at
  `../star_detection/threshold_mask/mod.rs:31`.

  **Impact.** Refinement can mask a bright/crowded tile and then put all of that source
  flux back into its background estimate. Regular stride can alias CFA/row/column
  patterns. Cubic overshoot can create non-positive RMS even when all nodes are
  positive; the threshold floor then turns an invalid uncertainty model into an
  extremely permissive detection region rather than surfacing the failure.

  **Change.** Extend tile statistics with valid count/fraction and validity state.
  Use deterministic stratified or reservoir sampling over valid pixels. Repair invalid
  nodes from valid neighbors as specified and fail if the grid cannot be repaired.
  Median-filter axes independently where possible. Interpolate `log(sigma)` or variance,
  exponentiate/square-root, enforce a documented floor, and validate every output
  sample before thresholding.

  **Validation.** Pin all-masked, sparsely valid, one/two-node-axis, periodic-CFA, and
  positive-node overshoot fixtures. Assert that masked values are never read into tile
  statistics, repaired nodes are exact, and every output background/RMS is finite with
  RMS strictly positive. An all-invalid grid must return a typed error.

- [ ] **P0 — Replace center-RMS kernel normalization with the variance-aware matched statistic.**

  **Contract.** Section 7.2 defines numerator `A`, information `H`, flux,
  `flux_variance`, usable-template fraction, and significance from every valid kernel
  sample; section 7.3 covers correlated noise (`03-star-detection.md:509` and
  `03-star-detection.md:549`).

  **Evidence.** `matched_filter` normalizes a unit-sum kernel by one global
  `sqrt(sum(K^2))` factor and explicitly claims that the original output-pixel noise
  map then makes thresholding correct at
  `../star_detection/convolution/mod.rs:45` and
  `../star_detection/convolution/mod.rs:101`. Detection compares that output against
  `stats.noise[q]` at `../star_detection/detector/stages/detect.rs:99`. This equality is
  exact only when independent variance is effectively constant over the footprint;
  no mask or convolved `K^2 V` denominator exists. Mirrored edge samples are counted as
  if they were distinct observations.

  **Impact.** Vignetting, gradients in sky/read noise, defects, demosaic covariance,
  and edges change the null variance without changing the threshold denominator.
  Therefore one nominal `sigma_threshold` has position-dependent false-positive and
  completeness behavior. The code comment overstates the statistical guarantee.

  **Change.** Implement the documented scalar reference for `A(q)` and `H(q)` with
  masks and nonuniform variance, then optimize its separable components where valid.
  Reject insufficient usable-template information rather than mirror-duplicating
  samples. For correlated products, empirically normalize the null by mesh and inflate
  covariance as documented. Rename the existing path to an explicit
  stationary-noise approximation if it remains as a selectable fast mode.

  **Validation.** Hand-compute `A`, `H`, flux, variance, and significance for small
  kernels with one variance/mask change at a time. On large blank fields, measure a
  unit-width normalized null and threshold exceedance rate independently by mesh and
  distance from edges. Compare the optimized path exactly against the scalar reference.

- [ ] **P1 — Implement high/low hysteresis footprints and apply area/edge policy after deblending.**

  **Contract.** Sections 7.6 and 8 require a high seed threshold plus lower footprint
  threshold, parent preservation through deblending, maximum-area policy only on final
  leaves/complex fallbacks, and an image-invalid error when the edge margin consumes
  the frame (`03-star-detection.md:595`, `03-star-detection.md:629`, and
  `03-star-detection.md:634`).

  **Evidence.** `DetectionConfig` contains only one `sigma_threshold` at
  `../star_detection/config.rs:236`, and `detect` creates one threshold mask at
  `../star_detection/detector/stages/detect.rs:95`. Both deblender paths discard
  components over `max_area` before processing at
  `../star_detection/detector/stages/detect.rs:194` and
  `../star_detection/detector/stages/detect.rs:230`. The final retain checks minimum
  area and edge only at `../star_detection/detector/stages/detect.rs:159`. An invalid
  edge margin emits a warning and then produces an empty result at
  `../star_detection/detector/stages/detect.rs:145`.

  **Impact.** A high threshold provides too little footprint for topology and
  measurement, while lowering the one threshold increases noise bridges. Crowded
  islands disappear wholesale before the code intended to split them. Large final
  artifacts are retained asymmetrically, and an invalid per-image configuration is
  indistinguishable from a starless exposure.

  **Change.** Add validated `peak_threshold` and `footprint_threshold`, label low-mask
  components containing high-mask seeds, and keep parents through deblend. Apply
  min/max area to final leaves; represent oversized unsplit islands with a complex or
  overflow flag. Validate edge and kernel/stamp margins against image dimensions before
  work and return a typed error.

  **Validation.** Pin strict equality for both thresholds, seedless low islands, and
  low-footprint growth. A parent over `max_area` containing several legal children must
  emit them; an oversized unsplit leaf must be rejected/flagged explicitly. Invalid
  margins must fail before buffers or diagnostics change.

- [ ] **P1 — Make parallel CCL labels and downstream tie order independent of scheduler timing.**

  **Contract.** Section 8.1 requires row-major first-contact labels, and section 14.3
  requires exact catalog order across thread counts (`03-star-detection.md:616` and
  `03-star-detection.md:1181`).

  **Evidence.** Small masks are labeled sequentially in row order at
  `../star_detection/labeling/mod.rs:362`. Large masks process horizontal strips in
  parallel with one shared atomic union-find at
  `../star_detection/labeling/mod.rs:419`. Each strip obtains provisional labels from
  a global `fetch_add` at `../star_detection/labeling/mod.rs:751`, and final dense IDs
  are assigned by provisional numeric order at
  `../star_detection/labeling/mod.rs:818`, not by the component's first row-major
  pixel. Candidate work is then parallel and equal-flux sorting preserves whatever
  prior order reached the filter.

  **Impact.** Atomic uniqueness is not deterministic identity: different strip
  schedules can associate the smallest provisional ID with a different component.
  Most unequal-flux catalogs are hidden by the later sort, but equal/tied sources,
  source IDs, deblend first-eight choices, and exact serialized order can vary with
  thread count.

  **Change.** Give every component a deterministic key such as its minimum linear
  pixel index after boundary union, sort roots by that key, and then dense-relabel.
  Carry the key through candidates and use complete stable tie keys for peaks, fits,
  and catalog ordering. Keep parallel discovery, but never use atomic allocation order
  as semantic order.

  **Validation.** Run exhaustive small masks and large adversarial strip-boundary masks
  under thread counts `1, 2, 3, 8` and repeated schedules. Require bit-identical label
  maps, source IDs, ownership, and catalog order, including equal peaks and equal flux.

## Batch 3 — replace deblending shortcuts with one correct ownership model

- [ ] **P0 — Compute local maxima and prominence in the component's standardized residual domain, including plateaus.**

  **Contract.** Sections 8.2 and 9.4 require plateau maxima in normalized `Z`, saddle
  prominence, deterministic peak representation, and component-constrained topology
  (`03-star-detection.md:637` and `03-star-detection.md:741`).

  **Evidence.** Local deblending takes the raw prepared image rather than normalized
  detection values at `../star_detection/deblend/local_maxima/mod.rs:78`. Prominence is
  `raw_global_peak * min_prominence`, with no background subtraction, at
  `../star_detection/deblend/local_maxima/mod.rs:93`. A maximum must be strictly above
  all eight raw-image neighbors at `../star_detection/deblend/local_maxima/mod.rs:116`;
  neighbor labels are not checked, so four-connectivity mode can let a diagonally
  separate component suppress a peak. Once eight entries fill, later separated peaks
  are ignored before the final brightness sort at
  `../star_detection/deblend/local_maxima/mod.rs:141`.

  **Impact.** A background pedestal makes fractional prominence too permissive, a
  gradient changes split decisions, flat/saturated peaks disappear into the one-object
  fallback, and the eight retained peaks are scan-order-biased rather than the eight
  best. The configured CCL connectivity is not consistently respected.

  **Change.** Pass normalized residual/significance and the component footprint into a
  shared peak finder. Flood equal-valued plateaus, compare only neighbors permitted by
  configured connectivity, derive one deterministic plateau seed, and compute saddle
  prominence. When bounded to eight leaves, rank all candidates by documented quality
  before truncation and flag overflow.

  **Validation.** Hand-pin plateaus, saturation mesas, background offsets/gradients,
  diagonal four-connectivity components, separation equality, and more than eight
  peaks in adversarial scan order. Adding a constant pedestal must not change the leaf
  set.

- [ ] **P0 — Rebuild multithreshold contrast from background-subtracted branch flux at the merge saddle.**

  **Contract.** Section 9.2 defines levels in the standardized residual domain and
  branch contrast from nonnegative background-subtracted flux above the branch merge
  level relative to parent total flux (`03-star-detection.md:679`).

  **Evidence.** The implementation derives its low/high ladder from the minimum and
  maximum raw component pixels at
  `../star_detection/deblend/multi_threshold/mod.rs:394`, then thresholds raw values at
  `../star_detection/deblend/multi_threshold/mod.rs:461`. Root flux is the sum of all
  raw pixels at `../star_detection/deblend/multi_threshold/mod.rs:537`; child flux is
  the raw sum of only those pixels surviving the current, higher isophote at
  `../star_detection/deblend/multi_threshold/mod.rs:674`. These non-comparable sums are
  tested against one root-relative limit at
  `../star_detection/deblend/multi_threshold/mod.rs:758`. Internal BFS is always
  eight-connected at `../star_detection/deblend/multi_threshold/mod.rs:933`, regardless
  of `DetectionConfig::connectivity`.

  **Impact.** Background area dominates the root denominator, while child sums shrink
  with whichever threshold first exposes a split. Contrast therefore depends on sky
  pedestal, component footprint, and ladder discretization rather than source branch
  flux. Valid faint companions are suppressed unpredictably, and selecting
  four-connectivity does not control tree topology.

  **Change.** Either port the complete SEP/SExtractor tree semantics or implement the
  document's explicitly different standardized tree. Store merge level, parent/child
  relation, integrated branch flux above the saddle, and overflow flags. Thread the
  configured connectivity through every region operation. Cross-check the result
  against a small independent reference before optimizing it.

  **Validation.** Sweep background pedestal, gradient, ladder level count, contrast,
  connectivity, separation, and flux ratio on exact synthetic blends. Pedestal changes
  must preserve branch decisions; increasing level count must converge rather than
  reorder branches. Keep separate fixtures for documented Lumos behavior and pinned SEP
  compatibility if both modes are offered.

- [ ] **P0 — Preserve saddle-aware ownership and consume it in grouped measurement.**

  **Contract.** Sections 9.3 and 9.5 require one owner per parent pixel, deterministic
  saddle-aware gathering, and simultaneous or neighbor-masked measurement for crowded
  groups (`03-star-detection.md:725` and `03-star-detection.md:748`).

  **Evidence.** Both deblenders end in squared-Euclidean nearest-peak assignment at
  `../star_detection/deblend/mod.rs:92`; the multithreshold tree explicitly discards its
  hierarchy before calling the same helper at
  `../star_detection/deblend/multi_threshold/mod.rs:807`. `Region` retains only bbox,
  peak, peak value, and area. The measurement stage passes each region independently to
  `measure_star` at `../star_detection/detector/stages/measure.rs:18`, and
  `compute_star` reads every pixel in the complete square stamp at
  `../star_detection/centroid/mod.rs:743` without consulting bbox, label, or owner.

  **Impact.** A nominal split does not isolate either source scientifically. Voronoi
  cuts can cross isophotes, but even those cuts are ignored while centroid, flux, FWHM,
  and shape absorb the neighbor. Close-star biases persist and can then trigger the
  fixed duplicate filter, erasing one legitimate deblend.

  **Change.** Make deblend output an ownership/segmentation product plus blend-group
  metadata. Gather ambiguous pixels according to the documented saddle-aware rule.
  Fit overlapping sources jointly with shared/local background and full covariance;
  where grouped fitting is bounded out, mask neighbor-owned samples, require usable
  template information, and set an explicit crowded/fallback flag.

  **Validation.** For every parent, assert exactly one owner per usable pixel, exact
  area conservation, peak containment, and thread-independent ownership. Across two-
  and three-star separation/contrast sweeps, grouped measurement must reduce positional
  bias relative to the current independent full-stamp measurements.

- [ ] **P2 — Remove the unsafe generation-grid complexity from the deblender until the reference algorithm is correct.**

  **Why overengineered.** The optional multithreshold implementation is roughly one
  thousand lines and maintains two generation-stamped grids, recycled nested vectors,
  unchecked neighbor traversal, and a `u32` BFS queue. That machinery begins at
  `../star_detection/deblend/multi_threshold/mod.rs:30` and drives unchecked traversal
  at `../star_detection/deblend/multi_threshold/mod.rs:882`, while the core branch flux,
  connectivity, truncation, and ownership semantics above remain incorrect.
  `find_connected_regions_grid_into` stops after the first eight scan-order regions at
  `../star_detection/deblend/multi_threshold/mod.rs:856`, so the optimization also
  bakes a selection bias into behavior.

  **Impact.** The largest and least safe code in this stage protects allocation and
  hash costs of an algorithm whose scientific output cannot yet be trusted. It creates
  several representations of the same pixels and makes corrections harder to review.

  **Change.** First implement a compact scalar/reference deblender using explicit
  component-local arrays and deterministic labels, shared with local-maxima peak and
  ownership logic. Keep one bounded allocation per worker if profiling requires it,
  but avoid generation counters and unchecked access until correctness tests define
  exact behavior. Reintroduce targeted storage optimization only from release-profile
  evidence on realistic crowded components.

  **Validation.** Require exact equivalence between reference and optimized outputs for
  exhaustive tiny components and property-generated larger components before enabling
  an optimized backend. Benchmark component-size/density distributions from real data,
  including the current overflow/truncation cases, and record the crossover that
  justifies each retained optimization.

## Batch 4 — make centroid, PSF, and quality measurements defensible

- [ ] **P0 — Correct the windowed-centroid update and make convergence/failure explicit.**

  **Contract.** Section 10.5 requires signed masked samples, the factor-two update,
  movement/iteration acceptance, and a position covariance or flagged failure
  (`03-star-detection.md:855`).

  **Evidence.** `measure_star` repeats an ordinary weighted mean for a fixed number of
  iterations and exits early on movement at
  `../star_detection/centroid/mod.rs:350`; it records no converged flag and accepts the
  last iterate after all ten iterations. `refine_centroid` rectifies residuals and
  returns the weighted mean directly rather than the factor-two window correction at
  `../star_detection/centroid/mod.rs:524` and
  `../star_detection/centroid/mod.rs:543`.
  Stamp validity computes `width - stamp_radius` and `height - stamp_radius` without
  first checking the dimensions at `../star_detection/centroid/mod.rs:91`, so a tiny
  but otherwise constructible image can underflow/panic before returning `None`.

  **Impact.** The fallback centroid has a different fixed point and noise bias from the
  documented estimator, and nonconvergence is indistinguishable from success. Tiny
  image input reaches an arithmetic panic even though the public API presents detection
  as infallible.

  **Change.** Return a named centroid result with status, iterations, covariance, and
  movement. Use signed usable samples and the documented factor-two update, enforce the
  seed movement bound, and reject/flag iteration limit. Validate stamp geometry with
  checked comparisons such as `width >= 2 * radius + 1` before subtracting.

  **Validation.** Hand-compute one and multiple update steps, force convergence,
  iteration limit, zero weight, singular covariance, excessive motion, and every
  boundary size. Dense subpixel-phase simulations must show declared bias and
  covariance coverage rather than only “within a broad tolerance.”

- [ ] **P0 — Tighten PSF models and acceptance; do not use position-only convergence for shape measurements.**

  **Contract.** Sections 6.2 and 10 require pixel-integrated elliptical models,
  finite/boundary/residual/reduced-chi-square gates, positive-definite covariance, and
  `beta > 1` for a finite-total-flux 2-D Moffat (`03-star-detection.md:455`,
  `03-star-detection.md:810`, and `03-star-detection.md:840`).

  **Evidence.** Gaussian is axis-aligned and Moffat is circular/fixed-beta; both
  evaluate at pixel centers in
  `../star_detection/centroid/gaussian_fit/mod.rs:80` and
  `../star_detection/centroid/moffat_fit/mod.rs:169`. Configuration accepts any finite
  `0 < beta <= 10` at `../star_detection/config.rs:63`. Validation checks mainly stamp
  displacement and scale bounds at
  `../star_detection/centroid/gaussian_fit/mod.rs:260` and
  `../star_detection/centroid/moffat_fit/mod.rs:362`; production ignores residual,
  iterations, amplitude, background, and covariance. Most importantly, `measure_star`
  enables a position-only LM early exit and then consumes sigma/alpha as FWHM and
  eccentricity at `../star_detection/centroid/mod.rs:419`, while the optimizer declares
  convergence as soon as only `delta[0:2]` are small at
  `../star_detection/centroid/lm_optimizer.rs:294`.

  **Impact.** Width/background/amplitude may still be moving when the fit is labeled
  converged, yet those widths drive matched-filter diagnostics and rejection. Pixel
  phase biases undersampled fits; rotated/elliptical PSFs are forced into restricted
  models; and no uncertainty tells registration how much to trust the accepted center.

  **Change.** Implement pixel-integrated elliptical Gaussian/Moffat models with angle
  and valid beta semantics. Separate centroid-only optimization from a full-shape fit:
  if shape is consumed, require all relevant scaled parameters, gradient, and objective
  to converge. Compute the final normal-matrix covariance, apply residual/reduced-chi²,
  boundary, finite, and positive-definite gates, and preserve status/diagnostics in the
  source catalog.

  **Validation.** Cross-check every analytic derivative against high-accuracy central
  differences. Sweep pixel phase, undersampling, rotation, axis ratio, beta, SNR,
  background, masks, and parameter boundaries. Force each optimizer exit and prove
  only genuine full fits populate fit-derived FWHM/covariance; repeated noise draws
  must calibrate the reported 2-D positional uncertainty.

- [ ] **P1 — Replace fixed saturation, one-sided FWHM filtering, misleading shape names, and fixed-radius deduplication.**

  **Contract.** Section 11 requires metadata/mask-driven saturation, explicitly defined
  shape metrics, a robust two-sided stellar PSF consistency check, and duplicate
  selection based on uncertainty with a local-FWHM-scaled radius
  (`03-star-detection.md:915`, `03-star-detection.md:928`,
  `03-star-detection.md:956`, and `03-star-detection.md:991`).

  **Evidence.** Saturation is a fixed normalized `0.95` raw derived-plane peak at
  `../star_detection/star.rs:33` and is applied before all other filters at
  `../star_detection/detector/stages/filter.rs:36`. `roundness1/2` claim DAOFIND
  semantics in `../star_detection/star.rs:24`, but the implementation uses marginal
  maxima and a nonnegative bilateral-asymmetry hypotenuse at
  `../star_detection/centroid/mod.rs:867`. FWHM reference comes from the brightest half
  and rejects only values above `median + k*MAD` at
  `../star_detection/detector/stages/filter.rs:75`. Duplicate removal keeps the first
  flux-sorted source within one global configured distance at
  `../star_detection/detector/stages/filter.rs:94`.

  **Impact.** Non-normalized/high-dynamic-range data receive false saturation decisions;
  median filtering can hide saturation; narrow cosmic/hot residuals survive the FWHM
  stage; and close legitimate binaries/deblended sources can be deleted because the
  brightest rather than most precise measurement wins. Metric names invite callers to
  use thresholds from algorithms they do not implement.

  **Change.** Use the permanent saturation mask and metadata clipping limit on original
  source samples. Rename current shape metrics to their actual formulas or implement
  exact DAOFIND definitions on the correct planes. Build a robust isolated stellar PSF
  model and reject both narrow and broad inconsistent sources with flags. Deduplicate
  only detections of the same physical peak across scales/plateaus, use local FWHM and
  covariance, and never collapse distinct ownership leaves solely by distance.

  **Validation.** Sweep pixel scale/normalization and clipped plateaus for saturation;
  pin the exact named shape formula against an independent reference; inject narrow and
  broad outliers around a known FWHM distribution; and verify close binaries survive
  while deliberate multi-scale duplicates collapse to the lowest-variance entry.

- [ ] **P2 — Collapse model-specific handwritten SIMD until the scalar scientific model is complete and profiled.**

  **Why overengineered.** Gaussian and Moffat fitting each duplicate normal-equation
  and chi-square accumulation across scalar, AVX2, and NEON implementations. Dispatch
  starts at `../star_detection/centroid/gaussian_fit/mod.rs:153` and
  `../star_detection/centroid/moffat_fit/mod.rs:232`; the architecture-specific files
  include their own exponential/power approximations and dozens of explicit Hessian
  accumulators. Weighted fitting bypasses those SIMD overrides and returns to the
  scalar default at `../star_detection/centroid/lm_optimizer.rs:198`. Meanwhile the
  production result intentionally discards fit amplitude/background/residual/iterations
  at `../star_detection/centroid/gaussian_fit/mod.rs:62`, and no covariance or complete
  acceptance contract exists.

  **Impact.** Thousands of lines and four unsafe architecture backends multiply review
  and numerical-equivalence burden around a center-sampled restricted model whose
  scientific output is incomplete. Future pixel integration, rotation, masks, and
  covariance will otherwise require another round of synchronized kernel rewrites.

  **Change.** Keep one f64 scalar/reference LM path while correcting model and result
  semantics. Share generic accumulation and use compiler-vectorizable structure or a
  single portable vector abstraction only after profiling the complete weighted model.
  Reintroduce specialized kernels solely for a measured dominant operation, with the
  scalar implementation remaining the correctness oracle.

  **Validation.** Establish end-to-end profiles by centroid method, stamp size, star
  count, and noise-model mode before and after simplification. Any retained optimized
  kernel must match scalar parameters, objective, convergence status, covariance, and
  accept/reject decision across randomized and boundary fixtures, not only individual
  exponential values.

## Batch 5 — bound configuration and make diagnostics actionable

- [ ] **P1 — Validate image-dependent geometry and bound every size-derived arithmetic operation.**

  **Contract.** Sections 2, 6, 8, 10, and 14 require invalid geometry/configuration to
  return expected errors and require bounded kernel, tree, stamp, and capture sizes.

  **Evidence.** `DetectionConfig::validate` requires nonzero deblend separation but
  gives it and `edge_margin` no upper bound at
  `../star_detection/config.rs:281`. Local and multithreshold code squares separation
  with unchecked `usize` arithmetic at
  `../star_detection/deblend/local_maxima/mod.rs:96` and
  `../star_detection/deblend/multi_threshold/mod.rs:387`. Edge validation computes
  `2 * edge_margin` at `../star_detection/detector/stages/detect.rs:150`. Stamp bounds
  subtract radius from dimensions at `../star_detection/centroid/mod.rs:103`. The
  public detector neither validates dimensions nor returns errors.

  **Impact.** Legal public configuration values can overflow in debug builds, wrap in
  release builds, or turn intended rejection into arbitrary behavior. Small images can
  panic during measurement. These are ordinary external configuration/image failures,
  not internal logic errors.

  **Change.** Validate bounded physical ranges at configuration construction and use
  checked/image-relative validation before detection. Express squared distances in a
  wider checked type or compare without overflow. Derive a single required margin from
  convolution support, deblend neighborhood, annulus, and fit stamp, and report which
  constraint makes an image unusable.

  **Validation.** Exercise `usize::MAX`, square-root boundary values, zero/tiny images,
  kernels larger than images, and exact valid/invalid margin boundaries in debug and
  release. Every case must return the same typed result without overflow or panic.

- [ ] **P1 — Expand diagnostics from aggregate funnel counts to stage health and per-source reasons.**

  **Contract.** Section 14.2 requires timing, valid/masked/interpolated/saturated counts,
  mesh repair/RMS summaries, null calibration, PSF source/range, deblend failure and
  overflow, fit/fallback/covariance failures, rejection counts, and optional debug
  products (`03-star-detection.md:1159`).

  **Evidence.** `Diagnostics` records threshold pixels, component/candidate/deblend
  counts, post-centroid count, filter totals, medians, and FWHM bootstrap provenance at
  `../star_detection/detector/mod.rs:53`. `DetectResult` counts a component as
  “deblended” only when more than one region is returned at
  `../star_detection/detector/stages/detect.rs:28`; it has no failed/overflow/too-complex
  state. `filter_map` silently drops every measurement failure at
  `../star_detection/detector/stages/measure.rs:27`.

  **Impact.** An empty/weak catalog cannot be attributed to invalid background RMS,
  swallowed parents, fit nonconvergence, edge/stamp rejection, peak overflow, or
  cancellation latency. Aggregate tuning can therefore improve one hidden failure mode
  while worsening another, and production incidents cannot reproduce the stage that
  lost sources.

  **Change.** Use typed stage outcome counters and per-source flags from Batch 1. Add
  inexpensive timings and distribution summaries; capture large maps/labels only on
  request. Distinguish “not split,” “split,” “overflow,” “too complex,” and algorithm
  failure. Make every `None` measurement path produce a reason before catalog selection.

  **Validation.** Force each failure/rejection branch with one controlled fixture and
  assert exact counters plus source flags. Verify optional captures are absent by
  default, bounded when enabled, and do not change numerical results or ordering.

- [ ] **P2 — Correct the stale spline-edge statement in the implementation audit.**

  **Documentation error.** Section 15.4 says “the spline edge behavior can extrapolate
  before the first tile center” at `03-star-detection.md:1291`.

  **Evidence.** Y interpolation explicitly clamps `ty` to `[0,1]` at
  `../star_detection/background/mod.rs:176`. X passes a possibly negative starting
  parameter, but the scalar and SIMD segment evaluators clamp every lane before
  evaluating the cubic at `../star_detection/background/simd/mod.rs:117` and
  `../star_detection/background/simd/mod.rs:174`. Thus pixels before the
  first center receive the first-node value; they do not extrapolate.

  **Change.** Remove only the extrapolation clause. Replace it with the actual remaining
  issue: sigma is interpolated directly and is not validated/floored as a positive
  quantity. Keep the separate statements about invalid tiles, small-grid median
  filtering, and approximate even-count medians.

  **Validation.** Add/pin a nonconstant two-axis grid fixture proving constant extension
  before/after the first/last centers and a separate fixture demonstrating the required
  positive-RMS invariant.

## Recommended implementation order

1. Batch 1: define the input bundle, error/cancellation API, immutable measurement
   planes, variance semantics, and rich raw catalog. Every later algorithm depends on
   these types.
2. Batch 2: make background/RMS valid, implement the variance-aware statistic and
   hysteresis, and make CCL/component behavior deterministic.
3. Batch 3: replace both deblending paths with a shared standardized peak/tree/owner
   model and consume ownership in grouped measurement.
4. Batch 4: correct centroid and PSF measurement/acceptance, then replace misleading
   filters and only afterward decide which fit kernels deserve specialization.
5. Batch 5: close overflow/geometry paths, expand diagnostics, and update the one stale
   audit statement throughout the work.

Do not retune thresholds, presets, FWHM limits, or duplicate radii against the current
derived-plane catalog. Their numerical meaning will change after the variance,
measurement-plane, ownership, and covariance corrections.

## Open questions

- [ ] **Choose the primary source-flux contract.** Is star detection intended to emit
  scientifically reusable per-band/model photometry, or only a registration-quality
  amplitude? The former requires calibrated channel response and full covariance; the
  latter should be named as a registration amplitude rather than `total flux`. This
  decision changes the `DetectedSource` schema but not the need to stop measuring the
  median-filtered detection plane.

- [ ] **Choose compatibility versus Lumos-specific multithreshold behavior.** If exact
  SEP/SExtractor compatibility is a product goal, port and pin the complete branch
  gathering/cleaning/flag behavior. If not, name the algorithm differently and make the
  standardized-residual/saddle-ownership equations in section 9 the only contract.

- [ ] **Set explicit runtime targets before retaining specialized SIMD fits.** Record
  the maximum acceptable detection time by sensor size, expected star count, and
  centroid mode. Without that budget and profiles of the corrected weighted model,
  there is no evidence that four architecture-specific fit kernels are the right
  complexity to keep.
