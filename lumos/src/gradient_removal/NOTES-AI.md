# gradient_removal Module

## Overview

Single-file module (`mod.rs`, ~770 lines + ~450 lines tests). Background gradient extraction and removal for astrophotography. Automatic grid-based sample placement with brightness rejection. Two modeling methods: polynomial surface fitting (degrees 1-4) and thin-plate spline (TPS) RBF. Two correction modes: subtraction (light pollution) and division (vignetting).

## Architecture

- `GradientRemovalConfig` builder with validation (asserts on invalid input)
- `remove_gradient()` main entry: sample -> fit -> correct pipeline
- `remove_gradient_image()` convenience wrapper for `AstroImage` (per-channel)
- `GradientRemovalError` for expected failures (insufficient samples, singular matrix)
- Parallel gradient evaluation via rayon `par_chunks_mut`

### Pipeline Detail

1. `generate_samples()` -- grid-based placement with brightness rejection
   - Computes global robust stats (median + MAD-sigma)
   - Places samples on uniform grid (`samples_per_line` x `samples_per_line`)
   - Each sample: 5x5 local median (`sample_radius=2`)
   - Rejects samples where `local_median > global_median + tolerance * sigma`
2. `fit_polynomial_gradient()` or `fit_rbf_gradient()` -- model fitting
3. `apply_correction()` -- subtract (offset by gradient median) or divide (normalize by gradient mean)

## Industry Standard Comparison

Reference tools: PixInsight ABE/DBE, Siril Background Extraction, GraXpert, photutils Background2D.

### What Aligns With Standards

- **Dual-method approach**: Matches PixInsight (ABE=polynomial, DBE=splines) and Siril (polynomial + RBF). GraXpert also offers RBF, Splines, and Kriging.
- **Correct TPS kernel**: `r^2 * ln(r)` matches standard thin-plate spline formulation (Wikipedia, Wolfram MathWorld, Geometric Tools reference docs).
- **Polynomial coordinate normalization**: Maps to [-1,1], standard practice for numerical stability. GSL, MATLAB, and academic references all recommend this.
- **Robust statistics**: Median + MAD-based sigma with correct 1.4826 Gaussian conversion factor. Siril uses the same formula: `median + tolerance * MAD`.
- **Degree range 1-4**: Matches Siril (max 4, "beyond this the model is generally unstable"). Siril docs: "a too high degree can give strange results like overcorrection."
- **Subtraction vs division**: Correct semantic mapping. Siril docs: subtraction for "additive effects like light pollution", division for "multiplicative phenomena such as vignetting or differential atmospheric absorption."
- **Grid-based sampling with brightness rejection**: Same approach as Siril (samples_per_line + tolerance in MAD units) and PixInsight ABE (automatic grid with statistical rejection).

### What Differs From Standards

See Issues section below for details.

## Issues Found

### Significant: Normal Equations Solver Squares Condition Number

- **Location**: `solve_least_squares()` (line ~502)
- **Problem**: Computes `A^T * A` then solves via Gaussian elimination. This squares the condition number: `cond(A^T*A) = cond(A)^2`. For degree-4 polynomial (15 terms) with normalized coordinates, condition numbers can reach 10^8+, causing loss of ~8 digits of precision in f64.
- **Industry standard**: QR decomposition (good balance of stability/speed) or SVD (best robustness for ill-conditioned problems). Harvard AM205 notes: "By forming A^T*A, we square the condition number of the problem matrix." The BYU ACME conditioning/stability lab notes: "Householder QR, stabilized modified Gram-Schmidt and the SVD are stable, while the normal equations and regular modified Gram-Schmidt are both unstable." UChicago REU paper on least squares methods confirms QR is standard for moderate-size problems.
- **Singular check `1e-12`**: Too coarse for squared condition numbers; legitimate near-singular problems may pass.
- **Fix**: Replace with QR decomposition or SVD. nalgebra `DMatrix::svd()` handles rank-deficient cases gracefully. For the small matrix sizes involved (6x6 to 15x15 for polynomials), performance difference is negligible.

### Significant: TPS Coordinates Not Normalized

- **Location**: `fit_scalar_tps()` (line ~628-652)
- **Problem**: Polynomial fitting normalizes coordinates to [-1,1], but TPS uses raw pixel coordinates. For a 6000x4000 image, maximum distance r ~7200 pixels, giving `r^2*ln(r) ~ 4.6e8`. Affine terms (1, x, y) have magnitudes (1, 6000, 4000). This ~5 orders of magnitude scale mismatch between kernel values and affine terms causes severe ill-conditioning of the TPS system matrix.
- **Industry standard**: Wikipedia on TPS: "centre and (optionally) scale the coordinates to a unit box." Wood's thin plate regression splines paper recommends: "subtracting the mean from each covariate to center it near 0 to avoid collinearity." GDAL mailing list documents real-world failures: "with a few hundred control points the differences become meters, and above a thousand points the error can reach fifty meters" -- caused by unnormalized coordinates. ALGLIB's TPS implementation documentation explicitly recommends coordinate normalization.
- **Fix**: Normalize all coordinates to [0,1] before building TPS matrix. Denormalize is unnecessary since evaluation uses the same normalized coords.

