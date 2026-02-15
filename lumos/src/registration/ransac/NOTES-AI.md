# RANSAC / MAGSAC++ Module

Robust estimation of geometric transformations from noisy 2D point
correspondences. Comparison against original papers and reference
implementations (OpenCV USAC, danini/magsac, PixInsight, Siril,
SupeRANSAC).

Files: `mod.rs` (~650 lines), `magsac.rs` (~280 lines),
`transforms.rs` (~435 lines), `tests.rs` (~2240 lines).

## Architecture

```
RansacEstimator::estimate(matches, ref_stars, target_stars, type)
  |
  +-- Extract points + confidences from PointMatch
  +-- Build sorted index by confidence (descending)
  +-- ransac_loop(ref, target, n, min_samples, type, sample_fn)
        |
        +-- MagsacScorer::new(max_sigma)
        +-- Loop: sample -> degeneracy check -> estimate -> plausibility check
        |         -> score_hypothesis (MAGSAC++) -> LO-RANSAC -> update best
        +-- Adaptive early termination
        +-- Final LS refinement on all inliers
```

Public API:
- `RansacEstimator::new(params)` / `estimate(matches, ref, target, type)`
- `estimate_transform(ref, target, type)` -- direct estimator dispatch
- `adaptive_iterations(inlier_ratio, sample_size, confidence)`

## MAGSAC++ Scoring (`magsac.rs`)

### Paper Reference

Barath et al., "MAGSAC++, a Fast, Reliable and Accurate Robust Estimator,"
CVPR 2020. The core idea: instead of a fixed inlier/outlier threshold,
marginalize over all noise scales sigma in [0, sigma_max].

### Mathematical Derivation

For 2D point correspondences (k=2 degrees of freedom), residual distances
follow a chi distribution with 2 DOF scaled by sigma. The MAGSAC++ loss
per point integrates the inlier likelihood over sigma:

```
loss(r) = integral_0^sigma_max [ density_term(r, sigma) ] dsigma
```

After integration (closed-form for k=2), the result is:

```
loss(r^2) = sigma_max^2/2 * gamma(1, x) + r^2/4 * (1 - gamma(1, x))
```

where:
- `x = r^2 / (2 * sigma_max^2)`
- `gamma(1, x) = 1 - exp(-x)` (lower incomplete gamma for shape=1)

Points with `r^2 > chi_quantile * sigma_max^2` are classified as outliers
and receive constant loss = `sigma_max^2 / 2`.

The chi-squared 99% quantile for k=2 is `chi2_0.99(2) = 9.2103`, giving
an effective threshold of `sqrt(9.21) * sigma_max ~ 3.03 * sigma_max`.

### Comparison with Reference Implementations

**Our implementation** (`magsac.rs:63-75`):
```rust
let x = residual_sq / (2.0 * self.max_sigma_sq);
let gx = gamma_k2(x);  // 1 - exp(-x)
self.max_sigma_sq / 2.0 * gx + residual_sq / 4.0 * (1.0 - gx)
```

**danini/magsac** (`magsac.h`, `getModelQualityPlusPlus`):
```cpp
loss = maximum_sigma_2_per_2 * stored_lower_incomplete_gamma_values[x]
     + squared_residual / 4.0 * (stored_complete_gamma_values[x] - gamma_value_of_k);
loss *= two_ad_dof_plus_one_per_maximum_sigma;
```

**OpenCV USAC** (`quality.cpp`, `MagsacQualityImpl`):
```cpp
loss = 1 - (maximum_sigma_2_per_2 * stored_lower_incomplete_gamma_values[x]
     + squared_residual * 0.25 * (stored_complete_gamma_values[x] - gamma_value_of_k))
     * norm_loss;
```

Key differences:
1. **`gamma_value_of_k` correction**: Reference implementations subtract
   `gamma_value_of_k` (~0.01 for k=2) from the complete gamma term. For k=2,
   `complete_gamma - gamma_value_of_k` = `Gamma(1) - P(1, chi_quantile/2)`
   = `1 - (1-exp(-chi_quantile/2))` = `exp(-chi_quantile/2)` ~ 0.01.
   Our code uses `(1 - gamma_k2(x))` which equals `exp(-x)` -- the same
   functional form but without subtracting the small constant. At the
   threshold boundary, this creates a ~1% discrepancy. **Impact: negligible**
   -- model ranking is preserved since the offset is constant across all
   points for a given model.

