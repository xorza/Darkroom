# Stage 4 — Registration Implementation Specification

Registration maps every calibrated light frame into one common coordinate system without losing
the sub-pixel information that stacking is meant to recover. This document is the normative design
for that stage. It covers reference selection, catalog matching, robust geometric estimation,
nonlinear distortion, match refinement, validation, inverse-mapped resampling, uncertainty and mask
propagation, and sequence-wide registration.

The specification distinguishes the **target implementation** from the dated audit of Lumos in
§15. Statements using **must**, **must not**, **should**, and **may** are requirements and
recommendations for the target. The audit describes what the code does on **2026-07-21**; it is not
permission to preserve a weaker behavior.

The design is based on the original astronomical catalog-matching literature, modern robust-model
estimation, and source inspection of Astroalign, Siril, astrometry.net, MAGSAC++, and SWarp. Pinned
upstream revisions and primary sources are listed in §17.

---

## 1. Scope, goals, and non-goals

### 1.1 Scope

This stage begins with calibrated images and the source catalogs produced by Stage 3. It ends with:

1. a validated mapping from the chosen output/reference grid to every target/source frame;
2. a one-to-one catalog of matched stars and statistically meaningful residual diagnostics;
3. an aligned image, validity mask, propagated variance, and geometric support for ordinary
   stacking; or the unresampled mapping required by drizzle; and
4. enough diagnostics to reject an unreliable registration rather than silently blur the stack.

It covers relative registration of repeated exposures, multi-session data, moderate mosaics, and
separate-filter frames with sufficient common sources. Absolute plate solving is not required, but a
trusted WCS may provide the initial mapping. Blind all-sky solving is a distinct problem: use a quad
index such as astrometry.net, then enter this pipeline at match recovery and refinement.

### 1.2 Accuracy objective

Registration error convolves the stellar point-spread function. If the PSF is approximately
Gaussian with per-axis width `sigma_psf = FWHM / 2.354820045`, and the registration error is
isotropic with radial RMS `r_rms`, the expected FWHM broadening factor is

```text
FWHM_out / FWHM_in
    = sqrt(1 + r_rms^2 / (2 * sigma_psf^2)).
```

Therefore the allowable radial RMS for a configured fractional broadening `epsilon` is

```text
r_rms_limit
    = sigma_psf * sqrt(2 * ((1 + epsilon)^2 - 1)).
```

For `epsilon = 0.01`, `r_rms_limit ~= 0.085 * FWHM`; for `epsilon = 0.02`, it is
`~= 0.121 * FWHM`. The target must express its accuracy gate in this physical form, with an optional
absolute pixel ceiling. A fixed two-pixel gate is not meaningful across different sampling and
seeing.

### 1.3 Design principles

- Match **relative geometry**, not local star appearance. Isolated stars are nearly
  descriptor-less blobs; triangle or quad asterisms supply identity.
- Carry centroid uncertainty. A bright, isolated 0.03-pixel centroid and a faint 0.4-pixel centroid
  are not equally informative.
- Fit the simplest model supported by held-out data. Extra degrees of freedom absorb centroid noise
  and extrapolate badly outside the control-point hull.
- Treat a minimal model only as a hypothesis. The delivered transform must be a robust,
  non-minimal refinement.
- Compose every geometric operation and resample the original calibrated pixels at most once.
- Never confuse geometric support, interpolation noise, data validity, and photometric scaling.
- Reject ambiguous or physically implausible mappings explicitly.
- Make every tie, retry, and random choice reproducible from a recorded seed.

### 1.4 Non-goals

This stage does not repair poor tracking within one exposure, undo saturated-star blooming, infer a
time-dependent rolling-shutter model, or make a non-overlapping mosaic match without WCS/catalog
information. A local warp must not be used to disguise optical or tracking failures that change the
PSF itself.

---

## 2. Coordinate, input, and output contracts

### 2.1 Pixel geometry

All internal image coordinates are zero-based, with integer coordinates at pixel centers:

```text
center of pixel (column x, row y) = (x, y)
continuous image footprint       = [-0.5, width - 0.5] x [-0.5, height - 0.5]
```

Catalog positions, transforms, Jacobians, and polynomial evaluation use `f64`. Pixel samples and
stored variance planes may remain `f32`, but interpolation accumulators should use at least `f64`
in the scalar reference implementation. FITS `CRPIX` is one-based; conversion to or from this
internal convention must add or subtract one exactly once.

The output/reference coordinate is `p = (x, y)`. The target/source coordinate is `q`. The canonical
registration mapping is deliberately defined in the direction needed by an inverse-mapped warp:

```text
q = W(p; theta)                         (output/reference -> target/source)
output[p] samples target at W(p; theta)
```

For a purely linear model, `W = T`. With pre-linear distortion,

```text
W(p) = T(D(p)),       D(p) = p + d(p).
```

Every public type, residual, test fixture, and serialized transform must use this direction. If an
API also exposes `W^-1`, it must name the direction rather than calling one transform simply
“forward.”

### 2.2 Required star observation

Each registration-catalog entry must retain its stable index into the full Stage-3 catalog and
contain:

```text
StarObservation {
    catalog_index,
    position: [x, y],
    position_covariance: [[var_x, cov_xy], [cov_xy, var_y]],
    flux,
    flux_uncertainty,
    snr,
    fwhm,
    eccentricity,
    flags,
    selection_score,
}
```

The position covariance must be finite, symmetric, and positive definite after adding a small
numerical floor. It is the covariance of the centroid in pixel coordinates, not the width of the
stellar PSF. If the current detector cannot provide it, a compatibility path may estimate
`sigma_centroid ~= FWHM / (2.3548 * SNR)` and inflate it using synthetic calibration, but it must
set a diagnostic flag. `FWHM / 2` is a seeing scale, not centroid uncertainty, and must not be used
as if it were one.

The matcher must reject non-finite positions; non-positive covariance eigenvalues; saturated,
cosmic-ray, edge-truncated, or failed-fit sources; and entries whose uncertainty exceeds the
configured registration limit. Flux may be signed after calibration, but a source used for pattern
matching must have a valid positive detection S/N.

### 2.3 Image planes

An image submitted for warping consists of:

- one or three calibrated linear value planes;
- a variance plane per value plane, or a documented scalar/noise model from which one can be
  generated;
- bit masks for invalid input, saturation, defects, cosmic-ray correction, interpolation, and any
  other condition required by downstream stacking; and
- dimensions and reference metadata.

The registration detection plane is not a substitute for these science planes. All channels from
one exposure normally share one geometric warp. Separately captured filters may require a common
base geometry plus filter-specific differential terms (§12.4).

### 2.4 Registration result

A successful pairwise result must contain at least:

```text
RegistrationSolution {
    mapping: W,
    inverse_mapping: optional W_inverse,
    model_kind,
    parity,
    parameter_covariance,
    matches: [RegistrationMatch],
    residual_summary,
    spatial_residual_summary,
    transform_domain_summary,
    stage_diagnostics,
    random_seed,
}

RegistrationMatch {
    reference_catalog_index,
    target_catalog_index,
    residual_vector: W(p) - q,
    residual_pixels,
    mahalanobis_squared,
    robust_weight,
    used_in_final_fit,
}
```

The result must retain the full residual vector. A scalar distance alone cannot reveal a coherent
rotation, shear, curl, or field-dependent bias.

### 2.5 Residual covariance

For a matched reference observation `(p_i, C_p,i)` and target observation `(q_i, C_q,i)`, define

```text
r_i(theta) = W(p_i; theta) - q_i
J_p,i      = dW / dp evaluated at p_i
S_i        = C_q,i + J_p,i * C_p,i * J_p,i^T + sigma_sys^2 * I
d_i^2      = r_i^T * S_i^-1 * r_i.
```

`sigma_sys` is a fitted isotropic floor for unmodeled centroid systematics, not a free way to excuse
a bad transform. Estimate it after an initial robust fit by finding the smallest `sigma_sys >= 0`
for which the median accepted `d_i^2` equals the median of `chi-square(2)`,
`2 * ln(2) ~= 1.386294361`. The scalar root is monotone and must be solved by bounded bisection.
If the median is already at or below the target when `sigma_sys=0`, retain zero; adding variance
cannot increase `d_i^2`. The bisection upper bound is the configured systematic cap, and failure to
reach the target before that cap is a registration failure.
Refit with the resulting covariance, but retain both the raw pixel residual and the pre-inflation
diagnostics. Cap `sigma_sys` at a configured fraction of the PSF; exceeding the cap is a registration
failure.

For two dimensions,

```text
P(chi-square(2) <= x) = 1 - exp(-x / 2),
quantile(p)            = -2 * ln(1 - p).
```

Useful squared gates are `9.21034` at 99%, `13.81551` at 99.9%, and `11.829` at
approximately 99.73%.

---

## 3. Sequence reference and output grid

### 3.1 Two-pass reference selection

The first surviving frame and the frame with the largest raw star count are poor general defaults.
A crowded but blurred or badly framed exposure can win both rules. Reference selection should be a
catalog-only first pass:

1. Detect stars in every frame and discard frames that fail Stage-3 quality gates.
2. Build inexpensive preview registrations between temporal neighbors and a deterministic sample of
   sharp, high-S/N candidates. Do not resample pixels.
3. Build a graph whose vertices are frames and whose edges are validated preview transforms. Store
   edge overlap, residual covariance, inlier count, and transform uncertainty.
4. Work in the largest component that satisfies the requested minimum frame count.
5. For every candidate reference in that component, predict the transformed closed image
   footprints and compute:
   - the median pairwise overlap with the candidate;
   - the common-intersection area;
   - the median path transform variance for frames without a direct edge;
   - the candidate FWHM, usable star count, background quality, and saturation burden.
