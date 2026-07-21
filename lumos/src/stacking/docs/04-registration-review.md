# Registration specification versus implementation review

Date: 2026-07-21

Scope: production code implementing [`04-registration.md`](04-registration.md),
including catalog preparation, triangle construction and voting, robust transform
estimation, guided match recovery, SIP/TPS distortion, transform/result contracts,
inverse-mapped resampling, the RAM and streaming pipeline handoffs, and the public
drizzle direction seam. Test-only, benchmark, fixture, and real-data-test code was
not reviewed.

## Outcome

The document is a strong target specification and its dated implementation audit is
mostly accurate. All four P0 gaps and the fourteen P1 gaps in §15 are confirmed by
the production code. The implementation has useful foundations—consistent
reference-to-target transform direction, local triangle generation, bounded RANSAC,
normalized homography DLT, deterministic vote tie-breaking, single-pass pull
resampling, multiple reconstruction kernels, and separate support/confidence maps—but
it does not yet satisfy the scientific registration contract in §§1–14.

The most serious additional defect is not stated explicitly in §15: configured
`min_matches` is checked only against the triangle-vote seed list. RANSAC may retain
only the model's two-to-four minimal inliers, recovery may leave that set unchanged,
and `register` will accept it if its training RMS is below the fixed pixel ceiling.
A nearly unconstrained transform can therefore be reported as successful despite the
default eight-match acceptance setting.

The major correctness risks are:

1. SIP learns a target-frame residual but applies it as a pre-linear correction, so
   any non-identity rotation, scale, shear, or projective base transform maps the
   fitted vector through the wrong basis.
2. Registration ignores centroid covariance, source flags, and the full valid
   catalogs. Seeing width is substituted for astrometric uncertainty, then used as
   one global pixel threshold for every observation.
3. The progressive weighted sampler can terminate with the confidence formula for
   uniform sampling before it ever reaches its uniform phase.
4. Final acceptance is training-RMS-only. It does not enforce the configured final
   match count, spatial span, PSF-derived blur budget, held-out performance, overlap,
   or transform-domain validity.
5. `warp` cannot represent a reference/union/intersection/explicit output grid and
   copies source dimensions and metadata into reference-coordinate output pixels.
6. Warp quality is incomplete: no variance or masks are propagated, partial-kernel
   fallback is not identified, material minification is not anti-aliased, and no
   photometric/Jacobian convention is declared.

The recommended work below is organized by dependency. It deliberately places
statistical and geometric correctness before further SIMD or nonlinear-model work.

## Current production flow

```text
flux-sorted Vec<Star>
  -> validate only finite position and finite FWHM
  -> take first max_stars entries
  -> local k-neighbor triangles
  -> raw sorted-side ratios + fixed 0.01 invariant box
  -> unnormalized integer votes
  -> greedy one-to-one seed selection
  -> weighted top-quarter / top-half / uniform RANSAC phases
  -> global seeing-derived Euclidean residual loss
  -> unweighted least-squares local/final refit
  -> greedy nearest-neighbor recovery on the same truncated catalogs
  -> optional fixed-order SIP from target-frame residual vectors
  -> accept by final training RMS <= fixed pixel ceiling
  -> RegistrationResult { transform, optional SIP, scalar residuals, elapsed time }
  -> allocate output with source dimensions and source metadata
  -> warp each channel independently
  -> make coverage/confidence in a separate transform pass
  -> stack any sample with coverage > 1e-3 and confidence > 0
```

The stored `Transform` maps reference/output coordinates to target/source
coordinates, as the pull warp requires. Drizzle instead consumes an input-to-common
forward transform, but the public types do not name or bundle that inverse direction.

## Contract coverage

| Specification area | Production status | Main evidence |
|---|---|---|
| Stable `StarObservation` with centroid covariance and flags | Missing | registration accepts detector `Star` directly |
| Spatially distributed matching catalog and full recovery catalog | Missing | first `max_stars` flux-sorted entries are used for every stage |
| Uncertainty-aware triangle invariant | Missing | raw side ratios and fixed absolute tolerance |
| Global correspondence assignment | Missing | greedy vote and recovery conflict handling |
| Parity branches and bounded retries | Missing | one optional orientation-equality check, one attempt |
| Statistically valid RANSAC termination | Incorrect | weighted progressive draws use uniform early-stop formula |
| Exact MAGSAC++ and covariance-weighted final fit | Missing | inspired scalar loss plus binary-inlier unweighted refits |
| Configured final match minimum | Incorrect | gate applies only before RANSAC |
| Full-domain transform validation | Missing | one matrix determinant plus origin-derived scale/rotation |
| PSF/spatial held-out model selection | Missing | first rung with training RMS at most 0.5 px |
| Correct pre-transform SIP target | Incorrect | target-frame residual fitted before the base transform |
| Stable robust SIP solve and order selection | Missing | fixed order, normal equations, pointwise clipping |
| Explicit output geometry and output metadata | Missing | source dimensions and metadata are copied |
| Variance, mask, and resampling-correlation propagation | Missing | only value, coverage, and white-noise confidence are returned |
| Actual-kernel support and fallback flags | Incorrect | partial Lanczos value uses bilinear while coverage remains Lanczos |
| Anti-aliasing and photometric Jacobian policy | Missing | fixed reconstruction kernels only |
| Two-pass sequence reference and global refinement | Missing | maximum raw star count only, direct pairwise transforms only |
| Actionable retained diagnostics | Missing | pipeline retains only accepted count and dropped indices |