### Significant: TPS Regularization Scaling is Arbitrary

- **Location**: `fit_scalar_tps()` (line ~618)
- **Problem**: `regularization = smoothing^2 * 1000.0` uses a magic constant unrelated to data scale. Same `smoothing=0.5` gives different effective regularization for different image sizes/sample counts.
- **Industry standard**: MATLAB's `tpaps` uses a data-dependent default: the favorable range for p is near `1/(1 + h^3/6)` where h is the average spacing of data sites. The regularization is set so that `(1-p)/p` equals the average of the diagonal entries of the kernel matrix A, making it automatically adapt to data scale and density. PixInsight's SurfaceSpline smoothing is also relative to data (smoothing factor 0-1 controls the balance between interpolation and approximation). Siril's smoothing is described as a relative parameter (0-100%).
- **Fix**: After coordinate normalization, compute mean inter-point distance `d_mean`. Use MATLAB-style formula: `lambda = ((1-p)/p) * mean(diag(K))` where K is the kernel matrix and p is the user smoothing parameter. This automatically scales with sample geometry.

### Significant: Sample Box Too Small (5x5 pixels)

- **Location**: `generate_samples()` (line ~330, `sample_radius=2`)
- **Problem**: Only 25 pixels per sample. High variance from read noise, especially in faint background. PixInsight DBE encourages "larger samples that cover more pixels" for "images with large regions of background sky" because "small deviations in pixel color and level are averaged away as more pixels are included." Photutils Background2D documentation states box_size "should generally be larger than the typical size of sources in the image" (typical: 50x50 or larger). Siril uses similar grid-cell averaging with larger regions.
- **Fix**: Default to at least `sample_radius=5` (11x11=121 pixels). Make configurable via `GradientRemovalConfig`. Consider scaling with image dimensions. PixInsight DBE notes a trade-off: larger samples are more neutral but smaller samples allow more precise gradient identification in complex regions.

### Moderate: No Iterative Sample Rejection

- **Location**: `generate_samples()` (line ~311-353)
- **Problem**: Single-pass rejection: compute global median/sigma, reject samples above threshold. If a large bright nebula covers significant area, it biases the initial median and sigma upward, causing insufficient rejection.
- **Industry standard**: photutils Background2D defaults to `SigmaClip(sigma=3.0, maxiters=10)` iterative sigma clipping. Additionally, `exclude_mesh_percentile=10` rejects meshes where >10% of pixels were clipped. Siril recomputes statistics after rejection; Siril docs mention tolerance is in MAD units. PixInsight ABE performs structure detection. GraXpert documentation emphasizes that "it is vital that samples are not placed over nebula regions."
- **Fix**: Iterate: reject, recompute stats, re-reject until convergence or max iterations (10). Alternatively, use segmentation-based source masking (detect bright sources first, mask them, then sample). photutils also supports user-provided masks for known source regions.

### Moderate: O(n * W * H) TPS Evaluation Does Not Scale

- **Location**: `fit_scalar_tps()` inner loop (line ~673-688)
- **Problem**: For every pixel, sums over all n sample kernel evaluations. With 6000x4000 image and 256 samples (16x16 grid): 6.1 billion kernel evaluations, each involving sqrt and ln. Even with SIMD, this is prohibitively slow for large images.
- **Industry standard**: PixInsight's `GridInterpolation` and `PointGridInterpolation` classes provide "highly efficient discretized implementations" for fast surface spline evaluation on large grids. Their `RecursivePointSurfaceSpline` uses quadtree-based decomposition for local subsplines. Research paper "A Fast Hybrid Approach for Approximating a Thin-Plate Spline Surface" (Researchgate) demonstrates coarse-grid TPS evaluation with cubic spline interpolation to full resolution. The paper notes this is particularly effective because "TPS interpolation function is only calculated on a coarse grid and at the other points is approximated by cubic spline interpolation."
- **Fix**: Evaluate on coarse grid (e.g., every 32-64 pixels), bilinearly or bicubically interpolate. Reduces kernel evaluations by ~1000x. Background gradients are inherently smooth, so interpolation error is negligible. PixInsight limits direct TPS to ~2000-3000 nodes due to O(N^3) system solve; coarse-grid evaluation addresses the separate O(N*W*H) evaluation cost.

### Moderate: TPS System Solved with Gaussian Elimination Instead of Robust Solver

