# RANSAC Module

Robust estimation of geometric transformations (translation, Euclidean, similarity, affine, homography) from noisy point correspondences using RANSAC with local optimization and SIMD-accelerated inlier counting.

## Structure

- `mod.rs` — `RansacEstimator`, transform estimation functions, sampling utilities (~580 lines)
- `simd/mod.rs` — Runtime SIMD dispatch and scalar fallback (~115 lines)
- `simd/sse.rs` — AVX2 and SSE2 inlier counting (~230 lines)
- `simd/neon.rs` — NEON inlier counting for aarch64 (~130 lines)
- `tests.rs` — 47 tests covering all transform types, edge cases, plausibility, SIMD parity

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
2. Estimate candidate transform
3. Plausibility check (rotation/scale bounds) — reject before expensive inlier count
4. Count inliers via SIMD (AVX2/SSE2/NEON with scalar fallback)
5. If promising + LO enabled: iterative refinement on inlier set (LO-RANSAC)
6. Update best model if score improved
7. Adaptive early termination: `N = log(1-confidence) / log(1-w^n)`
8. Final refinement: re-estimate with all inliers via least squares

### Scoring

Truncated inverse-distance score (MSAC-like):
```
score = sum((threshold² - dist²) * 1000) for all inliers
```
Points closer to the model contribute more. This is better than pure inlier counting (RANSAC) but uses a fixed threshold unlike MAGSAC++.

### Progressive RANSAC (`estimate_progressive`)

3-phase sampling guided by match confidence:
- Phase 1 (0–33% iters): weighted sample from top 25% by confidence
- Phase 2 (33–66%): weighted sample from top 50%
- Phase 3 (66–100%): uniform random (convergence guarantee)

Uses Algorithm A-Res (reservoir sampling with weights) for weighted selection.

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

## Comparison with Best Practices

### Literature Review

| Technique | Reference | Status in Our Code |
|-----------|-----------|-------------------|
| **RANSAC** (Fischler & Bolles, 1981) | Basic random sampling + inlier counting | Implemented |
| **MSAC** (Torr & Zisserman, 2000) | Truncated quadratic scoring | Partially implemented (we use inverse-distance scoring, similar spirit) |
| **LO-RANSAC** (Chum et al., 2003) | Local optimization of best hypothesis | Implemented (iterative LS refinement) |
| **PROSAC** (Chum & Matas, 2005) | Progressive sampling from sorted correspondences | Partially implemented (3-phase pool, not true PROSAC) |
| **LO+-RANSAC** (Lebeda et al., 2012) | Improved LO with shrinking threshold + inner RANSAC | Not implemented |
| **MAGSAC++** (Barath & Matas, 2020) | Threshold-free scoring via marginalization | Not implemented |
| **SPRT** (Chum & Matas, 2008) | Preemptive model verification | Not implemented |
| **GC-RANSAC** (Barath & Matas, 2018) | Graph-cut based local optimization | Not implemented |
| Degeneracy checks | SupeRANSAC (2025) | Not implemented |
| **Adaptive iteration** (standard) | `N = log(1-p)/log(1-w^n)` | Implemented |
| **Plausibility constraints** | Domain-specific rejection | Implemented (rotation + scale bounds) |
| **SIMD inlier counting** | Intel PCL optimization (AVX2/NEON) | Implemented |

### Comparison with Rust Ecosystem

| Crate | Features | Notes |
|-------|----------|-------|
| `sample-consensus` + `arrsac` (rust-cv) | Generic RANSAC traits, ARRSAC variant, SPRT | Trait-based abstractions; good for generic use |
| `inlier` | RANSAC/MSAC/MAGSAC/ACRANSAC, PROSAC, local optimization, learned priors | Most comprehensive; large dependency |
| `linear-ransac` | Line fitting with auto-threshold | Too specialized |

### Verdict: Keep Custom Implementation

Reasons to keep:
- **Astronomy-specific**: Plausibility constraints (rotation/scale bounds), progressive sampling from match confidences, integration with `PointMatch` and triangle matching pipeline.
- **SIMD integration**: Direct AVX2/SSE2/NEON for inlier counting with the exact scoring function we need.
- **Simplicity**: ~580 lines for the core, easy to modify for domain-specific needs.
- **Sufficient for star matching**: With 200 stars and 2-point samples, standard RANSAC converges in <50 iterations for typical inlier ratios (>50%).

Reasons to consider `inlier` crate:
- If MAGSAC++ (threshold-free) scoring becomes important.
- If PROSAC's formal progressive sampling shows measurable speedup.
- If the pipeline handles significantly noisier data (>50% outliers).

## Suggested Improvements

### 1. Use MSAC scoring consistently (minor, correctness)

**Current**: Score uses `(threshold² - dist²) * 1000`, which is a truncated quadratic — essentially MSAC scoring. But the threshold comparison uses strict `<` instead of `<=`, and the scalar multiplier `1000.0` with `usize` truncation loses precision for near-threshold points.

**Suggested**: Use `f64` for scores internally to avoid truncation artifacts. The `* 1000` + `as usize` conversion loses sub-pixel discrimination. Recent research (RANSAC Scoring Functions, Dec 2025) confirms MSAC scoring is the strongest simple baseline — no need to change the formula, just fix the precision.