## Batch 1 — make a successful registration scientifically meaningful

- [ ] **P0 — Enforce `min_matches` on the final recovered assignment, not only on triangle seeds.**

  **Contract.** Guided recovery and final acceptance require a one-to-one assignment
  with the configured match minimum and two-dimensional spatial span
  (`04-registration.md:775` and `04-registration.md:968`).

  **Evidence.** `RegistrationMatchingConfig` describes `min_matches` as the minimum
  accepted pair count at `../registration/config.rs:94`, but `register` checks it only
  against the pre-RANSAC vote output at `../registration/mod.rs:160`. RANSAC accepts
  `best_inliers.len() >= min_samples` at
  `../registration/ransac/mod.rs:409`, where `min_samples` is only two, three, or four.
  After recovery there is no count or span gate; the only final check is RMS at
  `../registration/mod.rs:187`.

  **Impact.** Eight voted seed edges can collapse to two Euclidean inliers or four
  homography inliers and still produce an accepted zero/low-training-residual result.
  Such a model has no redundancy, unreliable extrapolation, and can sharply misalign
  most of the image while reporting excellent RMS.

  **Change.** Make final acceptance consume a named validation result and require at
  least `matching.min_matches` final one-to-one inliers, the model-specific larger
  minimum when applicable, configured convex-hull/grid coverage, and nondegenerate
  two-dimensional span. Apply the gate after every recovery/refit/SIP pass. Do not
  use the pre-RANSAC edge count as evidence that the delivered model is constrained.

  **Validation.** Construct seed lists with eight voted edges but only the minimal
  geometrically consistent subset. Assert typed rejection for Euclidean, similarity,
  affine, and homography. Hand-check the exact boundary at `min_matches - 1`,
  `min_matches`, collinear final matches, and adequate count with inadequate hull
  coverage.

- [ ] **P0 — Replace the `&[Star]` registration boundary with validated observations carrying covariance, flags, and stable source identity.**

  **Contract.** §§2.2, 2.5, and 4.1 require stable catalog indices, positive-definite
  centroid covariance, source flags, flux uncertainty, and spatial/uncertainty-based
  selection (`04-registration.md:118`, `04-registration.md:198`, and
  `04-registration.md:293`).

  **Evidence.** Detector `Star` has no covariance, flags, flux uncertainty, or stable
  catalog field at `../star_detection/star.rs:8`. Registration validates only finite
  position and finite—not positive—FWHM at `../registration/mod.rs:197`, derives one
  noise scale as `max(median_fwhm / 2, 0.5)` at `../registration/mod.rs:134`, and
  discards every property except position after taking the first flux-sorted entries
  at `../registration/mod.rs:138`.

  **Impact.** A high-S/N 0.03-pixel centroid and a marginal 0.5-pixel centroid have
  equal influence, while the global threshold represents PSF width rather than
  centroid error. Saturated, failed-fit, edge-truncated, and otherwise unsuitable
  sources cannot be excluded by a typed contract. Invalid negative FWHM can also
  enter the median and silently fall through to the 0.5-pixel floor.

  **Change.** Introduce `RegistrationObservation` and `RegistrationInputCatalog` inputs
  that retain original indices and validated covariance/flags. Until Stage 3 emits a
  measured covariance, use the explicitly flagged compatibility estimate in §2.2 and
  calibrate its inflation. Carry covariance through triangles, residuals, recovery,
  fitting, and result diagnostics; reject invalid observations individually with
  recorded counts instead of failing an otherwise usable catalog on the first bad
  entry.

  **Validation.** Use analytical anisotropic covariance cases and Monte Carlo centroid
  draws to verify Mahalanobis residuals. Assert exact hard-flag filtering, stable
  original indices after selection, negative/nonfinite covariance rejection, and
  that a low-uncertainty source influences the fitted transform more than a noisy one
  by the hand-computed matrix weight.

