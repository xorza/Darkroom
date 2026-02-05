# Image Registration Module

Production-grade astronomical image alignment: star matching, robust transformation estimation, high-quality warping, and distortion correction.

## Table of Contents

1. [Pipeline Overview](#pipeline-overview)
2. [Algorithm Research and Industry Comparison](#algorithm-research-and-industry-comparison)
3. [Triangle Matching](#triangle-matching)
4. [RANSAC Estimation](#ransac-estimation)
5. [Transform Types](#transform-types)
6. [Distortion Correction](#distortion-correction)
7. [Interpolation (Image Warping)](#interpolation-image-warping)
8. [Phase Correlation](#phase-correlation)
9. [Astrometry (Plate Solving)](#astrometry-plate-solving)
10. [Quality Metrics](#quality-metrics)
11. [Module Structure](#module-structure)
12. [Configuration Reference](#configuration-reference)
13. [Performance](#performance)
14. [Pending Improvements](#pending-improvements)
15. [References](#references)

---

## Pipeline Overview

```
Star Positions (sorted by brightness)
  |
  v
Triangle Formation (k-d tree neighbors)
  |
  v
Invariant-Space KDTree Matching & Voting
  |
  v
Greedy Match Resolution (one-to-one)
  |
  v
RANSAC with Progressive Sampling (PROSAC-style)
  |
  +---> LO-RANSAC refinement on promising hypotheses
  |
  v
[SIP Distortion Correction] -- optional polynomial refinement
  |
  v
Output: Transform + Matched Star Pairs + Quality Metrics
```

The pipeline receives pre-detected stars sorted by brightness and produces a geometric transformation mapping reference coordinates to target coordinates, along with quality metrics. Image warping is a separate step applied after registration.

---

## Algorithm Research and Industry Comparison

### How Other Software Solves This Problem

#### Siril (Free/Open Source)

Siril uses triangle similarity matching on the brightest 20 stars, building N(N-1)(N-2)/6 triangles. RANSAC (from OpenCV) rejects outlier matches before computing the homography. Supports Shift, Euclidean, Similarity, Affine, and Homography transforms. Since v1.3, SIP distortion coefficients correct star positions before linear transformation. Configurable `maxstars` (100-2000). Default transform is Homography.

**Key difference from our implementation:** Siril uses brute-force O(n^3) triangle formation on a small star subset (20 stars by default). We use k-d tree based triangle formation on up to 200 stars, which is O(n*k^2) and scales better.

#### Astroalign (Python)

Astroalign characterizes three-point asterisms using geometric hashes invariant to translation, rotation, and scaling. Triangle invariants are generated from 5 nearest neighbors via KDTree. A second KDTree matches source invariants against target invariants with radius search (r=0.1). RANSAC then estimates the affine transformation. Accepts transforms matching 80% of triangle matches or 10 stars minimum.

**Key similarity:** Like Astroalign, we use a KDTree on the invariant feature space for matching triangle patterns. This handles varying feature density gracefully and avoids bin boundary artifacts that hash tables suffer from.

#### PixInsight StarAlignment (Commercial)

Uses iterative convergence with increasingly flexible models. First iteration generates a projective (linear) model. Subsequent iterations apply thin-plate splines, generate new star pair matches, and run RANSAC with increasing tolerances. Converges when the corrector matrix approaches identity. Uses surface simplifiers based on PCA to concentrate control points where distortion is highest (typically corners) while using fewer in low-distortion areas (center).

**Key difference:** PixInsight's iterative approach starts with a rigid model and progressively adds flexibility. Our pipeline fits a single global model (homography) then optionally adds SIP polynomial correction. PixInsight's approach is more robust for severe distortion but more complex. Their PCA-based point selection is more sophisticated than our uniform grid.

#### Astrometry.net (Plate Solving)

Uses quadrilaterals instead of triangles. For each quad of 4 stars, computes 5 distance ratios (d2/d1 through d6/d1) that are invariant to rotation, scale, and flipping. These hash codes are compared against pre-computed catalog hashes. After finding quad matches, Bayesian verification checks if reference catalog stars appear at predicted positions in the image.

**Key difference:** Astrometry.net solves a harder problem (blind identification against a catalog of billions of stars) using quads for higher discriminating power. Our triangle-based approach is sufficient for image-to-image registration where both images show the same field.

#### ASTAP (Free)

Similar quad-based approach to Astrometry.net. Selects brightest stars, forms tetrahedrons of 4 closest stars, computes 6 distance ratios. For low star counts (<30), extracts triples from quads as fallback. Uses linear astrometric solution (6 plate constants) for both stacking and solving.

### Algorithm Design Decisions

#### Why Triangles Instead of Quads

Triangles need fewer stars per pattern (3 vs 4), forming more patterns from sparse fields. Triangle ratio space is 2D (two independent ratios after normalization), making KDTree-based matching efficient. Quads offer higher discriminating power (5 independent ratios) but are primarily needed for blind plate solving against large catalogs, not for image-to-image registration where the star fields largely overlap.

#### Why KDTree on Invariants Instead of Hash Table

A KDTree on the 2D invariant space (side ratios) provides exact radius queries with no bin boundary artifacts. For our typical workload (~2000 triangles), O(log n) lookup is negligible. The KDTree approach is simpler (no bin count parameter to tune) and handles non-uniform invariant density naturally.

#### Why PROSAC-style Progressive Sampling

Standard RANSAC samples uniformly at random. When triangle matching produces confidence scores (vote counts), ignoring them wastes information. PROSAC (Progressive Sample Consensus) biases early iterations toward high-confidence matches, finding correct models faster. Our implementation uses a 3-phase strategy:
- Phase 1 (first 33% of iterations): sample from top 25% highest-confidence matches
- Phase 2 (33-66%): sample from top 50%
- Phase 3 (66-100%): uniform random (classic RANSAC)

This is based on Chum and Matas (2005) "Matching with PROSAC" which demonstrated up to orders of magnitude speedup over uniform RANSAC.

#### Why LO-RANSAC

Standard RANSAC assumes that a model estimated from an outlier-free minimal sample is consistent with all inliers. This rarely holds in practice due to noise. LO-RANSAC (Chum, Matas, Kittler 2003) adds a local optimization step: when a promising hypothesis is found, iteratively re-estimate the model using all current inliers. This typically improves inlier count by 10-20% and speeds up convergence by 2-3x because better models enable earlier adaptive termination.

### Implementation Comparison Summary

| Aspect | This Implementation | Astroalign | Siril | LSST |
|--------|---------------------|-----------|-------|------|
| Triangle formation | K-NN adaptive (k=5-20) | K-NN fixed (k=4) | Brute-force O(n³) | K-NN |
| Max stars | 200 | 50 | 20 | variable |
| Invariant lookup | K-d tree | K-d tree | Hash table | K-d tree |
| Tolerance | 1% | ~5% | configurable | 1% |
| Orientation check | Yes | No | No | Yes |
| RANSAC scoring | MAGSAC++ | Inlier count | OpenCV | MSAC |
| Local optimization | LO-RANSAC | No | No | Yes |
| Progressive sampling | 3-phase PROSAC-style | No | No | Yes |
| SIMD acceleration | AVX2/SSE/NEON | No | No | Yes |
| Distortion correction | SIP polynomial | None | SIP | SIP + TPS |

**Strengths vs. alternatives:**
- More scalable than Siril (O(n·k²) vs O(n³) triangle formation)
- Handles larger star sets than Astroalign (200 vs 50 stars)
- LO-RANSAC and PROSAC-style sampling (not in Astroalign or Siril)
- SIMD-accelerated inlier counting and interpolation

**Gaps vs. alternatives:**
- SIP reference point not FITS-compatible (uses centroid, not CRPIX)

---

## Triangle Matching

### Algorithm

1. **Build k-d tree** from star positions for efficient neighbor queries
2. **Form triangles** from each star and its k-nearest neighbors. k scales with star count: `k = min(n_ref, n_target) / 3`, clamped to [5, 20]
3. **Compute descriptors**: for triangle with sorted sides s0 <= s1 <= s2, store ratios (s0/s2, s1/s2). These are invariant to translation, rotation, and uniform scale
4. **Hash reference triangles** into a 2D grid (default 100x100 bins) keyed by the two ratios
5. **Match target triangles**: for each target triangle, compute its ratios, look up the hash bin (+/- tolerance), and for each matching reference triangle, vote for the 3 vertex correspondences
6. **Resolve matches**: sort by vote count descending, greedily assign one-to-one (each star used at most once)

### Filtering Steps

| Filter | Default | Effect |
|--------|---------|--------|
| `max_stars` | 200 | Limits to brightest N stars from each image |
| Min triangle side | 1e-10 | Rejects degenerate triangles |
| Min triangle area^2 | 1e-6 | Rejects collinear/flat triangles |
| `ratio_tolerance` | 0.01 (1%) | Triangle side ratios must match within this tolerance |
| `check_orientation` | true | Rejects mirror-image matches |
| `min_votes` | 3 | Star pair must receive >= 3 votes from different triangles |
| One-to-one constraint | always | Greedy assignment, highest vote count wins conflicts |

### Vote Matrix

For small star counts (n_ref * n_target < 250,000), a dense matrix is used for O(1) increment and lookup. For larger counts, a sparse HashMap avoids excessive memory.

### Two-Step Matching (Optional)

When enabled, Phase 1 uses relaxed tolerance (5x `ratio_tolerance`) to find initial matches, then Phase 2 refines with strict tolerance. Useful for difficult cases with high positional noise. Disabled by default.

---

## RANSAC Estimation

### Entry Point

**`estimate(matches, ref_stars, target_stars, type)`** - Takes `PointMatch` objects from triangle matching, extracts positions and confidence scores, and uses PROSAC-style 3-phase progressive sampling. High-confidence matches are sampled more often in early iterations for faster convergence.

### RANSAC Loop

```
initialize best_score = 0

for iteration in 1..max_iterations:
    sample minimal point set (uniform or weighted)
    estimate transform from sample
    count inliers (SIMD-accelerated)

    if LO-RANSAC enabled and inliers >= min_samples:
        local_optimization: iteratively re-estimate from inliers
        (up to lo_max_iterations rounds)

    if score > best_score:
        update best model
        compute adaptive iteration bound
        if iterations >= adaptive bound: break

final: refine transform via least-squares on all best inliers
```

### Adaptive Termination

Uses the standard RANSAC formula: `N = log(1 - confidence) / log(1 - w^n)` where w is the inlier ratio and n is the minimal sample size. With 99.9% confidence and 50% inlier ratio, a homography (n=4) needs ~291 iterations. This enables early stopping when a good model is found.

### Score Function

Inlier score is not just a count but a weighted sum: `score += (threshold^2 - dist^2) * 1000` for each inlier. Points closer to the model contribute more, preventing a model with many marginal inliers from beating one with fewer but tighter inliers.

### SIMD Acceleration

Inlier counting is the RANSAC hotpath (called once per iteration per point). SIMD implementations process multiple point pairs simultaneously:
- **x86_64**: AVX2 (4 points at a time) or SSE4.1 (2 points) with runtime dispatch
- **aarch64**: NEON (2 points)
- Fallback: scalar implementation

---

## Transform Types

| Type | DOF | Min Points | Matrix Form | Use Case |
|------|-----|------------|-------------|----------|
| Translation | 2 | 1 | `[1 0 tx; 0 1 ty; 0 0 1]` | Dithered frames, small offsets |
| Euclidean | 3 | 2 | `[cos -sin tx; sin cos ty; 0 0 1]` | Rigid body, field rotation |
| Similarity | 4 | 2 | `[s*cos -s*sin tx; s*sin s*cos ty; 0 0 1]` | **Most astrophotography** |
| Affine | 6 | 3 | `[a b tx; c d ty; 0 0 1]` | Differential refraction, shear |
| Homography | 8 | 4 | `[a b c; d e f; g h 1]` | Wide field, perspective effects |

**Recommendation:** Use Similarity for most astrophotography (handles translation + rotation + uniform scale from focus drift or atmospheric effects). Use Homography for wide-field imaging where perspective effects are significant. Use Affine when differential atmospheric refraction causes directional distortion.

---

## Distortion Correction

### Available Models

#### SIP Polynomial (FITS Standard)
The primary distortion model for the registration pipeline. Applies polynomial corrections to pixel coordinates before the linear transformation:
```
u' = u + sum(A_pq * u^p * v^q)  for p+q <= order
v' = v + sum(B_pq * u^p * v^q)
```
Order 2 handles barrel/pincushion distortion. Order 3 handles mustache distortion. Fitted via least-squares on RANSAC inlier residuals.

#### Brown-Conrady Radial
Classic radial distortion model: `r' = r * (1 + k1*r^2 + k2*r^4 + k3*r^6)`. Compatible with OpenCV conventions. Good for camera lens calibration.

#### Brown-Conrady Tangential
Decentering distortion from imperfect lens alignment: `dx = 2*p1*x*y + p2*(r^2 + 2*x^2)`. Two parameters (p1, p2).

#### Field Curvature (Petzval)
Models curved focal plane effects: `r' = r * (1 + c1*r^2 + c2*r^4)`. Can be initialized from physical Petzval radius.

#### Thin-Plate Splines
Non-parametric model using radial basis functions. Minimizes bending energy for smooth interpolation. Does not require a specific distortion model — adapts to arbitrary local deformations. Best for mosaics and images with complex distortion patterns.

### Comparison with Other Software

**PixInsight** uses thin-plate splines iteratively with increasing flexibility. **Siril** uses SIP coefficients following the FITS/WCS standard. Our implementation supports both: SIP for the standard pipeline (integrated with RANSAC), TPS for advanced cases requiring non-parametric correction.

---

## Interpolation (Image Warping)

| Method | Kernel | Quality | Relative Speed | Notes |
|--------|--------|---------|----------------|-------|
| Nearest | 1x1 | Poor | 1.0x | Preview only |
| Bilinear | 2x2 | Fair | 0.9x | SIMD-accelerated |
| Bicubic | 4x4 | Good | 0.5x | Catmull-Rom |
| Lanczos2 | 4x4 | Very Good | 0.4x | |
| **Lanczos3** | 6x6 | Excellent | 0.3x | **Default** |
| Lanczos4 | 8x8 | Best | 0.2x | Maximum quality |

Lanczos kernel: `L(x) = sinc(x) * sinc(x/a)` for |x| < a, else 0.

Implementation uses pre-computed lookup tables (1024 samples per unit interval) for fast kernel evaluation. Optional output clamping reduces Lanczos ringing artifacts near sharp edges by constraining output to the [min, max] range of the sampled neighborhood.

---

## Phase Correlation

FFT-based coarse alignment for detecting large translational offsets:

1. Compute 2D FFT of both images
2. Cross-power spectrum: `(F1 * conj(F2)) / |F1 * conj(F2)|`
3. Inverse FFT to get correlation surface
4. Peak location = integer translation
5. Sub-pixel refinement via parabolic, Gaussian, or centroid fitting

Optional log-polar transform detects rotation and scale differences by converting them to translational shifts in log-polar space.

Useful as a pre-alignment step before star matching when images have large offsets (>10% of image size).

---

## Astrometry (Plate Solving)

Blind identification of sky coordinates from star positions, using quad-based geometric hashing against star catalogs (Gaia DR3, UCAC4).

Algorithm:
1. Form tetrahedron patterns from brightest detected stars
2. Compute scale/rotation-invariant hash codes (5 distance ratios per quad)
3. Match against pre-computed catalog hash tables
4. RANSAC for robust affine transformation estimation
5. Output WCS solution (reference pixel, sky coordinates, CD matrix, optional SIP)

This module reuses the same RANSAC estimator, k-d tree, and transform types as the registration pipeline.

---

## Quality Metrics

### RMS Error Interpretation

| RMS Error | Quality | Typical Cause |
|-----------|---------|---------------|
| < 0.3 px | Excellent | Clean data, good seeing |
| 0.3-0.5 px | Very Good | Normal sub-pixel alignment |
| 0.5-1.0 px | Good | Slight distortion or centroid noise |
| 1.0-2.0 px | Fair | Significant distortion, poor seeing |
| > 2.0 px | Poor | Wrong model, bad data, or insufficient stars |

### Quality Score

Composite 0.0-1.0 score:
```
score = 0.40 * exp(-rms/2)           -- error component
      + 0.25 * min(inliers/50, 1)    -- match count (saturates at 50)
      + 0.20 * inlier_ratio           -- inliers / total matches
      + 0.15 * overlap_fraction        -- estimated image overlap
```

### Validation Thresholds

| Check | Threshold | Action on Failure |
|-------|-----------|-------------------|
| Minimum inliers | 4 | Registration rejected |
| Maximum RMS | 5.0 px | Registration rejected |
| Minimum inlier ratio | 0.3 (30%) | Registration rejected |

### Quadrant Consistency

`check_quadrant_consistency()` computes per-quadrant RMS errors to detect cases where registration is only correct in part of the image (e.g., due to uncorrected distortion). A difference > 2 pixels between quadrants flags inconsistency.

---

## Module Structure

```
registration/
├── mod.rs                    Public API and re-exports
├── config.rs                 All configuration types
├── transform.rs              Transform matrix and TransformType
├── pipeline/
│   ├── mod.rs                Registrator, warp functions
│   └── result.rs             RegistrationResult, RegistrationError
├── triangle/
│   └── mod.rs                Invariant-space KDTree, vote matrix, match resolution
├── ransac/
│   ├── mod.rs                RANSAC, LO-RANSAC, PROSAC, transform estimators
│   └── simd/
│       ├── mod.rs            Runtime SIMD dispatch
│       ├── sse.rs            x86_64 AVX2/SSE4.1 inlier counting
│       └── neon.rs           ARM NEON inlier counting
├── phase_correlation/
│   └── mod.rs                FFT cross-correlation, log-polar rotation detection
├── interpolation/
│   ├── mod.rs                Lanczos, bicubic, bilinear kernels + LUT
│   └── simd/
│       ├── mod.rs            SIMD dispatch for warping
│       └── sse.rs            SIMD-accelerated bilinear warp
├── spatial/
│   └── mod.rs                K-d tree, k-nearest neighbors, triangle formation
├── distortion/
│   ├── mod.rs                ThinPlateSpline, DistortionMap
│   ├── sip.rs                SIP polynomial (FITS standard)
│   ├── radial.rs             Brown-Conrady radial distortion
│   ├── tangential.rs         Brown-Conrady tangential distortion
│   └── field_curvature.rs    Petzval field curvature
├── quality/
│   └── mod.rs                QualityMetrics, overlap estimation, quadrant checks
├── astrometry/
│   ├── mod.rs                Plate solving API
│   ├── solver.rs             Plate solver implementation
│   ├── quad_hash.rs          Quad formation and geometric hashing
│   ├── wcs.rs                WCS coordinate system
│   └── catalog.rs            Star catalog access (Gaia, UCAC4)
└── tests/
    ├── real_data.rs           Tests with calibrated light frames
    └── synthetic/             Synthetic star field tests
        ├── transform_types.rs
        ├── image_registration.rs
        ├── warping.rs
        └── robustness.rs
```

---

## Configuration Reference

### RegistrationConfig

| Parameter | Default | Description |
|-----------|---------|-------------|
| `transform_type` | Homography | Maximum transformation complexity |
| `min_stars_for_matching` | 10 | Minimum stars required to attempt registration |
| `min_matched_stars` | 8 | Minimum matches after triangle matching |
| `max_residual_pixels` | 2.0 | Maximum acceptable RMS error |

### TriangleMatchConfig

| Parameter | Default | Description |
|-----------|---------|-------------|
| `max_stars` | 200 | Use brightest N stars from each image |
| `ratio_tolerance` | 0.01 | Side ratio matching tolerance (1%) |
| `min_votes` | 3 | Minimum votes to accept a star match |
| `check_orientation` | true | Reject mirrored triangle matches |

### RansacConfig

| Parameter | Default | Description |
|-----------|---------|-------------|
| `max_iterations` | 2000 | Maximum RANSAC iterations |
| `max_sigma` | 1.0 | Maximum noise scale for MAGSAC++ scoring (~3px effective threshold) |
| `confidence` | 0.995 | Target confidence for adaptive termination |
| `min_inlier_ratio` | 0.3 | Minimum inlier fraction to accept model |
| `seed` | None | Random seed for deterministic behavior |
| `use_local_optimization` | true | Enable LO-RANSAC refinement |
| `lo_max_iterations` | 10 | Maximum local optimization iterations |

### SipCorrectionConfig

| Parameter | Default | Description |
|-----------|---------|-------------|
| `enabled` | false | Enable SIP distortion correction |
| `order` | 3 | Polynomial order (2-5) |

---

## Performance

### Typical Times (200 stars, 6000x4000 image)

| Stage | Time | Complexity |
|-------|------|-----------|
| Triangle formation + matching | ~5-15 ms | O(n * k^2) |
| RANSAC (2000 iterations) | ~20-80 ms | O(iter * n) |
| Lanczos3 warp | ~200-500 ms | O(W * H * kernel^2) |
| Phase correlation | ~10 ms | O(N^2 * log N) |

### SIMD Speedups

| Operation | Speedup | Architecture |
|-----------|---------|-------------|
| RANSAC inlier counting | ~1.6x | AVX2, NEON |
| Bilinear warping | ~1.6x | AVX2/SSE4.1, NEON |

---

## Pending Improvements

Prioritized by impact.

### High Priority

#### 1. MAGSAC++ Threshold-Free Scoring

~~**Problem:** The fixed `inlier_threshold` (default 2.0px) requires manual tuning for different seeing conditions. A threshold optimal for 1.3px FWHM fails for 2-3px FWHM.~~

**IMPLEMENTED:** MAGSAC++ scoring replaces MSAC. The `max_sigma` parameter controls the noise scale for marginalization. The effective threshold is approximately `3.03 * max_sigma` (based on the 99% χ² quantile for 2 degrees of freedom).

**Reference:** Barath, D., et al. (2020). "MAGSAC++, a fast, reliable and accurate robust estimator." CVPR 2020.

#### 2. SIP Reference Point (CRPIX Support)

~~**Problem:** SIP polynomial fitting uses the centroid of input points as reference, not CRPIX (image center). This makes coefficients incompatible with FITS headers and tools like DS9, SAOImage, and astropy WCS.~~

**IMPLEMENTED:** `SipConfig::reference_point` allows specifying the polynomial reference point. Use `Some(crpix)` for FITS-compatible coefficients (image center), or `None` to use the centroid of input points (default).

```rust
pub struct SipConfig {
    pub order: usize,
    pub reference_point: Option<DVec2>,  // None = centroid, Some(crpix) = image center
}
```

**Reference:** Shupe, D., et al. (2005). "The SIP Convention for Representing Distortion in FITS Image Headers."

### Medium Priority

#### 3. FWHM-Aware Sigma Selection

~~**Problem:** Optimal `max_sigma` depends on star FWHM/seeing, but users must manually adjust it.~~

**IMPLEMENTED:** The `register()` function automatically derives `max_sigma` from the median FWHM of input stars. Formula: `max_sigma = median_fwhm * 0.5` (floor at 0.5px). This provides optimal noise tolerance for the seeing conditions without manual tuning.

#### 4. SIP Fit Quality Diagnostics

**Problem:** No way to diagnose when SIP polynomial order is too high (overfitting) or too low (underfitting).

**Solution:** Add fit quality metrics:

```rust
pub struct SipFitQuality {
    pub rms_residual: f64,
    pub max_residual: f64,
    pub condition_number: f64,
    pub effective_rank: usize,
}
```

**Effort:** ~30 lines. **Benefit:** Better debugging when distortion correction fails.

#### 5. Sigma-Clipping in SIP Fitting

**Problem:** SIP fitting trusts RANSAC inlier set completely. Marginal inliers can degrade polynomial fit.

**Solution:** Optional sigma-clipping within SIP fitting (LSST approach): 3 iterations, 3-sigma rejection.

**Effort:** ~50 lines. **Benefit:** Marginal robustness improvement for outlier-contaminated inlier sets.

#### 6. Confidence Calculation Improvement

In `resolve_matches()` (`triangle/mod.rs:214`), the confidence formula uses `max_possible_votes = (n-2)*(n-1)/2` which is a loose theoretical upper bound that makes all confidences very small. A more useful confidence metric would be:
- Relative to the maximum observed votes in the current match set
- Or based on the vote distribution (z-score: how many standard deviations above mean)

This matters because `estimate()` uses confidence scores to guide progressive RANSAC sampling. Better confidence values lead to faster convergence.

### Low Priority

#### 7. FITS Header I/O for SIP Coefficients

Read/write A_pq/B_pq keywords from FITS headers. Not a blocker for registration (which is pre-WCS), but useful for interoperability with other tools.

**Effort:** ~100 lines.

#### 8. Weighted Least Squares in Transform Refinement

Use match confidence (vote counts) as weights in final least-squares refinement. Marginal improvement (1-2% better residuals).

**Effort:** ~20 lines.

#### 9. Tabur-Style Ordered Triangle Search

Process triangles by rarity in invariant space (outliers first) for early termination. Only matters for large point sets (500+ stars); current approach is fast enough for typical use (200 stars).

**Effort:** ~30 lines. **Benefit:** 5-10% speedup for 500+ stars.

### Completed (Historical)

- ~~Two-Step Matching~~ — Removed. RANSAC handles outlier rejection.
- ~~Transform Plausibility Checks~~ — Fixed. `max_rotation` and `scale_range` parameters added.
- ~~Guided Matching / Post-RANSAC Match Recovery~~ — Fixed. `recover_matches()` implemented.
- ~~Iterative Model Refinement~~ — Fixed. `TransformType::Auto` upgrades Similarity to Homography if needed.

### Current Limitations (Not Planned)

These are known limitations that are acceptable for the current scope:

- **Single-model assumption.** The pipeline fits one global transformation. Images with severe field-dependent distortion may benefit from local models (tile-based registration or TPS warping, as PixInsight does). SIP correction partially addresses this.
- **Star detection is external.** Registration quality depends on star detection quality (centroid accuracy, false positive rate). The module assumes stars are pre-detected and sorted by brightness.

### Not Needed

These techniques were evaluated and found unnecessary for this use case:

- **True PROSAC** — Not needed. Star registration has >50% inliers after triangle matching; standard RANSAC converges in ~18 iterations. Current 3-phase heuristic is sufficient.
- **Quad matching** — For blind plate solving against large catalogs only. Triangles are sufficient for image-to-image registration.
- **Feature-based matching (SIFT/ORB)** — Star centroids are already optimal point features for astronomical images.

---

## References

### Papers

- Chum, O., Matas, J., Kittler, J. (2003). "Locally Optimized RANSAC." DAGM 2003, LNCS vol. 2781.
- Chum, O., Matas, J. (2005). "Matching with PROSAC - Progressive Sample Consensus." CVPR 2005.
- Fischler, M.A., Bolles, R.C. (1981). "Random Sample Consensus." Communications of the ACM, 24(6).
- Lang, D. et al. (2010). "Astrometry.net: Blind Astrometric Calibration of Arbitrary Astronomical Images."
- Beroiz, M. et al. (2019). "Astroalign: A Python Module for Astronomical Image Registration." arXiv:1909.02946.

### Software

- [Siril](https://siril.readthedocs.io/en/latest/preprocessing/registration.html) - Free astronomical image processing, triangle matching + RANSAC
- [Astroalign](https://astroalign.quatrope.org/) - Python asterism matching with KDTree invariant search
- [PixInsight StarAlignment](https://www.pixinsight.com/tutorials/sa-distortion/index.html) - Iterative TPS distortion correction
- [Astrometry.net](https://astrometry.net) - Blind plate solving with quad hashing
- [ASTAP](https://www.hnsky.org/astap_astrometric_solving.htm) - Quad-based pattern recognition for plate solving

### Related Research

- [Triangle Algorithm Improved by Geometric Hull](https://ieeexplore.ieee.org/document/5504468/) - IEEE, 2010
- [Registration Based on Geometric Constraints and Homography](https://www.mdpi.com/2072-4292/15/7/1921) - Remote Sensing, 2023
- [Dynamic Distance Ratio Matching](https://www.mdpi.com/2072-4292/17/1/62) - Remote Sensing, 2025
- [Multilayer Voting Algorithm for Star Sensors](https://pmc.ncbi.nlm.nih.gov/articles/PMC8124596/) - Sensors, 2021
- [Robust Star Map Matching for Dense Star Scenes](https://www.mdpi.com/2072-4292/16/11/2035) - Remote Sensing, 2024
- [RANSAC for Robotic Applications: A Survey](https://pmc.ncbi.nlm.nih.gov/articles/PMC9824669/) - Comprehensive RANSAC variants review
- [Optimal Spatial Distribution of Tie-Points in RANSAC-based Registration](https://www.tandfonline.com/doi/full/10.1080/22797254.2020.1724519) - European Journal of Remote Sensing, 2020
