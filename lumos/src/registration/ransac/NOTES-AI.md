# RANSAC / MAGSAC++ Module

Robust estimation of geometric transformations from noisy 2D point
correspondences. Comparison against original papers and reference
implementations (OpenCV USAC, danini/magsac, PixInsight, Siril).

Files: `mod.rs` (~650 lines), `magsac.rs` (~170 lines),
`transforms.rs` (~425 lines), `tests.rs` (~2260 lines).

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

**Verdict**: The scoring formula is a correct k=2 specialization of the
MAGSAC++ marginalized likelihood. The closed-form `gamma_k2(x) = 1 - exp(-x)`
replaces the general gamma LUT needed for k>2. Differences from reference
implementations are normalization constants that do not affect model selection.

### Recent Analysis (2025)

Piedade et al., "RANSAC Scoring Functions: Analysis and Reality Check"
(arXiv:2512.19850, Dec 2025) proved that MAGSAC++ scoring is numerically
equivalent to a simple Gaussian-Uniform (GaU) likelihood model. They found
all scoring functions (MSAC, MAGSAC++, GaU, learned) perform identically
when properly tuned. This validates our simplified implementation -- the
complex normalization in reference code adds no practical benefit.

### Preemptive Scoring

`score_hypothesis` (`mod.rs:620-647`) accepts `best_score` and exits early
when cumulative loss exceeds `-best_score`. This is a correct optimization
that avoids scoring all N points when a hypothesis is clearly worse than
the current best. Standard technique, also used in OpenCV USAC.

## RANSAC Loop (`mod.rs`)

### Progressive 3-Phase Sampling (`mod.rs:459-483`)

Sampling strategy inspired by PROSAC (Chum & Matas, CVPR 2005):

| Phase | Iterations | Pool | Method |
|-------|-----------|------|--------|
| 1 | 0-33% | Top 25% by confidence | Weighted (A-Res) |
| 2 | 33-66% | Top 50% by confidence | Weighted (A-Res) |
| 3 | 66-100% | Full set | Uniform random |

Weight formula: `(confidence + 0.1)^2` (`mod.rs:447`).

**Comparison with true PROSAC**: PROSAC uses a mathematically derived growth
function `T'(n)` that grows the sampling set one element at a time based on
the current best inlier ratio. This provides theoretical guarantees about
convergence speed. Our 3-phase approach is a coarser approximation with
fixed boundaries.

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

Applied in the main loop (`mod.rs:347-354`) only when `inlier_ratio >=
min_inlier_ratio`. This prevents premature termination when the current
best has very few inliers. Correct behavior.

### LO-RANSAC (`mod.rs:173-241`)

**Paper reference**: Chum, Matas, Kittler, "Locally Optimized RANSAC,"
DAGM 2003. The full LO-RANSAC prescribes:
1. Least-squares re-estimation on inliers
2. Inner RANSAC with non-minimal samples on the inlier set
3. Iterative threshold shrinking

**Our implementation**: Only step (1) with iterative re-scoring. Loop:
re-estimate from inliers -> re-score -> update if improved -> repeat up to
`lo_max_iterations` (default 10). Convergence check: stop if inlier count
and score both fail to improve.

**Trigger condition** (`mod.rs:318-320`): LO runs only when a new-best
hypothesis is found (`score > best_score`). This matches standard
LO-RANSAC and OpenCV USAC behavior. The theory guarantees that LO is
applied at most O(log k) times where k is total iterations drawn.

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

### Degeneracy Detection (`mod.rs:573-608`)

Checks both reference and target point samples:
- **Coincident pairs**: `distance^2 < 1.0` (1 pixel minimum)
- **Collinear triples**: `|cross_product| < 1.0`

**Not handled**: For homography with 4 points, three collinear + one
off-line creates a rank-deficient DLT matrix. The SVD solver returns a
poor model that gets low MAGSAC++ score and is effectively rejected.
Not a correctness issue but wastes an iteration.

### Plausibility Checks (`mod.rs:148-162`)

Domain-specific rejection of implausible hypotheses before expensive
scoring. Checks rotation angle and scale factor against configured bounds.
Applied to both initial hypotheses and LO-refined results (`mod.rs:333`).

This is a custom feature not found in standard RANSAC literature. It is
useful for astrophotography where physically implausible transforms
(large rotations, extreme scales) indicate mismatched star pairs rather
than valid geometric relationships.

### Buffer Management

Pre-allocated buffers for samples, inliers, and LO working space
(`mod.rs:264-270`). Partial Fisher-Yates shuffle with undo
(`mod.rs:533-567`) avoids O(n) re-initialization per iteration.