- [ ] **P0 — Replace scalar-RMS-only output with the complete solution, residual, uncertainty, and stage-diagnostic contract.**

  **Contract.** §§2.4, 10, and 14.1 require residual vectors, Mahalanobis distances,
  robust weights, parameter/mapping uncertainty, spatial residual summaries, domain
  diagnostics, actual RNG seed, and typed stage failures (`04-registration.md:165`,
  `04-registration.md:939`, and `04-registration.md:1370`).

  **Evidence.** `StarMatch` stores only two indices and scalar distance at
  `../registration/result/mod.rs:182`; `RegistrationResult` stores only the base
  transform, optional SIP fit, matches, and elapsed time at
  `../registration/result/mod.rs:193`. Although four `RansacFailureReason` variants
  exist at `../registration/result/mod.rs:35`, every estimator failure is emitted as
  `NoInliersFound` with configured maximum iterations and zero best inliers at
  `../registration/mod.rs:290`. The estimator's actual iteration count is retained in
  an otherwise unread field at `../registration/ransac/mod.rs:173`.

  **Impact.** Callers cannot distinguish degeneracy from estimator exhaustion, audit
  coherent residual vectors, evaluate field extrapolation, reproduce an unseeded
  result, or determine whether a low global RMS hides a corner failure. The ad hoc
  `quality_score` at `../registration/result/mod.rs:261` compresses the only two
  available metrics and can be mistaken for an acceptance probability.

  **Change.** Return a `RegistrationSolution` matching §2.4 and a structured failure
  carrying stage counts. Record the concrete seed chosen even when the caller supplied
  `None`, actual attempts/degeneracies/best score, raw and standardized residual
  vectors, assignment identity, parameter covariance, spatial cells, domain bounds,
  and every hard acceptance gate. Keep UI quality as a derived convenience only.

  **Validation.** Force each documented failure stage and assert its exact reason and
  counts. Recompute RMS/vector summaries independently from returned matches; replay
  with the recorded seed and require identical assignments, parameters, and
  diagnostics.

- [ ] **P0 — Replace fixed training-RMS model selection with the PSF, held-out, spatial, overlap, and full-domain acceptance ladder.**

  **Contract.** The accuracy objective, domain proof, model selection, and acceptance
  rules are specified in §§1.2, 7.6, 7.7, and 10.3
  (`04-registration.md:37`, `04-registration.md:719`,
  `04-registration.md:739`, and `04-registration.md:968`).

  **Evidence.** `Auto` accepts the first Euclidean/Similarity/Affine rung with training
  RMS at most the hard-coded 0.5 pixels and otherwise returns Homography at
  `../registration/mod.rs:229` and `../registration/mod.rs:249`. The final default
  ceiling is a fixed 2.0 pixels at `../registration/config.rs:209` and
  `../registration/config.rs:228`. Plausibility reads one rotation and one first-column
  scale at `../registration/ransac/mod.rs:190`; general transform validity checks only
  finiteness and one determinant at `../registration/transform.rs:321`.

  **Impact.** Model selection depends on sampling in pixels, not allowable PSF
  broadening. Flexible models are judged on the data that fit them and can win by
  absorbing centroid noise. A nonsingular homography with a horizon, scale explosion,
  parity change, negligible target overlap, or uncontrolled corner behavior can pass.

  **Change.** Evaluate the documented model ladder on one frozen recovered assignment
  using deterministic spatial folds. Require PSF-derived RMS/p95 gates, standardized
  goodness-of-fit, minimum spatial coverage/overlap, and conservative full-domain
  Jacobian/denominator/parity bounds. Upgrade only on the specified held-out
  improvement; fail if no model passes rather than returning Homography by default.

  **Validation.** Render known PSFs and verify the analytical blur gate. Use exact
  Euclidean, scale, shear, and projective truths plus noise-only controls to verify
  model choice. Exercise a horizon just outside/inside the image, a fold between grid
  samples, extreme corner scale, wrong parity, and insufficient overlap.

## Batch 2 — make matching and robust estimation statistically valid

- [ ] **P0 — Build spatially distributed asterism catalogs, retain full catalogs for recovery, and add bounded retry/parity branches.**

  **Contract.** §§4.1, 5.5, and 5.6 require uncertainty-ranked spatial selection,
  separate parity hypotheses, and retries that change one recall dimension at a time
  (`04-registration.md:293`, `04-registration.md:455`, and
  `04-registration.md:472`).

  **Evidence.** Both inputs are truncated to their first `max_stars` at
  `../registration/mod.rs:138`, and that same truncated position list is passed to
  triangle matching and recovery. There is exactly one `match_triangles` call at
  `../registration/mod.rs:152`. `TriangleConfig` exposes only a boolean
  orientation-equality check at `../registration/triangle/mod.rs:24`; voting discards
  opposite orientation at `../registration/triangle/voting.rs:176` rather than
  evaluating an explicit reflected branch.

  **Impact.** A bright central cluster can leave scale/shear/distortion unconstrained
  at the field edge, while faint but precise or spatially valuable stars are
  unavailable even after a good mapping exists. Mirrored data cannot register, and a
  single unlucky catalog size/tolerance/neighborhood choice fails without a bounded
  diagnostic retry.

  **Change.** Prepare a small spatially distributed asterism subset plus the full
  validated catalog with stable indices. Implement the staged retry schedule and
  explicit allowed parity branches from the document; keep their votes, RANSAC
  hypotheses, and diagnostics separate. Use optional priors only as recorded gates or
  ranking evidence.

  **Validation.** Create a crowded central cluster with necessary edge controls, a
  sparse-field retry boundary, and preserving/reversing parity fixtures. Assert the
  exact selected original indices, fixed retry order, no cross-parity vote mixing,
  and successful full-catalog edge recovery.