2. **Normalization**: Reference implementations apply `two_ad_dof_plus_one /
   sigma_max` scaling and `norm_loss` normalization. We skip this. Since we
   use negative total loss as score (higher = better), and all models use
   the same scorer, the relative ranking is identical. **Impact: none**.

3. **Outlier handling**: Our code assigns constant `sigma_max^2/2` to
   outliers. OpenCV skips outliers entirely (they contribute 0 to loss).
   danini/magsac assigns `outlier_loss = maximum_sigma * 2^(DoF-1) *
   lower_gamma_value_of_k`. Since outlier count is constant across model
   candidates, all three approaches preserve relative model ranking.
   **Impact: none for scoring**.

4. **Boundary discontinuity**: The comment on `magsac.rs:46` says
   "ensuring continuity" but `outlier_loss = sigma_max^2/2` is not
   exactly equal to the formula's value at the threshold. At
   `r^2 = threshold_sq`:
   - `x = chi_quantile/2 = 4.605`, `gamma(1, 4.605) = 0.990`
   - `loss = 0.495*sigma_max^2 + 0.023*sigma_max^2 = 0.518*sigma_max^2`
   - `outlier_loss = 0.500*sigma_max^2`
   - Discontinuity: ~3.6% of sigma_max^2.
   The test at line 209 acknowledges this. **Impact: negligible** -- the
   jump is small and happens at an extreme where the point is nearly
   certainly an outlier anyway. The danini/magsac reference also uses an
   approximate outlier loss constant.

**Verdict**: The scoring formula is a correct k=2 specialization of the
MAGSAC++ marginalized likelihood. The closed-form `gamma_k2(x) = 1 - exp(-x)`
replaces the general gamma LUT needed for k>2. Differences from reference
implementations are normalization constants that do not affect model selection.

### Recent Analysis (Piedade et al. 2025)

Piedade et al., "RANSAC Scoring Functions: Analysis and Reality Check"
(arXiv:2512.19850, Dec 2025) proved that MAGSAC++ scoring is numerically
equivalent to a simple Gaussian-Uniform (GaU) likelihood model. Key findings:

1. **All scoring functions perform identically** when properly tuned with
   dense threshold validation: MSAC, MAGSAC++, GaU, and even learned
   scoring functions.

2. **MAGSAC++ derivation has three errors** (degree counting, likelihood
   confusion, weight justification) that fortuitously cancel, producing
   results equivalent to GaU for k=4. For k=2 (our case), the equivalence
   also holds.

3. **Threshold sensitivity is equivalent** across methods -- contrary to
   prior claims, MAGSAC++ is not meaningfully less sensitive to threshold
   choice than MSAC when the comparison accounts for different
   parameterization scales.

4. **Practical implication**: Our simplified formula (no normalization, no
   gamma_value_of_k correction) is validated. The complex normalization in
   reference implementations adds no benefit -- it only affects absolute
   score magnitude, not model ranking.

This means we could replace MAGSAC++ with simple MSAC (truncated squared
residual) and get identical results, as long as the threshold is properly
set. The MAGSAC++ formulation is kept because it provides a principled way
to set the threshold from sigma_max (noise scale), which is more intuitive
than a raw pixel threshold.

### Preemptive Scoring

`score_hypothesis` (`mod.rs:623-651`) accepts `best_score` and exits early
when cumulative loss exceeds `-best_score`. This is a correct optimization
that avoids scoring all N points when a hypothesis is clearly worse than
the current best. Standard technique, also used in OpenCV USAC and
SupeRANSAC (called "upper-bound strategy" in their paper).

## RANSAC Loop (`mod.rs`)

### Progressive 3-Phase Sampling (`mod.rs:459-483`)

Sampling strategy inspired by PROSAC (Chum & Matas, CVPR 2005):

| Phase | Iterations | Pool | Method |
|-------|-----------|------|--------|
| 1 | 0-33% | Top 25% by confidence | Weighted (A-Res) |
| 2 | 33-66% | Top 50% by confidence | Weighted (A-Res) |
| 3 | 66-100% | Full set | Uniform random |

Weight formula: `(confidence + 0.1)^2` (`mod.rs:451`).

**Comparison with true PROSAC**: PROSAC uses a mathematically derived growth
function `T'(n)` that grows the sampling set one element at a time based on
the current best inlier ratio. This provides theoretical guarantees about
convergence speed. Our 3-phase approach is a coarser approximation with
fixed boundaries.

