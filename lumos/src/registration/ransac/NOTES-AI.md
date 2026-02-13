# RANSAC / MAGSAC++ Module - Research Analysis

Comparison against original papers and reference implementations (OpenCV USAC,
danini/magsac). Files: `mod.rs`, `magsac.rs`, `transforms.rs`, `tests.rs`.

## MAGSAC++ Scoring (magsac.rs)

Paper: Barath et al., "MAGSAC++", CVPR 2020.

### Per-Point Loss Formula

**Ours** (lines 120-133): `loss = σ²_max/2 * γ(1,x) + r²/4 * (1 - γ(1,x))`
where `x = r²/(2σ²_max)` and γ(1,x) = 1-exp(-x) for k=2.

**OpenCV** (MagsacQualityImpl): same structure but subtracts `gamma_value_of_k`
(≈0.01 for k=2) from the second term's gamma, and applies normalization factors
`two_ad_dof_plus_one_per_maximum_sigma` and `norm_loss` scaling to [0,1].

**Difference**: We omit the `-gamma_value_of_k` correction (~1% at boundary)
and the normalization. Ranking is preserved so practical impact is negligible.

**Verdict**: Structurally correct k=2 specialization.

### Gamma LUT (lines 14-68)

1024-entry LUT for γ(1,x) over [0, 4.605] with linear interpolation. Max error
~2.5e-6. `CHI_QUANTILE_SQ = 9.21` is correct (χ²₀.₉₉(2) = 9.2103). For k=2,
direct `1-exp(-x)` would be equally fast and exact -- LUT is unnecessary but
harmless.

### Outlier Loss (lines 102-105)

Ours: constant `σ²_max/2` penalty. OpenCV: outliers contribute 0 (skipped).
Both preserve model ranking since outlier count is constant per model.

### IRWLS -- Not Implemented

MAGSAC++ paper uses IRWLS for weighted model estimation (weights from sigma
marginalization). We use MAGSAC++ only for *scoring*, with binary inlier
selection for estimation via LO-RANSAC. Impact is moderate -- IRWLS helps
borderline cases, but star registration has clean inlier/outlier separation.

### Sigma Marginalization

Correctly implements the closed-form result of integrating over σ ∈ [0, σ_max].
The gamma-based loss formula is the pre-integrated result. This is correct.

## RANSAC Loop (mod.rs)

### Progressive 3-Phase Sampling (lines 447-469)

Phase 1 (0-33%): top 25% by confidence, weighted. Phase 2 (33-66%): top 50%.
Phase 3 (66-100%): uniform random.

This is closest to **PROSAC** (Chum & Matas 2005, quality-sorted progressive
sampling) but simplified with fixed phase boundaries. **P-NAPSAC** (Barath 2019)
uses *spatial* neighborhoods instead. Weight formula `(c+0.1)^2` (line 435)
is ad-hoc. A true PROSAC with continuous growth would be more principled and
potentially 10-100x faster in best case.

Weighted sampling uses Algorithm A-Res (Efraimidis & Spirakis 2006) with
`select_nth_unstable` for O(n) partitioning. Correct and efficient.

### Adaptive Iteration Count (transforms.rs lines 12-32)

`N = ceil(log(1-p) / log(1-w^n))` -- exactly matches the standard formula
(Fischler & Bolles 1981). Edge cases handled correctly.

### LO-RANSAC (lines 164-234)

Paper (Chum, Matas, Kittler 2003) prescribes: (1) LS re-estimation on inliers,
(2) inner RANSAC with non-minimal samples, (3) iterative threshold shrinking.

**Ours**: Only step (1) with iterative re-scoring. Default 10 iterations
(matches OpenCV USAC). Missing inner RANSAC and threshold shrinking.

**Issue**: LO triggers on *every* hypothesis with `inlier_buf.len() >= min_samples`
(line 310), not just new-best models. This wastes computation. Standard
LO-RANSAC and OpenCV run LO only on new best models.

## Transform Estimation (transforms.rs)

### Translation (lines 53-66)
Average displacement. Exact LS solution. Correct.