- [ ] **P0 — Replace raw triangle ratios and absolute degeneracy constants with covariance-propagated log invariants and scale-aware role checks.**

  **Contract.** §§5.1–5.3 specify baseline/FWHM/uncertainty gates, normalized area,
  side-order ambiguity rejection, log-ratio covariance, and exact Mahalanobis
  invariant matching (`04-registration.md:336`, `04-registration.md:374`, and
  `04-registration.md:395`).

  **Evidence.** `Triangle` stores only two raw ratios and orientation at
  `../registration/triangle/geometry.rs:18`. It uses a `1e-10` side floor and
  `1e-6` raw Heron area-squared floor at
  `../registration/triangle/geometry.rs:3`, assigns vertex roles by sorted side
  length even when the sides are observationally indistinguishable at
  `../registration/triangle/geometry.rs:47`, and compares each ratio to a fixed
  absolute tolerance at `../registration/triangle/geometry.rs:119`.

  **Impact.** Triangle quality changes with image scale and centroid precision.
  Near-isosceles noise can swap vertex identities across frames, producing confident
  wrong point votes. A global invariant box is simultaneously too wide for precise
  large triangles and too narrow for uncertain small ones.

  **Change.** Store side covariance including shared-vertex cross terms, reject
  ambiguous side ordering, form the documented log invariant and covariance floor,
  use the conservative k-d-tree radius followed by the exact Mahalanobis gate, and
  express baseline/area cuts in FWHM-, uncertainty-, and dimensionless units.

  **Validation.** Hand-propagate one triangle covariance and cross-check it by Monte
  Carlo. Test scale invariance, the exact near-isosceles three-sigma boundary,
  elongated and small-baseline rejection, and invariant-query recall at the
  conservative bounding radius.

- [ ] **P1 — Normalize ambiguous triangle evidence and solve point correspondences globally.**

  **Contract.** §§5.3–5.4 require each triangle's candidate likelihoods to sum to one,
  distinct triangle-pair support, row/column margins, and maximum-weight bipartite
  assignment (`04-registration.md:395` and `04-registration.md:431`).

  **Evidence.** Every accepted triangle pair adds one unweighted integer vote per
  vertex at `../registration/triangle/voting.rs:164`; crowded invariant bins can
  therefore cast unlimited evidence. `resolve_matches` sorts by vote count and greedily
  consumes endpoints at `../registration/triangle/voting.rs:194` and
  `../registration/triangle/voting.rs:227`. Its confidence is only votes divided by
  the maximum resolved vote at `../registration/triangle/voting.rs:244`.

  **Impact.** An ambiguous triangle can dominate many distinctive triangles, and an
  early high-vote edge can block two edges with greater total support. The resulting
  confidence is not an ambiguity measure yet drives weighted RANSAC sampling.

  **Change.** Deduplicate triangle-pair IDs, weight candidates by normalized
  Mahalanobis likelihood, retain distinct support counts, then solve the sparse/dense
  maximum-weight assignment with dummy unmatched nodes. Derive sampling rank from vote
  weight and both local margins; do not present normalized raw count as confidence.

  **Validation.** Hand-construct ambiguous invariant bins and a 2x2/3x3 conflict where
  greedy is suboptimal. Assert exact normalized weights, distinct supports, optimal
  assignment, margins, and identical dense/sparse results.

- [ ] **P0 — Use a sampler whose termination probability matches its actual draws and always record the realized seed.**

  **Contract.** §§7.1, 7.3, and 14.2 require uniform sampling with the corresponding
  finite-population/degeneracy-aware termination rule, or a proven progressive method
  with its own termination, plus a recorded seed (`04-registration.md:538`,
  `04-registration.md:625`, and `04-registration.md:1391`).

  **Evidence.** The first two phases sample with confidence weights from only the top
  quarter and half of edges at `../registration/ransac/mod.rs:33` and
  `../registration/ransac/mod.rs:523`. The loop nevertheless calls the independent
  uniform ratio formula at `../registration/ransac/mod.rs:396`, implemented as
  `(I/M)^s` at `../registration/ransac/transforms.rs:15`, and may break before the
  uniform phase. A missing seed is drawn randomly at
  `../registration/ransac/mod.rs:151` but is never returned.

  **Impact.** The advertised `confidence` has no statistical meaning for early
  termination and can miss a valid hypothesis concentrated outside the favored pool.
  Failures and marginal successes cannot be reproduced from `RegistrationResult`.

  **Change.** Prefer uniform draws without replacement and the exact finite-population
  nondegenerate-subset probability in §7.3. If progressive sampling is retained,
  implement the named method and its termination proof rather than phase heuristics.
  Generate the concrete seed before constructing the estimator and store it in every
  success/failure diagnostic.

  **Validation.** Enumerate small `M, I, s` sets and compare exact draw counts to the
  formula, including degenerate subsets and confidence endpoints. Demonstrate a case
  whose inliers lie outside the top quarter, exercise the maximum-iteration path, and
  byte-compare replayed seeded results.