### 2. Add sample degeneracy check (minor, robustness)

**Current**: No check for degenerate samples. For similarity (2 points), if both sampled points are nearly identical, the transform estimation will produce garbage or return `None` from the `ref_var < 1e-10` check.

**Suggested**: Before calling `estimate_transform`, check that sampled points are sufficiently spread. For 2-point similarity, check `dist(p1, p2) > min_distance`. For 3-point affine, check that points are not collinear. For 4-point homography, check no 3 are collinear. This avoids wasted iterations on degenerate samples.

### 3. True PROSAC instead of 3-phase heuristic (moderate, speed)

**Current**: `estimate_progressive` uses a coarse 3-phase pool expansion (25% → 50% → 100%) with weighted reservoir sampling within each phase.

**Suggested**: Implement true PROSAC (Chum & Matas, 2005): sort correspondences by confidence, progressively expand the sampling set one point at a time, drawing the newest point in each sample. This has a formal convergence guarantee and can be 10–100x faster than uniform RANSAC when the quality ordering is good (which it is for triangle matching confidences). Falls back to RANSAC naturally when ordering is uninformative.

### 4. Remove code duplication between `estimate` and `estimate_progressive` (simplification)

**Current**: `estimate` and `estimate_progressive` are ~100 lines each with nearly identical logic (model estimation, plausibility check, inlier counting, LO, adaptive termination, final refinement). The only difference is sampling strategy.

**Suggested**: Extract the common RANSAC loop into a single function parameterized by a sampling strategy (uniform vs weighted/progressive). This eliminates ~80 lines of duplication and ensures bug fixes apply to both paths.

### 5. Simplify `refine_transform` (simplification)

**Current**: `refine_transform` is a trivial wrapper around `estimate_transform` — the function body is literally `estimate_transform(ref_points, target_points, transform_type)`. It adds no value.

**Suggested**: Remove `refine_transform` and call `estimate_transform` directly at the refinement sites. The name "refine" suggests something more sophisticated (iterative re-weighting, shrinking threshold) but it's just a re-estimation.

### 6. Use `select_nth_unstable` for weighted sampling (minor, perf)

**Current**: `weighted_sample_into` does a full `O(n log n)` sort to find top-k keys. The comment acknowledges this and notes it's acceptable for n≤200.

**Suggested**: Use `select_nth_unstable_by` for `O(n)` average-case selection of top-k. This is the same optimization applied to the k-d tree build. Not critical for n=200 but cleaner.

### 7. Pre-compute threshold² (minor, clarity)

**Current**: `threshold_sq = threshold * threshold` is computed in multiple places: `count_inliers_scalar_impl`, each SIMD function, and implicitly in the SIMD broadcast.

**Suggested**: Compute `threshold_sq` once in `count_inliers_simd` and pass it to all implementations, or store it alongside `threshold` in config. Minor cleanup.

### 8. Avoid `Vec` allocation in `local_optimization` inner loop (moderate, perf)

**Current**: Each LO iteration allocates two `Vec<DVec2>` for inlier points (`inlier_ref`, `inlier_target`) and `count_inliers` allocates a `Vec<usize>` for the result.

**Suggested**: Pre-allocate these buffers outside the LO loop and reuse them. For the inlier index vector, change `count_inliers` to accept a `&mut Vec<usize>` buffer (similar to `radius_indices_into` in the k-d tree). This eliminates ~10 allocations per LO cycle.

## References

- [RANSAC (Wikipedia)](https://en.wikipedia.org/wiki/Random_sample_consensus)
- [MSAC/MLESAC (Torr & Zisserman, 2000)](https://www.robots.ox.ac.uk/~vgg/publications/2000/Torr00/torr00.pdf)
- [LO-RANSAC (Chum et al., 2003)](https://link.springer.com/chapter/10.1007/978-3-540-45243-0_31)
- [PROSAC (Chum & Matas, 2005)](https://cmp.felk.cvut.cz/~matas/papers/chum-prosac-cvpr05.pdf)
- [LO+-RANSAC (Lebeda et al., 2012)](https://cmp.felk.cvut.cz/software/LO-RANSAC/Lebeda-2012-Fixing_LORANSAC-BMVC.pdf)
- [MAGSAC++ (Barath & Matas, 2020)](https://openaccess.thecvf.com/content_CVPR_2020/papers/Barath_MAGSAC_a_Fast_Reliable_and_Accurate_Robust_Estimator_CVPR_2020_paper.pdf)
- [RANSAC Scoring Functions: Analysis and Reality Check (Dec 2025)](https://arxiv.org/html/2512.19850)
- [SupeRANSAC (2025)](https://arxiv.org/html/2506.04803v1)
- [OpenCV USAC Framework](https://docs.opencv.org/4.x/de/d3e/tutorial_usac.html)
- [rust-cv/arrsac](https://github.com/rust-cv/arrsac)
- [inlier crate](https://github.com/soraxas/inlier)
- [sample-consensus crate](https://github.com/rust-cv/sample-consensus)