- **Location**: `fit_scalar_tps()` calls `solve_linear_system()` (line ~661)
- **Problem**: The TPS system `[K+lambda*I, P; P^T, 0] * [w; a] = [v; 0]` is solved with the same Gaussian elimination used for polynomial normal equations. The TPS system matrix is symmetric but not positive-definite (due to the `P^T, 0` block), so Gaussian elimination with partial pivoting is acceptable in principle. However, the combined effect of unnormalized coordinates and this solver amplifies numerical errors.
- **Industry standard**: PixInsight uses the Bunch-Kaufman diagonal pivoting method for symmetric indefinite systems (documented in PCL `SurfaceSpline` class). This is specifically designed for symmetric matrices that are not positive-definite. MATLAB's `tpaps` uses analogous robust solvers.
- **Fix**: After normalizing coordinates (which is the primary fix), the current Gaussian elimination with partial pivoting becomes adequate for the moderate matrix sizes involved (n+3 where n is typically 100-300). If larger sample counts are needed, switch to Bunch-Kaufman or LDLT decomposition.

### Moderate: Division Correction Uses Mean Instead of Median

- **Location**: `apply_correction()` (line ~713)
- **Problem**: Subtraction path uses `compute_median(gradient)` (robust), but division path uses `gradient.iter().sum() / len` (arithmetic mean). Mean is sensitive to extreme gradient values at image corners/edges.
- **Industry standard**: Standard flat-field correction normalizes the flat by its mean or median. The Astropy CCD Reduction Guide states "Typically the mean or median is scaled to 1.0 before combining." However, median is more robust to outliers. For gradient images that may have extreme values at corners (especially with high-degree polynomials or poorly constrained TPS), median is safer. PixInsight DBE uses a smoothing-based normalization. Siril uses median-based statistics throughout.
- **Fix**: Use median for both normalization paths. The gradient image is already smooth (no outliers from noise), so mean and median should be close. But median prevents edge effects from high-degree polynomial extrapolation.

### Moderate: Per-Channel Sample Selection Lacks Shared-Position Option

- **Location**: `remove_gradient_image()` (line ~282-308)
- **Problem**: Each channel processed independently through `remove_gradient_simple()`. All channels use the same config thresholds but generate independent sample positions. For narrowband data or H-alpha-rich regions, red channel may have bright emission that is background in broadband context. Different sample positions per channel can cause color shifts.
- **Industry standard**: PixInsight DBE uses the same sample positions for all channels, with per-channel weights. This ensures color consistency in the gradient model. Siril processes all channels together with shared sample grid.
- **Potential improvement**: Option to share sample positions across channels (compute once on luminance or average, apply to all channels) for color consistency.

### Minor: Median Does Not Average Middle Pair for Even N

- **Location**: `compute_median()` (line ~734), `compute_robust_statistics()` (line ~364)
- **Problem**: Uses `sorted[len/2]` instead of averaging the two middle elements for even-length arrays. For large N (full image pixels), the difference is negligible. For small sample sets (25 pixels in 5x5 box), it introduces a slight bias.

### Minor: polynomial_terms Has Unreachable Default Branch

- **Location**: `polynomial_terms()` (line ~474)
- **Problem**: `_ => 6` fallback is unreachable because config asserts degree 1-4.
- **Fix**: Replace with `unreachable!()` or exhaustive match on 1..=4.

### Minor: No Clamping of Corrected Values

- **Location**: `apply_correction()` (line ~700-726)
- **Problem**: Subtraction can produce negative values (physically meaningless for flux). Division can produce very large values if gradient approaches zero. No clamping applied.
- **Fix**: Clamp corrected values to `[0.0, ...]`. For division, also clamp the normalized gradient denominator more aggressively (current floor is 0.001, which may be insufficient).

### Minor: Doc Example Uses Wrong Module Path

- **Location**: line 35
- **Problem**: `lumos::stacking::gradient_removal` but module is `lumos::gradient_removal`.

## Feature Gaps vs Industry Tools

| Feature | This Module | PixInsight DBE | Siril | GraXpert | photutils |
|---------|-------------|----------------|-------|----------|-----------|
| Polynomial fitting | Yes (1-4) | Yes (ABE, arbitrary degree) | Yes (1-4) | No | No |
| TPS/RBF interpolation | Yes (TPS only) | Yes (TPS, Gaussian, multiquadric, variable order) | Yes (TPS) | Yes (RBF, Splines, Kriging) | No (grid-based) |
| AI-based extraction | No | No | Via GraXpert plugin | Yes (default) | No |
| Manual sample placement | No | Yes (primary workflow) | Yes | Yes | N/A |
| Iterative rejection | No | Yes (structure detection) | Yes (recomputation) | N/A (manual) | Yes (sigma=3, maxiters=10) |
| Source masking | No | Yes (star detection) | Tolerance-based | Manual | Yes (user mask input) |
| Coarse-grid evaluation | No | Yes (GridInterpolation) | Unknown | Unknown | Yes (box grid + interpolation) |
| Quadtree local TPS | No | Yes (RecursivePointSurfaceSpline) | No | No | N/A |
| Per-sample box size config | No | Yes (variable radius + weights) | No | No | Yes (box_size parameter) |
| Per-channel shared samples | No | Yes (shared positions, per-channel weights) | Yes | Yes | N/A |
| Dithering correction | No | No | Yes (anti-banding) | No | No |
| Mesh exclusion percentile | No | No | No | No | Yes (exclude_mesh_percentile) |
| Edge padding/cropping | No | No | No | No | Yes (edge_method) |

