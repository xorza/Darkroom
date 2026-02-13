# star_detection Module - Code Review vs Industry Standards

## Overview
Full astronomical source detection pipeline: grayscale conversion, tiled background/noise estimation, optional FWHM auto-estimation, matched filter convolution + thresholding + CCL + deblending, sub-pixel centroiding (moments/Gaussian/Moffat), quality filtering. Designed for image registration in astrophotography stacking.

## What It Does Well
- Exceptional architecture mirroring SExtractor (Bertin & Arnouts 1996) with DAOFIND-style metrics
- Both SExtractor-style multi-threshold and DAOFIND-style local maxima deblending
- BufferPool avoids repeated heap allocs across frames
- DeblendBuffers with generation-counter O(1) clearing
- SIMD throughout: threshold mask, convolution, median filter, Gaussian/Moffat fitting (AVX2+FMA/NEON)
- Correct Levenberg-Marquardt with fused normal-equation building
- Moffat fitting with PowStrategy optimization for integer/half-integer beta
- Parallel CCL with RLE + atomic union-find (benchmarked threshold at 65K pixels)
- Comprehensive quality metrics: flux, FWHM, eccentricity, SNR, sharpness, GROUND, SROUND, L.A.Cosmic
- 4 preset configurations (wide_field, high_resolution, crowded_field, precise_ground)
- Excellent test coverage including synthetic and real data

## Issues Found

### P1 Bug: Matched Filter Noise Map Not Scaled After Convolution
- File: detector/stages/detect.rs:55-69, 76-78
- After convolution, noise per pixel is reduced (averaging effect)
- Original unconvolved noise map used for thresholding
- Effective sigma threshold higher than configured -> misses faint stars
- Fix: Divide noise map by `sqrt(sum(kernel^2))` after convolution, or multiply sigma threshold by correction factor

### P1 Design: Hardcoded Radius-1 Dilation Before Labeling
- File: detector/stages/detect.rs:73-80
- Always applied, not configurable
- Merges star pairs within 2 pixels, inflates areas, adds background pixels
- Not part of SExtractor or DAOFIND algorithms
- 8-connectivity already handles undersampled PSFs
- Fix: Remove or gate behind config flag

### P2: SExtractor Mode Estimator Missing
- File: background/tile_grid.rs:176
- Uses sigma-clipped median; SExtractor uses `Mode = 2.5*Median - 1.5*Mean`
- Mode is ~30% less noisy and better handles crowded fields
- Fix: Implement mode estimator with fallback to median when mode/median disagree >30%

### P2: Sharpness Calculation Differs From DAOFIND
- File: centroid/mod.rs:335-338
- Computes `peak / core_flux` instead of DAOFIND's `(data_peak - data_mean_surrounding) / convdata_peak`
- Does not use convolved image at all
- Metric still works as discriminator but not comparable to published thresholds

### P2: GROUND Roundness Missing Factor of 2, Uses Raw Max Not Gaussian Fit
- File: centroid/mod.rs:353-358
- DAOFIND: `2.0 * (Hx - Hy) / (Hx + Hy)` where H = Gaussian fit amplitude
- Implementation: `(max_x - max_y) / (max_x + max_y)` using raw marginal maxima
- Noisier, different numerical range (half DAOFIND range)

### P2: SROUND Uses Marginal Asymmetry Instead of Quadrant Symmetry
- File: centroid/mod.rs:360-371
- DAOFIND uses quadrant-based formula on convolved image
- Implementation uses marginal distribution half-sums on unconvolved image
- Different metric entirely

### P2: SNR Uses Full Square Stamp Area
- File: centroid/mod.rs:325
- Uses `(2*radius+1)^2` as npix (up to 961 pixels)
- Should use circular aperture (~2-3 FWHM radius)
- Overestimates noise, produces systematically lower SNR than photometric theory

### P3: Background Interpolation is Bilinear, Not Bicubic Spline
- File: background/mod.rs:92-107
- SExtractor uses natural bicubic-spline; current uses bilinear
- Can cause threshold discontinuities at tile boundaries

### P3: Gaussian Fit Model Lacks Rotation Angle
- File: centroid/gaussian_fit/mod.rs
- Has sigma_x, sigma_y but no theta parameter
- SExtractor includes rotation angle for tracking errors / field rotation

### P3: Filter Stage Rejection Categories Are Not Independent
- File: detector/stages/filter.rs:26-43
- Cascading if-else: star counted in only one rejection category
- Diagnostics misleading for parameter tuning
- Fix: Flag each criterion independently

### P3: FWHM From Second Moments Has No Pixel Discretization Correction
- File: centroid/mod.rs:311-312
- For undersampled PSFs (FWHM < 3px), second moment biased upward ~5-10%
- Standard correction: `sigma_corrected^2 = sigma_measured^2 - 1/12`

### P3: Fit Parameters Discarded When Using GaussianFit/MoffatFit
- File: centroid/mod.rs:225-245
- Profile fitting provides sigma_x, sigma_y (or alpha, beta) for more accurate FWHM/eccentricity
- Second-moments-based metrics used instead, wasting higher-quality fit info
