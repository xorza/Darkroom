# gradient_removal Module - Code Review vs Industry Standards

## Overview
Background gradient extraction and removal for astrophotography. Auto grid-based sample placement with brightness rejection. Two modeling methods: polynomial surface fitting (degrees 1-4) and thin-plate spline (TPS) RBF. Two correction modes: subtraction (light pollution) and division (vignetting).

## What It Does Well
- Dual-method approach mirrors PixInsight (ABE=polynomial, DBE=splines) and SIRIL
- Correct TPS kernel r^2*ln(r)
- Coordinate normalization to [-1,1] for polynomial fitting
- Robust statistics (median + MAD-based sigma, correct 1.4826 factor)
- Parallel evaluation with rayon par_chunks_mut
- Builder pattern with validation
- Good test coverage

## Issues Found

### Significant: Normal Equations Solver Numerically Unstable for Higher Degrees
- File: lines 311-335 (solve_least_squares)
- Computes A^T*A which squares condition number
- For degree-4 (15 terms), condition number can be very large
- Singular matrix check (1e-12) is coarse
- Fix: Replace with QR decomposition or SVD (nalgebra provides SVD::solve())

### Significant: TPS Regularization Scaling is Arbitrary
- File: line 371
- `regularization = smoothing^2 * 1000.0` - magic constant unrelated to data
- Should scale with number of samples and/or mean inter-point distance
- Same smoothing gives different results for different image sizes
- Fix: Use data-dependent scaling like `smoothing^2 * mean_dist^2 * n`

### Significant: TPS Coordinates Not Normalized
- File: lines 380-393
- Polynomial fitting normalizes to [-1,1], TPS uses raw pixel coordinates
- For 6000x4000 image: r^2*ln(r) at r=7000 gives ~4.3e8
- Massive scale mismatch with affine terms -> poor conditioning
- Fix: Normalize coordinates to [0,1] before TPS matrix construction

### Significant: Sample Box Size Too Small (5x5)
- File: line 196 (sample_radius=2)
- PixInsight DBE defaults to radius 5-25 (11x11 to 51x51)
- 25 pixels has very high variance
- Fix: Default at least radius 5, make configurable, scale with image dimensions

### Moderate: No Iterative Sample Rejection
- File: lines 174-212
- Single-pass rejection: compute global stats, reject above threshold
- Large bright nebula can skew initial median/sigma
- Industry standard: iterative sigma clipping (photutils defaults maxiters=10)
- Fix: Repeat reject/recompute until convergence

### Moderate: Division Correction Uses Mean Instead of Median
- File: lines 420-429
- Subtraction path uses median (correct), division path uses mean
- Mean sensitive to gradient model outliers at corners
- Fix: Use median for both

### Moderate: O(n*W*H) TPS Evaluation Doesn't Scale
- File: lines 394-403
- For 6000x4000 with 1024 samples: ~24 billion kernel evaluations
- GraXpert uses precomputed lookup tables and spatial partitioning
- Fix: Evaluate on coarse grid, bilinearly interpolate

### Minor: Median Doesn't Average Two Middle Elements for Even N
- File: lines 432-440, 229
- `sorted[sorted.len() / 2]` instead of averaging middle pair
- Technically incorrect for small sample sets

### Minor: polynomial_terms Has Unreachable Default Branch
- File: lines 278-285
- `_ => 6` unreachable because config asserts degree 1-4
- Fix: Use unreachable!()

### Minor: No Clamping of Corrected Values
- File: lines 410-429
- Subtraction can produce negatives, division can produce very large values
- Fix: Clamp to [0, max] or at least [0, ...]

### Minor: Doc Example Uses Wrong Module Path
- File: line 34
- `lumos::stacking::gradient_removal` but module is `lumos::gradient_removal`

### Moderate: Per-Channel Sample Selection Not Independent for RGB
- File: lines 200-220
- Same config/threshold for all channels
- H-alpha emission may be background in red but not in broadband