**Comparison with SupeRANSAC sampling**: SupeRANSAC uses PROSAC for
epipolar geometry and P-NAPSAC (PROSAC + spatial locality) for homography
and pose problems. P-NAPSAC "extends PROSAC by integrating spatial
coherency prior" -- sampling points that are spatially close. For star
registration, spatial coherence offers no benefit since stars are sparse.

**Practical impact for star registration**: Low. With ~200 matched points,
min_samples=2 (similarity), and >50% inlier ratio after triangle matching,
standard RANSAC converges in ~18 iterations. The sampling strategy barely
matters -- the loop takes microseconds. PROSAC's 10-100x speedup applies
to scenarios with large minimal samples (m=7 for fundamental matrix) and
thousands of SIFT matches at 20% inliers.

Weighted sampling uses Algorithm A-Res (Efraimidis & Spirakis 2006) with
`select_nth_unstable` for O(n) average-case partitioning. Implementation
is correct.

### Adaptive Iteration Count (`transforms.rs:13-33`)

```
N = ceil(log(1 - confidence) / log(1 - w^n))
```

Exactly matches the standard formula (Fischler & Bolles, 1981). Edge cases:
- `w <= 0` or `w >= 1`: returns 1
- `w^n >= 1`: returns 1
- `log_outlier >= 0`: returns 1000 (fallback)

Applied in the main loop (`mod.rs:350-358`) only when `inlier_ratio >=
min_inlier_ratio`. This prevents premature termination when the current
best has very few inliers. Correct behavior.

### LO-RANSAC (`mod.rs:173-245`)

**Paper reference**: Chum, Matas, Kittler, "Locally Optimized RANSAC,"
DAGM 2003. The full LO-RANSAC prescribes:
1. Least-squares re-estimation on inliers
2. Inner RANSAC with non-minimal samples on the inlier set
3. Iterative threshold shrinking

**Our implementation**: Only step (1) with iterative re-scoring. Loop:
re-estimate from inliers -> re-score -> update if improved -> repeat up to
`lo_max_iterations` (default 10). Convergence check: stop if inlier count
and score both fail to improve.

**Trigger condition** (`mod.rs:322-324`): LO runs only when a new-best
hypothesis is found (`score > best_score`). This matches standard
LO-RANSAC and OpenCV USAC behavior. The theory guarantees that LO is
applied at most O(log k) times where k is total iterations drawn.

**Comparison with SupeRANSAC LO**: SupeRANSAC uses GC-RANSAC (graph-cut
energy minimization with spatial coherence term) for < 2000 correspondences,
and nested RANSAC (sample 7m points from inliers, where m is minimal
sample size) for larger sets. Our iterative LS is the simplest approach.

**Comparison with "Fixing LO-RANSAC"** (Lebeda, Matas, Chum, BMVC 2012):
The fix addresses the original LO-RANSAC's sensitivity to the inner
RANSAC threshold by using a series of decreasing thresholds. Our iterative
LS approach sidesteps this by not having an inner RANSAC at all -- the
full inlier set is used for each re-estimation, which is simpler but
potentially less robust to structured outliers near the boundary.

**Missing features**:
- Inner RANSAC (sampling non-minimal subsets from inliers) -- would help
  when inlier set contains borderline outliers
- Threshold shrinking -- would tighten the model progressively

**Impact**: Low for star registration. Star matches have clean
inlier/outlier separation (large residual gap between true matches and
mismatches). The iterative LS approach converges quickly.

### Degeneracy Detection (`mod.rs:577-612`)

Checks both reference and target point samples:
- **Coincident pairs**: `distance^2 < 1.0` (1 pixel minimum)
- **Collinear triples**: `|cross_product| < 1.0`

**Comparison with SupeRANSAC**: SupeRANSAC uses problem-specific
degeneracy checks:
- Homography: tests for "twisted" (self-intersecting) quadrilaterals
  using cross-product orientation checks
- Pose/Rigid: detects collinear 3D points via cross product magnitude
- Model-level: verifies homography determinant falls in plausible range

**What we do that SupeRANSAC does not need to**: We check degeneracy in
both ref and target samples. For star registration this is correct -- both
sets can have near-coincident points (close double stars, spurious
detections). SupeRANSAC does not check target-side degeneracy because in
computer vision the target is the projected point, not an independent set.

