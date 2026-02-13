# gradient_removal Module

## Overview

Single-file module (`mod.rs`, ~770 lines + ~450 lines tests). Background gradient extraction and removal for astrophotography. Automatic grid-based sample placement with brightness rejection. Two modeling methods: polynomial surface fitting (degrees 1-4) and thin-plate spline (TPS) RBF. Two correction modes: subtraction (light pollution) and division (vignetting).

## Architecture

- `GradientRemovalConfig` builder with validation (asserts on invalid input)
- `remove_gradient()` main entry: sample -> fit -> correct pipeline
- `remove_gradient_image()` convenience wrapper for `AstroImage` (per-channel)
- `GradientRemovalError` for expected failures (insufficient samples, singular matrix)
- Parallel gradient evaluation via rayon `par_chunks_mut`

## Industry Standard Comparison

Reference tools: PixInsight ABE/DBE, Siril Background Extraction, GraXpert, photutils Background2D.

### What Aligns With Standards

- **Dual-method approach**: Matches PixInsight (ABE=polynomial, DBE=splines) and Siril (polynomial + RBF)
- **Correct TPS kernel**: `r^2 * ln(r)` matches standard thin-plate spline formulation
- **Polynomial coordinate normalization**: Maps to [-1,1], standard practice for numerical stability
- **Robust statistics**: Median + MAD-based sigma with correct 1.4826 Gaussian conversion factor
- **Degree range 1-4**: Matches Siril (max 4, "beyond this the model is generally unstable")
- **Subtraction vs division**: Correct semantic mapping (subtract for additive LP, divide for vignetting)
- **Grid-based sampling with brightness rejection**: Same approach as Siril and PixInsight ABE

### What Differs From Standards

See Issues section below for details.

## Issues Found

### Significant: Normal Equations Solver Squares Condition Number

- **Location**: `solve_least_squares()` (line ~502)
- **Problem**: Computes `A^T * A` then solves via Gaussian elimination. This squares the condition number: `cond(A^T*A) = cond(A)^2`. For degree-4 polynomial (15 terms) with normalized coordinates, condition numbers can reach 10^8+, causing loss of ~8 digits of precision in f64.
- **Industry standard**: QR decomposition (good balance of stability/speed) or SVD (best robustness for ill-conditioned problems). Harvard AM205 notes: "By forming A^T*A, we square the condition number of the problem matrix." nalgebra provides `SVD::solve()` and QR factorization.
- **Singular check `1e-12`**: Too coarse for squared condition numbers; legitimate near-singular problems may pass.
- **Fix**: Replace with QR decomposition or SVD. nalgebra `DMatrix::svd()` handles rank-deficient cases.

### Significant: TPS Coordinates Not Normalized

- **Location**: `fit_scalar_tps()` (line ~628-652)
- **Problem**: Polynomial fitting normalizes coordinates to [-1,1], but TPS uses raw pixel coordinates. For a 6000x4000 image, maximum distance r ~7200 pixels, giving `r^2*ln(r) ~ 4.6e8`. Affine terms (1, x, y) have magnitudes (1, 6000, 4000). This ~5 orders of magnitude scale mismatch between kernel values and affine terms causes severe ill-conditioning of the TPS system matrix.
- **Industry standard**: Wikipedia on TPS: "centre and (optionally) scale the coordinates to a unit box." Research literature recommends normalization to [0,1] or [-1,1].
- **Fix**: Normalize all coordinates to [0,1] before building TPS matrix. Denormalize is unnecessary since evaluation uses the same normalized coords.

### Significant: TPS Regularization Scaling is Arbitrary

- **Location**: `fit_scalar_tps()` (line ~618)
- **Problem**: `regularization = smoothing^2 * 1000.0` uses a magic constant unrelated to data scale. Same `smoothing=0.5` gives different effective regularization for different image sizes/sample counts.
- **Industry standard**: Siril's smoothing is a relative parameter (0-100%). MATLAB's `tpaps` uses a data-dependent regularization scaled by the bending energy. Regularization should scale with number of samples and mean inter-point distance.
- **Fix**: Compute mean inter-point distance `d_mean`, use `regularization = smoothing^2 * d_mean^2 * n` or similar data-dependent formula. After coordinate normalization, the magic constant becomes less problematic but still not ideal.

### Significant: Sample Box Too Small (5x5 pixels)

- **Location**: `generate_samples()` (line ~330, `sample_radius=2`)
- **Problem**: Only 25 pixels per sample. High variance from read noise, especially in faint background. PixInsight DBE default minimum radius is 1 (3x3=9 pixels) but typical usage is radius 5-25 (11x11 to 51x51). Photutils Background2D uses box sizes of 50x50 or larger. Siril uses similar grid-cell averaging.
- **Fix**: Default to at least `sample_radius=5` (11x11=121 pixels). Make configurable via `GradientRemovalConfig`. Consider scaling with image dimensions.

