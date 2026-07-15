# RANSAC Transform Estimation

The RANSAC stage fits a configured geometric transform to triangle-voted `PointMatch` values while
rejecting false correspondences. Its estimator and results are crate-internal; `RansacConfig` is
public through the composed `RegistrationConfig`.

## Algorithm

1. Convert matched indices into reference/target point pairs and vote-derived confidences.
2. Draw minimal samples with a three-phase progressive policy: the strongest quarter, strongest
   half, then the full set.
3. Reject coincident or collinear samples and implausible rotation/scale hypotheses.
4. Score residuals with the continuous robust loss in `magsac.rs`; `max_sigma` is derived at
   runtime from median stellar FWHM.
5. Refine promising hypotheses with LO-RANSAC over the current inliers.
6. Tighten the iteration bound from the observed inlier ratio and configured confidence.
7. Refit the winner from all inliers.

The scorer is MAGSAC-inspired but is not the paper's closed-form MAGSAC++ rho. Its effective inlier
cutoff is approximately `3.03 * max_sigma`; scoring remains continuous inside that support.

## Transform solvers

| Model | Minimum points | Solver |
|-------|----------------|--------|
| Translation | 1 | Mean displacement |
| Euclidean | 2 | Procrustes with unit scale |
| Similarity | 2 | Procrustes with uniform scale |
| Affine | 3 | Hartley-normalized least squares |
| Homography | 4 | Hartley-normalized DLT with SVD |

`transforms.rs` also supplies the direct refits used during match recovery.

## Configuration

| Field | Default | Meaning |
|-------|---------|---------|
| `max_iterations` | `2000` | Hypothesis ceiling |
| `confidence` | `0.995` | Adaptive-termination target |
| `min_inlier_ratio` | `0.3` | Floor used by adaptive termination |
| `seed` | `None` | Random or deterministic sampling |
| `local_optimization` | `true` | Enable LO-RANSAC |
| `lo_iterations` | `10` | LO refinement ceiling |
| `max_rotation` | 10 degrees | Optional plausibility bound |
| `scale_range` | `0.8..1.2` | Optional uniform-scale bound |

`RegistrationConfig::wide_field` removes the rotation and scale bounds. `mosaic` removes the
rotation bound and widens the scale interval.

## Layout

- `mod.rs`: configuration, sampling, degeneracy/plausibility checks, scoring loop, and LO.
- `magsac.rs`: robust continuous residual scorer.
- `transforms.rs`: model-specific direct and least-squares fits.
- `tests.rs`: recovery, degeneracy, bounds, determinism, and model accuracy.