**Not handled**: For homography with 4 points, three collinear + one
off-line creates a rank-deficient DLT matrix. The SVD solver returns a
poor model that gets low MAGSAC++ score and is effectively rejected.
Not a correctness issue but wastes an iteration.

**Not handled**: "Twisted" quadrilateral check for homography (4 points
where the polygon self-intersects). SupeRANSAC adds this; not needed
for star registration where points are well-separated.

### Plausibility Checks (`mod.rs:148-162`)

Domain-specific rejection of implausible hypotheses before expensive
scoring. Checks rotation angle and scale factor against configured bounds.
Applied to both initial hypotheses and LO-refined results (`mod.rs:337`).

**Comparison with SupeRANSAC**: SupeRANSAC validates homography determinant
and rotation matrix properties (`R^T*R ~ I`, `det(R) ~ +1`). Our checks
serve the same purpose (reject physically impossible transforms) but are
domain-specific to astrophotography where rotation and scale have known
physical constraints.

**Assessment**: This is a valuable custom feature. It filters out
degenerate hypotheses that would waste MAGSAC++ scoring time. No changes
needed.

### Buffer Management

Pre-allocated buffers for samples, inliers, and LO working space
(`mod.rs:268-274`). Partial Fisher-Yates shuffle with undo
(`mod.rs:537-571`) avoids O(n) re-initialization per iteration.

**Buffer reuse**: `local_optimization` takes `inlier_buf: &mut Vec<usize>`
as an output parameter and writes improved inlier sets directly into it
via `std::mem::swap`. No allocations occur after initial setup. A local
`scratch_inliers` Vec is used for scoring candidates, and only swapped
into `inlier_buf` when an improvement is found.

Note: `local_optimization` allocates one `Vec<usize>` (scratch_inliers)
per call (`mod.rs:191`). Since LO is called at most O(log k) times, this
is negligible. Moving it to a pre-allocated buffer in `ransac_loop` would
eliminate it entirely.

## Transform Estimation (`transforms.rs`)

### Translation (`transforms.rs:54-67`)

Average displacement: `t = (1/n) * sum(target_i - ref_i)`.
Exact least-squares solution. Min points: 1. Correct.

### Euclidean (`transforms.rs:74-106`)

Constrained Procrustes with scale fixed at 1.0.

Cross-covariance matrix H from centered points:
```
H = [sxx sxy]    where sxx = sum(rc_x * tc_x), etc.
    [syx syy]
```

Rotation angle: `theta = atan2(sxy - syx, sxx + syy)`

This is the closed-form solution from the polar decomposition of H.
For k=2, `atan2(H_01 - H_10, H_00 + H_11)` gives the optimal rotation
angle that maximizes `trace(R^T * H)` subject to `det(R) = 1`.

Translation: `t = centroid_target - R * centroid_ref` (with scale=1).

Min points: 2. **Correct** -- verified against Procrustes analysis
literature (Umeyama, 1991).

### Similarity (`transforms.rs:109-163`)

Procrustes with scale. Same cross-covariance approach as Euclidean, plus:

```
scale = ((sxx + syy) * cos(theta) + (sxy - syx) * sin(theta)) / ref_var
```

where `ref_var = sum(||ref_i - centroid_ref||^2)`.

This is `trace(R^T * H) / sum(||ref_centered||^2)`, which is the standard
Procrustes scale formula (Umeyama 1991, Eggert et al. 1997).

Degenerate guards: `ref_var < 1e-10` and `scale <= 0.0`.
Min points: 2. **Correct**.

Minor: allocates `Vec<DVec2>` for centered points. Negligible for the
small point sets in RANSAC (2-50 points typically).

### Affine (`transforms.rs:170-260`)

Solves `target = A * ref + b` via normal equations with Hartley
normalization and explicit 3x3 matrix inverse (cofactor expansion).

1. **Normalize** both point sets using `normalize_points()`: translate
   centroid to origin, scale so average distance = sqrt(2). Same
   normalization as the homography estimator.

2. **Solve** the 3x3 normal equations in normalized space:

   System: `[sum_xx sum_xy sum_x] [a]   [sum_x_tx]`
           `[sum_xy sum_yy sum_y] [b] = [sum_y_tx]`
           `[sum_x  sum_y  n    ] [e]   [sum_tx  ]`

   Two systems solved (one for target x, one for target y) using the same
   inverse. Determinant check: `|det| < 1e-10`.