6. Select lexicographically: maximum number of directly registerable frames; maximum median
   overlap; minimum median path variance; minimum FWHM; maximum spatially distributed usable-star
   count; lowest original frame index.

This is deterministic and avoids inventing an opaque weighted score. Siril’s two-pass mode follows
the same high-level lesson: inspect the sequence before fixing the reference, using image quality
and framing rather than blindly accepting frame zero.

If a final frame can only be reached through graph composition, use the composed mapping only to
bootstrap direct correspondences to the chosen reference. Refit those correspondences directly
whenever possible. Never resample through intermediate frames.

### 3.2 Output geometry

The output-grid policy must be explicit:

- `Reference`: exact dimensions, pixel centers, and metadata of the chosen reference; default for a
  conventional stack.
- `Intersection`: largest configured rectangle or polygon supported by every accepted frame.
- `Union`: bounding grid containing all transformed footprints, with coverage marking sparse
  regions; useful for mosaics.
- `Explicit`: caller-supplied dimensions and WCS/pixel transform.

Because `W` maps output/reference coordinates into a target, that target's valid footprint in the
output system is `W^-1(source_footprint)`. Union/intersection calculations must use this inverse
direction; applying `W` to source corners reverses the contract. An explicit inverse may be
evaluated analytically, or boundary points may be solved with a convergent inverse routine whose
failures mark the affected footprint unsupported.

Footprint calculations transform the four **pixel-boundary** corners, not the four center pixels.
Nonlinear transforms must sample every edge adaptively until the deviation from a straight chord is
below the geometry tolerance; corners alone are insufficient for a curved SIP boundary.

The warped image metadata belongs to the output grid, normally the reference, not to the source
image being sampled. The unwarped reference should enter the stack directly with full support and
unit resampling confidence.

---

## 4. Registration-catalog preparation

### 4.1 Quality and spatial selection

Pattern matching should use reliable sources distributed over the field. Taking only the globally
brightest stars overweights a dense cluster and leaves scale, shear, and distortion weakly
constrained at the edges.

Given a configured limit `N_match`, prepare each catalog as follows:

1. Apply the hard validity and flag cuts from §2.2.
2. Define the astrometric variance score
   `u_i = trace(C_i) + sigma_floor^2`; break ties by higher S/N, then higher flux, then original
   catalog index.
3. Divide the image into a grid with approximately `ceil(sqrt(N_match))` cells, adjusted for aspect
   ratio. Sort each cell by the ordering above.
4. Visit nonempty cells in Morton order and take one source from each; repeat round-robin until
   `N_match` is reached or the cells are empty.
5. Preserve the mapping to the original catalog. Do not renumber away source identity.

The full valid catalog, including sources not selected for asterisms, remains available for match
recovery. Typical first attempts use 50–200 spatially distributed stars. More stars are a retry,
not an unconditional improvement: the number of possible false asterism matches rises rapidly with
catalog size.

### 4.2 Optional priors

A trusted WCS, telescope dither, mount rotation, plate scale, or previous-frame transform may bound
translation, rotation, scale, and parity. Priors must be optional and recorded in diagnostics.
Untrusted or incomplete metadata may rank hypotheses but must not veto a well-supported geometric
solution.

Flux similarity may help only when both frames use comparable passbands and exposure response.
Estimate a robust log-flux offset from provisional matches and use a broad likelihood; never require
equal flux across filters, variable stars, clouds, or differing saturation.

---

## 5. Asterism matching

Groth’s catalog matcher established the central technique: triangles are invariant to translation,
rotation, uniform magnification, and—with an explicit parity choice—coordinate inversion. Modern
frame-to-frame implementations such as Astroalign and Siril use the same structure. Astrometry.net
uses four-star quads because a blind all-sky index needs much greater distinctiveness.

### 5.1 Triangle construction

Construct local triangles to avoid the `O(N^3)` complete set:

1. Build a deterministic two-dimensional k-d tree over the selected positions.
2. For every anchor, query the `k` nearest **other** points. A default `k` in `[5, 10]` gives
   `O(N k^2)` triangles.
3. Form the anchor with every unordered neighbor pair.
4. Sort each index triple and deduplicate the global list.

For triangle vertices `a`, `b`, and `c`, compute the three side lengths and their covariance before
sorting. For a side between positions `p_a` and `p_b`,

```text
s_ab              = ||p_a - p_b||
ds_ab / dp_a      = (p_a - p_b) / s_ab
ds_ab / dp_b      = -ds_ab / dp_a.
```

Place those gradients in a six-component Jacobian over `(a_x,a_y,b_x,b_y,c_x,c_y)` and propagate
the block-diagonal vertex covariance. Cross-covariances between sides sharing a vertex must be
retained. Sort the sides to `s0 <= s1 <= s2`, carrying the covariance permutation and the vertex
opposite each side.

Reject a triangle if any condition holds:

- any value or covariance is non-finite;
- `s0` is smaller than `max(4 * median_FWHM, 8 * sigma_position_max)`;
- `s2 / s0 > 10`;
- normalized doubled area
  `area_quality = abs(cross(p_b-p_a, p_c-p_a)) / s2^2` is below `0.02`;
- either adjacent sorted-side gap is not at least three propagated standard deviations; near-
  isosceles/equilateral side ordering otherwise makes vertex roles unstable; or
- the signed area is numerically indistinguishable from zero.

The numerical defaults are starting points and must be tested against the target data, but each
criterion is scale-aware. A fixed raw area-squared threshold is not.

### 5.2 Invariant and uncertainty

Use log side ratios:

```text
z = [ln(s0 / s2), ln(s1 / s2)].
```

Logs turn a fractional side error into an approximately additive error and make scale priors
natural. Propagate side covariance with

```text
J_zs = [[1/s0,    0, -1/s2],
        [   0, 1/s1, -1/s2]]
C_z  = J_zs * C_s * J_zs^T + tau_invariant^2 * I.
```

`tau_invariant` is a calibrated floor for PSF/model systematics. Store the signed orientation of
the vertices in canonical opposite-side order. A parity-preserving frame pair must have equal
orientation; a parity-reversing pair must have opposite orientation.

### 5.3 Invariant search

Build a k-d tree over one catalog’s `z` values. A varying covariance cannot be represented by one
Euclidean tree radius, so use the tree only for a conservative bounding query and apply the exact
test afterward. One exact construction is to store
`lambda_target_max = max_B lambda_max(C_z,B)` after catalog filtering. For reference triangle `A`,
query Euclidean radius

```text
r_query(A) = sqrt(gate * (lambda_max(C_z,A) + lambda_target_max)),
gate       = 13.81551.
```

Every vector passing the Mahalanobis gate lies inside that ball because
`Delta^T S_z^-1 Delta >= ||Delta||^2 / lambda_max(S_z)`. Uncertainty-binned trees may tighten this
bound, but must prove that they cannot omit a valid candidate.

For reference triangle `A` and target triangle `B`, define

```text
Delta = z_A - z_B
S_z   = C_z,A + C_z,B
d_z^2 = Delta^T * S_z^-1 * Delta.
```

Accept the invariant match when `d_z^2 <= 13.81551` (99.9% for two dimensions), parity agrees with
the current parity branch, and optional log-scale/rotation priors pass their propagated gates.
Reject ill-conditioned `S_z`; do not replace its inverse with zero.

Each accepted triangle pair maps vertices by their opposite sorted-side roles. Give the pair raw
likelihood `l = exp(-d_z^2 / 2)`. If one triangle has several candidates, normalize its likelihoods
to sum to one before voting, preventing a crowded invariant bin from casting unlimited evidence.
For each of the three implied point pairs, accumulate both normalized weight and an integer count of
distinct supporting triangle-pair IDs. Repeated generation of the same deduplicated triangle pair
must contribute only once.

### 5.4 Point-correspondence extraction

Discard point-pair edges with fewer than three distinct triangle-pair supports. Solve the remaining
sparse maximum-weight bipartite assignment, including zero-weight dummy nodes so either side may
remain unmatched. Hungarian assignment is adequate at a few hundred stars; a sparse min-cost-flow
solver is preferable for larger catalogs. Greedy highest-vote assignment is deterministic but can
be globally suboptimal when two bright stars compete for the same target.

For every retained pair, record:

```text
support_count
vote_weight
row_margin = assigned_weight - strongest_other_weight for the reference star
col_margin = assigned_weight - strongest_other_weight for the target star
confidence = vote_weight * max(0, row_margin) * max(0, col_margin).
```

Use zero as the strongest-other weight when no competing real edge exists. The margins rank RANSAC
verification; they are not probabilities. A globally optimal assignment may retain an edge with a
negative local margin to resolve a conflict, which correctly gives it zero ranking confidence but
does not discard it before geometric verification. Require at least the configured number of point
matches and enough spatial span for the requested transform.

### 5.5 Parity

Run parity-preserving and, only when configured, parity-reversing matching as separate hypothesis
branches. Do not mix their votes. A normal German-equatorial meridian flip is ordinarily a
near-180-degree rotation and preserves parity; reflection is needed only when the image coordinate
convention or optical path actually mirrors the field.

Rigid/similarity reflection is represented explicitly as

```text
q = t + s * R(theta) * F_e * p,
F_e = diag(e, 1),       e in {+1, -1}.
```

An affine or homography may also have negative local Jacobian determinant, but the parity must stay
constant over the valid image domain.

### 5.6 Bounded retries

A retry schedule must change one source of recall at a time and re-run full geometric verification.
For example:

1. 75 sources, nominal invariant covariance gate;
2. 150 sources, same gate;
3. 200 sources and `k + 2` neighbors;
4. a modestly larger invariant floor; and
5. optional opposite parity or WCS-assisted recovery.

Never simultaneously flood the matcher with faint sources, a wide invariant radius, both parities,
and a high-neighbor graph. Groth’s false-match analysis shows why: triangle counts scale cubically
for a complete graph and coincidental matches rise roughly with the square of that count.

### 5.7 When triangles are insufficient

- With a good WCS or previous-frame prediction, skip blind asterisms and perform gated bipartite
  recovery (§8), then robustly verify the model.
- For very small overlap or an unknown scale over many orders of magnitude, use astrometry.net-style
  quad hashing and full-field hypothesis verification.
- For translation-only high-overlap sequences, phase correlation may seed the translation, but it
  must be refined against stellar centroids and checked for periodic/edge ambiguity.

---

## 6. Geometric models

Use one of the following mappings before optional distortion:

| Model | DOF | Minimal pairs | Mapping | Appropriate use |
|---|---:|---:|---|---|
| Translation | 2 | 1 | `q = p + t` | Pure dithers with no rotation |
| Euclidean | 3 | 2 | `q = R p + t` | Tracked frames, fixed scale |
| Similarity | 4 | 2 | `q = s R p + t` | Small plate-scale change |
| Affine | 6 | 3 | `q = A p + t` | Differential scale/shear |
| Homography | 8 | 4 | projective ratio | Planar approximation, wider fields |

Parameterize similarity scale as `s = exp(alpha)` so it remains positive. Reflection is the
discrete parity term from §5.5, not a negative scale.

For homography parameters `(a,b,c,d,e,f,g,h)` with `H_33 = 1`,

```text
w = g*x + h*y + 1
u = (a*x + b*y + c) / w
v = (d*x + e*y + f) / w.
```

The spatial Jacobian needed for covariance, anti-aliasing, and fold detection is

```text
d(u,v)/d(x,y)
  = (1/w) * [[a - g*u, b - h*u],
             [d - g*v, e - h*v]].
```

The simplest plausible model should win. Homography is not a generic “more accurate” transform:
its projective terms are weakly constrained in a narrow field and can create a horizon or extreme
corner magnification. Stable optical distortion belongs in a calibrated WCS or a controlled
low-order distortion model, not in arbitrary projective freedom.

---

## 7. Robust transform estimation and refinement

### 7.1 Minimal hypotheses

Use RANSAC-style hypothesis generation on the assigned asterism correspondences. The default
sampler must draw uniformly without replacement so the classical confidence calculation remains
valid. An exact PROSAC or Progressive-NAPSAC implementation may replace it only together with that
method’s termination rule. A heuristic “best quarter, best half, then uniform” sampler must not
claim the confidence of uniform RANSAC if it terminates before the uniform phase.

Record a 64-bit seed. Sort candidate correspondences by stable catalog indices before sampling, and
break equal model scores by: more accepted inliers, lower raw RMS, simpler model, then
lexicographically smaller minimal-sample index tuple.

Reject a minimal sample before fitting when:

- a reference or target index repeats;
- two required points are separated by less than their propagated three-sigma uncertainty or the
  configured pixel floor;
- an affine triple is nearly collinear after coordinate normalization;
- a homography quadruple has any three nearly collinear or its DLT design matrix lacks a
  well-separated one-dimensional nullspace; or
- its convex-hull area is too small relative to the catalog footprint.

Degeneracy thresholds must use normalized coordinates or dimensionless area ratios, not fixed raw
cross products.

### 7.2 Exact MAGSAC++ scoring

Plain RANSAC counts points below one threshold; MSAC minimizes `sum min(r^2, t^2)`. The target should
use exact MAGSAC++ scoring and sigma-consensus++ refinement, or name a simpler implemented estimator
honestly. A saturating exponential inspired by MAGSAC is not MAGSAC++.

Score the dimensionless Mahalanobis radius `r = sqrt(d_i^2)`. Set the maximum residual threshold to
the two-dimensional 99.9% gate,

```text
tau_max   = sqrt(13.815510558) ~= 3.716922
k         = 3.64
sigma_max = tau_max / k.
```

For point correspondences MAGSAC++ uses `n = 4` degrees of freedom. Define the normalization and
arguments

```text
C(n) = 1 / (2^(n/2) * Gamma(n/2))
x    = r^2 / (2 * sigma_max^2)
x_k  = k^2 / 2.
```

For `0 <= r <= tau_max`, the loss is

```text
rho(r) = C(n) * 2^((n+1)/2) / sigma_max
         * [sigma_max^2/2 * gamma((n+1)/2, x)
            + r^2/4 * (Gamma((n-1)/2, x) - Gamma((n-1)/2, x_k))],
```

where lowercase `gamma` is the lower incomplete gamma and uppercase `Gamma(a,x)` is the upper
incomplete gamma; both are unregularized. For `r > tau_max`, use the constant

```text
rho_out = C(n) * sigma_max * 2^((n-1)/2)
          * gamma((n+1)/2, x_k),
```

which equals `rho(tau_max)`. Minimize `sum rho(r_i)`; equivalently maximize its negative or
reciprocal, but do not mix score conventions. The inspected MAGSAC implementation omits the global
`C(n)` factor from scoring because it cannot change model ranking; an implementation may do the
same only if it applies that choice consistently to every point and score bound.

Precompute lower/upper incomplete-gamma lookup tables over `x in [0, x_k]` at sufficient resolution
and verify them against a high-precision scalar implementation. Linear interpolation is preferable
to nearest-bin lookup. Increase table resolution until dense validation bounds normalized-loss
absolute error below `1e-9` and relative error below `1e-6` away from zero; record the chosen table
size in one constant. The sigma-consensus++ IRLS weight is zero above `tau_max`; at or below it,

```text
w_robust(r) = C(n) * 2^((n-1)/2) / sigma_max
              * [Gamma((n-1)/2, x) - Gamma((n-1)/2, x_k)].
```

This is `rho'(r)/r`. Evaluate `r=0` from its finite analytical limit rather than dividing by an
epsilon. A common positive factor may be removed from all weights, but not point-dependent terms.

The hard `tau_max` classification is used only for inlier diagnostics, adaptive iteration count,
and match recovery. The model score remains continuous.

### 7.3 Iteration count

For uniform sampling without replacement within a draw, let the best current hypothesis have `I`
hard inliers among `M` correspondences and let the minimal sample size be `s`. If every all-inlier
subset is nondegenerate, the probability of one usable all-inlier sample is

```text
P_good = choose(I, s) / choose(M, s)
       = product_(j=0..s-1) (I-j) / (M-j).

N = ceil(log(1 - p) / log(1 - P_good)).
```

Here `p` is the required success confidence. The familiar `(I/M)^s` expression is for independent
draws with replacement and is slightly optimistic for a distinct sample. Evaluate the product in
log space when necessary.

Degeneracy changes the probability. If `G_I` is the number of nondegenerate `s`-subsets wholly
inside the current inlier set, the rigorous value is

```text
P_good = G_I / choose(M, s).
```

For the small seed sets and `s <= 4` here, enumerate and cache subset degeneracy, or compute a
certified lower bound. Never substitute `choose(I,s)` when some of those subsets are degenerate and
still advertise the resulting confidence as guaranteed. Recompute `N` whenever the lower bound on
`G_I` improves, clamp it to the configured minimum and maximum, and handle `G_I=0` and
`G_I=choose(M,s)` without `log(0)`. Degenerate draws count as attempts for the external work budget
and are diagnosed separately. A default `p >= 0.995` is appropriate; deterministic tests must
exercise the maximum-iteration path. As in classical RANSAC, this confidence is conditional on a
nondegenerate all-inlier minimal sample entering the refinement basin; it is not a proof that noisy
data satisfy the model. The absolute iteration cap and final validation remain mandatory.

### 7.4 Initial non-minimal solvers

- **Translation:** solve generalized weighted mean displacement. With matrix weights `W_i=S_i^-1`,
  `t = (sum W_i)^-1 * sum W_i(q_i-p_i)`.
- **Euclidean/similarity:** use weighted Kabsch/Umeyama for an isotropic-weight initialization.
  Enforce proper rotation in the selected parity branch. With anisotropic covariance, finish with
  the nonlinear refinement below.
- **Affine:** Hartley-normalize each point set to zero centroid and mean radius `sqrt(2)`. Whiten the
  `2N x 6` design rows and solve directly with pivoted QR or SVD. Do not form normal equations.
- **Homography:** Hartley-normalize, build the full `2N x 9` DLT matrix, and take the right singular
  vector of the smallest singular value. Denormalize as
  `H = T_target^-1 * H_normalized * T_reference`. DLT minimizes algebraic error and is only an
  initializer.

The minimal and non-minimal solvers must reject non-finite output and report rank/condition
diagnostics. Absolute determinant thresholds are not scale-invariant.

### 7.5 Covariance-aware nonlinear refinement

Every accepted hypothesis must be polished by iteratively reweighted nonlinear least squares on all
current inliers. At each outer iteration:

1. Evaluate `W`, the spatial Jacobian `J_p`, residual covariance `S_i`, Mahalanobis residuals, and
   MAGSAC++ weights.
2. Freeze `S_i` for the inner step, Cholesky-whiten each two-vector residual and its parameter
   Jacobian, and solve the damped least-squares system with QR or SVD.
3. Evaluate the true robust objective at the trial parameters. Accept only a finite decrease;
   decrease damping on acceptance and increase it on rejection.