## Background on Professional Algorithms

### PixInsight ABE (Automatic Background Extraction)
- Polynomial surface fitting with automatic grid-based sample placement
- Statistical weighting of samples for background representativeness
- Lower degree polynomials recommended (oscillation risk with high degrees)
- Fully automatic (drag-and-drop instance)

### PixInsight DBE (Dynamic Background Extraction)
- Spline-based model (typically 2nd-order splines)
- Manual or semi-automatic sample placement (primary workflow)
- Smoothing factor 0.0 (exact interpolation) to 1.0 (maximum smoothing)
- Per-sample weights (0.0-1.0) per channel
- Larger sample boxes recommended for neutral background estimation
- Smaller samples for complex gradient regions (with higher smoothing)
- Uses Bunch-Kaufman solver for symmetric indefinite systems
- Limited to ~2000-3000 direct nodes; RecursivePointSurfaceSpline for more
- GridInterpolation for fast full-resolution evaluation

### Siril Background Extraction
- Polynomial (1-4) and RBF modes
- Tolerance in MAD units: `median + tolerance * MAD`
- RBF requires fewer samples than polynomial for complex gradients
- Dithering option to prevent color banding in 16-bit output
- Division mode for flat-field-like multiplicative corrections

### GraXpert
- Three traditional methods: RBF, Splines, Kriging
- AI mode: no manual sample placement needed
- Manual sample point selection for traditional methods
- Integrated into Siril as plugin

### photutils Background2D
- Grid-based: divides image into boxes, computes sigma-clipped stats per box
- Default sigma clipping: sigma=3, maxiters=10
- Mesh exclusion: boxes with >10% clipped pixels rejected
- Low-resolution background map interpolated to full resolution
- BkgZoomInterpolator (spline, default) or BkgIDWInterpolator (IDW)
- User-provided masks for known source regions
- Edge handling: pad or crop to integer box multiples

### MATLAB tpaps (Thin-Plate Smoothing Spline)
- Smoothing parameter p in [0,1]: p=0 gives linear least-squares, p=1 gives interpolation
- Data-dependent default: favorable range near `1/(1 + h^3/6)` where h = average spacing
- Regularization: `(1-p)/p` set to average of diagonal entries of kernel matrix
- Minimizes weighted sum: `p * E(f) + (1-p) * R(f)` (data fidelity + roughness penalty)

## Priority Recommendations

1. **TPS coordinate normalization** -- Easiest fix, largest stability impact. Normalize to [0,1] before matrix construction. GDAL TPS bug reports demonstrate real-world failures from unnormalized coordinates.
2. **Replace normal equations with QR/SVD** -- Prevents silent precision loss for degree 3-4 polynomials. nalgebra has the solvers. Matrix sizes are tiny (max 15x15), so no performance concern.
3. **Increase default sample radius** -- Change from 2 to 5+. Low effort, significant noise reduction. Make configurable.
4. **Coarse-grid TPS evaluation** -- Required for practical use on full-resolution images. Evaluate on 64x64 grid, bilinearly interpolate. PixInsight uses GridInterpolation for this exact purpose.
5. **Iterative sigma clipping** -- Add convergence loop around rejection step, sigma=3, max 10 iterations, matching photutils defaults.
6. **Data-dependent TPS regularization** -- Use MATLAB tpaps-style formula: `(1-p)/p = mean(diag(K))` for automatic scaling.
7. **Use median for division normalization** -- One-line fix for consistency and robustness.

## Test Coverage

Good coverage of configuration validation, helper functions, and integration tests for all code paths (polynomial, RBF, subtraction, division, error cases). Tests use small 64x64 images with synthetic gradients.

Missing:
- Numerical stability tests with large coordinate ranges (simulating real 6000x4000 images)
- TPS convergence tests (smoothing=0 should near-interpolate sample values)
- Edge cases with bright objects in sample regions (verifying rejection works)
- Tests verifying polynomial degree actually affects fit quality (degree 1 vs 4 on quadratic gradient)
- Tests verifying division correction preserves flux ratios (not just reduces variance)
- Tests with known analytical gradient solutions (exact coefficient recovery)