3. **Denormalize**: `A = T_target^{-1} * A_norm * T_ref`

**Comparison with OpenCV**: OpenCV uses SVD for affine estimation, which
avoids squaring the condition number. With Hartley normalization, the
normal equations approach is well-conditioned for all practical
coordinate ranges. The 3x3 inverse is verified by cofactor analysis.

**Comparison with SupeRANSAC**: SupeRANSAC uses "Normalized DLT" for
homography non-minimal solver, plus Levenberg-Marquardt refinement with
MAGSAC++ weighting. We use LS without weighting. Adding MAGSAC++ weights
to the final LS step would improve sub-pixel accuracy.

Min points: 3. **Correct**.

### Homography (`transforms.rs:263-340`)

DLT (Direct Linear Transform) with Hartley normalization:

1. **Normalize** both point sets: translate centroid to origin, scale so
   average distance = sqrt(2). This is the Hartley normalization (Hartley,
   "In Defense of the Eight-Point Algorithm," TPAMI 1997) that ensures
   numerical stability by conditioning the DLT matrix.

2. **Build 2n x 9 design matrix A**: Each point pair contributes 2 rows:
   ```
   [-x -y -1  0  0  0  x*x'  y*x'  x']
   [ 0  0  0 -x -y -1  x*y'  y*y'  y']
   ```

3. **Solve via SVD of A directly** (not A^T*A): The null space vector is
   the last row of V^T. This preserves condition number kappa instead of
   kappa^2. For the minimal 4-point case (8x9 matrix), zero-padded to 9x9
   so nalgebra's thin SVD produces all 9 right singular vectors.

4. **Denormalize**: `H = T_target^-1 * H_normalized * T_ref`

5. **Scale** so `H[8] = 1`.

**Comparison with Hartley & Zisserman**: Matches the standard DLT algorithm
from "Multiple View Geometry in Computer Vision" (Algorithm 4.2). The
normalization and direct SVD approach are best practices.

**Comparison with SupeRANSAC**: SupeRANSAC preprocesses correspondences
"such that centroid is translated to origin and average distance to origin
is sqrt(2)" -- the same Hartley normalization we use. They also apply
Levenberg-Marquardt refinement after the initial algebraic estimate. We
rely on LO-RANSAC for iterative refinement instead.

Min points: 4. **Correct**.

### Hartley Normalization (`transforms.rs:343-385`)

Isotropic scaling to average distance sqrt(2), centered at origin. Applied
to both affine and homography estimation.

**Verification**: The Hartley normalization is correctly implemented:
- Centroid computed and subtracted
- Average distance to origin computed
- Scale factor = sqrt(2) / avg_dist
- Normalization transform matrix correctly constructed
- Edge case: avg_dist < 1e-10 returns identity (degenerate, all points
  coincident)

**Literature match**: Hartley (TPAMI 1997) specifies exactly this
procedure: "isotropic scaling...the points are translated so that their
centroid is at the origin...they are then scaled so that the average
distance from the origin is sqrt(2)." Our implementation matches.

### Summary Table

| Type | DOF | Min Pts | Method | Numerical Stability |
|------|-----|---------|--------|-------------------|
| Translation | 2 | 1 | Average displacement | Exact |
| Euclidean | 3 | 2 | Constrained Procrustes | Excellent |
| Similarity | 4 | 2 | Procrustes + scale | Excellent |
| Affine | 6 | 3 | Hartley norm + normal equations (3x3 inverse) | Excellent |
| Homography | 8 | 4 | DLT + Hartley norm + SVD | Excellent |

## Comparison with Industry Implementations

### SupeRANSAC (Barath et al. 2025)

SupeRANSAC (arXiv:2506.04803) is a comprehensive unified RANSAC framework
that integrates and evaluates all major RANSAC components. Their key finding
for scoring: **MAGSAC++ is adopted as the default scoring function**, with
the authors noting it achieves "the best trade-off in terms of average model
accuracy and robustness to parameter choices." This directly validates our
choice of MAGSAC++ scoring.

The full SupeRANSAC pipeline includes:

| Component | SupeRANSAC | Our Implementation | Gap |
|-----------|-----------|-------------------|-----|
| Scoring | MAGSAC++ | MAGSAC++ (k=2 closed-form) | None |
| LO | GC-RANSAC (<2000 pts) / nested RANSAC (>2000 pts) | Iterative LS | Low impact |
| Sampling | PROSAC / P-NAPSAC (problem-specific) | 3-phase progressive | Low impact |
| Early rejection | SPRT (statistical pre-test) | Preemptive scoring only | Low impact |
| Degeneracy | Problem-specific (twisted quad, collinearity) | Coincidence + collinearity | Adequate |
| Final refinement | Cauchy-weighted IRWLS with halving thresholds | LS on binary inlier set | Improvement possible |
| Model validation | Determinant/rotation checks | Rotation + scale bounds | Domain-specific |

For our star registration use case (200 matches at 50%+ inlier ratio), the
additional components provide diminishing returns.

### OpenCV USAC Framework

OpenCV's Universal RANSAC (Raguram et al., TPAMI 2013, updated by
Barath et al.) includes: PROSAC sampling, MAGSAC/MAGSAC++ scoring,
LO-RANSAC (inner RANSAC + iterative LS), SPRT early model rejection,
GC-RANSAC (graph-cut spatial coherence), degeneracy detection (DEGENSAC
for homography).

**Features we have**: MAGSAC++ scoring, LO-RANSAC (simplified), adaptive
termination, degeneracy checks, progressive sampling.

**Features we lack**:
- SPRT (Sequential Probability Ratio Test) for early model rejection
- GC-RANSAC spatial coherence
- DEGENSAC homography degeneracy (3 collinear points)
- Inner RANSAC in LO step
- True PROSAC growth function

### PixInsight StarAlignment

Uses RANSAC with triangle matching, iterative refinement with thin plate
splines (TPS), and successive approximation. Key difference: PixInsight
applies RANSAC "successively with increasing tolerances" as a
predictor-corrector. Our approach uses a single RANSAC pass with LO
refinement, which is simpler and sufficient for well-matched star fields.

### Siril

Uses triangle similarity matching + OpenCV RANSAC. Supports shift,
Euclidean, similarity, affine, and homography. Recommends homography for
wide-field images. Since v1.4 (Dec 2025), supports SIP distortion
correction during alignment. Similar architecture to ours.

### GC-RANSAC (Barath & Matas, CVPR 2018)

Replaces LO step with graph-cut energy minimization incorporating spatial
coherence. Not relevant for star registration -- stars are sparse and
spatially well-separated, so spatial coherence constraints add nothing.
SupeRANSAC uses GC-RANSAC as its default LO but notes it is primarily
beneficial for dense correspondence sets.

## Issues Found

### Active Issues

1. **Misleading comment on outlier_loss continuity** (`magsac.rs:46`):
   Comment says "ensuring continuity" but `outlier_loss = sigma_max^2/2` is
   NOT exactly continuous at the threshold. Actual loss at threshold is
   ~0.518*sigma_max^2, giving a ~3.6% discontinuity. The test at line 209
   acknowledges this. **Fix**: Update the comment to say "approximate
   continuity" or compute exact loss at threshold boundary.
   **Severity**: Cosmetic (misleading comment, behavior is correct).

2. **Progressive phase boundaries don't adapt** (`mod.rs:468`):
   `let phase = iteration * 3 / max_iter` uses the original `max_iter`,
   not the adaptively reduced count. After early convergence detection
   reduces effective iterations, the phase progression doesn't adjust.
   In practice, with ~18 iterations for 50% inlier ratio, only phase 1
   runs. **Severity**: Negligible for typical workloads.

3. **LO scratch_inliers allocation** (`mod.rs:191`): `local_optimization`
   allocates a new `Vec<usize>` for `scratch_inliers` on each call. Since
   LO is called at most O(log k) times this is negligible, but it could
   be pre-allocated in `ransac_loop` and passed as a parameter.
   **Severity**: Negligible (micro-optimization).

4. ~~**`#[allow(dead_code)]` on pub fields**~~ -- **FIXED**: targeted per-field
   `#[allow(dead_code)] // Used in tests` annotations.

### Resolved Issues (from previous analysis)

- LO buffer replacement defeating reuse -- FIXED (output parameter + swap)
- Affine estimation without Hartley normalization -- FIXED
- Direct SVD for homography DLT (was using A^T*A) -- FIXED
- Target point degeneracy check -- FIXED
- Gamma LUT replaced with closed-form `1-exp(-x)` -- FIXED
- Confidence default mismatch (0.999 vs 0.995) -- FIXED

