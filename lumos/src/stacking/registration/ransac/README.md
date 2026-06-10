# RANSAC Module

Robust estimation of geometric transformations (translation, Euclidean, similarity, affine, homography) from noisy point correspondences using RANSAC with MAGSAC++ scoring and local optimization.

## Structure

- `mod.rs` — `RansacEstimator`, transform estimation, sampling utilities (~500 lines)
- `magsac.rs` — MAGSAC++ scorer with gamma function LUT (~100 lines)
- `transforms.rs` — Transform estimation (similarity, affine, homography)
- `tests.rs` — Tests covering all transform types, edge cases, plausibility, degeneracy

## Usage

Called from the pipeline (`pipeline/mod.rs`) via `estimate()` after triangle matching, using `PointMatch` confidences for progressive sampling. Also calls `estimate_transform()` directly during unmatched-star recovery.

## API

- `RansacEstimator::new(params) -> Self`
- `estimate(matches, ref_stars, target_stars, transform_type) -> Option<RansacResult>`

Internal:
- `estimate_transform(ref, target, type) -> Option<Transform>` — dispatch to specific estimator
- `estimate_similarity(ref, target)` — Procrustes analysis (centroid, covariance, angle, scale)
- `estimate_affine(ref, target)` — normal equations (3x3 solve via Cramer's rule)
- `estimate_homography(ref, target)` — DLT with point normalization and SVD via nalgebra
- `adaptive_iterations(inlier_ratio, sample_size, confidence)` — RANSAC iteration formula
- `score_hypothesis(ref, target, transform, scorer, &mut inliers)` — MAGSAC++ scoring

## Algorithm

### RANSAC with Progressive Sampling (`estimate`)

1. Extract point pairs and confidences from `PointMatch` objects
2. Build sorted index by confidence for progressive sampling
3. Sample using 3-phase strategy (see below)
4. Degeneracy check — reject coincident/collinear samples
5. Estimate candidate transform
6. Plausibility check (rotation/scale bounds) — reject before expensive scoring
7. Score hypothesis with MAGSAC++ (continuous marginalized likelihood)
8. If promising + LO enabled: iterative refinement on inlier set (LO-RANSAC)
9. Update best model if score improved
10. Adaptive early termination: `N = log(1-confidence) / log(1-w^n)`
11. Final refinement: re-estimate with all inliers via least squares

### MAGSAC++ Scoring

MAGSAC++ (Barath & Matas, 2020) replaces fixed-threshold scoring with marginalized likelihood:
```
loss(r²) = ∫₀^σ_max (1 - γ(k/2, r²/2σ²)) · (1/σ_max) dσ
```
For 2D points (k=2 DOF), this simplifies to gamma function lookup via `γ(1,x) = 1 - e^(-x)`.

Benefits over MSAC:
- **No threshold tuning**: The `max_sigma` parameter specifies the noise scale, not a hard cutoff
- **Soft scoring**: Points contribute proportionally to their likelihood, not binary inlier/outlier
- **Effective threshold**: Points with residuals > ~3·max_sigma are treated as outliers (99% χ² quantile)

### Progressive Sampling Strategy

3-phase sampling guided by match confidence:
- Phase 1 (0–33% iters): weighted sample from top 25% by confidence
- Phase 2 (33–66%): weighted sample from top 50%
- Phase 3 (66–100%): uniform random (convergence guarantee)

Uses Algorithm A-Res (reservoir sampling with weights) with `select_nth_unstable` for O(n) selection.

### LO-RANSAC

When a new best model is found:
1. Re-estimate transform using all current inliers (least squares)
2. Recount inliers with refined transform
3. Repeat if improved, up to `lo_max_iterations` (default 10)

### Transform Types

| Type | DOF | Min Points | Method |
|------|-----|-----------|--------|
| Translation | 2 | 1 | Average displacement |
| Euclidean | 3 | 2 | Similarity with scale=1 |
| Similarity | 4 | 2 | Procrustes (centroid + covariance) |
| Affine | 6 | 3 | Normal equations (Cramer's rule) |
| Homography | 8 | 4 | DLT + SVD (nalgebra), point normalization |

### Configuration Defaults

| Parameter | Default | Rationale |
|-----------|---------|-----------|
| `max_iterations` | 2000 | Matches OpenCV/PixInsight |
| `max_sigma` | 1.0 px | MAGSAC++ noise scale (~3px effective threshold) |
| `confidence` | 0.995 | OpenCV default; avoids premature termination |
| `min_inlier_ratio` | 0.3 | Handles partial overlap / noisy matches |
| `use_local_optimization` | true | LO-RANSAC (Chum et al., 2003) |
| `lo_max_iterations` | 10 | Convergence typically in 3–5 |
| `max_rotation` | 10° | Tracked mounts; set `None` for mosaics |
| `scale_range` | (0.8, 1.2) | Same telescope; set `None` for mixed setups |

## Techniques

| Technique | Reference | Status |
|-----------|-----------|--------|
| **RANSAC** (Fischler & Bolles, 1981) | Basic random sampling + inlier counting | Implemented |
| **MAGSAC++** (Barath & Matas, 2020) | Marginalized likelihood scoring | Implemented (gamma LUT, max_sigma param) |
| **LO-RANSAC** (Chum et al., 2003) | Local optimization of best hypothesis | Implemented (iterative LS refinement) |
| **PROSAC** (Chum & Matas, 2005) | Progressive sampling from sorted correspondences | Partial (3-phase pool, not true PROSAC) |
| Degeneracy checks | SupeRANSAC (2025) | Implemented (coincidence + collinearity) |
| **Adaptive iteration** (standard) | `N = log(1-p)/log(1-w^n)` | Implemented |
| **Plausibility constraints** | Domain-specific rejection | Implemented (rotation + scale bounds) |

Custom implementation chosen over Rust ecosystem crates (`arrsac`, `inlier`, `linear-ransac`) for astronomy-specific plausibility constraints and tight coupling with the triangle matching pipeline.

## Declined

- **True PROSAC** (Chum & Matas, 2005): PROSAC's 10–100x speedup applies to high-outlier-rate scenarios with large minimal sample sizes (e.g. m=7 fundamental matrix, thousands of SIFT matches at 20% inliers). For star registration with ~200 points, m=2 (similarity), and >50% inliers after triangle matching, standard RANSAC converges in ~18 iterations — the loop takes microseconds regardless of sampling strategy. The current 3-phase heuristic is sufficient.

## References

- [RANSAC (Wikipedia)](https://en.wikipedia.org/wiki/Random_sample_consensus)
- [MAGSAC++ (Barath & Matas, 2020)](https://openaccess.thecvf.com/content_CVPR_2020/papers/Barath_MAGSAC_a_Fast_Reliable_and_Accurate_Robust_Estimator_CVPR_2020_paper.pdf)
- [LO-RANSAC (Chum et al., 2003)](https://link.springer.com/chapter/10.1007/978-3-540-45243-0_31)
- [PROSAC (Chum & Matas, 2005)](https://cmp.felk.cvut.cz/~matas/papers/chum-prosac-cvpr05.pdf)
- [SupeRANSAC (2025)](https://arxiv.org/html/2506.04803v1)
- [OpenCV USAC Framework](https://docs.opencv.org/4.x/de/d3e/tutorial_usac.html)