- [ ] **P0 — Implement covariance-weighted robust nonlinear refinement instead of combining a MAGSAC name with unweighted binary-inlier least squares.**

  **Contract.** §§7.2, 7.4, and 7.5 require exact MAGSAC++ scoring/weights,
  covariance-whitened nonlinear refinement, stable rectangular solvers, and final
  parameter covariance (`04-registration.md:563`, `04-registration.md:659`, and
  `04-registration.md:676`).

  **Evidence.** The scorer explicitly states that it is not MAGSAC++ at
  `../registration/ransac/magsac.rs:1` and implements a simpler saturating
  exponential at `../registration/ransac/magsac.rs:62`. Local and final optimization
  call the same unweighted `estimate_transform` on hard inliers at
  `../registration/ransac/mod.rs:260` and `../registration/ransac/mod.rs:419`.
  Affine forms and explicitly inverts normal equations at
  `../registration/ransac/transforms.rs:166` and
  `../registration/ransac/transforms.rs:217`; homography DLT is never polished against
  geometric residual.

  **Impact.** The robust score selects hypotheses under one objective, but refinement
  jumps to another objective and equal-weights all accepted centroids. Affine
  conditioning is squared, projective parameters remain an algebraic initializer,
  and no parameter uncertainty is available for mapping or recovery gates.

  **Change.** Either implement exact MAGSAC++/sigma-consensus as specified or rename
  and document a deliberately simpler robust estimator without false guarantees. In
  either case, finish with covariance-whitened robust nonlinear least squares using
  QR/SVD, true objective acceptance, convergence status, and parameter covariance.
  Replace affine normal equations; retain normalized DLT only as homography
  initialization.

  **Validation.** Cross-check loss/weight tables against a high-precision reference,
  solve anisotropic weighted fixtures by hand, verify every accepted refinement step
  lowers the true objective, and show homography geometric error decreases after DLT.
  Compare estimated parameter covariance with repeated synthetic fits.

## Batch 3 — correct recovery and distortion before adding more nonlinear machinery

- [ ] **P0 — Replace order-dependent nearest-neighbor recovery with converged global assignment over the full valid catalogs.**

  **Contract.** §8 requires all gated edges, exact covariance cost, dummy-node global
  assignment, full-catalog recovery, identity-based convergence, and final robust
  refit (`04-registration.md:775`).

  **Evidence.** Recovery builds its tree from the same brightness-capped target slice
  and scans the reference slice in index order at `../registration/mod.rs:393` and
  `../registration/mod.rs:436`. Each source claims only its nearest currently unused
  target at `../registration/mod.rs:443`. It stops when the count is unchanged at
  `../registration/mod.rs:462`, even if pair identities changed, and restores the
  original seed transform/matches whenever legitimate rejection reduces count at
  `../registration/mod.rs:483`.

  **Impact.** Early references can steal better targets from later sources; spatial
  edge/faint controls omitted from asterisms can never be recovered; changed
  assignments can escape without a corresponding refit; and known-bad seed outliers
  may be resurrected solely to preserve count.

  **Change.** Generate every covariance-gated edge from the full catalogs, solve one
  global min-cost assignment with explicit unmatched cost, robustly refit, recalibrate
  the systematic floor, and repeat until both assignment identity and normalized
  parameters converge. Remove the count-preservation fallback and revalidate all seed
  matches under the same final gate.

  **Validation.** Use crossed nearest-neighbor conflicts, a constant-count identity
  swap, rejected seed outliers, and valid edge controls outside `max_stars`. Assert the
  exact optimal assignment each pass and one final refit after convergence.

- [ ] **P0 — Fit SIP corrections in the base transform's pre-linear reference domain.**

  **Contract.** §9.3 requires `T^-1(q) - p`, or joint minimization of
  `T(p + d(p)) - q`, for a correction composed before `T`
  (`04-registration.md:833`).

  **Evidence.** `fit_from_transform` sets polynomial targets to
  `target - transform(reference)` at
  `../registration/distortion/sip/mod.rs:182`. `WarpTransform` later applies that
  correction to the reference point before the base transform at
  `../registration/transform.rs:339` and `../registration/transform.rs:369`.

  **Impact.** For an affine base `T(p)=A p+t`, the current correction produces
  `A(q-T(p))` where the required correction is `A^-1(q-T(p))`; even a pure non-unit
  scale gets the magnitude wrong, and rotation/shear changes the vector direction.
  Identity-plus-distortion fixtures conceal the defect.

  **Change.** Validate base invertibility, initialize targets with
  `T^-1(q)-p` and their propagated covariance, then jointly alternate/refine base and
  polynomial under the composite target-frame residual. Keep the no-constant/no-linear
  constraint and report the exact mapping/export convention.

  **Validation.** Recover each basis term after a known rotation, scale, affine shear,
  and homography. Compare the composite mapping to analytical truth over controls and
  held-out boundary points, and require the existing raw-target-residual construction
  to fail those deliberately non-identity fixtures.

