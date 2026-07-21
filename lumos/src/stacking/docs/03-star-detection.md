# Stage 3 — Star detection and registration-catalog construction

## 1. Purpose and required result

This stage converts one calibrated, linear image into a catalog of point-source
measurements suitable for geometric registration. It does **not** produce
science-grade photometry merely because the catalog contains a flux field.
Registration needs repeatable positions, realistic position uncertainties, clean
source classification, and good coverage of the field. Those requirements are
different from maximizing the number of detections or estimating total stellar
flux as accurately as possible.

The implementation described here has the following ordered responsibilities:

1. validate the image, mask, variance, metadata, and coordinate convention;
2. construct one or more *detection* planes without changing the measurement data;
3. estimate spatially varying background and blank-sky noise;
4. estimate the point-spread function (PSF) scale if it was not supplied;
5. compute a variance-aware point-source detection statistic;
6. form connected detections and split blends;
7. measure every candidate on calibrated, unconvolved pixels;
8. attach flags and uncertainties rather than silently discarding information;
9. select a clean, spatially distributed registration catalog; and
10. return diagnostics that distinguish “empty sky” from a failed algorithm.

The central architectural rule is:

> Filtering, channel combination, whitening, median filtering, and thresholding
> may create temporary detection products. Centroids, saturation, PSF parameters,
> and fluxes must be measured from the original calibrated measurement planes,
> with the original masks and variance propagated into the measurement.

