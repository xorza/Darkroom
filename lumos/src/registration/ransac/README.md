# RANSAC Module

Robust estimation of geometric transformations (translation, Euclidean, similarity, affine, homography) from noisy point correspondences using RANSAC with local optimization and SIMD-accelerated inlier counting.

## Structure

- `mod.rs` — `RansacEstimator`, transform estimation functions, sampling utilities (~500 lines)
- `simd/mod.rs` — Runtime SIMD dispatch and scalar fallback (~115 lines)
- `simd/sse.rs` — AVX2 and SSE2 inlier counting (~230 lines)
- `simd/neon.rs` — NEON inlier counting for aarch64 (~130 lines)
- `tests.rs` — 55 tests covering all transform types, edge cases, plausibility, SIMD parity, degeneracy

## Usage

Called from two places:

1. **Pipeline** (`pipeline/mod.rs`) — `estimate_with_matches()` after triangle matching, using `PointMatch` confidences for progressive sampling. Also calls `estimate_transform()` directly during unmatched-star recovery.
2. **Astrometry solver** (`astrometry/solver.rs`) — `estimate()` for plate solving with quad-hash matches.

## API

- `RansacEstimator::new(config) -> Self`
- `estimate(ref_points, target_points, transform_type) -> Option<RansacResult>`
- `estimate_progressive(ref_points, target_points, confidences, transform_type) -> Option<RansacResult>`
- `estimate_with_matches(matches, ref_stars, target_stars, transform_type) -> Option<RansacResult>`

Internal:
- `estimate_transform(ref, target, type) -> Option<Transform>` — dispatch to specific estimator
- `estimate_similarity(ref, target)` — Procrustes analysis (centroid, covariance, angle, scale)
- `estimate_affine(ref, target)` — normal equations (3x3 solve via Cramer's rule)
- `estimate_homography(ref, target)` — DLT with point normalization and SVD via nalgebra
- `adaptive_iterations(inlier_ratio, sample_size, confidence)` — RANSAC iteration formula
- `count_inliers(ref, target, transform, threshold)` — SIMD-dispatched inlier counting

## Algorithm

### Standard RANSAC (`estimate`)

1. Random sample `min_points` correspondences (Floyd's algorithm)
2. Degeneracy check — reject coincident/collinear samples
3. Estimate candidate transform
4. Plausibility check (rotation/scale bounds) — reject before expensive inlier count
5. Count inliers via SIMD (AVX2/SSE2/NEON with scalar fallback)
6. If promising + LO enabled: iterative refinement on inlier set (LO-RANSAC)
7. Update best model if score improved
8. Adaptive early termination: `N = log(1-confidence) / log(1-w^n)`
9. Final refinement: re-estimate with all inliers via least squares

Both `estimate` and `estimate_progressive` share a single `ransac_loop` implementation parameterized by sampling strategy.

### Scoring

MSAC scoring (truncated quadratic):
```
score = sum(threshold² - dist²) for all inliers
```
Points closer to the model contribute more. Uses native `f64` precision throughout (no integer truncation). This is better than pure inlier counting (RANSAC) but uses a fixed threshold unlike MAGSAC++.

### Progressive RANSAC (`estimate_progressive`)

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
| `inlier_threshold` | 2.0 px | Tighter than OpenCV (3.0); matches Astroalign for sub-pixel |
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
| **MSAC** (Torr & Zisserman, 2000) | Truncated quadratic scoring | Implemented (f64, threshold² − dist²) |
| **LO-RANSAC** (Chum et al., 2003) | Local optimization of best hypothesis | Implemented (iterative LS refinement) |
| **PROSAC** (Chum & Matas, 2005) | Progressive sampling from sorted correspondences | Partial (3-phase pool, not true PROSAC) |
| Degeneracy checks | SupeRANSAC (2025) | Implemented (coincidence + collinearity) |
| **Adaptive iteration** (standard) | `N = log(1-p)/log(1-w^n)` | Implemented |
| **Plausibility constraints** | Domain-specific rejection | Implemented (rotation + scale bounds) |
| **SIMD inlier counting** | AVX2/SSE2/NEON with scalar fallback | Implemented |

Custom implementation chosen over Rust ecosystem crates (`arrsac`, `inlier`, `linear-ransac`) for astronomy-specific plausibility constraints, direct SIMD integration, and tight coupling with the triangle matching pipeline.

## Future Work

- **MAGSAC++**: Threshold-free scoring via marginalization, if fixed-threshold MSAC proves limiting.
- **LO inner-loop allocation**: Pre-allocate buffers for LO refinement (low priority, typical iteration count is 3–5).

## Declined

- **True PROSAC** (Chum & Matas, 2005): PROSAC's 10–100x speedup applies to high-outlier-rate scenarios with large minimal sample sizes (e.g. m=7 fundamental matrix, thousands of SIFT matches at 20% inliers). For star registration with ~200 points, m=2 (similarity), and >50% inliers after triangle matching, standard RANSAC converges in ~18 iterations — the loop takes microseconds regardless of sampling strategy. The current 3-phase heuristic is sufficient.

## References

- [RANSAC (Wikipedia)](https://en.wikipedia.org/wiki/Random_sample_consensus)
- [MSAC/MLESAC (Torr & Zisserman, 2000)](https://www.robots.ox.ac.uk/~vgg/publications/2000/Torr00/torr00.pdf)
- [LO-RANSAC (Chum et al., 2003)](https://link.springer.com/chapter/10.1007/978-3-540-45243-0_31)
- [PROSAC (Chum & Matas, 2005)](https://cmp.felk.cvut.cz/~matas/papers/chum-prosac-cvpr05.pdf)
- [MAGSAC++ (Barath & Matas, 2020)](https://openaccess.thecvf.com/content_CVPR_2020/papers/Barath_MAGSAC_a_Fast_Reliable_and_Accurate_Robust_Estimator_CVPR_2020_paper.pdf)
- [SupeRANSAC (2025)](https://arxiv.org/html/2506.04803v1)
- [OpenCV USAC Framework](https://docs.opencv.org/4.x/de/d3e/tutorial_usac.html)