- [ ] **P1 — Replace fixed-order SIP normal equations and pointwise clipping with stable weighted order selection and full-field validation.**

  **Contract.** §§9.4–9.5 require whitened QR/SVD, robust composite refinement,
  five-times-term control density, spatial cross-validation, hull/quadrant coverage,
  and conservative correction/Jacobian domain gates
  (`04-registration.md:862` and `04-registration.md:897`).

  **Evidence.** The configured order is fitted directly and needs only three times the
  term count at `../registration/distortion/sip/mod.rs:164`. Sigma clipping may leave
  only `terms.len()` controls at `../registration/distortion/sip/mod.rs:236`. The fit
  constructs `A^T A` and solves it by Cholesky/LU at
  `../registration/distortion/sip/mod.rs:425`. Diagnostics cover only surviving
  control residuals/corrections, while `max_correction` samples a fixed grid ending at
  `floor(size / spacing) * spacing` and can omit the true boundary at
  `../registration/distortion/sip/mod.rs:353`.

  **Impact.** Polynomial conditioning is squared, outlier rejection ignores
  observation uncertainty, high orders can fit sparse clustered controls, and a low
  training residual can coexist with extreme or folded behavior between controls or
  near an unsampled edge.

  **Change.** Fit normalized rectangular systems by pivoted QR/SVD with covariance and
  robust weights. Try orders upward, require documented spatial support, choose by
  frozen spatial folds, and validate the exact pixel-boundary footprint with adaptive
  conservative Jacobian/correction bounds. Reject nonfinite inputs/output and any
  underconstrained post-clipping fit.

  **Validation.** Use clustered-versus-distributed controls, near-rank-deficient power
  bases, outliers with unequal covariance, a higher-order overfit, and a correction/fold
  peak between coarse nodes. Assert exact boundary inclusion for nonmultiple grid
  spacings.

- [ ] **P2 — Delete the unwired TPS implementation until a validated distortion policy actually selects and consumes it.**

  **Why overengineered.** The 459-line module declares itself WIP, globally suppresses
  dead-code warnings, and says no production code calls it at
  `../registration/distortion/tps/mod.rs:1`; nevertheless it implements a second
  dense solver, transformation model, diagnostics, and distortion map. The parent
  registers the module unconditionally at `../registration/distortion/mod.rs:29`.

  **Impact.** Lumos carries and compiles a complete alternative nonlinear warp that
  has no result representation, no pipeline/config path, no composition convention,
  no field-domain guard, and no extrapolation policy. It duplicates numerical code
  while the active SIP path remains incorrect.

  **Change.** Remove `tps` and the blanket allowance now. Reintroduce TPS only if
  held-out residuals demonstrate that corrected SIP/base models cannot meet the
  declared accuracy target, and then implement the normalized correction-only,
  regularized, cross-validated, bounded-extrapolation contract in §9.6 as one complete
  production feature.

  **Validation.** The deletion is complete when no production dead-code allowance or
  TPS symbol remains and the registration surface/build is unchanged. A future
  reintroduction must prove regularization selection, affine identifiability, hull
  behavior, and composite-domain validity before exposing configuration.

## Batch 4 — make warping a complete science-data operation

- [ ] **P0 — Make the output grid, dimensions, metadata, and transform directions explicit.**

  **Contract.** §§2.1, 3.2, and 11.8 define `W` as output/reference-to-source,
  explicit Reference/Intersection/Union/Explicit grids, boundary-based footprints,
  and output-grid metadata (`04-registration.md:86`, `04-registration.md:264`, and
  `04-registration.md:1122`).

  **Evidence.** Public `warp` says and implements source-sized/source-metadata output
  at `../registration/resample/mod.rs:37` and
  `../registration/resample/mod.rs:55`; the internal plane function also requires
  equal input/output dimensions at `../registration/resample/plane/mod.rs:19`.
  `RegistrationResult` exposes only the reference-to-target `Transform` at
  `../registration/result/mod.rs:219`, while public drizzle requires the opposite
  input-to-common direction at `../drizzle/accumulator.rs:20` and applies it directly
  at `../drizzle/accumulator.rs:282`.

  **Impact.** Standalone warp cannot align different dimensions or create a mosaic
  grid, and its metadata describes the sampled source rather than the produced
  coordinate system. Passing `result.transform()` directly into `DrizzleFrame` moves
  pixels in the wrong direction; the required manual inverse is easy to miss and its
  semantics are not represented by the type system.

  **Change.** Add an `OutputGrid` with dimensions, pixel transform/WCS, and metadata;
  accept separate source and output dimensions throughout resampling/quality. Name
  mapping directions in types or methods and return both the pull mapping and a
  validated source-to-output mapping for drizzle. Compute nonlinear footprints from
  pixel-boundary edges and typed inverse failures.

  **Validation.** Check exact identity/integer centers, unequal dimensions, all four
  grid policies, reference metadata, boundary corners, nonlinear adaptive edges, and
  a registration-to-drizzle impulse whose displacement proves the direction.

- [ ] **P0 — Propagate input validity masks, variance, flags, and resampling correlation rather than synthesizing confidence from white noise alone.**

  **Contract.** §§2.3, 11.5, and 11.6 require science planes, hard-invalid tap removal,
  `sum(a_i^2 V_i)` variance, propagated flags, geometric support, and a declared
  correlation model (`04-registration.md:150`, `04-registration.md:1059`, and
  `04-registration.md:1086`).

  **Evidence.** `WarpResult` contains only image, coverage, and confidence at
  `../registration/resample/mod.rs:19`. Sampling functions receive only a value plane
  at `../registration/resample/plane/mod.rs:13`; quality estimates confidence from
  kernel weight sums alone at `../registration/resample/quality/mod.rs:60`. No input
  variance or mask can influence tap selection, output validity, or flags.

  **Impact.** Saturated, defect-corrected, cosmic-ray, and invalid pixels are
  interpolated as ordinary data. Heteroscedastic source variance is lost, neighboring
  output covariance is unreported, and downstream combine may interpret an ideal
  white-noise coefficient factor as actual inverse variance.

  **Change.** Warp a coherent frame product containing value/variance/masks; build one
  actual tap set, drop hard-invalid inputs, apply the declared signed-kernel fallback,
  propagate per-bit semantics, and calculate variance from the exact normalized
  coefficients. Return geometric support, actual-kernel support, variance/confidence,
  flags, and equivalent noise area/correlation metadata as separate quantities.

  **Validation.** Hand-compute bilinear and signed-kernel variance, invalid-tap
  renormalization/fallback, and exact flag thresholds. Verify adjacent-output
  covariance from shared taps and prove fill values never enter stacking regardless
  of their numeric value.