Photutils follows the same important separation: `DAOStarFinder` searches a
convolved image but fits its marginal centroids on the unconvolved input image.
SExtractor likewise separates detection processing from measurement and carries
weight maps and extraction flags through the catalog. See the
[Photutils DAOStarFinder documentation](https://photutils.readthedocs.io/en/stable/api/photutils.detection.DAOStarFinder.html)
and [SExtractor processing overview](https://sextractor.readthedocs.io/en/latest/Processing.html).

### 1.1 Normative language

`MUST`, `MUST NOT`, `SHOULD`, and `MAY` are requirements in the usual RFC sense.
Sections 2–14 specify the target implementation. Section 15 audits the current
Lumos implementation; it is descriptive, not permission to retain a known flaw.

### 1.2 Non-goals

This stage does not:

- apply a display stretch, gamma, tone curve, white balance, or gamut transform;
- infer a WCS or solve the frame transform;
- repair calibration defects that Stage 2 should have masked;
- promise unbiased multiband photometry from a synthetic detection plane; or
- resolve severely crowded fields without a simultaneous PSF model.

## 2. Input, output, and coordinate contracts

### 2.1 Required input bundle

The detector SHOULD receive a named input structure rather than a bare
`AstroImage`:

```text
StarDetectionInput
    measurement_planes[c] : calibrated linear samples, common dimensions
    permanent_mask         : per-pixel bit flags
    variance | sky_rms     : optional per-pixel uncertainty product
    saturation_level[c]    : optional valid sensor limit in plane units
    gain[c]                : optional electrons per image unit
    read_noise[c]          : optional electrons RMS
    cfa/demosaic metadata  : sampling and correlation provenance
    exposure metadata      : identifiers and normalization provenance
```

All planes MUST have the same width and height, contain finite values wherever
the mask says the sample is valid, and remain alive until measurement finishes.
An empty image, zero width, mismatched plane dimensions, non-finite configuration,
or a mask/variance shape mismatch is an error. “No accepted stars” is a valid
successful result and must not be conflated with those errors.

The permanent mask is a bit set, not one boolean. At minimum it must be able to
represent:

```text
INVALID       NaN, infinity, missing data, or otherwise unusable sample
SATURATED     clipped sensor value or a sample belonging to a saturated plateau
DEFECT        bad/hot/dead pixel or bad column from calibration
COSMIC_RAY    Stage-2 cosmic-ray detection
INTERPOLATED  sample synthesized by defect repair or demosaicing
USER_MASK     externally excluded region
```

Detection normally excludes `INVALID | SATURATED | DEFECT | COSMIC_RAY |
USER_MASK`. `INTERPOLATED` may be used for detection, but must be recorded because
it changes the noise correlation and must not be treated as an independent sensor
sample in a precision fit.

### 2.2 Pixel coordinates

Internally, pixel-center coordinates are zero based:

```text
first pixel center = (x, y) = (0.0, 0.0)
x increases with column index
y increases with row index
valid centers: 0 <= x <= width - 1, 0 <= y <= height - 1
```

All seeds, centroids, bounding boxes, distances, PSF models, and registration
consumers MUST use this convention. A FITS table that uses the FITS pixel
convention adds 1 to both coordinates exactly once at its serialization boundary.
Astrometry.net explicitly notes the FITS `(1,1)` first-pixel-center convention;
mixing it with a zero-based internal catalog causes an exact one-pixel transform
error.

### 2.3 Required catalog

The internal source record needs more than the current `Star` fields. A complete
registration measurement contains at least:

```text
source_id
x, y                              f64 pixel-center position
position_covariance_xx, xy, yy   pixel^2
detection_snr, detection_scale
flux, flux_variance              explicitly named flux definition
local_background, local_sky_rms
peak_value
fwhm_major, fwhm_minor, theta
eccentricity
fit_reduced_chi2, fit_iterations
segmentation_id, footprint_area, valid_fit_pixels
nearest_neighbour_distance
flags
```

Flags must include, where applicable, `EDGE`, `MASKED_CORE`, `SATURATED`,
`INTERPOLATED_CORE`, `BLENDED`, `DEBLEND_FAILED`, `NEIGHBOUR_CONTAMINATED`,
`FIT_FAILED`, `FIT_BOUNDARY`, `BAD_COVARIANCE`, `BAD_BACKGROUND`, and
`PSF_MISMATCH`. Retaining a flagged raw catalog before final selection is vital for
diagnostics and for future policies that may tolerate different defects.

The result also returns:

```text
StarDetectionResult
    raw_sources
    registration_sources
    background_summary
    psf_summary
    rejection_counts_by_flag_or_rule
    stage_timings
    configuration_and_algorithm_version
```

## 3. Detection planes and multichannel images

### 3.1 Measurement data are immutable

No operation in this section may overwrite `measurement_planes`. A median filter
is nonlinear, modifies PSF width and subpixel phase, suppresses saturation
plateaus, and changes flux. It is acceptable only as a named temporary detection
product. A convolved image is likewise never a measurement plane.

Detection after demosaicing inherits correlated, color-dependent noise. Whenever
possible, detect on a linear pre-demosaic luminance-like plane or propagate the
demosaicer's covariance information. If detection is performed on demosaiced RGB,
the empirical renormalization in §7.5 is required.

### 3.2 Known source color: optimal linear combination

At one pixel, let the background-subtracted channel vector be `d`, its channel
covariance be `C`, and the expected relative source response be `q`. The
minimum-variance unbiased scalar response is

```text
w = C^-1 q / (q^T C^-1 q)
D = w^T d
Var(D) = 1 / (q^T C^-1 q)
```

For independent channels this reduces to `w_c ∝ q_c / variance_c`. Plain
inverse-variance weighting is the special case `q_c = 1`; it assumes equal source
amplitudes in all channels. It is not optimal for an unknown stellar spectral
energy distribution, and a strongly red or blue star can be diluted by the other
bands. The assumed `q`, covariance convention, and resulting weights must be saved
in diagnostics.

### 3.3 Unknown source color

There is no single linear combination that is optimal for every color. Use one of
these explicit policies:

1. **Registration band:** select a configured plane, normally green/luminance,
   and use only that plane for both detection and measurement.
2. **Per-band union:** compute detections independently per plane, merge candidates
   within a PSF-scaled radius, then fit the chosen registration plane or fit all
   planes with one shared `(x,y)` and independent fluxes.
3. **Chi-square discovery image:** after each channel is background-subtracted,
   PSF-filtered, and standardized, compute `Q = sum_c S_c^2`. Under independent
   sky noise, `Q` has a chi-square distribution with `k` degrees of freedom. Set a
   threshold from that distribution, not by treating `sqrt(Q)` as a normal sigma.
   `Q` is for candidate discovery only; it is nonlinear and must never supply
   flux, shape, peak, or centroid measurements.

The chi-square approach follows the multiband detection idea in
[Szalay, Connolly & Szokoly (1999)](https://arxiv.org/abs/astro-ph/9811086).
For ordinary Bayer astrophotography, a configured registration plane or a
per-band union is easier to reason about and test.

### 3.4 Optional artifact-suppression plane

If residual CFA structure demands a 3x3 median detection plane, name it
`artifact_suppressed_detection`, retain the unsmoothed detection plane, and set a
diagnostic. Estimate its blank-sky noise empirically after filtering; neither the
original MAD nor the independent-pixel convolution formula remains valid. Every
accepted seed must still be refit on unfiltered measurement pixels.

## 4. Noise and variance semantics

Incorrectly combining an empirical sky RMS with a read-noise term counts the read
noise twice. The API must choose one of the following models and record which one
was used.

### 4.1 Empirical blank-sky model

Let `sigma_sky(i)` be the RMS measured from calibrated blank-sky pixels. It already
contains sky photon noise, read noise, quantization, and any calibration/demosaic
effects visible in those pixels. For a source model `s_i` in normalized image units
and gain `G` electrons per normalized unit:

```text
V_i = sigma_sky(i)^2 + max(s_i, 0) / G + V_extra(i)
```

`V_extra` contains variance not represented by nearby blank sky, such as propagated
flat-field uncertainty. Do **not** add `(read_noise_e / G)^2` again.

### 4.2 Physics-built variance model

If variance is propagated from raw sensor terms rather than measured from sky:

```text
V_i = max(m_i, 0) / G + (read_noise_e / G)^2 + V_calibration(i)
```

where `m_i` is the expected total pre-background-subtraction signal in normalized
units. The same model must be used consistently for detection, fitting, flux
uncertainty, and simulation tests.

### 4.3 Covariance

The formulas above describe diagonal variance. Demosaicing, resampling, denoising,
median filtering, and some calibration operations create covariance between
pixels. The exact matched filter uses the covariance matrix (§7.2). If storing a
full covariance is impractical, the implementation must empirically normalize the
detection statistic and inflate fitted covariance using blank-sky residuals. It
must not silently call correlated samples independent.

## 5. Spatial background and blank-sky RMS

SExtractor's proven design is a grid of robust mesh estimates, a median filter on
that grid, and smooth interpolation. Its official background documentation
describes iterative 3-sigma clipping, the `2.5 median - 1.5 mean` mode estimate,
median filtering, and bicubic-spline interpolation:
[Modeling the background](https://sextractor.readthedocs.io/en/latest/Background.html).

The target algorithm below keeps that structure while making masks, invalid
meshes, edge behavior, and RMS positivity explicit.

### 5.1 Mesh geometry

Given tile size `T`, define

```text
grid_width  = ceil(width / T)
grid_height = ceil(height / T)
tile(gx, gy) = [gx*T, min((gx+1)*T, width))
             x [gy*T, min((gy+1)*T, height))
tile center = ((x0 + x1 - 1)/2, (y0 + y1 - 1)/2)
```

Edge tiles are truncated, not padded or mirrored. Use all finite, permanently
unmasked pixels by default. With `tile_area` actual pixels, a tile is valid only if

```text
good_count >= max(32, ceil(min_good_fraction * tile_area))
```

where the default `min_good_fraction` is `0.5`. A tile with too few samples is
invalid; it must never fall back to using its masked source pixels.

Sampling is normally unnecessary for a 64x64 mesh. If a cap is needed for very
large tiles, use deterministic stratified or reservoir sampling over the valid
pixel list. A fixed two-dimensional stride is forbidden because it aliases Bayer,
row, column, and periodic readout patterns.

### 5.2 Per-mesh robust estimate

For valid samples `v`, perform this exact array-domain estimator:

Throughout this algorithm, sort finite f64 values for statistics. For odd `n`, the
median is `v[n/2]`; for even `n`, it is
`0.5 * (v[n/2-1] + v[n/2])`. MAD uses the same median convention on absolute
deviations. Convert to f32 only when storing the finished grid.

1. Set the working set to every valid sample.
2. For at most `clip_iterations` (default 3):
   - compute the median `m`;
   - compute `MAD = median(|v_i - m|)`;
   - set `sigma = 1.482602218505602 * MAD`;
   - if `sigma == 0`, retain only samples equal to `m` and stop;
   - otherwise retain samples satisfying `|v_i - m| <= 3 sigma`;
   - stop if no sample was removed.
3. Recompute final median `m`, arithmetic mean `mu`, MAD, and `sigma` from the
   retained set.
4. Estimate the background:

```text
if sigma > 0 and abs(mu - m) < 0.3 * sigma:
    background = 2.5*m - 1.5*mu
else:
    background = m
```

5. Mark the tile invalid if the result is non-finite, no samples remain, or the
   RMS is not positive. A truly constant synthetic image may use a configured
   numerical noise floor, but real data with zero measured noise is a diagnostic,
   not evidence of infinite SNR.

This is intentionally not promised to bit-match SExtractor. SExtractor and SEP
derive the statistics from a quantized clipped histogram; Lumos uses sorted
array-domain median and MAD. The statistical contract above is what tests should
pin.

### 5.3 Repair invalid grid nodes

If every grid node is invalid, return `BackgroundUnavailable`. Otherwise replace
each invalid node with the nearest valid node in squared grid-coordinate distance;
ties choose the smallest `(gy, gx)` in row-major order. Do this from an immutable
copy of validity so a filled node cannot become a new source for another fill.
Record the number and locations of repaired nodes.

Median-filter both background and log-RMS grids with the available neighbors in a
3x3 window, including grids narrower than three nodes. The filter output must be
computed from an immutable input grid. Interpolating `log(sigma)` and exponentiating
prevents a cubic interpolator from creating a negative RMS.

### 5.4 Interpolation

Use separable natural cubic splines at the actual tile-center coordinates. For
samples `(x_j, f_j)`, natural end conditions are `f''_0 = f''_(n-1) = 0`; interior
second derivatives solve

```text
h_(j-1) f''_(j-1)
+ 2(h_(j-1) + h_j) f''_j
+ h_j f''_(j+1)
= 6[(f_(j+1)-f_j)/h_j - (f_j-f_(j-1))/h_(j-1)]
```

with `h_j = x_(j+1)-x_j`. In interval `j`, for
`a=(x_(j+1)-x)/h_j` and `b=(x-x_j)/h_j`, evaluate

```text
f(x) = a*f_j + b*f_(j+1)
     + ((a^3-a) f''_j + (b^3-b) f''_(j+1)) * h_j^2 / 6
```

First spline down the grid's y direction for each grid column, then across x for
each output row. With one node along an axis, use that value; with two, use linear
interpolation. Clamp query coordinates to the first and last tile center rather
than extrapolating beyond them. Interpolate background directly and `log(sigma)`
for RMS, then return `sigma = max(exp(log_sigma), sigma_floor)`.

Use a per-pixel numerical floor

```text
sigma_floor = max(configured_absolute_floor,
                  16 * f32::EPSILON * max(1, abs(background)))
```

with `configured_absolute_floor = 1e-6` for Lumos' normalized f32 images. Reaching
the floor sets a diagnostic; it prevents division by zero but is not a substitute
for a physical noise estimate.

Natural bicubic interpolation can overshoot the background around steep gradients.
The repaired/median-filtered grid, finite checks, and source-masking refinement are
therefore not optional details.

### 5.5 Iterative source masking

Crowded fields bias the first mesh estimate upward. For `N` configured refinement
passes:

1. compute the current background and RMS maps;
2. form a provisional significance mask `I - B > mask_sigma * sigma`, with
   `mask_sigma` normally equal to the detection threshold;
3. OR this mask with the permanent invalid/source mask;
4. dilate only the newly detected-source portion with a Euclidean disk of radius
   `ceil(max(configured_dilation, 2.5 * current_fwhm))`;
5. recompute all tiles while excluding the combined mask.

Never replace an all-masked tile with its unmasked contaminated pixels. It becomes
an invalid grid node and follows §5.3. Stop after exactly `N` passes for deterministic
behavior; optionally report the maximum background-grid change for diagnostics.

### 5.6 Per-source local background

Detection always uses the global maps. Measurement may refine the sky around one
source with an annulus:

```text
r_inner = max(fit_radius, 3 * fwhm_major)
r_outer = r_inner + max(4, ceil(2 * fwhm_major))
```

Exclude permanent-mask pixels, every segmentation footprint dilated by one FWHM,
and pixels outside the frame. Apply the same median/MAD 3-sigma clip. Require at
least `max(20, 0.25 * nominal_annulus_pixels)` valid pixels. Return the median sky,
blank-sky RMS, valid count, and estimated background variance. If the annulus fails,
use the global map and set `BAD_BACKGROUND`; never silently mix annulus sky with a
global-map RMS.

## 6. PSF scale and template construction

### 6.1 FWHM bootstrap

A matched filter needs a PSF scale before the final catalog exists. If metadata or
a configured FWHM is unavailable, bootstrap it as follows:

1. Start with the configured fallback FWHM `f0`; default 4 pixels.
2. Run a high-threshold (`max(6 sigma, 1.5 * normal_threshold)`) candidate search
   using either no filter or the three-template bank `{0.7 f0, f0, 1.4 f0}`.
3. Reject masked, saturated, edge, blended, and non-isolated candidates. Isolation
   requires no other seed within `4 f0`.
4. Fit a pixel-integrated elliptical Gaussian or Moffat model on the original
   measurement plane. Keep finite fits with positive-definite covariance,
   `0.5 <= FWHM <= 30`, and acceptable residuals.
5. Require at least `min_psf_stars` (default 10). Otherwise retain `f0` and record
   `ConfiguredFallback` plus the number of usable stars.
6. Compute median `m` and scaled MAD `s = 1.4826022185 * MAD`. Replace `s` with
   `max(s, 0.05*m)` to avoid a zero-width clipping interval. Retain widths with
   `|FWHM-m| <= 3s`, then take the median of the survivors.

Use symmetric clipping. Removing only broad outliers retains cosmic rays and hot
pixels at the narrow end. A second bootstrap iteration with the new FWHM is enough;
continued iteration adds instability without useful information.

For severe field curvature, estimate a spatial PSF model after the initial catalog:
fit robust low-order surfaces to `log(FWHM_major)`, `log(FWHM_minor)`, and orientation
using bright isolated stars. Fall back to the global median wherever local support is
insufficient.

### 6.2 Pixel-integrated template

The template `P_i(x0,y0)` is the fraction of unit source flux expected in pixel
`i`, not the continuous PSF sampled at the pixel center:

```text
P_i(x0,y0) = integral over pixel i of p(x-x0, y-y0) dx dy
sum over the untruncated template P_i = 1
```

This distinction prevents pixel-phase centroid and width bias in undersampled
images. Anderson & King show why the detector-integrated effective PSF, rather
than only the optical PSF, is the relevant model for high-precision astrometry:
[Toward High-Precision Astrometry with WFPC2](https://arxiv.org/abs/astro-ph/0006325).

For an axis-aligned Gaussian, integrate exactly with differences of `erf` in x and
y. For a rotated Gaussian or Moffat profile, use fixed-order two-dimensional
Gauss-Legendre quadrature per pixel; 4x4 nodes are the minimum and 8x8 is the
reference path for tests. Normalize after truncation. A Gaussian radius of 4 major
axis sigma encloses `1-exp(-8)`, approximately 99.966% of circular two-dimensional
flux. Record the truncation and reject any kernel whose finite sum is non-positive.

The normalized elliptical Gaussian is

```text
p(u) = exp(-0.5 * u^T Sigma^-1 u) / (2*pi*sqrt(det(Sigma)))
```

For a normalized elliptical Moffat with `beta > 1`,

```text
p(r_e) = (beta - 1) / (pi * alpha_major * alpha_minor)
         * [1 + r_e^2]^(-beta)
r_e^2 = u_major^2/alpha_major^2 + u_minor^2/alpha_minor^2
FWHM_axis = 2 * alpha_axis * sqrt(2^(1/beta) - 1)
```

Template orientation is counter-clockwise from positive x. State this convention
once in the API and test 0, 45, and 90 degree kernels.

## 7. Variance-aware point-source detection

### 7.1 Residual image

For each detection plane form

```text
D_i = I_i - B_i
```

Do not clamp negative residuals. Negative sky fluctuations are required for the
detection statistic to have zero mean. Masked pixels receive zero statistical
weight; they are not filled by reflection, mirroring, or a local median.

### 7.2 Independent-noise matched filter

For a template centered at trial position `q`, variance `V_i`, and usable-pixel
indicator `M_i` (`1` usable, `0` excluded), compute in f64:

```text
A(q) = sum_i M_i * P_i(q) * D_i / V_i
H(q) = sum_i M_i * P_i(q)^2 / V_i

flux_hat(q) = A(q) / H(q)
flux_variance(q) = 1 / H(q)
S(q) = A(q) / sqrt(H(q))
```

Require finite `H > 0` and a configurable template-energy fraction. The sums
below cover the complete nominal truncated kernel; out-of-frame and masked samples
have `M_i = 0`:

```text
E_valid(q) = sum_i M_i * P_i(q)^2
E_full(q)  = sum_i P_i(q)^2
E_valid(q) / E_full(q) >= min_kernel_information
```

with default `0.8`. This handles masks and image boundaries without pretending
missing pixels contain zero signal. It also makes `S` a unit-normal detection
statistic under the independent Gaussian-noise model. SEP implements this
variance-aware numerator and denominator, and its derivation generalizes to full
covariance; see [SEP matched-filter documentation](https://sep.readthedocs.io/en/latest/filter.html).

For stationary variance `V_i = sigma^2`, this reduces to

```text
S = sum(P_i D_i) / (sigma * sqrt(sum(P_i^2)))
```

Dividing an ordinary convolution only by `sqrt(sum(P^2))` but comparing it to the
RMS at the kernel center is valid only when noise is locally stationary and
uncorrelated.

### 7.3 Correlated-noise form

For residual vector `D`, template `P`, and covariance matrix `C` over the valid
template footprint:

```text
S = P^T C^-1 D / sqrt(P^T C^-1 P)
```

This is the optimal linear statistic. A practical implementation may whiten the
image and template with a locally estimated stationary noise power spectrum. If it
does not model `C`, it must apply §7.5 and label the statistic
`EmpiricallyNormalized`, not “exact SNR.” Zackay & Ofek discuss matched filtering
for optimal point-source detection in
[How to Coadd Images I](https://arxiv.org/abs/1512.06872).

### 7.4 Multiple scales

When seeing is uncertain or spatially variable, evaluate a small geometric bank,
for example `{0.7 f, f, 1.4 f}`. For each pixel retain the greatest statistic and
its scale. Candidate non-maximum suppression is performed jointly in `(x,y,scale)`:
two maxima within `0.5 * min(FWHM_a,FWHM_b)` represent the same seed, and the larger
`S` wins; exact ties choose the smaller scale, then `(y,x)`.

Searching multiple correlated templates increases the trials factor. The requested
threshold must therefore be calibrated on blank-sky simulations using the same
bank, rather than claiming that every `S > 4` peak has a one-template Gaussian
false-alarm probability.

### 7.5 Empirical null normalization

Even with an analytic variance map, estimate the null distribution of `S` in each
background mesh after masking provisional sources and permanent defects:

```text
mu_S    = median(S_blank)
sigma_S = 1.4826022185 * median(abs(S_blank - mu_S))
Z       = (S - mu_S) / sigma_S
```

Interpolate `mu_S` and positive `sigma_S` as in §5. Require enough blank samples;
otherwise use neighboring valid meshes. On well-modeled independent noise,
`mu_S ~= 0` and `sigma_S ~= 1`. Large deviations are diagnostics for correlation,
bad background, or an incorrect variance map. Threshold the empirically normalized
`Z` when it is enabled.

### 7.6 Threshold semantics

Use two thresholds:

```text
peak_threshold      default 4.0 sigma
footprint_threshold default 2.5 sigma, <= peak_threshold
```

A pixel is inside a provisional footprint iff `Z > footprint_threshold`; strict
`>` is part of the contract. A footprint survives only if it contains at least one
seed with `Z > peak_threshold`. This hysteresis grows a statistically significant
core into enough of its lower-SNR PSF footprint for topology and deblending without
allowing a 2.5-sigma fluctuation to create a source by itself.

Thresholds are configuration values, not universal astronomical constants. Validate
them by completeness and false-positive tests for each camera/pipeline, especially
after resampling or demosaicing.

## 8. Connected components and seed extraction

### 8.1 Labeling

Label the footprint mask with configured 4- or 8-connectivity; 8-connectivity is
the default for star images. Labels are assigned deterministically in row-major
first-contact order. For every component collect:

```text
label, area, bounding box, touches_edge
maximum Z and its location
number of peak-threshold pixels
permanent-mask overlap
```

Apply `min_area` only after a component has been built. Do not discard a large
parent component before deblending: a crowded island can exceed `max_area` while
containing many valid stars. `max_area` may reject an unsplit final leaf or trigger
a “too complex” fallback, but it cannot bypass the deblender.

If `2 * edge_margin >= min(width,height)`, configuration is invalid for that image;
return an error instead of a warning followed by an empty catalog.

### 8.2 Local maxima, including plateaus

Inside each component, find 8-connected plateaus of equal `Z` values. A plateau is
a maximum if no neighbor outside it has a greater `Z`. Represent it by the
`D`-positive weighted centroid of plateau pixels; if the positive weight is zero,
use their arithmetic centroid. Its peak coordinate for deterministic comparisons is
the smallest row-major plateau pixel.

This rule detects a saturated or quantized flat top instead of requiring one pixel
to be strictly greater than all eight neighbors. Saturated plateaus remain flagged
and normally do not enter the registration catalog.

Suppress seed pairs closer than
`max(configured_min_separation, 0.5 * local_fwhm)`. Sort candidates by descending
`Z`, then ascending `(y,x)`; greedily keep a seed only if it is not within the
strict separation radius of an earlier kept seed.

“Prominence” means the peak-to-saddle difference in the same standardized topology
image, not a fraction of the brightest raw pixel. Raw peak fractions change when a
background pedestal or gradient is added and therefore are not valid prominence.

## 9. Deblending

Deblending and measurement of blends are separate problems. A perfect segmentation
map does not make an independent square-stamp measurement unbiased if that stamp
still contains the neighbor.

### 9.1 Reference SExtractor/SEP behavior

SExtractor and SEP create a hierarchy at exponentially spaced thresholds between
the detection threshold and the component peak. SEP defaults to 32 thresholds and
minimum contrast 0.005. A branch is significant only if its integrated intensity
above its branch threshold exceeds the configured fraction of the root object's
integrated flux, and a parent splits only when at least two children are
significant. SEP then probabilistically gathers low-level pixels into the surviving
branches and optionally CLEANs detections explainable by a brighter object's
profile. These details are visible in SEP's `src/deblend.c` and `src/extract.c`, not
just in the high-level phrase “SExtractor-style.”

The target Lumos algorithm uses the same tree idea but operates in standardized
coordinates so spatial noise gradients do not change the topology.

### 9.2 Variance-aware multithreshold tree

For one parent footprint use the empirically normalized matched-filter statistic
from §7.5 as the topology image:

```text
R_i = Z_i
```

for topology. Let `r0 = footprint_threshold` and `rp` be the component maximum.
If `rp <= r0`, return the unsplit parent. For `N >= 2` levels define

```text
r_k = r0 * (rp / r0)^(k/N),  k = 0..N
```

At each level, label 8-connected pixels belonging to the parent for which
`R_i >= r_k`. Discard level components smaller than `min_area`. Associate a
level-`k+1` component with the unique level-`k` component containing it; ties or a
non-unique parent indicate an internal logic error.

For a branch `b` born or evaluated at level `r_k`, define its excess-significance
volume

```text
Q_b = sum over pixels i in b of max(R_i - r_k, 0)
Q_root = sum over pixels i in root of max(R_i - r0, 0)
```

A child is significant when all are true:

```text
child area >= min_area
Q_child >= min_contrast * Q_root
distance(child_peak, sibling_peak) >= min_separation
```

Commit a split only if at least two children are significant. Otherwise retain the
parent branch and continue following its strongest descendant. Traverse top-down
with stable ordering by descending `Q`, then `(peak_y, peak_x)`. Enforce explicit
limits on thresholds, nodes, children, and recursion depth; overflow sets
`DEBLEND_FAILED` and returns the unsplit parent rather than dropping it.

The above `Q` is deliberately an internal detection contrast, not physical flux.
Scientific flux is measured later on original pixels.

### 9.3 Final pixel ownership

Assign the parent footprint to retained leaves with a marker-controlled watershed
on `-R`:

1. seed each retained leaf at its maximum plateau;
2. process pixels in descending `R`, with row-major order breaking equal-value ties;
3. a pixel reached by one label inherits it;
4. a pixel simultaneously reached by multiple labels is a watershed boundary;
5. after all parent pixels are processed, assign boundary pixels to the label whose
   pixel-integrated PSF predicts the greatest value there; exact ties use leaf ID.

This preserves the component, is deterministic, and respects saddles better than a
nearest-peak Voronoi split. Assert that every parent pixel has exactly one final
owner and that leaf areas sum to the parent area.

### 9.4 Fast local-maxima mode

A local-maxima-only mode MAY be retained for sparse fields. It must use plateau
maxima and saddle prominence from §8.2, and its final ownership still follows the
watershed above. Name it `LocalMaximaWatershed`; do not describe nearest-peak
Voronoi assignment with a raw global-peak fraction as equivalent to SExtractor.

### 9.5 Crowded measurement

If another retained source lies within the fit stamp, either:

- fit all neighboring sources simultaneously with one local background plane and
  the same local PSF; or
- mask pixels owned by neighbors and require enough remaining template information.

The simultaneous model is preferred. Measuring each deblended seed in an
unrestricted square stamp biases centroid, flux, shape, and SNR toward its neighbor.

## 10. Centroid and PSF measurement

Vakili & Hogg found that ordinary center-of-light moments do not approach the
Cramér-Rao bound, while PSF-aware methods can:
[Do fast stellar centroiding methods saturate the Cramér-Rao lower bound?](https://arxiv.org/abs/1610.05873).
Use a pixel-integrated PSF fit for the registration position when it succeeds and
an explicitly flagged windowed centroid as the fallback.

### 10.1 Fit stamp

For local major-axis FWHM `f`, use

```text
radius = clamp(ceil(2.0 * f), 4, configured_max_radius)
```

and a square stamp centered on the seed. Exclude permanent-invalid pixels rather
than substituting values. If less than 80% of the unmasked template information or
fewer than the number of fit parameters plus 5 pixels remain, fail the fit. A stamp
crossing the frame boundary sets `EDGE`; it is not mirrored.

### 10.2 Preferred simultaneous PSF model

For source `j`, normalized pixel-integrated PSF `P_ij`, flux `F_j`, and a local
background plane, model valid pixel `i` as

```text
m_i = b0 + bx*(x_i-x_ref) + by*(y_i-y_ref)
      + sum_j F_j * P_ij(x_j, y_j, PSF_shape_at_j)
```

The default fit keeps PSF shape fixed from bright isolated stars and solves source
positions, positive fluxes, and the background plane. Faint stars do not contain
enough information to fit two widths, orientation, centroid, flux, and background
independently. A high-SNR diagnostic mode may fit an elliptical shape, but it must
report the additional covariance.

Minimize

```text
chi2 = sum_i (I_i - m_i)^2 / V_i
```

using the consistent variance convention in §4. With an empirical sky model,
update `V_i = sigma_sky_i^2 + max(sum_j F_j P_ij,0)/G + V_extra_i` after accepted
iterations. Without gain, keep the empirical variance fixed.

Analytic derivatives or derivatives of the quadrature-integrated PSF are strongly
preferred. Finite differences must use a scale-aware central step and be compared
against analytic/numerical reference derivatives in tests.

### 10.3 Optimizer acceptance

A Levenberg–Marquardt implementation must expose its termination reason. Suggested
defaults are 50 accepted/rejected iterations, initial damping `1e-3`, damping times
10 after rejection and divided by 10 after acceptance. Stop successfully when both

```text
max scaled parameter step < 1e-6
relative chi2 improvement < 1e-8
```

or when the centroid movement is below `1e-4` pixel and relative chi-square change
is below `1e-8`. Reject rather than accept a result when:

- the iteration limit is reached without a convergence condition;
- any parameter, residual, or covariance is non-finite;
- flux is non-positive;
- the position moves more than `min(1.5 pixels, 0.5 FWHM)` from its seed;
- a parameter rests on a hard bound;
- the normal matrix is singular or not positive definite;
- fewer than the required valid pixels remain; or
- the fitted model is grossly inconsistent with the data.

“Grossly inconsistent” should be calibrated, but a default registration filter may
require `0.25 <= reduced_chi2 <= 4` when the variance is trusted. If variance scale
is empirical, use robust residual quantiles and report reduced chi-square without
pretending its textbook distribution is exact.

### 10.4 Position covariance

At the accepted solution compute

```text
C_parameters = inverse(J^T W J)
```

where `J` is the model Jacobian and `W=diag(1/V_i)`. If the absolute variance is
unknown and estimated only up to scale, multiply by reduced chi-square. The 2x2
`(x,y)` block is the reported position covariance. Require positive eigenvalues and
a finite condition number; otherwise set `BAD_COVARIANCE` and exclude the source
from uncertainty-ranked registration.

The major position variance is the larger eigenvalue of this block, not the
stellar-shape major axis.

### 10.5 Windowed-centroid fallback

Given current position `(x_t,y_t)`, Gaussian window sigma `s_w`, radius
`r_max = 4 s_w`, valid measurement residuals `D_i`, and
`w_i = exp(-r_i^2/(2 s_w^2))`, iterate

```text
x_(t+1) = x_t + 2 * sum(w_i D_i (x_i-x_t)) / sum(w_i D_i)
y_(t+1) = y_t + 2 * sum(w_i D_i (y_i-y_t)) / sum(w_i D_i)
```

using signed background-subtracted samples and excluding masked pixels. Start with
`s_w = max(FWHM/2.354820045, 0.7)` and the seed position. Stop at movement below
`2e-4` pixel or after 16 iterations. Reject a non-positive denominator, non-finite
step, any step over one pixel, or an unconverged result. The factor 2 is part of the
SExtractor windowed-position update; omitting it is a different estimator. See
[SExtractor windowed positional parameters](https://sextractor.readthedocs.io/en/latest/PositionWin.html).

Return the fallback with `FIT_FAILED | WINDOWED_CENTROID`. Its uncertainty must be
estimated from its Jacobian/noise propagation or an empirically calibrated error
model; a made-up fixed subpixel error is not acceptable.

## 11. Measurements, classification, and flags

### 11.1 Flux and SNR

Every flux field must name its estimator. For registration, use the fitted PSF
flux `F` when the fit succeeds. A useful diagnostic aperture flux is

```text
F_ap = sum_i a_i * (I_i - B_i)
```

where `a_i` is the exact fractional overlap of pixel `i` with the aperture.
Residuals remain signed; replacing every negative residual with zero biases faint
flux positive, biases size, and makes blank apertures appear to contain signal.

For independent pixels with a separately estimated local background,

```text
Var(F_ap) = sum_i a_i^2 V_i
          + (sum_i a_i)^2 Var(B_hat)
```

plus covariance terms when samples are correlated. If the background is the median
of `n` independent Gaussian annulus samples, the asymptotic variance is
approximately `pi * sigma^2 / (2n)`; sigma clipping, masks, and correlation change
that value, so bootstrap or an effective-sample correction is preferable. For a
PSF fit, take flux variance from the full parameter covariance, including covariance
with background and neighboring stars.

Define

```text
SNR = flux / sqrt(flux_variance)
```

and store detection SNR separately from measurement SNR. Do not use the full square
stamp pixel count for an aperture that is circular, weighted, masked, or PSF-fit.

### 11.2 Peak and saturation

`peak_value` is the maximum calibrated measurement-plane value in the source core,
before background subtraction. A source is saturated if the permanent saturation
mask intersects the core or if any core pixel reaches the metadata-derived clipping
level. A fixed threshold such as `0.95` is valid only if the loader guarantees that
exact normalization and preserves clipped samples; it is not a general saturation
test.

Default registration selection rejects saturated sources. A future wing-only
saturated-star fitter may retain them, but it must mask the plateau, fit a calibrated
PSF to the wings, and expose a separate uncertainty policy.

### 11.3 Shape

For an accepted elliptical PSF fit, report major/minor FWHM and orientation directly
from the fitted shape or the local PSF model. For a covariance-like source shape
matrix with ordered eigenvalues `lambda_major >= lambda_minor > 0`, Gaussian-
equivalent definitions are

```text
FWHM_major = 2.354820045 * sqrt(lambda_major)
FWHM_minor = 2.354820045 * sqrt(lambda_minor)
eccentricity = sqrt(1 - lambda_minor/lambda_major)
```

State whether the matrix is an unweighted moment, Gaussian-windowed/deconvolved
moment, or model covariance. Values from different definitions are not interchangeable.

Estimate the frame's stellar FWHM distribution only from isolated, unsaturated,
high-SNR sources. With median `m` and scaled MAD
`s = max(1.4826022185*MAD, 0.05*m)`, a default stellar-width filter is symmetric:

```text
abs(FWHM - m) <= 3*s
```

The shape threshold may be relaxed at field edges when a spatial PSF model predicts
optical coma there. That is preferable to a single global eccentricity cut that
systematically removes one part of the image.

### 11.4 Sharpness and DAOFIND roundness names

Do not attach DAOFIND names to different formulas. Photutils' current implementation
and documentation define:

- `SROUND`/`roundness1` from bilateral versus four-fold symmetry of the **convolved**
  cutout, with the center removed and a factor of 2;
- `GROUND`/`roundness2 = 2(Hx-Hy)/(Hx+Hy)`, where `Hx` and `Hy` are amplitudes from
  weighted marginal Gaussian fits to the **unconvolved** image; and
- sharpness as `(unconvolved central pixel - mean of unconvolved surrounding
  pixels) / convolved peak` within the kernel mask.

See [Photutils DAOStarFinder](https://photutils.readthedocs.io/en/stable/api/photutils.detection.DAOStarFinder.html)
and its open-source `photutils/detection/daofinder.py`. If Lumos keeps
`peak/background-subtracted-3x3-flux`, marginal-peak difference, or marginal
half-asymmetry metrics, rename them descriptively, for example
`core_peak_fraction`, `marginal_peak_imbalance`, and `marginal_asymmetry`. Their
empirical thresholds then need Lumos-specific calibration.

Fit residuals are generally more interpretable classifiers than loosely named
roundness values:

```text
reduced chi-square
maximum standardized residual
core residual / flux
axis ratio and orientation residual relative to local PSF
nearest-neighbour contamination
```

A cosmic ray is normally masked by Stage 2. As a defensive fallback, reject sources
that are narrower than the locally expected PSF, have a high core residual, and
intersect a Laplacian/cosmic mask. Do not infer “cosmic ray” from one sharpness
number alone.

### 11.5 Duplicate semantics

Deblending decides whether close peaks are distinct physical candidates. Duplicate
removal only merges repeated detections of the same peak across channels, scales,
or algorithms. Cluster detections with radius

```text
r_duplicate = max(configured_floor, 0.25 * local_FWHM)
```

and keep the measurement with the smallest valid major position variance; then use
higher detection SNR and stable `(y,x)` as tie breakers. A configured zero radius
must take an explicit disabled fast path. Duplicate removal must not undo a valid
deblend merely because two stars are closer than a global 8-pixel constant.

## 12. Registration-catalog selection

The raw source catalog is not yet the best transform catalog. Astrometry.net's
pipeline sorts source lists, removes line-like artifacts, and spatially uniformizes
the sources before solving; see its
[code overview](https://astrometry.net/doc/code.html). Lumos should make this a
separate, deterministic selection step.

### 12.1 Eligibility

Default registration eligibility requires:

```text
no INVALID, SATURATED, EDGE, FIT_FAILED, BAD_COVARIANCE,
   DEBLEND_FAILED, MASKED_CORE, or NEIGHBOUR_CONTAMINATED flag
finite x, y and positive-definite position covariance
measurement SNR >= configured minimum
FWHM and shape consistent with the local stellar PSF
nearest neighbour far enough for the chosen measurement model
```

Keep the reason count for every failed rule. The policy may allow a windowed-
centroid fallback when too few PSF fits survive, but that relaxation must appear in
diagnostics.

### 12.2 Quality ordering

Stable-sort eligible sources by this lexicographic key:

1. major position variance ascending;
2. absolute `log(reduced_chi2)` ascending when meaningful;
3. detection SNR descending;
4. flux descending;
5. `y`, then `x`, ascending.

Position uncertainty ranks the quantity registration actually consumes. Flux is
only a late tie breaker: the brightest stars may saturate, cluster near a nebula,
or all occupy the same portion of the frame.

### 12.3 Spatial uniformization

Given desired catalog size `K`, image width `W`, height `H`, and approximately
`C = max(1, ceil(K/2))` spatial cells:

```text
nx = max(1, round(sqrt(C * W/H)))
ny = max(1, ceil(C/nx))
cell_x = min(nx-1, floor(x/W * nx))
cell_y = min(ny-1, floor(y/H * ny))
```

Insert sources into cells in the quality order above. Emit the first item from
each non-empty cell in row-major cell order, then the second item from each cell,
and so on until `K` sources are emitted or all cells are exhausted. This is the
same useful principle as Astrometry.net's `uniformize.py`: quality order is
preserved within a cell while successive sweeps give every region an opportunity
to contribute.

If the downstream transform needs a minimum geometric span, verify that selected
positions cover configured fractions of x and y and are not nearly collinear. If
coverage fails, return the best available list with a `POOR_SPATIAL_COVERAGE`
diagnostic; do not fabricate sources.

### 12.4 Sequence consistency

Registration quality also depends on repeatability across frames. Later stages may
prefer sources found in several neighboring frames, but Stage 3 must not use a
frame-specific flux-only cut that changes the catalog discontinuously with small
seeing or transparency changes. Position uncertainty, local-PSF consistency, and
spatial sweeps are more stable selection inputs.

## 13. End-to-end reference algorithm

The following pseudocode fixes stage ordering and data ownership. Helper functions
return named results and errors, not ambiguous tuples.

```text
detect_stars(input, config):
    validate_input_and_config(input, config)

    detection_products = build_detection_planes(input.measurement_planes,
                                                  input.permanent_mask,
                                                  config.channel_policy)

    background = estimate_mesh_background(detection_products.primary,
                                          input.permanent_mask,
                                          config.background)

    psf = supplied_or_bootstrap_psf(detection_products,
                                    input.measurement_planes,
                                    input.permanent_mask,
                                    background,
                                    config.psf)

    if config.background.refinement_passes > 0:
        background = refine_background_with_source_masks(..., psf)

    detection = matched_filter_significance(detection_products,
                                             input.variance_or_sky_rms,
                                             input.permanent_mask,
                                             background,
                                             psf,
                                             config.detection)

    normalized = empirical_null_normalize(detection,
                                          input.permanent_mask,
                                          background,
                                          config.detection)

    footprints = hysteresis_components(normalized,
                                       config.footprint_threshold,
                                       config.peak_threshold,
                                       config.connectivity)

    deblended = []
    for footprint in footprints in label order:
        deblended.extend(deblend_or_flag(footprint, normalized,
                                        detection_products.primary,
                                        background, psf, config.deblend))

    groups = build_overlapping_fit_groups(deblended, psf, config.measurement)

    raw_sources = parallel_map_in_stable_slots(groups, group ->
        fit_group_on_measurement_pixels(group,
                                        input.measurement_planes,
                                        input.permanent_mask,
                                        input.variance_or_sky_rms,
                                        background, psf,
                                        config.measurement))

    attach_shape_classification_flags(raw_sources, psf, config.quality)
    deduplicate_cross_scale_and_cross_channel(raw_sources, config.duplicate)
    registration_sources = select_and_uniformize(raw_sources, input.dimensions,
                                                  config.registration_catalog)

    return StarDetectionResult(raw_sources, registration_sources,
                               background.summary, psf.summary, diagnostics)
```

Parallel workers may process tiles, components, and independent fit groups, but
their outputs occupy preassigned indices and are compacted only after joining. Do
not let scheduler completion order determine labels, source IDs, tie breaks, or
catalog order.

## 14. Failure handling, diagnostics, and performance

### 14.1 Expected errors

Return a typed error for invalid configuration/input, no valid background tiles,
non-finite or non-positive noise maps, allocation failure where recoverable, and
cancellation. A complex individual blend, failed individual PSF fit, or rejected
source is not a whole-frame error; return a flagged source or a counted rejection.

### 14.2 Diagnostics

At minimum report:

```text
valid/masked/interpolated/saturated pixel counts
valid and repaired background tiles
median and range of background, sky RMS, and empirical S normalization
PSF origin, FWHM/axis range, and bootstrap star count
pixels above footprint and peak thresholds
component count and size distribution
deblend splits/failures/overflow counts
fit success, fallback, boundary, and covariance-failure counts
rejections by every quality rule
raw, eligible, and spatially selected source counts
spatial coverage metrics
```

Intermediate background, RMS, normalized significance, labels, and segmentation
maps should be optionally capturable for debugging without being mandatory members
of every result.

### 14.3 Determinism

All comparisons state strict versus inclusive behavior. All equal-value cases use
stable keys. Reductions that affect catalog values use f64 and a deterministic
order, or an explicitly tested reproducible pairwise reduction. Results must be
identical across configured thread counts within the documented floating-point
tolerance, and catalog order must be exactly identical.

### 14.4 Memory and cancellation

Reuse frame-sized planes and per-thread scratch, but never alias a detection product
with immutable measurement data. Bound deblend nodes, fit-group size, kernel radius,
and diagnostic capture. Check cancellation between background passes, convolution
row blocks, component batches, and fit groups. Releasing pooled buffers is part of
every error and cancellation path.

## 15. Current Lumos implementation audit

This audit reflects the repository inspected on 2026-07-21. Module paths are given
instead of line numbers because line numbers drift.

### 15.1 Current pipeline and defaults

`StarDetector::detect` currently performs:

```text
prepare one grayscale plane
-> estimate/refine tiled background
-> fixed or bootstrapped FWHM
-> Gaussian convolution, threshold, CCL, deblend
-> centroid and metrics
-> quality filters, flux sort, FWHM filter, duplicate removal
```

Important default values are:

| Area | Current default |
|---|---|
| background | 64-pixel tiles, 3 clip iterations, no refinement, dilation 3 |
| detection | 4 sigma, 8-connectivity, min area 5, max area 500, edge 10 |
| PSF | circular, FWHM 4 pixels, auto-estimation off |
| deblend | local maxima, separation 3, raw peak fraction 0.3, max 8 leaves |
| optional multithreshold | disabled; contrast 0.005 when enabled |
| measurement | weighted moments, global background, no sensor model |
| final filter | SNR 10, eccentricity 0.6, sharpness 0.7, roundness 0.5 |
| duplicate/FWHM | 8-pixel duplicate radius, upper 3-MAD FWHM rejection |

The configuration also provides wide-field, high-resolution, crowded-field, and
precision-ground presets. Defaults are not correctness guarantees; their statistical
meaning depends on the issues below.

### 15.2 What is already sound and worth retaining

- The detector is decomposed into preparation, background, FWHM, detection,
  measurement, and filtering stages with reusable buffers and extensive unit,
  synthetic, SIMD/scalar, memory-budget, and real-data tests.
- Background meshes use sigma-clipped median/MAD and the `2.5 median - 1.5 mean`
  mode switch, filter the tile grid, and interpolate with natural cubic splines.
- Matched filtering subtracts background without clipping negative residuals,
  normalizes the Gaussian kernel, supports an elliptical kernel, and has optimized
  separable/SIMD paths.
- Thresholding has explicit 4/8-connectivity, packed masks, deterministic tests,
  component diagnostics, and both fast and multithreshold deblenders.
- FWHM bootstrapping records whether a real estimate or fallback was used and
  applies symmetric median/MAD clipping to bootstrap widths.
- Gaussian and Moffat LM fitting, adaptive windowed shape moments, local annulus
  background, sensor-model configuration, flux sorting, sparse duplicate lookup,
  and rejection diagnostics already provide useful building blocks.

### 15.3 P0 correctness and data-contract gaps

1. **Detection and measurement share a derived plane.** `stages/prepare.rs`
   inverse-MAD-variance combines RGB and applies a 3x3 median whenever CFA metadata
   remains set. `stages/measure.rs` then measures on that same plane. Consequently
   centroid, FWHM, flux, peak, saturation, and SNR are measurements of a synthetic,
   possibly nonlinear median-filtered image rather than the calibrated source
   planes. The code comment that inverse-variance combination is optimal for an
   unknown flat SED is also too broad: it is optimal only for its assumed channel
   response vector.
2. **There is no mask or propagated variance input.** Bad pixels, hot columns,
   saturation, cosmic rays, interpolated samples, and user masks cannot be excluded
   consistently from background, convolution, deblending, fitting, or metrics.
3. **The matched-filter threshold is not variance-aware.** `convolution/mod.rs`
   uses stationary-kernel energy normalization and `threshold_mask` compares the
   result with the RMS at the output pixel. Noise variation inside the footprint,
   masks, demosaic correlation, and median-filter correlation are absent from the
   statistic; image edges are mirrored rather than represented as reduced template
   information.
4. **Flux and SNR are positively biased.** `centroid/mod.rs::compute_star` clamps
   every background-subtracted stamp sample to zero, sums the full square stamp,
   and uses that full pixel count for noise. The reported flux is not the Gaussian
   or Moffat fit flux. The optional `NoiseModel` adds read noise to an empirical
   sky RMS that already normally contains read noise, so its documented semantics
   double count read noise.
5. **Saturation is not metadata-driven.** `star.rs` uses a fixed normalized peak
   threshold of 0.95, and the stored peak is the candidate region's raw derived-plane
   peak. A median-filtered detection plane can suppress the very plateau the check
   needs.
6. **Deblend ownership is ignored during measurement.** A deblended `Region` only
   supplies a seed/bounding summary. Each source is measured in a complete square
   stamp without neighbor masking or simultaneous fitting, so crowded-star positions
   and metrics remain biased after a nominally successful split.

### 15.4 P1 algorithm gaps

1. Background estimation caps a tile at 1024 samples with a regular 2-D stride in
   the unmasked path, which can alias periodic sensor/CFA patterns. If refinement
   masks every sample, the tile falls back to all pixels and reintroduces the source
   contamination. There is no invalid-tile/minimum-good-fraction state. The 3x3 grid
   median is skipped unless both grid dimensions are at least three, and the spline
   edge behavior can extrapolate before the first tile center. The fast median helper
   selects the upper middle sample for even counts instead of the target midpoint
   convention, producing a small avoidable offset in even-sized fixtures.
2. Components larger than `max_area` are discarded in
   `detector/stages/detect.rs` **before** deblending. A crowded island can therefore
   lose every valid star. The final child filter checks minimum area and edge but
   does not symmetrically apply a maximum leaf area. An edge margin that consumes
   the whole image only logs a warning and filters every region instead of returning
   an invalid-for-image configuration error.
3. Local-maxima deblending in `deblend/local_maxima` requires strict single-pixel
   maxima, measures prominence as a fraction of the component's brightest raw
   derived-plane value, caps peaks at eight, and uses nearest-peak Voronoi ownership.
   It misses plateaus and is sensitive to background pedestal and gradient.
4. Multithreshold deblending in `deblend/multi_threshold` is not equivalent to
   SExtractor despite the current description. Its levels, detection floor, branch
   flux sums, and peaks use raw derived-plane values rather than background-
   subtracted normalized detection statistics or intensity above branch threshold.
   It uses eight-connectivity internally and assigns final pixels by nearest peak. The
   root-relative contrast correction is good, but the measurement domain and pixel
   gathering remain different from SEP/SExtractor.
5. `WeightedMoments` clamps residuals positive and applies an ordinary weighted
   mean update without SExtractor's factor 2. Hitting the ten-iteration limit is
   still accepted because convergence state is not returned.
6. The Gaussian fit is axis-aligned and cannot represent rotated ellipticity; the
   Moffat fit is circular with fixed beta. Both evaluate the profile at pixel centers
   instead of integrating over pixels, return no production position covariance,
   and accept too little fit-quality information. Moffat configuration also accepts
   `0 < beta <= 1`, for which a unit-total-flux two-dimensional Moffat profile is not
   integrable. These limitations matter most for undersampled data, precisely where
   subpixel registration is sensitive to pixel phase.
7. Local annuli ignore permanent masks and neighboring footprints. Ten samples are
   considered sufficient regardless of nominal annulus area, and a failed annulus
   silently falls back to the global map.
8. FWHM bootstrapping inherits the same derived-plane measurements and does not
   require isolation. Final FWHM filtering builds its reference from the brightest
   half of the catalog and rejects only broad upper outliers, not anomalously narrow
   detections. Duplicate removal keeps the brightest entry within a fixed global
   separation rather than the most precise one and can merge distinct deblended
   stars; zero separation lacks an explicit disabled fast path for the spatial-hash
   branch.

### 15.5 P1 naming and catalog gaps

- `Star::roundness1` is documented as DAOFIND GROUND, but the implementation uses
  the maxima of marginal sums rather than fitted marginal Gaussian amplitudes and
  omits Photutils' factor 2.
- `Star::roundness2` is documented as DAOFIND SROUND, but the implementation is the
  non-negative hypotenuse of left/right and top/bottom marginal imbalance. DAOFIND
  SROUND is a signed convolved-image quadrant symmetry statistic.
- Lumos sharpness is background-subtracted peak divided by positive-clipped 3x3
  core flux, not DAOFIND sharpness.
- `Star` has no flags, position covariance, flux variance, background, fit quality,
  axes/orientation, segmentation ID, mask coverage, neighbor distance, or detection
  scale. Downstream code cannot distinguish a precise isolated fit from a fallback
  centroid with hidden contamination.
- The returned catalog is sorted only by flux after global quality cuts. There is
  no uncertainty ranking or spatial uniformization for registration.

### 15.6 Recommended implementation order

The safest dependency order is:

1. introduce input masks/variance, richer flags/catalog, and keep measurement planes
   separate from detection products;
2. make background estimation mask-correct with invalid tiles and positive RMS;
3. replace center-RMS convolution thresholding with the variance-aware statistic and
   empirical null normalization;
4. measure original pixels with pixel-integrated PSF fits and covariance;
5. make deblending operate on standardized residuals and honor ownership in grouped
   measurement;
6. replace misleading metrics/names and add uncertainty/spatial catalog selection;
7. only then retune thresholds and presets on synthetic and real datasets.

Retuning current constants before fixing their statistical domains would bake the
existing biases into new defaults.

## 16. Verification requirements

Every new or modified non-GUI algorithm requires exact tests. Real-image snapshots
are useful regressions but cannot establish correctness because truth is unknown.

### 16.1 Background tests

- Hand-compute median, MAD, clip membership, mode/median switch, invalid-tile fill,
  and interpolation for tiny grids.
- Constant, linear-gradient, vignetting, nebulosity, crowded, and masked synthetic
  fields must recover known background and RMS within predeclared tolerances.
- Verify that all-masked tiles never inspect masked values and all-invalid grids
  return an error.
- Compare full-pixel and capped-sampling paths on Bayer-like, row-periodic, and
  column-periodic patterns to catch sampling alias.
- Assert RMS is finite and positive at every output pixel, including edges and
  one-/two-node grid axes.

### 16.2 Matched-filter tests

- For a hand-sized template and nonuniform variance, assert exact `A`, `H`, flux,
  flux variance, and `S` from the equations in §7.2.
- Mask each kernel pixel in turn and verify the result against a scalar reference.
- Cross-check separable, full 2-D, SIMD, and scalar paths for the same integrated
  template.
- On at least `10^7` blank independent Gaussian samples, verify mean near 0, variance
  near 1, and exceedance rates consistent with the configured threshold and number
  of template trials.
- Repeat with demosaic/resampling correlation and verify empirical normalization
  restores the calibrated null width.
- Inject stars across subpixel phases, FWHM, axis ratio, angle, masks, field position,
  and flux; plot/measure completeness at fixed false-positive rate.

### 16.3 Labeling and deblending tests

- Exhaustive small masks must match scalar 4- and 8-connectivity references.
- Pin strict threshold equality, row-major labels, plateaus, edge contact, and
  separation equality.
- Two- and three-star blends sweep separation, contrast, FWHM, background gradient,
  and noise. Assert exact leaf count where resolvable and graceful unsplit flags
  where not.
- Check every parent pixel receives one owner, leaf areas sum exactly to parent
  area, no peak is outside its leaf, and thread count cannot change ownership.
- Components over `max_area` must reach the deblender; overflow must return an
  unsplit flagged component rather than disappear.
- Compare a compatibility fixture with pinned SEP output while keeping Lumos-specific
  standardized-tree tests separate.

### 16.4 Centroid and fit tests

- Generate pixel-integrated Gaussian and Moffat stars at a dense subpixel grid,
  including FWHM below 2 pixels. Assert signed centroid bias versus pixel phase is
  below the declared target and has no repeating phase pattern.
- Cross-check model derivatives with high-accuracy central differences.
- Sweep SNR, background, gain, read noise, masks, saturation, ellipticity, rotation,
  beta, stamp size, and neighbor separation.
- Simultaneous blends must recover both positions better than independent full-stamp
  fits on the same fixture.
- For repeated noise realizations, standardized errors
  `(estimate-truth)/reported_sigma` must have mean near 0 and RMS near 1. Check joint
  2-D covariance coverage, not only x and y separately.
- Force every optimizer termination path and assert only genuine convergence is
  accepted. Boundary, singular, non-finite, insufficient-pixel, and iteration-limit
  cases must set the correct flags.
- Windowed fallback tests must pin the factor-2 update and convergence/rejection rules.

### 16.5 Catalog and invariance tests

- Uniformization must emit the exact expected sequence for hand-built cells,
  preserve within-cell quality order, handle empty cells, and stop exactly at `K`.
- Adding a constant background, multiplying image/background/RMS by one positive
  scale, or changing thread count must preserve positions and selection as predicted
  by the equations.
- RGB tests need red-only, green-only, blue-only, and neutral stars with unequal
  noise; known-color combination, per-band union, and chi-square discovery must each
  demonstrate their documented behavior.
- Verify FITS serialization adds the coordinate offset exactly once.
- End-to-end synthetic registration fixtures must show that uncertainty-ranked,
  spatially uniform catalogs improve transform recovery versus flux-only sorting.

## 17. Sources and inspected open-source implementations

### 17.1 Primary and official references

- Bertin & Arnouts, 1996, [SExtractor: Software for source extraction](https://aas.aanda.org/articles/aas/pdf/1996/08/ds1060.pdf).
- SExtractor 2.24.2 documentation:
  [processing](https://sextractor.readthedocs.io/en/latest/Processing.html),
  [background](https://sextractor.readthedocs.io/en/latest/Background.html), and
  [windowed positions](https://sextractor.readthedocs.io/en/latest/PositionWin.html).
- Barbary and SEP contributors,
  [SEP matched-filter derivation](https://sep.readthedocs.io/en/latest/filter.html)
  and [extraction API](https://sep.readthedocs.io/en/latest/api/sep.extract.html).
- Stetson, 1987,
  [DAOPHOT — A Computer Program for Crowded-Field Stellar Photometry](https://articles.adsabs.harvard.edu/pdf/1987PASP...99..191S).
- Photutils,
  [DAOStarFinder 3.0 documentation](https://photutils.readthedocs.io/en/stable/api/photutils.detection.DAOStarFinder.html).
- Vakili & Hogg, 2016,
  [Do fast stellar centroiding methods saturate the Cramér-Rao lower bound?](https://arxiv.org/abs/1610.05873).
- Zackay & Ofek, 2017,
  [How to Coadd Images I: Optimal Source Detection and Photometry](https://arxiv.org/abs/1512.06872).
- Anderson & King, 2000,
  [Toward High-Precision Astrometry with WFPC2 I](https://arxiv.org/abs/astro-ph/0006325).
- Szalay, Connolly & Szokoly, 1999,
  [Simultaneous Multicolor Detection of Faint Galaxies](https://arxiv.org/abs/astro-ph/9811086).
- Lang et al., 2010,
  [Astrometry.net: Blind astrometric calibration](https://arxiv.org/abs/0910.2233)
  and [Astrometry.net code structure](https://astrometry.net/doc/code.html).

### 17.2 Source trees inspected

The source-level audit used these exact revisions, cloned under `.tmp/refs/`:

| Project | Revision | Relevant code |
|---|---|---|
| [SEP](https://github.com/sep-developers/sep) | [`93b3ac52`](https://github.com/sep-developers/sep/tree/93b3ac52e0f6cb26449204dc8bc8c3cf65602f0f) | `src/background.c`, `convolve.c`, `deblend.c`, `extract.c` |
| [SExtractor](https://github.com/astromatic/sextractor) | [`c011a00e`](https://github.com/astromatic/sextractor/tree/c011a00e38325817d8cd3c47be07386d7d957213) | `src/back.c`, `filter.c`, `clean.c`, `winpos.c` |
| [Photutils](https://github.com/astropy/photutils) | [`7d7bc607`](https://github.com/astropy/photutils/tree/7d7bc6072f0c473d80f40c2dfa505bb5d04b18f2) | `photutils/detection/daofinder.py`, detection/segmentation code |
| [Astrometry.net](https://github.com/dstndstn/astrometry.net) | [`623b3c31`](https://github.com/dstndstn/astrometry.net/tree/623b3c31a7a5566c1fde8d0a32445aa2ee31b8b3) | `util/simplexy.c`, `uniformize.py`, solver documentation |
| [Siril](https://gitlab.com/free-astro/siril) | [`8ce9baa3`](https://gitlab.com/free-astro/siril/-/tree/8ce9baa37215ae9783de16fa9e0d7a610303588d) | `src/algos/star_finder.c`, `PSF.c`, registration callers |

Notable implementation lessons from the audit:

- SEP uses the full inverse-variance matched statistic when a noise array exists;
  simple convolution is explicitly a different mode.
- SEP/SExtractor deblending evaluates a threshold hierarchy, root-relative branch
  contrast, pixel gathering, cleaning, and flags; copying only exponential levels
  and a contrast number does not reproduce it.
- Photutils separates convolved detection from unconvolved marginal measurement and
  gives precise, inspectable definitions for DAOFIND sharpness/roundness.
- Astrometry.net's `simplexy` background-subtracts, PSF-smooths, labels blobs, and
  searches peaks, while its solver preparation separately sorts, removes line-like
  artifacts, and spatially uniformizes the catalog.
- Siril detects peaks on a Gaussian-smoothed image, handles plateau/saturation cases,
  then fits original pixels with Gaussian/Moffat profiles and rejects candidates
  using width, roundness, residual, amplitude, and profile checks. Its exact choices
  are not all adopted here, but the separation of candidate filtering from original-
  pixel PSF fitting is directly relevant.