### Euclidean (lines 73-105)
Constrained Procrustes (scale=1). `angle = atan2(sxy-syx, sxx+syy)` from
cross-covariance H = ref^T * target. Sign convention verified: H_ij computed
as `Σ r_i_comp * t_j_comp`, rotation formula matches. Correct.

### Similarity (lines 108-162)
Procrustes with scale. `scale = trace(R^T*H) / Σ||r_i-r̄||²` expanded as
`((sxx+syy)*cos + (sxy-syx)*sin) / ref_var`. Degenerate guards at ref_var
and scale checks. Minor: allocates centered-point Vecs (negligible for small
sets). Correct.

### Affine (lines 165-248)
Normal equations with explicit 3x3 inverse. Condition number squared vs SVD,
but adequate for typical coordinate ranges. Matrix inverse verified by cofactor
analysis -- all 9 entries correct. OpenCV uses SVD here.

### Homography (lines 251-312)
DLT with Hartley normalization (centroid=0, avg dist=sqrt(2)). Builds 9x9
A^T A directly, solves via SVD. Denormalization: `H = T_tar^-1 * H_norm * T_ref`.

**Numerical note**: SVD of A^T A squares condition number vs direct SVD on
2n×9 matrix A. OpenCV and danini/magsac use direct SVD. With Hartley
normalization this is usually fine but less robust for ill-conditioned cases.

### Minimum Sample Sizes
Translation=1, Euclidean=2, Similarity=2, Affine=3, Homography=4. All correct.

### Degeneracy Detection (mod.rs lines 561-596)
Checks: coincident pairs (<1px), collinear triples (cross product <1.0).
Not handled: 3-collinear + 1 off-line in homography (rank-deficient A^T A).
The SVD solver returns a poor model rejected by MAGSAC++ scoring.

## Issues Found

1. **LO runs on every hypothesis** (moderate) -- should only run on new-best.
   Line 310 checks `inlier_buf.len() >= min_samples` but should also check
   `score > best_score`.
2. **Missing IRWLS** (moderate) -- paper's key contribution for model accuracy.
   Impact smaller for well-separated star matches.
3. **No SPRT** (minor) -- for 50-500 point sets, benefit is minimal.
4. **A^T A for homography** (minor) -- direct SVD on A more robust.
5. **Outlier penalty differs** (negligible) -- constant penalty vs skip.
6. **Fixed phase boundaries** (minor) -- PROSAC growth function more principled.

## Missing Features

- IRWLS polishing with sigma-marginalized weights
- SPRT early model rejection
- PROSAC continuous progressive sampling
- Preemptive scoring (break when loss exceeds best)
- Inner RANSAC / threshold shrinking in LO
- Graph-Cut spatial coherence (OpenCV USAC)

## Potential Improvements (Prioritized)

1. **LO on new-best only**: Check `score > best_score` before LO. Simple, high impact.
2. **Preemptive scoring**: Pass `best_score` to `score_hypothesis`, break early.
3. **IRWLS final polish**: 3-5 IRWLS iterations after model selection.
4. **Direct SVD for homography**: SVD on full 2n×9 matrix A.
5. **PROSAC sampling**: Replace 3-phase with continuous growth.
6. **Inline exp(-x)**: Replace GammaLut with direct computation for k=2.

## References

- Barath, Matas. "MAGSAC." CVPR 2019. https://arxiv.org/abs/1803.07469
- Barath et al. "MAGSAC++." CVPR 2020. https://arxiv.org/abs/1912.05909
- Chum, Matas, Kittler. "LO-RANSAC." DAGM 2003
- Lebeda, Matas, Chum. "Fixing LO-RANSAC." BMVC 2012
- Chum, Matas. "PROSAC." CVPR 2005
- Barath et al. "Progressive NAPSAC." 2019. https://arxiv.org/abs/1906.02295
- Hartley. "In Defense of the Eight-Point Algorithm." TPAMI 1997
- Fischler, Bolles. "Random Sample Consensus." CACM 1981
- OpenCV USAC: https://docs.opencv.org/4.x/de/d3e/tutorial_usac.html
- danini/magsac: https://github.com/danini/magsac