**Issue**: `inlier_buf = lo_inliers` at `mod.rs:335` replaces the
pre-allocated buffer with a new Vec from `local_optimization`, defeating
buffer reuse for subsequent iterations. The old buffer becomes `lo_inliers`
(via the return from `local_optimization`) and may be re-allocated on
next LO call. Not a correctness bug but creates unnecessary allocations.

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

Min points: 3. **Correct**.

### Homography (`transforms.rs:252-329`)

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

Min points: 4. **Correct**.

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

The full SupeRANSAC pipeline includes: MAGSAC++ scoring, GC-RANSAC for
local optimization, progressive NAPSAC sampling, SPRT for early model
rejection, and DEGENSAC for homography degeneracy. We implement the most
impactful component (MAGSAC++ scoring) while substituting simpler
alternatives for the others (iterative LS for LO, 3-phase progressive
for PROSAC/NAPSAC). For our star registration use case (200 matches at
50%+ inlier ratio), the additional components provide diminishing returns.

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
uses TPS for distortion modeling on top of the global transform, which
is analogous to our SIP distortion correction in the broader pipeline.

### Siril

Uses triangle similarity matching + RANSAC. Supports all 5 transform
types (shift/Euclidean/similarity/affine/homography). Recommends
homography for wide-field images. Similar architecture to ours.

### GC-RANSAC (Barath & Matas, CVPR 2018)

Replaces LO step with graph-cut energy minimization incorporating spatial
coherence. Not relevant for star registration -- stars are sparse and
spatially well-separated, so spatial coherence constraints add nothing.

## Issues Found

### Active Issues

1. **LO buffer replacement** (`mod.rs:335`):
   `inlier_buf = lo_inliers` replaces the pre-allocated Vec with a new one
   from `local_optimization`, defeating buffer reuse. Should write into
   `inlier_buf` directly via `extend_from_slice` or pass it as output
   parameter. **Severity**: Low (extra allocations, no correctness impact).

2. **Progressive phase boundaries don't adapt** (`mod.rs:464`):
   `let phase = iteration * 3 / max_iter` uses the original `max_iter`,
   not the adaptively reduced count. After early convergence detection
   reduces effective iterations, the phase progression doesn't adjust.
   In practice, with ~18 iterations for 50% inlier ratio, only phase 1
   runs. **Severity**: Negligible for typical workloads.

3. **`#[allow(dead_code)]` on pub fields** (`mod.rs:126-131`):
   `RansacResult::iterations` and `::inlier_ratio` are `pub` with
   `#[allow(dead_code)]`. If they are public API, the annotation is
   misleading. If diagnostic-only, use `pub(crate)`.
   **Severity**: Code quality only.

4. ~~**Affine estimation uses normal equations without normalization**~~ -- **FIXED**.
   Now uses Hartley normalization (`normalize_points()` + denormalize via
   `compose`), same pattern as the homography estimator. Condition number
   is well-controlled for all practical coordinate ranges.

### Resolved Issues (from previous analysis)

- Direct SVD for homography DLT (was using A^T*A) -- FIXED
- Target point degeneracy check -- FIXED
- Gamma LUT replaced with closed-form `1-exp(-x)` -- FIXED
- Confidence default mismatch (0.999 vs 0.995) -- FIXED

## IRWLS -- Postponed

MAGSAC++ paper's key contribution for model estimation (not just scoring)
is IRWLS with sigma-marginalized weights. We use MAGSAC++ only for scoring,
with binary inlier selection for least-squares estimation in LO-RANSAC.

**Rationale for postponing**: Star registration has clean inlier/outlier
separation. Centroid accuracy (~0.1-0.3 px) is the limiting factor, not
the estimator's ability to handle borderline inliers. LO-RANSAC + SIP
distortion correction already achieve sub-pixel registration accuracy.
IRWLS would help in scenarios with gradual inlier/outlier transition
(e.g., high distortion fields), which are uncommon in practice.

## Potential Improvements (Prioritized)

1. **Fix LO buffer replacement** (`mod.rs:335`): Pass `inlier_buf` as
   output parameter to `local_optimization` or use `extend_from_slice`.
   Eliminates unnecessary allocations. Effort: 15 minutes.

2. ~~**Add point normalization to affine estimation**~~ -- **DONE**.
   Hartley normalization + denormalize via compose, same as homography.

3. **IRWLS final polish**: After RANSAC selects the best model, run 3-5
   IRWLS iterations with sigma-marginalized weights from MAGSAC++. Would
   improve accuracy for borderline inliers. Effort: 2 hours.

4. **True PROSAC**: Replace 3-phase heuristic with PROSAC growth function.
   Theoretical improvement for high-outlier-rate scenarios, negligible for
   current star registration workloads. Effort: 2 hours.

5. **Inner RANSAC in LO**: Sample non-minimal subsets from inliers instead
   of using the full set. Would help when inlier set contains structured
   outliers. Effort: 1 hour.

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