4. Recompute covariance and weights, then continue until both relative objective change and the
   normalized parameter step are below configured tolerances, or the iteration limit is reached.

Useful parameter Jacobians are:

```text
translation [tx, ty]:
  J_theta = [[1,0], [0,1]]

euclidean [tx, ty, angle], z = R(angle)*p:
  dW/dangle = [-z_y, z_x]

similarity [tx, ty, alpha, angle], z = exp(alpha)*R(angle)*p:
  dW/dalpha = z
  dW/dangle = [-z_y, z_x]

affine [a,b,tx,c,d,ty]:
  J_theta = [[x,y,1,0,0,0],
             [0,0,0,x,y,1]]

homography [a,b,c,d,e,f,g,h]:
  du/dtheta = [x/w,y/w,1/w,0,0,0,-u*x/w,-u*y/w]
  dv/dtheta = [0,0,0,x/w,y/w,1/w,-v*x/w,-v*y/w].
```

Local optimization should run only for a new best hypothesis. The final delivered fit must run
again after match recovery, even if the minimal RANSAC model already has low RMS.

Estimate parameter covariance from the final whitened Jacobian. For robust weights, report both the
Gauss-Newton approximation and a sandwich covariance if practical; flag singular or weakly
constrained directions rather than returning zeros.

### 7.6 Plausibility and domain checks

Validate the mapping over the entire requested output footprint, not only at matched stars:

- all sampled mappings and Jacobians are finite;
- homography denominator `w` keeps one sign and stays safely away from zero; because `w` is linear,
  checking all boundary corners proves this for a rectangular linear-only domain;
- Jacobian determinant keeps the selected parity and never approaches zero;
- singular values of the Jacobian remain within configured scale bounds;
- affine anisotropy/shear, homography perspective, rotation, and displacement remain within any
  trusted physical priors; and
- the transformed footprint overlaps the target by the required area.

For nonlinear distortion, evaluate an adaptive grid and every boundary segment (§9.5). Any fold,
near-singularity, or explosive corner correction invalidates the model. Acceptance must use
interval arithmetic, Bernstein-form polynomial bounds, or another conservative cell bound to prove
denominator and Jacobian-determinant signs over the continuous domain. Subdivide inconclusive cells;
if a configured minimum cell size is reached without proof, reject the mapping. Point sampling
alone cannot prove that a degree-5 polynomial has no fold between samples.

### 7.7 Model selection

`Auto` must evaluate the ladder

```text
Translation -> Euclidean -> Similarity -> Affine -> Homography
```

subject to enabled models and physical priors. Use the same final recovered correspondence set when
comparing adjacent models. Partition matches into five deterministic spatial folds by reference
position. For each model, refit on four folds and evaluate robust loss and raw RMS on the fifth.

Freeze assignments during one adjacent-model comparison. If an upgrade wins and its mapping changes
guided recovery, freeze the new assignment and repeat the comparison of both models; accept the
upgrade only if it still wins. This prevents a flexible model from winning merely because it was
evaluated on an easier, different subset of stars.

Accept the first model that satisfies all absolute gates:

1. full-fit radial RMS is below the PSF-broadening budget from §1.2;
2. total standardized residual is below its calibrated goodness-of-fit gate. With a frozen
   assignment, non-robust Gaussian fit, `N` matches, and rank-`k` model, the nominal degrees of
   freedom are `2N-k`, minus one more when a positive `sigma_sys` was fitted. Robust selection and
   estimated correspondences invalidate an exact chi-square claim, so production thresholds should
   be calibrated by parametric simulation; the analytical quantile remains a diagnostic;
3. no spatial fold exceeds the configured RMS/p95 budget;
4. transform-domain plausibility passes; and
5. the final match count and convex-hull coverage are sufficient for that model.

Upgrade only if the next model reduces median held-out robust loss by at least 10% and improves at
least four of five folds. This guards against an extra model fitting noise in the training points.
If no model passes the absolute gates, registration fails; returning the most general low-training-
RMS model is unsafe.

---

## 8. Guided match recovery

Triangle voting deliberately yields a conservative seed set. Once a plausible mapping exists,
recover matches from the **full valid catalogs**:

1. Transform every reference position and propagate its covariance with the current spatial
   Jacobian.
2. Build a target k-d tree and generate every candidate edge within the conservative Euclidean
   radius enclosing the Mahalanobis gate `d^2 <= 13.81551`. If `C_pred` is the transformed reference
   covariance including mapping uncertainty and `lambda_target_max` is the largest accepted target
   covariance eigenvalue, a safe per-reference radius is
   `sqrt(13.81551 * (lambda_max(C_pred) + lambda_target_max + sigma_sys^2))`.
3. Evaluate the exact Mahalanobis cost for each edge. If comparable-band flux is enabled, add a
   robust log-flux likelihood only after estimating the frame-to-frame flux offset.
4. Solve a global minimum-cost bipartite assignment. Let the real-edge cost be its `d^2` plus any
   declared nonnegative photometric term and let `G` be the accepted real-edge gate in the same cost
   units. Give each unmatched reference and each unmatched target a dummy cost `G/2`; replacing two
   unmatched endpoints by a real edge is then favorable exactly when its cost is below `G`. If a
   different unmatched prior is desired, configure and report it explicitly. Sequential nearest-neighbor
   claiming is order-dependent and can let one early source steal a better target from another.
5. Refit with the robust nonlinear procedure in §7.5, update `sigma_sys`, regenerate candidates, and
   repeat.

Stop when the assignment is identical and the normalized parameter change is below tolerance, or
after eight passes. A constant match count is not convergence if pair identities changed. Begin
with a wider gate only when transform covariance requires it, then shrink to the final standardized
gate. Revalidate every seed match; never restore a rejected seed merely to preserve the original
count.

The final assignment must be one-to-one, have at least the configured inlier count, span the image
in two dimensions, and be followed by one last full robust refit.

---

## 9. Nonlinear distortion

### 9.1 When to fit distortion

Fit nonlinear distortion only when a simpler model fails with a coherent spatial residual pattern,
there are enough high-quality matches over the field, and held-out error improves. If the same
camera/lens distortion is stable across a sequence, a calibrated per-instrument WCS is preferable
to fitting an independent high-order polynomial to every exposure.

### 9.2 SIP mapping

SIP applies quadratic-and-higher pixel corrections before a linear WCS transform. Around reference
point `c = (c_x,c_y)`, with `u=x-c_x`, `v=y-c_y`,

```text
D_x(p) = x + sum A_pq * u^p * v^q
D_y(p) = y + sum B_pq * u^p * v^q
2 <= p + q <= order
W(p)   = T(D(p)).
```

There are `(order+1)(order+2)/2 - 3` terms per axis: 3, 7, 12, and 18 at orders
2–5. Linear and constant terms are excluded because `T` owns them.

### 9.3 Correct fit direction

Given base mapping `T: reference -> target`, the polynomial target must be expressed in the
**pre-`T` reference coordinate domain**. For match `(p_i, q_i)`, compute

```text
c_i          = T^-1(q_i)
desired_d_i  = c_i - p_i.
```

Then fit `d(p_i) ~= desired_d_i`, or jointly minimize
`T(p_i + d(p_i)) - q_i`. Fitting the raw target-frame residual
`q_i - T(p_i)` directly as a pre-transform correction is wrong whenever the linear part of `T` is
not identity: for affine `T(p)=A p+t`, the required correction is
`A^-1(q-T(p))`, not `q-T(p)`.

This direction must be tested with rotation, scale, shear, and homography—not only identity-plus-
small-distortion fixtures.

For an invertible base transform, the approximate covariance of the fitted correction target is

```text
C_d,i = J_Tinv(q_i) * C_q,i * J_Tinv(q_i)^T + C_p,i + C_base,i,
```

where `C_base,i` is the mapped contribution of base-transform parameter uncertainty. Because the
base parameters and controls were estimated from overlapping data, the final joint composite fit is
the authoritative covariance calculation; this expression is the initialization.

### 9.4 Numerically stable robust fit

Choose `c` as the output-grid reference pixel when FITS-SIP export is required; an internal-only
fit may use the catalog centroid. Normalize

```text
u_n = (x - c_x) / L
v_n = (y - c_y) / L,
```

where `L` is the RMS or mean radial distance of accepted controls. Build the power-basis design in
normalized coordinates. Whiten each two-dimensional correction by its propagated covariance and
solve the rectangular system with pivoted QR or SVD. Do not solve polynomial normal equations;
high-degree power bases already have challenging conditioning, and `A^T A` squares it.

Run the same MAGSAC/IRLS logic as the base fit, recomputing assignment if the correction moves a
source materially. Alternate base-transform and polynomial refinement until the composite robust
objective converges. Retain the no-linear-term constraint throughout.

If normalized internal coefficients `a_pq` produce a pixel correction
`L * a_pq * u_n^p * v_n^q`, the equivalent FITS-SIP coefficient is

```text
A_pq = a_pq * L^(1 - p - q),
```

with the corresponding rule for `B_pq` and a correctly converted one-based `CRPIX`.
This is a true FITS-SIP representation only when `T` is the linear pixel-to-intermediate-world
mapping required by the SIP convention. A polynomial composed before an arbitrary relative
homography is SIP-shaped internal distortion, but must not be serialized as a standards-compliant
SIP WCS without deriving the corresponding WCS model. The simple coefficient scaling also assumes
`c` equals the internal zero-based location corresponding to `CRPIX`; if another origin was used,
translate the polynomial exactly and absorb the induced constant and linear terms into `T` before
export.

### 9.5 Order selection and validation