- [ ] **P1 — Make support and confidence describe the kernel that actually produced each value, with a configured inclusion threshold and fallback flag.**

  **Contract.** §§11.6–11.7 require actual-kernel support, stable signed-kernel
  fallback, explicit edge policy, and an `EDGE_FALLBACK`/invalid indication
  (`04-registration.md:1086` and `04-registration.md:1115`).

  **Evidence.** A partial Lanczos window produces edge-extended bilinear data at
  `../registration/resample/row/mod.rs:379`, but quality retains nominal Lanczos
  coverage while substituting bilinear confidence at
  `../registration/resample/quality/mod.rs:109`. Bilinear itself clamps a footprint
  coordinate to the nearest pixel center before sampling at
  `../registration/resample/kernel/mod.rs:149`, whereas its coverage is computed from
  the unclamped nominal taps at `../registration/resample/quality/mod.rs:70`. Combine
  accepts any coverage above the global hard-coded `1e-3` at
  `../combine/mod.rs:9` and `../combine/cache/mod.rs:774`.

  **Impact.** Coverage can describe neither the edge-extended bilinear kernel nor the
  nominal requested operation consistently, and no bit reveals the fallback. Tiny
  nominal support may still admit a replicated edge value at full interpolation
  confidence, biasing border normalization and rejection.

  **Change.** Choose one declared boundary rule per kernel and have sampling return a
  named tap/result record containing requested kernel, actual kernel, coefficients,
  nominal support, actual valid support, and fallback reason. Enforce a configurable
  minimum actual support before emitting valid science data; propagate the fallback
  bit into stacking.

  **Validation.** Hand-check positions at `-0.5`, `0`, `width-1`, `width-0.5`, and
  adjacent representable values for every kernel. Verify requested/actual support,
  confidence, validity, and flags exactly, including partial Lanczos and near-zero
  signed normalization.

- [ ] **P1 — Add Jacobian-driven anti-aliasing and an explicit surface-brightness versus flux-per-pixel policy.**

  **Contract.** §§11.3–11.4 require anti-aliasing when `dW/dp` minifies and a declared
  photometric meaning with the corresponding determinant factor
  (`04-registration.md:1030` and `04-registration.md:1044`).

  **Evidence.** `WarpParams` contains only interpolation method and border value at
  `../registration/config.rs:46`; row sampling selects a fixed kernel without a
  spatial Jacobian at `../registration/resample/plane/mod.rs:24`. No registration warp
  code computes determinant, singular values, footprint integration, or photometric
  mode.

  **Impact.** Material downscaling/shear aliases high-frequency structure, while
  integrated-flux pixels under non-unit scale have incorrect total flux and variance.
  A constant surface-brightness fixture cannot distinguish intended behavior because
  the API never states which quantity pixels represent.

  **Change.** Extend the warp contract with `PhotometricMode` and compute `W` plus its
  Jacobian. Keep fixed Lanczos for validated near-identity transforms; use convergent
  adaptive integration/area resampling or require drizzle for material footprint
  changes. Apply `abs(det J)` and its square only in flux-per-pixel mode and record the
  choice.

  **Validation.** Use minifying checkerboards/sinusoids, a constant surface-brightness
  field, and an isolated source under known scale/shear. Assert analytical value,
  total flux, and variance for both modes and the exact threshold where adaptive
  filtering engages.

- [ ] **P2 — Establish one fused scalar warp reference before maintaining per-plane SIMD kernels around an incomplete data contract.**

  **Why overengineered.** Public warp loops over channels and invokes a full mapping
  and interpolation pass for each at `../registration/resample/mod.rs:62`, then runs a
  separate full transform pass for quality at `../registration/resample/mod.rs:71`.
  The row layer already contains architecture-specific bilinear and Lanczos kernels,
  LUT gathers, unsafe AVX2/FMA/SSE/NEON paths, and incremental stepping starting at
  `../registration/resample/row/mod.rs:102`, while none of those paths can propagate
  variance, masks, flags, Jacobians, or actual tap identity.

  **Impact.** RGB+quality evaluates the same nonlinear SIP/homography coordinate four
  times, risks policy drift between value and quality paths, and multiplies the work
  needed to add every missing science plane. Optimizing a plane-only interface first
  has made the correct coherent-frame operation harder to express.

  **Change.** Define a single f64 scalar row/pixel reference that evaluates mapping,
  Jacobian, actual taps, all co-registered channels, variance, masks, support, and
  diagnostics once. Profile it, then vectorize proven hotspots behind that same
  semantic unit. Retain existing SIMD only where it can be cross-checked against and
  consume the exact same tap policy; delete parallel policy implementations.

  **Validation.** Require scalar/SIMD equality within declared numerical bounds over
  all kernels, channels, masks, variance, boundaries, affine/homography/SIP mappings,
  negative samples, and row tails. Benchmark end-to-end RGB warp rather than isolated
  plane kernels before deciding which specializations earn their complexity.