## What We Do Correctly (Verified Against Literature)

1. **MAGSAC++ scoring formula**: Correct k=2 specialization. Closed-form
   gamma replaces LUT. Validated by Piedade et al. (2025) as equivalent
   to optimal GaU likelihood. Adopted as default by SupeRANSAC (2025).

2. **Preemptive scoring**: Standard early-exit optimization, matches
   SupeRANSAC and OpenCV USAC.

3. **Adaptive iteration count**: Textbook formula (Fischler & Bolles 1981).
   Edge cases handled correctly. Applied only above min_inlier_ratio.

4. **LO-RANSAC trigger**: Only on new-best hypotheses. Matches OpenCV USAC
   behavior and theoretical O(log k) guarantee.

5. **Hartley normalization**: Correct isotropic scaling (avg dist = sqrt(2),
   centered at origin). Applied to both affine and homography. Matches
   the standard exactly (Hartley, TPAMI 1997).

6. **DLT homography**: Direct SVD on full rectangular A (preserves kappa
   vs kappa^2). Zero-padding for minimal case. Correct denormalization.
   Matches Hartley & Zisserman Algorithm 4.2.

7. **Procrustes estimation**: Translation, Euclidean, and Similarity all
   use correct closed-form Procrustes solutions (Umeyama 1991).

8. **Degeneracy detection**: Both ref and target sides checked. Coincidence
   + collinearity covers the main failure modes for star registration.

9. **Plausibility checks**: Domain-specific, applied before scoring (saves
   compute) and after LO (prevents LO drift). Good custom feature.

10. **Buffer management**: Pre-allocated, swap-based updates, partial
    Fisher-Yates with undo. No per-iteration allocations in main loop.

## What We Don't Do But Should Consider

### IRWLS Final Refinement (Medium Priority)

MAGSAC++ paper's key contribution beyond scoring is IRWLS with
sigma-marginalized weights. SupeRANSAC uses "Cauchy-weighted IRWLS with
iteratively halving thresholds" for final refinement. We use binary
inlier selection + unweighted LS.

**What it would give us**: Soft weighting of borderline inliers in the
final model. Points near the threshold get downweighted rather than
equally included. Improves sub-pixel accuracy.

**Why postponed**: Star registration has clean inlier/outlier separation.
Centroid accuracy (~0.1-0.3 px) is the limiting factor, not the estimator.
LO-RANSAC + SIP distortion correction already achieve sub-pixel accuracy.
IRWLS would help in scenarios with gradual inlier/outlier transition
(high distortion fields), which are uncommon.

**Effort**: ~2 hours. After RANSAC, run 3-5 IRWLS iterations with weights
`w_i = loss'(r_i^2) / r_i^2` where `loss'` is the MAGSAC++ loss derivative.

### SPRT Early Model Rejection (Low Priority)

Sequential Probability Ratio Test: evaluate model on randomly shuffled
points using Wald's SPRT to reject bad models early, potentially faster
than our preemptive scoring. OpenCV USAC and SupeRANSAC both use SPRT.

**Why not needed**: Our preemptive scoring already provides early exit.
With ~200 points and MAGSAC++ scoring (outlier loss = sigma_max^2/2),
bad models are rejected within the first few points. SPRT provides
theoretical optimality (minimizes expected number of evaluations) but
the practical difference is negligible at N=200.

### SIMD-Accelerated Scoring (Low Priority)

The `score_hypothesis` inner loop (`mod.rs:636-648`) applies a 3x3 matrix
transform and computes squared distance for each point. This could be
vectorized with AVX2 (4 f64 lanes) for ~3x throughput.

**Why not needed**: With ~200 points and ~18 iterations, scoring takes
~100us total. SIMD optimization would save ~70us. Not worth the complexity.

### Homography Determinant Check (Low Priority)

SupeRANSAC validates that the homography determinant falls in a "predefined
plausible range." We check rotation and scale bounds, which covers the
same territory for similarity/affine but not for homography specifically.

**Why not needed**: Our plausibility checks on rotation and scale already
catch most degenerate homographies. For star registration, homography is
rarely used (affine or similarity handles most cases). The SVD solver
naturally produces well-conditioned solutions when Hartley normalization
is applied.

## What We Do That's Not Needed (Unnecessary Complexity)