### Moderate: No Iterative Sample Rejection

- **Location**: `generate_samples()` (line ~311-353)
- **Problem**: Single-pass rejection: compute global median/sigma, reject samples above threshold. If a large bright nebula covers significant area, it biases the initial median and sigma upward, causing insufficient rejection.
- **Industry standard**: photutils defaults to `sigma=3, maxiters=10` iterative sigma clipping. Siril recomputes statistics after rejection. PixInsight ABE performs structure detection. GraXpert documentation emphasizes that "it is vital that samples are not placed over nebula regions."
- **Fix**: Iterate: reject, recompute stats, re-reject until convergence or max iterations (10). Alternatively, use segmentation-based source masking (detect bright sources first, mask them, then sample).

### Moderate: O(n * W * H) TPS Evaluation Does Not Scale

- **Location**: `fit_scalar_tps()` inner loop (line ~673-688)
- **Problem**: For every pixel, sums over all n sample kernel evaluations. With 6000x4000 image and 256 samples (16x16 grid): 6.1 billion kernel evaluations, each involving sqrt and ln. Even with SIMD, this is prohibitively slow for large images.
- **Industry standard**: Evaluate TPS on a coarse grid (e.g., 64x64), then bilinearly or bicubically interpolate to full resolution. Research paper "A Fast Hybrid Approach for Approximating a Thin-Plate Spline Surface" demonstrates this. Background gradients are inherently smooth, so interpolation error is negligible.
- **Fix**: Evaluate on coarse grid (e.g., every 32-64 pixels), bilinearly interpolate. Reduces kernel evaluations by ~1000x. Alternatively, use truncated Laurent expansions for O(log n) per-point evaluation.

### Moderate: Division Correction Uses Mean Instead of Median

- **Location**: `apply_correction()` (line ~713)
- **Problem**: Subtraction path uses `compute_median(gradient)` (robust), but division path uses `gradient.iter().sum() / len` (arithmetic mean). Mean is sensitive to extreme gradient values at image corners/edges.
- **Fix**: Use median for both normalization paths.

### Moderate: Per-Channel Sample Selection Not Independent

- **Location**: `remove_gradient_image()` (line ~282-308)
- **Problem**: Each channel processed independently through `remove_gradient_simple()`, which is correct. However, all channels use the same config thresholds. For narrowband data or H-alpha-rich regions, red channel may have bright emission that is background in broadband context. Sample positions may differ per channel, which is actually correct behavior for independent processing.
- **Potential improvement**: Option to share sample positions across channels (compute once on luminance, apply to all channels) for color consistency.

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
- **Fix**: Clamp corrected values to `[0.0, ...]`. For division, also clamp the normalized gradient denominator more aggressively.

### Minor: Doc Example Uses Wrong Module Path

- **Location**: line 35
- **Problem**: `lumos::stacking::gradient_removal` but module is `lumos::gradient_removal`.

## Feature Gaps vs Industry Tools

| Feature | This Module | PixInsight DBE | Siril | GraXpert |
|---------|-------------|----------------|-------|----------|
| Polynomial fitting | Yes (1-4) | Yes (ABE) | Yes (1-4) | No |
| TPS/RBF interpolation | Yes | Yes (multiple RBF types) | Yes | Yes (default) |
| Kriging | No | Planned | No | Yes |
| AI-based extraction | No | No | Via GraXpert plugin | Yes |
| Manual sample placement | No | Yes (primary workflow) | Yes | Yes |
| Iterative rejection | No | Yes (structure detection) | Yes (recomputation) | N/A (manual) |
| Source masking | No | Yes (star detection) | Tolerance-based | Manual |
| Multiple RBF types | TPS only | TPS, Gaussian, multiquadric | TPS | Multiple |
| Coarse-grid TPS eval | No | Unknown | Unknown | Unknown |
| Per-sample radius config | No | Yes | No | No |
| Dithering correction | No | No | Yes | No |

## Priority Recommendations

1. **TPS coordinate normalization** - Easiest fix, largest stability impact. Normalize to [0,1] before matrix construction.
2. **Replace normal equations with QR/SVD** - Prevents silent precision loss for degree 3-4 polynomials. nalgebra has the solvers.
3. **Increase default sample radius** - Change from 2 to 5+. Low effort, significant noise reduction.
4. **Coarse-grid TPS evaluation** - Required for practical use on full-resolution images. Evaluate on 64x64 grid, bilinearly interpolate.
5. **Iterative sigma clipping** - Add convergence loop around rejection step, max 10 iterations.
6. **Data-dependent TPS regularization** - Scale by inter-point distance and sample count.

## Test Coverage

Good coverage of configuration validation, helper functions, and integration tests for all code paths (polynomial, RBF, subtraction, division, error cases). Tests use small 64x64 images with synthetic gradients. Missing: numerical stability tests with large coordinate ranges, TPS convergence tests, edge cases with bright objects in sample regions.