Try orders from 2 upward. For order `m`, require at least five times the term count in well-
distributed controls, controls in all four field quadrants, at least `m+1` occupied grid rows and
columns, a configured minimum control-hull area, and a full-rank acceptably conditioned design.
Use the same five spatial folds as model selection, and require every training fold to retain these
conditions. Accept a higher order only if median held-out robust loss falls by at least 10%, four of
five folds improve, and all domain checks pass.

Report and gate:

- train and held-out RMS, p95, maximum residual, and standardized residuals;
- points used/rejected and control-point convex-hull coverage;
- maximum correction on controls, boundary, and an adaptive full-field grid;
- minimum/maximum composite Jacobian determinant and singular values;
- coefficient/design condition number; and
- the largest extrapolation distance outside the control-point hull.

The diagnostic adaptive grid must subdivide a cell when mapped midpoint deviation or Jacobian
variation exceeds tolerance. The acceptance proof uses the conservative bounds required by §7.6,
not samples alone. Check the exact output boundary. A grid ending at a multiple of `spacing` must
not accidentally skip `width-0.5` or sample outside it.

### 9.6 TPS alternative

A regularized thin-plate spline can model smooth residuals that SIP cannot:

```text
f(p) = a0 + a1*x + a2*y + sum_i w_i * U(||p-p_i||),
U(r) = r^2 * ln(r),       U(0)=0.
```

Fit the block system `[K+lambda I, P; P^T, 0]` in normalized coordinates with a stable symmetric
solver. When TPS is a correction composed with a separately fitted base transform, remove its
constant and linear freedom or absorb that affine component into the base after every alternating
step; otherwise the parameterization is not identifiable. Select `lambda` by spatial
cross-validation. TPS is an interpolator, not a safe extrapolator: outside the control hull, blend
smoothly back to the validated base transform or mark the region unsupported. SIP remains
preferable when FITS interoperability matters.

---

## 10. Validation and uncertainty product

### 10.1 Required residual summaries

For the final assignment report:

- raw radial RMS, median, MAD, p68, p95, p99, and maximum in pixels;
- per-axis mean, RMS, and covariance;
- median and upper quantiles of `d^2`;
- robust objective, inlier count/ratio, and fitted `sigma_sys`;
- metrics for both the model-fitting subset and all recovered matches; and
- the same statistics in a fixed spatial grid.

Plot-free diagnostics still need spatial vector information: for each cell report count, mean
`dx,dy`, RMS, and p95. A low global RMS can hide opposite corner errors that cancel in the mean.

### 10.2 Transform uncertainty over the field

Given parameter covariance `C_theta` and parameter Jacobian `J_theta(p)`, estimate mapping
uncertainty

```text
C_map(p) = J_theta(p) * C_theta * J_theta(p)^T + sigma_sys^2 * I.
```

Evaluate it at the center, corners, and adaptive grid. Store maximum radial mapping sigma and an
optional low-resolution uncertainty map. This distinguishes a tightly constrained center from an
unconstrained extrapolated corner even when both have no nearby residual sample.

### 10.3 Acceptance

A frame is accepted only if all of the following hold:

- catalog, pattern, RANSAC, and recovered-match minima pass;
- PSF-derived raw RMS and p95 gates pass;
- standardized goodness-of-fit and bounded `sigma_sys` pass;
- spatial coverage and held-out model selection pass;
- transform and distortion domain checks pass; and
- predicted output support meets the configured minimum.

An ad hoc quality number may be provided for UI sorting, but it must not replace the individual
diagnostics or the hard acceptance contract.

---

## 11. Warping and resampling

### 11.1 Single-pass inverse mapping

For each output pixel center `p`, compute source coordinate `q=W(p)` and sample the original
calibrated source. Distortion, linear transform, output-grid offset, and any chromatic correction
must be composed in `W`. Forward scattering leaves holes; repeated pull resampling compounds blur.

Non-finite `q`, a projective horizon, or a coordinate outside the closed source footprint produces
zero support and the configured fill. The fill value must never enter stacking because support
controls inclusion.

### 11.2 Reconstruction kernels

For separable kernel `K`, with source samples `I[j,k]`,

```text
raw_weight_jk = K(q_x-j) * K(q_y-k)
a_jk          = raw_weight_jk / sum_in raw_weight
I_out(p)      = sum_in a_jk * I[j,k].
```

Supported kernels may include:

- **Nearest:** closest center; only for labels or exact integer shifts.
- **Bilinear:** `K(x)=max(0,1-|x|)`; stable and ring-free but visibly low-pass.
- **Keys bicubic:** support 2, with `a=-0.5`:

  ```text
  K(x) = (a+2)|x|^3 - (a+3)|x|^2 + 1,             |x| <= 1
       = a|x|^3 - 5a|x|^2 + 8a|x| - 4a,           1 < |x| < 2
       = 0.                                        otherwise
  ```

- **Lanczos-a:** support `a`:

  ```text
  K(x) = sinc(x) * sinc(x/a),   |x| < a
       = 0,                     otherwise
  sinc(x) = sin(pi*x)/(pi*x),   sinc(0)=1.
  ```

Lanczos-3 is a good near-identity science default, but its negative lobes ring around saturated
stars and sharp defects. Preserve signed calibrated data; do not clamp negative interpolation
values to zero.

### 11.3 Minification and nonlinear scale

A fixed Lanczos kernel is a reconstruction filter, not a complete anti-aliasing solution. Let
`J=dW/dp`. If an output pixel spans more than one source pixel (`J` has a singular value greater
than one), prefilter/integrate over that footprint. The preferred general solution is an adaptive
elliptical or polygon-area resampler driven by `J`; bounded supersampling is an acceptable scalar
reference when it integrates until value and area converge.

For near-identity same-camera stacks, fixed Lanczos is adequate. For material scale changes,
strong shear, sky reprojection, or intentional reconstruction of undersampled data, use adaptive
resampling, exact area overlap, or drizzle as appropriate. The `reproject` package makes the same
distinction between interpolation, adaptive anti-aliasing, and exact flux-conserving polygon
intersection.

### 11.4 Photometric semantics and Jacobian

The warp must declare whether pixels represent sampled surface brightness or integrated flux per
pixel:

- **Surface brightness:** interpolate without a determinant factor.
- **Flux per source pixel:** output-pixel flux must include the mapped source area. For a locally
  affine inverse mapping, multiply reconstructed surface brightness by `abs(det J)` after unit
  conversion. Multiply propagated variance by `abs(det J)^2` and covariance by the product of the
  determinant factors at the two output positions.

Do not apply a determinant blindly: calibrated astronomical formats vary in semantics, and drizzle
already accounts for geometric overlap. Test a constant surface-brightness field and an isolated
source separately under non-unit scaling.

### 11.5 Variance propagation

For independent valid input samples with normalized coefficients `a_j`,

```text
V_out = sum_j a_j^2 * V_j.
```

With input covariance, include cross terms. When only a scalar white-noise variance is available,
the relative inverse-variance multiplier is

```text
confidence = 1 / sum_j a_j^2.
```

This number can exceed one because interpolation averages independent input pixels. In
`FluxPerPixel` mode divide it by `abs(det J)^2`, or equivalently include the area factor in every
effective coefficient. It is not geometric coverage. Resampling also creates covariance between
neighboring outputs:

```text
Cov(out_p, out_r) = sum_j a_pj * a_rj * V_j.
```

The full matrix need not be stored, but the product must report the kernel/noise-correlation model
or equivalent noise area so later photometric uncertainty is not treated as spatially independent.

### 11.6 Masks and geometric support

For every sample compute independently:

1. **inside-footprint:** whether `q` lies inside the closed source footprint;
2. **kernel support:** `sum_in |w_j| / sum_all |w_j|` for the kernel actually used;
3. **valid-data support:** the absolute-weight fraction remaining after hard-invalid taps are
   removed;
4. **variance/confidence:** from the actual normalized coefficients; and
5. **propagated flags:** weighted or any-hit propagation according to each bit’s semantics.

Hard-invalid taps must not contribute value or variance. Positive kernels may renormalize remaining
valid taps above a configured support threshold. Truncating a signed kernel can make its signed sum
nearly zero; in that case either mark the output invalid or use a declared stable fallback such as
edge-extended bilinear and set an `INTERPOLATION_FALLBACK` bit.

Coverage and confidence must describe the kernel that produced the value. If partial Lanczos falls
back to bilinear, reporting nominal Lanczos coverage while bilinear produced the pixel is
inconsistent unless both nominal and actual support are exposed separately.

Suggested mask propagation:

- `NO_DATA`: set when hard-valid support is below threshold;
- `SATURATED` and `COSMIC_RAY`: set when any contributing tap above a small absolute-weight fraction
  carries the bit;
- `DEFECT_CORRECTED`: propagate its absolute-weight fraction or set an any-hit bit;
- `EDGE_FALLBACK`: set whenever the requested kernel was replaced; and
- `EXTRAPOLATED`: must never be treated as valid science data merely because a border fill exists.

### 11.7 Edge behavior

The scalar reference defines every boundary rule. High-performance SIMD must be bit-close to it.
Edge extension, tap dropping, signed renormalization, and fallback are different policies; choose
one explicitly per kernel. Verify coordinates at `-0.5`, `0`, `width-1`, and `width-0.5`, plus the
next representable values on both sides.

### 11.8 Multi-channel images and metadata

Evaluate `W(p)` once and reuse coefficients for all co-registered channels. Variance is propagated
per channel; masks/support may be shared only when their inputs and validity semantics are shared.
The output dimensions, WCS, and coordinate metadata come from the output grid. Source acquisition
metadata may be retained as provenance but must not describe the warped pixel coordinates.