## Batch 5 — make sequence behavior and retained outcomes match the pairwise rigor

- [ ] **P1 — Replace maximum-star-count reference selection with the documented preview graph and deterministic framing/quality policy.**

  **Contract.** §§3.1 and 12.1–12.3 require catalog-only preview edges, component and
  overlap analysis, deterministic multi-criterion reference selection, graph
  bootstrap, and optional global source-track refinement
  (`04-registration.md:234`, `04-registration.md:1133`, and
  `04-registration.md:1139`).

  **Evidence.** The public `Reference::Auto` contract says “most detected stars” at
  `../pipeline/config.rs:8`, and `select_reference` implements only
  `max_by_key(star_count)` at `../pipeline/align.rs:189`. Both RAM and streaming paths
  then register every target directly to that one reference at
  `../pipeline/align.rs:119` and `../pipeline/streaming.rs:251`.

  **Impact.** A blurred, crowded, saturated, poorly framed, or weakly connected frame
  can become the anchor. Frames reachable through overlaps are dropped, and pairwise
  noise is not jointly reduced even when the sequence contains enough redundant
  tracks.

  **Change.** Add the two-pass preview graph and lexicographic selection policy from
  §3.1. Use composed paths only to initialize direct-to-reference recovery, never for
  intermediate resampling. Add optional fixed-gauge global refinement when sequence
  precision justifies it.

  **Validation.** Include a blurred star-rich frame, a sharp but low-overlap frame, a
  graph-only distant frame, disconnected components, and tied candidates. Assert the
  chosen index, accepted component, direct/refitted mappings, and exactly one
  resampling of each original.

- [ ] **P1 — Retain each frame's registration solution or typed rejection instead of reducing the sequence outcome to counts and indices.**

  **Contract.** §§10, 13.5, and 14.1 require per-frame diagnostics and typed drop
  reasons to survive into the sequence result (`04-registration.md:939`,
  `04-registration.md:1352`, and `04-registration.md:1370`).

  **Evidence.** The RAM path converts a registration error to `Err(index)` at
  `../pipeline/align.rs:137`; the streaming path converts it to `None` at
  `../pipeline/streaming.rs:250`. `AlignmentSummary` retains only reference index,
  registered count, and dropped indices at `../pipeline/result.rs:11`.

  **Impact.** A completed stack cannot explain whether a frame failed catalog
  selection, matching, robust estimation, domain/overlap, SIP, or warp support, and it
  cannot expose accepted-frame residual fields or mapping uncertainty for quality
  control and reproducibility.

  **Change.** Store an input-order `FrameRegistrationOutcome` containing either the
  complete accepted solution/warp summary or a typed rejection with stage counts.
  Derive `registered` and `dropped` convenience views from it. Preserve these outcomes
  through both RAM and streaming paths without requiring tracing logs.

  **Validation.** Run a mixed sequence with one failure at each stage and assert exact
  input-order outcomes, typed reasons, accepted diagnostics, derived counts, and RAM/
  streaming equivalence.

## Recommended implementation order

1. Enforce final match/span acceptance immediately; it closes the newly discovered
   false-success path without waiting for the full redesign.
2. Introduce observation covariance/stable identity and the complete result/diagnostic
   types; all later algorithms depend on those contracts.
3. Correct SIP direction before enabling SIP presets on non-identity transforms.
4. Replace triangle/recovery assignment and RANSAC termination/refinement, then add
   full-domain/model-selection gates.
5. Redesign warp around explicit output geometry and coherent value/variance/mask
   taps; only then reconcile or restore SIMD specializations.
6. Build two-pass sequence selection and retained per-frame outcomes on the validated
   pairwise solution.

## Open questions

- [ ] **Choose the initial production robust estimator target.** Is exact MAGSAC++ a
  hard near-term requirement, or should the current loss be renamed and paired with a
  simpler, fully specified covariance-aware IRLS estimator first? The implementation
  and public diagnostics must make one honest claim.

- [ ] **Choose the science-pixel photometric convention.** Confirm whether ordinary
  registered frames represent surface brightness, flux per source pixel, or carry an
  explicit per-image mode. This determines Jacobian scaling for values and variance.

- [ ] **Choose the supported output-grid surface for the first warp redesign.** A
  Reference-only typed grid would close the current metadata/dimension bug quickly;
  Intersection/Union/Explicit can follow only if the pipeline needs them immediately.

- [ ] **Decide whether TPS has a demonstrated production dataset.** If none exists,
  delete the unwired module. If one exists, record why validated SIP/base models fail
  before accepting the complexity of a second nonlinear model.