1. **Progressive 3-phase sampling**: For ~200 matches at 50%+ inlier ratio,
   uniform random sampling converges in ~18 iterations. The 3-phase
   weighted sampling adds ~100 lines of code (weighted_sample_into,
   sorted_indices, confidence weights) for negligible benefit. The entire
   RANSAC loop takes microseconds regardless.

   **Recommendation**: Keep it. The code is clean, well-tested, and the
   overhead is zero (the alternative is simpler code, not faster code).
   It does provide marginal benefit for harder cases (low inlier ratio,
   many matches) which can occur with poor seeing or crowded fields.

2. **Algorithm A-Res for weighted sampling** (`mod.rs:496-530`):
   Full reservoir sampling with `u.powf(1/w)` keys and
   `select_nth_unstable` partitioning. For k=2-4 samples from a pool
   of ~50-100, a simpler rejection sampling approach would work.

   **Recommendation**: Keep. Implementation is correct and clean. The
   O(n) cost per sample is dominated by the transform estimation anyway.

## Potential Improvements (Prioritized)

1. **IRWLS final polish** (Medium): After RANSAC selects the best model,
   run 3-5 IRWLS iterations with sigma-marginalized weights from MAGSAC++.
   Would improve accuracy for borderline inliers. Effort: 2 hours.

2. **Fix misleading comment** (Easy): Update `magsac.rs:46` to say
   "approximate continuity" or compute exact loss at threshold boundary.
   Effort: 5 minutes.

3. **True PROSAC** (Low): Replace 3-phase heuristic with PROSAC growth
   function. Theoretical improvement for high-outlier-rate scenarios,
   negligible for current star registration workloads. Effort: 2 hours.

4. **Inner RANSAC in LO** (Low): Sample non-minimal subsets from inliers
   instead of using the full set. Would help when inlier set contains
   structured outliers. Effort: 1 hour.

5. **Hoist LO scratch_inliers allocation** (Trivial): Pre-allocate in
   `ransac_loop` and pass to `local_optimization`. Effort: 10 minutes.

## References

- Fischler, Bolles. "Random Sample Consensus." CACM 1981.
- Barath, Matas. "MAGSAC: Marginalizing Sample Consensus." CVPR 2019.
  https://arxiv.org/abs/1803.07469
- Barath et al. "MAGSAC++, a Fast, Reliable and Accurate Robust Estimator."
  CVPR 2020. https://arxiv.org/abs/1912.05909
- Chum, Matas, Kittler. "Locally Optimized RANSAC." DAGM 2003.
  https://cmp.felk.cvut.cz/~matas/papers/chum-dagm03.pdf
- Lebeda, Matas, Chum. "Fixing the Locally Optimized RANSAC." BMVC 2012.
  https://bmva-archive.org.uk/bmvc/2012/BMVC/paper095/paper095.pdf
- Chum, Matas. "Matching with PROSAC -- Progressive Sample Consensus."
  CVPR 2005. https://cmp.felk.cvut.cz/~matas/papers/chum-prosac-cvpr05.pdf
- Barath, Matas. "Graph-Cut RANSAC." CVPR 2018.
  https://cmp.felk.cvut.cz/~matas/papers/barath-2018-gc_ransac-cvpr.pdf
- Raguram et al. "USAC: A Universal Framework for Random Sample Consensus."
  TPAMI 2013.
- Barath et al. "Progressive NAPSAC." 2019.
  https://arxiv.org/abs/1906.02295
- Hartley. "In Defense of the Eight-Point Algorithm." TPAMI 1997.
- Hartley, Zisserman. "Multiple View Geometry in Computer Vision." 2003.
- Umeyama. "Least-Squares Estimation of Transformation Parameters Between
  Two Point Patterns." TPAMI 1991.
- Efraimidis, Spirakis. "Weighted Random Sampling with a Reservoir." 2006.
- Piedade et al. "RANSAC Scoring Functions: Analysis and Reality Check."
  arXiv:2512.19850, Dec 2025.
- Barath et al. "SupeRANSAC: Unified Random Sample Consensus." 2025.
  https://arxiv.org/abs/2506.04803
- OpenCV USAC: https://docs.opencv.org/4.x/de/d3e/tutorial_usac.html
- danini/magsac: https://github.com/danini/magsac
- Siril registration: https://siril.readthedocs.io/en/stable/preprocessing/registration.html
- PixInsight StarAlignment: https://www.pixinsight.com/tutorials/sa-distortion/index.html