---

## 12. Sequence-wide and special-case registration

### 12.1 Direct-to-reference pairwise mode

For a short, high-overlap same-camera sequence, register every target catalog directly to the
selected reference, then warp in parallel. This avoids transform-chain error and is the normal
fast path.

### 12.2 Registration graph

For long sessions or mosaics, retain valid pairwise edges in a graph. Edge cost should combine
mapping covariance, held-out RMS, and inverse overlap, not merely temporal distance. Use the graph
to initialize frames that do not directly match the reference, but recover/refit direct reference
correspondences whenever overlap permits.

### 12.3 Global refinement

For maximum precision, merge pairwise matches into source tracks and jointly refine all frame
mappings. Let `X_s` be the common/reference coordinate of source track `s`, and `W_f` map common
coordinates into frame `f`. Because `X_s` is a jointly estimated latent position rather than a
noisy fixed catalog input, use `S_fs = C_q,fs + sigma_sys,f^2 I` for each observation; do not add a
separately estimated `X_s` covariance inside the objective. Minimize

```text
sum_(f,s observed) rho(sqrt(
    (W_f(X_s) - q_fs)^T * S_fs^-1 * (W_f(X_s) - q_fs)
)).
```

Fix the reference mapping to identity to remove gauge freedom. Solve the sparse robust least-squares
problem by block-sparse LM or alternate between track positions and frame parameters. Reject whole
tracks that are inconsistent across frames. Masci, Makovoz, and Moshir use this global weighted
point-source principle for astronomical pointing refinement.

Global refinement reduces path accumulation and shares information from every overlap, but pixels
are still resampled only once from each original frame.

### 12.4 Separate filters and chromatic effects

Frames from different filters can have differential atmospheric refraction, lateral chromatic
aberration, or slightly different focus. Use a shared base model plus the smallest filter-specific
term justified by held-out residuals—often translation, occasionally low-order radial/color terms.
Do not use flux equality as a match requirement. Report per-filter residuals so a good combined RMS
cannot hide a color-dependent offset.

### 12.5 Moving targets

Stars and a comet/asteroid cannot both be stationary under one rigid mapping. Produce the stellar
registration first. A moving-target stack requires a separate ephemeris or measured motion model
composed with the stellar mapping, with its own diagnostics and single final resampling.

---

## 13. End-to-end pseudocode

### 13.1 Pairwise catalog registration

```text
function register_pair(reference_catalog, target_catalog, config):
    validate_config(config)
    ref_full = validate_and_filter(reference_catalog)
    tgt_full = validate_and_filter(target_catalog)
    require_minimum_catalogs(ref_full, tgt_full)

    ref_match = spatially_select(ref_full, config.max_match_stars)
    tgt_match = spatially_select(tgt_full, config.max_match_stars)

    seed_sets = []
    if config.trusted_initial_mapping exists:
        seed_sets += gated_assignment(ref_match, tgt_match,
                                      config.trusted_initial_mapping,
                                      wide_gate)

    for retry in config.asterism_retry_schedule:
        for parity in enabled_parities:
            ref_tri = construct_triangles(ref_match, retry, parity)
            tgt_tri = construct_triangles(tgt_match, retry, parity)
            tri_pairs = covariance_gated_invariant_matches(ref_tri, tgt_tri,
                                                            retry)
            votes = normalized_triangle_votes(tri_pairs)
            seeds = maximum_weight_assignment(votes,
                                               min_distinct_support=3)
            seed_sets += (parity, seeds)

        if any seed set passes minimum and spatial-span gates:
            break

    require_nonempty_seed_sets(seed_sets)

    candidates = []
    for (parity, seeds) in seed_sets:
        for model in enabled_model_ladder:
            hypothesis = uniform_ransac_magsac(seeds, model, parity, config)
            if hypothesis failed:
                continue
            refined = robust_nonlinear_refit(hypothesis, seeds)
            recovered = recover_global_assignment(ref_full, tgt_full, refined)
            final = robust_nonlinear_refit(recovered.mapping,
                                           recovered.matches)
            sigma_sys = calibrate_systematic_floor(final)
            final = robust_nonlinear_refit(final, recovered.matches, sigma_sys)
            final = recover_until_assignment_and_parameters_converge(
                final, ref_full, tgt_full)
            if domain_checks_pass(final):
                candidates += cross_validate(final, final.matches)

    base = choose_simplest_adequate_model(candidates,
                                          psf_blur_budget,
                                          spatial_cross_validation)
    require(base exists)

    if distortion_is_enabled_and_justified(base.residual_field):
        distortion_candidates = fit_sip_orders_in_correct_pretransform_domain(base)
        solution = choose_by_spatial_cross_validation_and_domain_checks(
            [base] + distortion_candidates)
    else:
        solution = base

    diagnostics = compute_full_residual_spatial_domain_and_covariance_metrics(solution)
    enforce_acceptance_contract(diagnostics, config)
    return solution + diagnostics
```

### 13.2 Uniform MAGSAC/LO-RANSAC

```text
function uniform_ransac_magsac(matches, model, parity, config):
    rng = seeded_rng(config.seed)
    best = none
    required_iterations = config.max_iterations
    iteration = 0

    while iteration < min(required_iterations, config.max_iterations):
        iteration += 1
        sample = uniform_unique_sample(rng, matches, model.min_pairs)
        if degenerate_in_either_catalog(sample, model):
            continue

        theta = fit_minimal(sample, model, parity)
        if not domain_and_prior_checks_pass(theta):
            continue

        total_loss, inliers = exact_magsac_loss(
            theta, matches,
            preemptive_loss_bound=(best.total_loss if best exists else infinity))
        if best does not exist or total_loss < best.total_loss:
            theta_lo = sigma_consensus_irls(theta, matches)
            loss_lo, inliers_lo = exact_magsac_loss(theta_lo, matches)
            if loss_lo < total_loss and domain checks pass:
                theta, total_loss, inliers = theta_lo, loss_lo, inliers_lo
            best = stable_tie_break(best, theta, total_loss, inliers, sample)
            p_good = exact_nondegenerate_all_inlier_probability(
                best.inliers, matches, model)
            required_iterations = min(
                required_iterations,
                bounded_required_draws(config.confidence, p_good))

    require(best has at least model.min_pairs inliers)
    final = sigma_consensus_irls(best.theta, matches)
    return final if it has lower loss and passes domain checks, otherwise best.theta
```

### 13.3 SIP fit

```text
function fit_sip(base_T, matches, order):
    terms = [(p,q) for every integer total where 2 <= total <= order
                   for every integer p where 0 <= p <= total
                   where q=total-p]
    require(matches.length >= 5 * terms.length)

    center, scale = normalized_coordinate_frame(reference_positions)
    for each match (p_i, q_i):
        desired_prelinear_i = inverse(base_T, q_i) - p_i
        covariance_i = J_base_inverse * C_q_i * J_base_inverse^T
                       + C_p_i + mapped_base_parameter_covariance
        row_i = evaluate_power_basis(normalize(p_i), terms)

    coefficients = robust_whitened_qr_or_svd(row, desired_prelinear,
                                              covariance)
    alternate base_T and coefficients under composite residual
        base_T(p + sip(p)) - q
    return coefficients only if spatial holdout and full-domain checks pass
```

### 13.4 Warp

```text
function warp_frame(source, output_grid, W, kernel, photometric_mode):
    allocate value, variance, mask, support, confidence on output_grid

    parallel for each output row y:
        for integer x where 0 <= x < output_width:
            p = [x, y]
            q, J = evaluate_mapping_and_jacobian(W, p)
            if q nonfinite or outside closed source footprint:
                write fill, infinite/invalid variance, NO_DATA, zero support/confidence
                continue

            actual_kernel = choose_antialiased_or_requested_kernel(J, q)
            taps = gather_taps_masks_variances(source, q, actual_kernel)
            sample = normalize_or_declared_fallback(taps)
            if valid support below threshold:
                write NO_DATA
                continue

            value = sum(sample.coefficient * tap.value)
            variance = sum(sample.coefficient^2 * tap.variance)
            if photometric_mode == FluxPerPixel:
                area_factor = abs(det(J))
                value *= area_factor
                variance *= area_factor^2
            mask = propagate_bits(taps, sample.coefficients)
            support = actual_absolute_weight_support(taps)
            confidence = base_variance / variance, when base variance is defined
            write outputs

    attach output-grid metadata and resampling-correlation diagnostics
    return warped product
```

### 13.5 Sequence

```text
detect all catalogs
build preview registration graph
choose reference and output grid by §3
initialize every reachable frame mapping
optionally build source tracks and globally refine all mappings
validate every frame independently
drop failed frames with typed reasons
warp accepted originals once, or pass mappings directly to drizzle
stack with propagated support, variance, masks, and frame diagnostics
```

---

## 14. Errors, determinism, and performance

### 14.1 Typed failures

Distinguish at least:

- invalid configuration or coordinate convention;
- insufficient valid/spatially distributed stars;
- no asterism candidates;
- ambiguous parity or correspondence assignment;
- robust estimator exhausted;
- degenerate/rank-deficient transform;
- implausible transform domain or insufficient overlap;
- match recovery failed or did not converge;
- no model satisfies cross-validation/blur gates;
- distortion underconstrained, singular, folded, or overfit;
- warp geometry overflow/non-finite mapping; and
- insufficient valid resampling support.

Diagnostics must retain stage counts even on failure: selected stars, triangles formed/rejected,
invariant pairs, vote edges, assigned seeds, hypotheses/degeneracies, best inliers, recovered
matches, and rejection reason.

### 14.2 Determinism

- Every RNG uses a recorded seed.
- k-d tree partition ties use coordinates then original index.
- Triangle side and candidate ties use stable catalog indices.
- Sparse-map iteration order must never affect votes, assignment, sampling, or output.
- Parallel reductions use deterministic ordering or compensated accumulators where result drift can
  change a gate.
- Retry order, model order, parity order, and assignment tie-breaking are fixed.

### 14.3 Complexity and memory

For `N` selected stars and `k` neighbors, local triangle construction is `O(N k^2)` with
`O(N k^2)` storage before deduplication. Invariant queries are approximately logarithmic plus
candidate density. A dense `N_ref x N_target` vote matrix is acceptable only below an explicit
memory threshold; otherwise use sorted sparse edges. Assignment is `O(N^3)` for dense Hungarian
and should switch to sparse min-cost flow when edge density is low.

RANSAC scoring is `O(iterations * matches)`. Preemptive loss bounds and local optimization only on
new best hypotheses reduce work without changing the result. Warp memory is output planes plus
per-thread row/tap scratch; it need not allocate a source-coordinate image.

SIMD and incremental stepping are optimizations after the scalar reference is correct. Homography
and SIP generally require per-pixel evaluation; affine-or-simpler mappings may step source
coordinates linearly across a row, but accumulated stepping error must be bounded and cross-checked
against direct evaluation.

---

## 15. Current Lumos implementation audit (2026-07-21)

This audit describes `lumos/src/stacking/registration/` and its pipeline callers on the date above.
It intentionally names divergences from the target contract.

### 15.1 What is already strong

- The transform direction is documented consistently: the stored transform maps reference
  coordinates to target coordinates, which is exactly the output-to-source direction needed by the
  pull warp.
- Catalog positions and FWHM are checked for finiteness, configurations are validated, and failures
  are typed.
- Matching uses local k-nearest-neighbor triangles, elongated/flat rejection, invariant k-d-tree
  queries, orientation checks, multiple votes, deterministic tie-breaking, and dense/sparse vote
  storage.
- `Auto` now implements its configured Euclidean -> Similarity -> Affine -> Homography ladder. The
  old document incorrectly listed the intermediate Euclidean and affine attempts as missing.
- RANSAC has bounded iterations, deterministic optional seeding, degeneracy checks, plausibility
  bounds, preemptive scoring, local optimization only on new best hypotheses, and a final
  non-minimal refit.
- Euclidean/similarity use closed-form Procrustes-style fits. Affine and homography use Hartley
  coordinate normalization; overdetermined homography solves the full rectangular DLT matrix by
  SVD rather than `A^T A`.
- Guided recovery expands the triangle seed set, and optional SIP is composed with the linear model
  before a single warp.
- Resampling supports nearest, bilinear, Keys bicubic, and Lanczos-2/3/4. Lanczos is normalized,
  signed calibrated samples are preserved, and unstable partial Lanczos falls back to bilinear.
- Warp returns separate coverage and interpolation-confidence planes; stacking uses coverage for
  inclusion and confidence for weighting.
- SIMD paths are extensively cross-checked against scalar references, including signed values and
  edges.

### 15.2 P0 correctness gaps

1. **SIP is fit in the wrong vector space for a non-identity base transform.**
   `fit_from_transform` fits `target - T(reference)` directly as the correction later applied
   *before* `T`. The required pre-transform target is `T^-1(target)-reference`, or a joint composite
   residual (§9.3). Rotation, scale, affine, and homography fixtures are required to expose this.
2. **RANSAC’s advertised confidence is not valid for the implemented sampling schedule.** The first
   two thirds of draws are weighted samples from the top quarter/half of candidates, but adaptive
   termination uses the uniform formula and can stop before the uniform phase. Either use uniform
   draws, or implement a sampler-specific termination proof.
3. **Public warp geometry is implicit.** `warp` allocates the source image’s dimensions and copies
   source metadata even though output coordinates are reference coordinates. The end-to-end
   pipeline assumes equal sensor dimensions and later replaces final metadata, but the standalone
   registration API cannot correctly represent different reference dimensions or an explicit
   union/intersection grid.
4. **Warp quality can describe a different kernel from the value.** At a partial Lanczos boundary,
   the value and confidence use bilinear fallback while coverage remains the nominal Lanczos
   absolute-weight fraction. The target requires actual and nominal support to be distinguished.

### 15.3 P1 precision and robustness gaps

1. Registration consumes `Star` positions without centroid covariance and treats all matches
   equally. The noise scale is `max(median_FWHM/2, 0.5)`, which measures seeing rather than centroid
   precision and can be orders of magnitude too broad for bright stars.
2. The triangle invariant uses a fixed absolute 0.01 box tolerance, an absolute area-squared floor,
   and deterministic sorted-side roles without propagated uncertainty. Near-isosceles role swaps
   are not rejected.
3. Point matches are resolved greedily rather than by global weighted assignment; vote confidence
   is only `votes / maximum_votes`.
4. The scorer is now a well-behaved monotone saturating exponential, fixing the older non-monotone
   formula described by the previous document. It is still **MAGSAC-inspired**, not the exact
   MAGSAC++ loss, and local refinement is unweighted binary-inlier least squares rather than
   sigma-consensus++ IRLS.
5. Affine still solves normalized normal equations, so its condition number is squared. Homography
   is not polished by nonlinear geometric-error minimization.
6. Minimal degeneracy checks use raw one-pixel/cross-product thresholds. A homography sample checks
   whether all four points are collinear, but does not explicitly reject every three-collinear
   quadruple or weak spatial span.
7. Plausibility checks read one rotation/scale from the matrix. They do not validate homography
   denominator sign, full-field Jacobian determinant, local scale, foldover, or overlap.
8. Match recovery processes only the brightness-capped matching catalogs, greedily claims the
   nearest target in reference order, and declares convergence when only the **count** is unchanged.
   Changed identities at the same count can stop before refit. Its safety fallback can restore the
   original set after legitimate outlier removal.
9. `Auto` uses raw training RMS `<= 0.5 px` rather than covariance, PSF-blur budget, spatial
   holdout, or extrapolation. The final default acceptance ceiling remains two pixels.
10. SIP order is fixed by configuration. It uses polynomial normal equations, has no spatial
    cross-validation, does not jointly refit the base transform, and does not gate full-field
    correction/Jacobian behavior. TPS is implemented and tested but not wired.
11. `RegistrationResult` stores only scalar residual magnitude and an ad hoc quality score. It has
    no residual vectors, position/parameter covariance, standardized residuals, held-out metrics,
    spatial field diagnostics, or domain summary.
12. Warp does not propagate source variance or masks. Its confidence assumes spatially independent
    homoscedastic input, and no output noise-correlation description is emitted.
13. Fixed kernels do not anti-alias material minification, and no explicit surface-brightness versus
    flux-per-pixel/Jacobian policy exists.
14. Automatic sequence reference selection maximizes star count only. It does not consider FWHM,
    framing, overlap, transform connectivity, or uncertainty, and there is no global multi-frame
    refinement.

### 15.4 Suggested implementation order

1. Correct SIP fit direction and add rotated/scaled/affine/homography distortion tests.
2. Make output grid/dimensions/metadata explicit and make quality describe the actual kernel.
3. Add centroid covariance to the Stage-3/Stage-4 contract and use Mahalanobis residuals throughout.
4. Replace the statistically invalid sampling/termination combination and implement robust weighted
   nonlinear final refinement.
5. Replace greedy recovery with global assignment over the full valid catalogs and fix convergence.
6. Add full-domain transform checks and PSF/spatial-cross-validation model selection.
7. Propagate mask/variance and resampling-correlation diagnostics.
8. Add two-pass reference selection and optional sequence-wide refinement.

---

## 16. Verification requirements

All new or changed non-GUI behavior requires exact tests. Randomized tests use recorded seeds and
compare against analytical truth or an independent implementation.

### 16.1 Coordinates and direction

- Identity and integer translations preserve exact pixel centers.
- A known reference-to-target transform maps catalog points and inverse-warp samples in the same
  direction.
- FITS one-based `CRPIX` round-trips without an off-by-one shift.
- Different reference/target dimensions produce the requested reference, intersection, union, and
  explicit grids with correct metadata.

### 16.2 Triangle matching

- Translation, rotation, positive scale, and configured reflection leave invariants unchanged.
- Propagated side/invariant covariance matches Monte Carlo perturbations.
- Near-isosceles role ambiguity is rejected at the exact uncertainty boundary.
- Elongation, normalized area, minimum baseline, and parity gates have hand-computed boundary cases.
- Dense and sparse vote storage produce identical weighted edges and assignments.
- Global assignment beats a constructed case where greedy resolution is suboptimal.
- False stars, missing stars, repeated coordinates, partial overlap, and cross-filter flux changes do
  not create a false accepted model.

### 16.3 Transform solvers

- Every model recovers hand-generated exact parameters at minimal and overdetermined counts.
- Weighted translation and anisotropic-covariance refinement match hand-solved normal systems.
- Coordinate shifts/scales over realistic and extreme image sizes do not change the denormalized
  answer.
- Affine QR/SVD and homography DLT are cross-checked against an independent library.
- Nonlinear homography refinement lowers geometric—not merely algebraic—error.
- Coincident, collinear, three-of-four-collinear, tiny-hull, singular, reflective, and projective-
  horizon cases fail with the correct reason.

### 16.4 Robust estimation

- Incomplete-gamma tables and `rho`, derivative, cutoff continuity, and outlier constant match a
  high-precision scalar MAGSAC++ reference at boundaries and dense interior samples.
- Uniform RANSAC iteration counts match the exact finite-population formula for known `I`, `M`,
  `s`, `p`, and enumerated degenerate subsets.
- Seeded runs are byte-deterministic; changing the seed changes hypotheses but not a strong final
  solution.
- Exact inliers plus 10%, 50%, 80%, and structured outliers recover truth within predicted
  covariance.
- Local optimization and final refit improve or preserve the robust objective; rejected trial steps
  cannot replace the best model.

### 16.5 Covariance and model selection

- Monte Carlo centroid draws give `d^2` distributed as `chi-square(2)` when covariance is correct.
- The `sigma_sys` bisection reaches median `2 ln 2` and trips its configured cap on model error.
- Parameter covariance predicts empirical transform scatter over repeated simulations.
- Euclidean truth selects Euclidean; scale selects Similarity; shear selects Affine; projective truth
  selects Homography; noise alone does not upgrade.
- A high-order model with lower training RMS but worse held-out corners loses spatial
  cross-validation.
- The PSF-blur gate is checked from a rendered Gaussian star and the analytical quadrature formula.

### 16.6 Match recovery

- Global recovery resolves crossed nearest-neighbor conflicts optimally.
- Pair identities changing at constant count trigger another refit.
- Seed outliers are removed and never restored by a count-preservation fallback.
- Full-catalog recovery finds valid faint/spatial edge controls omitted from asterism selection.
- Assignment, transform parameters, and robust objective converge together.

### 16.7 SIP and TPS

- Every polynomial basis term is recovered independently and in combination.
- A non-identity rotation, scale, affine map, and homography followed by known SIP correction is
  recovered in the correct pre-transform domain; the deliberately wrong raw-residual fit must fail
  these tests.
- Internal normalized and exported FITS coefficients produce identical corrections after `CRPIX`
  conversion.
- Robust fitting rejects injected control outliers.
- Spatial holdout selects the true order and rejects an overfit higher order.
- Adaptive boundary/grid checks detect a fold or correction peak between coarse grid nodes.
- TPS regularization changes behavior as expected, reproduces affine functions, and never silently
  extrapolates beyond policy.

### 16.8 Resampling

- Every kernel has exact center, constant, impulse, and linear-ramp tests.
- Scalar, SIMD, direct-coordinate, and incremental-step paths match over translations, rotations,
  affine transforms, homographies, SIP, negative samples, tiny images, and row tails.
- Border tests cover exact half-pixel footprint boundaries and neighboring floating-point values.
- Partial-kernel fallback sets the correct bit and reports support/confidence for the actual kernel.
- Hand-computed bilinear and signed-kernel variance equals `sum a_i^2 V_i`.
- Mask bits propagate at exact absolute-weight thresholds; invalid taps never contaminate values.
- Output-to-output covariance for adjacent samples matches the analytical shared-tap formula.
- Constant surface brightness and isolated total flux behave correctly under non-unit Jacobian for
  both declared photometric modes.
- Flux-mode variance changes by the square of the hand-computed Jacobian area factor.
- Minifying checkerboards and sinusoids verifies anti-aliasing; near-identity Lanczos retains its
  sharper expected response.
- Values outside support never influence stack normalization or rejection, regardless of border
  fill.

### 16.9 Sequence tests

- Two-pass selection rejects a sharpness/overlap-poor first frame and a blurred star-rich frame.
- A graph-connected sequence bootstraps a distant frame but resamples every original only once.
- Global refinement reduces weighted residual without moving the fixed reference gauge.
- Disconnected components, mosaic overlap, separate filters, parity branches, and moving-target
  composition have explicit success/failure fixtures.
- Real-data tests report registration RMS relative to FWHM, spatial residual maps, accepted/dropped
  frames, coverage, and run time—not only “completed successfully.”

---

## 17. Sources and inspected open-source implementations

### 17.1 Primary literature and standards

- E. J. Groth, 1986, “A Pattern-Matching Algorithm for Two-Dimensional Coordinate Lists,”
  *AJ* 91, 1244 — [ADS PDF](https://adsabs.harvard.edu/pdf/1986AJ.....91.1244G),
  [DOI](https://doi.org/10.1086/114099).
- F. Valdes, L. Campusano, J. D. Velasquez, and P. B. Stetson, 1995, “FOCAS Automatic
  Catalog Matching Algorithms,” *PASP* 107, 1119 —
  [DOI](https://doi.org/10.1086/133667).
- M. Beroiz, J. B. Cabral, and B. Sanchez, 2020, “Astroalign: A Python Module for
  Astronomical Image Registration” — [arXiv:1909.02946](https://arxiv.org/abs/1909.02946).
- M. A. Fischler and R. C. Bolles, 1981, “Random Sample Consensus,” *CACM* 24, 381 —
  [DOI](https://doi.org/10.1145/358669.358692).
- O. Chum, J. Matas, and J. Kittler, 2003, “Locally Optimized RANSAC” —
  [paper](https://cmp.felk.cvut.cz/~matas/papers/chum-dagm03.pdf).
- D. Barath et al., 2020, “MAGSAC++, a Fast, Reliable and Accurate Robust Estimator” —
  [arXiv:1912.05909](https://arxiv.org/abs/1912.05909); journal version,
  [DOI](https://doi.org/10.1109/TPAMI.2021.3103562).
- S. Umeyama, 1991, “Least-Squares Estimation of Transformation Parameters Between Two Point
  Patterns” — [DOI](https://doi.org/10.1109/34.88573),
  [PDF mirror](https://web.stanford.edu/class/cs273/refs/umeyama.pdf).
- R. Hartley, 1997, “In Defence of the 8-Point Algorithm” —
  [PDF](https://users.cecs.anu.edu.au/~hartley/Papers/fundamental/fundamental.pdf).
- D. L. Shupe et al., “The SIP Convention for Representing Distortion in FITS Image Headers” —
  [FITS registry specification](https://fits.gsfc.nasa.gov/registry/sip/SIP_distortion_v1_0.pdf),
  [Caltech record](https://authors.library.caltech.edu/records/fqnyw-qdw79/latest).
- F. J. Masci, D. Makovoz, and M. Moshir, 2004, “A Robust Algorithm for the Pointing Refinement
  and Registration of Astronomical Images” —
  [arXiv:astro-ph/0411322](https://arxiv.org/abs/astro-ph/0411322),
  [DOI](https://doi.org/10.1086/424495).
- A. S. Fruchter and R. N. Hook, 2002, “Drizzle: A Method for the Linear Reconstruction of
  Undersampled Images” — [arXiv:astro-ph/9808087](https://arxiv.org/abs/astro-ph/9808087),
  [DOI](https://doi.org/10.1086/338393).
- D. Lang et al., 2010, “Astrometry.net: Blind
  Astrometric Calibration of Arbitrary Astronomical Images” —
  [arXiv:0910.2233](https://arxiv.org/abs/0910.2233).

### 17.2 Official documentation

- [Siril registration documentation](https://siril.readthedocs.io/en/latest/preprocessing/registration.html)
  for global triangle matching, transform choices, two-pass reference selection, framing, and
  interpolation behavior.
- [OpenCV USAC tutorial](https://docs.opencv.org/4.x/de/d3e/tutorial_usac.html) for robust-estimator
  components and sampler/termination distinctions.
- [Astropy SIP API](https://docs.astropy.org/en/latest/api/astropy.wcs.Sip.html) for forward and
  inverse SIP roles and pixel-origin handling.
- [`reproject_adaptive`](https://reproject.readthedocs.io/en/stable/api/reproject.reproject_adaptive.html)
  and [`reproject_exact`](https://reproject.readthedocs.io/en/stable/api/reproject.reproject_exact.html)
  for adaptive anti-aliasing and flux-conserving spherical-polygon reprojection.

### 17.3 Pinned source revisions inspected

| Project | Revision | Relevant source behavior |
|---|---|---|
| [Astroalign](https://github.com/quatrope/astroalign/tree/76077b2583601611f5f8f2d52be47ca481f77ab1) | `76077b2583601611f5f8f2d52be47ca481f77ab1` | Five-neighbor triangle invariants, invariant k-d-tree matching, triangle RANSAC, correspondence cleanup, inverse warp |
| [MAGSAC](https://github.com/danini/magsac/tree/d259f8b3a8925025e45667241fb68629b07603bb) | `d259f8b3a8925025e45667241fb68629b07603bb` | Exact MAGSAC++ incomplete-gamma loss, `n=4`, `k=3.64`, preemptive scoring, adaptive termination |
| [astrometry.net](https://github.com/dstndstn/astrometry.net/tree/623b3c31a7a5566c1fde8d0a32445aa2ee31b8b3) | `623b3c31a7a5566c1fde8d0a32445aa2ee31b8b3` | Quad hashing, full-field verification, iterative correspondence/WCS refinement, SIP evaluation and fitting |
| [Siril](https://gitlab.com/free-astro/siril/-/tree/8ce9baa37215ae9783de16fa9e0d7a610303588d) | `8ce9baa37215ae9783de16fa9e0d7a610303588d` | Valdes-style triangle votes, iterative re-matching, robust transform fitting, registration modes and resampling |
| [SWarp](https://github.com/astromatic/swarp/tree/bf4f496f18c04a8d32022b45449ef8675ab9b3da) | `bf4f496f18c04a8d32022b45449ef8675ab9b3da` | Astronomical image inverse resampling and Lanczos-2/3/4 kernels |

These projects are evidence and cross-checks, not APIs Lumos must copy. Where the target differs—
notably covariance-aware residuals, spatial cross-validation, actual-kernel quality, and explicit
variance/mask propagation—the stricter contract in this document controls.
