# Stage 5 — Statistical stacking and drizzle reconstruction

This document specifies the final reconstruction stage of the Lumos astronomical
pipeline. It is deliberately split into two parts:

- Sections 1–9 are the **normative implementation contract**. `MUST`, `SHOULD`, and
  `MAY` have their usual requirements meaning. An implementation following these
  sections should be scientifically and numerically complete.
- Section 10 is a **source audit of Lumos as it exists on 2026-07-21**. It records
  working behavior and gaps; it is not permission to copy a known limitation into
  a new implementation.

The specification is grounded in Fruchter and Hook's variable-pixel linear
reconstruction paper, Rosner's generalized ESD test, and source-level comparison
with STScI `drizzle`, DrizzlePac, Siril, CCDProc, and SWarp. Revisions and links are
listed in §12.

The central rule is simple: **keep science values linear, keep geometry and
statistical weight separate, and propagate the coefficients actually used.** A
nice-looking image is not sufficient evidence that a stack is correct.

---

## 1. Scope and terminology

### 1.1 Two reconstruction paths

Stage 5 supports two related but non-interchangeable paths.

**Registered statistical stacking** first resamples every input onto one common
grid, then estimates each output sample independently from the values at that
location. A weighted, outlier-rejected mean is the normal light-frame product. A
median is a robust nonlinear alternative. This path is appropriate when the
existing sampling is adequate or when drizzle's dither requirements are not met.

**Drizzle** deposits each calibrated input pixel directly onto the output grid. It
shrinks the input pixel to a drop and distributes that drop by mapped overlap. It
therefore combines registration and resampling in one operation. Drizzle can
recover sampling lost to an undersampled detector only when the input exposures
provide complementary sub-pixel phases. It does not invent optical detail, and it
does not reject cosmic rays by itself.

Never pre-warp an image and then drizzle that warped result. That performs two
resampling operations, broadens the effective point-spread function (PSF), and
correlates noise twice. Drizzle MUST consume calibrated source-grid samples plus
their source-to-output mappings.

### 1.2 Symbol table

For frame `i`, channel `c`, source pixel `u`, and output pixel `o`:

| Symbol | Meaning |
|---|---|
| `d_iuc` | calibrated, linear source sample before Stage 5 normalization |
| `g_ic`, `b_ic` | multiplicative gain and additive offset |
| `x_iuc = g_ic d_iuc + b_ic` | normalized sample |
| `v_iuc` | variance of `x_iuc` in squared science units |
| `m_iu` | binary validity mask: 1 usable, 0 unusable |
| `q_iuc` | non-geometric statistical weight, normally `1/v_iuc` |
| `W_i(r)` | inverse warp: common/reference coordinate `r` to source coordinate |
| `F_i(u)` | forward map from source coordinate to common/reference coordinate |
| `s` | output pixel size divided by input pixel size |
| `S = 1/s` | output sampling factor; Lumos calls this `scale` |
| `p` | `pixfrac`, the drop width divided by the source-pixel width |
| `a_iuo` | fraction or kernel coefficient by which source pixel `u` contributes to output `o` |
| `k_o` | number of valid input samples contributing to output `o` |
| `N_eff,o` | effective number of independent weighted samples |

The subscript `c` is omitted when a value is shared by every channel.

### 1.3 Pixel coordinates are part of the API

Lumos uses integer-center coordinates: pixel `(x, y)` is centered at `(x, y)` and
occupies

```text
[x - 0.5, x + 0.5) × [y - 0.5, y + 0.5).
```

Every transform, bounding-box calculation, overlap routine, WCS update, test
fixture, and serialization format MUST use that convention. A half-pixel error is
not a harmless display offset: it changes dither phases and the reconstructed PSF.

Registration estimates the inverse-warp direction used by an interpolating warp:

```text
W_i : common/reference → source/target
source = W_i(reference)
```

Drizzle needs the opposite direction:

```text
F_i : source/target → common/reference
reference = F_i(source) = inverse(W_i)(source).
```

The direction MUST be encoded in types or constructors, not only in a comment. A
linear `Transform` can be inverted analytically. A `WarpTransform` containing SIP
distortion requires a numerical inverse or a separately fitted forward model; its
linear component alone is not an adequate substitute.

### 1.4 Required input record

Each frame entering Stage 5 SHOULD carry one coherent record:

```text
ReconstructionFrame
    image                  linear source samples
    variance               optional per-channel variance planes
    data_quality           detector/calibration bit mask
    user_mask              optional user/artifact mask
    forward_map            source → common map for drizzle
    inverse_map            common → source map for ordinary warping
    photometric_gain       one per channel
    background_offset      one per channel, or an explicitly modeled surface
    exposure_time
    source_pixel_area      angular area or WCS Jacobian
    PSF / FWHM diagnostics
    registration diagnostics and uncertainty
    provenance
```

Image, variance, masks, transform, normalization, and provenance MUST remain paired.
Parallel arrays whose indices can silently diverge are not an acceptable public
interface.

### 1.5 Separate five meanings often called “weight”

The implementation MUST not overload one plane with all of these meanings:

1. **Validity** answers whether a sample may be used at all.
2. **Geometric support** answers how much of an interpolation kernel or drizzle
   drop overlaps real data.
3. **Statistical weight** describes inverse uncertainty in a valid sample.
4. **Combination coefficient** is the actual multiplier in the output estimator.
5. **Coverage** describes how many exposures/geometric samples support an output
   location.

A zero validity mask excludes a sample. Small geometric support can either exclude
it or reduce its statistical influence according to the chosen reconstruction
model. Neither quantity is itself a detector variance.

---

## 2. Pre-combination conditioning

### 2.1 Validate before measuring statistics

Reject the job before allocation or accumulation when any of these conditions is
true:

- there are no frames;
- dimensions, channel layouts, CFA layouts, units, or filter identities are
  incompatible with the requested operation;
- science or variance planes contain unexpected non-finite values;
- a variance is negative or a supposedly valid sample has non-finite variance;
- a transform is singular, non-finite, crosses a projective horizon inside the
  used footprint, or cannot be inverted to the required tolerance;
- exposure, normalization, or manual weight values are non-finite or invalid;
- the chosen output grid is empty, overflows integer dimensions, or exceeds the
  configured memory/disk budget;
- fewer samples will remain than a selected rejection algorithm requires.

NaN MAY be accepted only when the ingestion contract explicitly defines it as a
masked sample and converts it to `m_iu = 0` before any arithmetic. Allowing an
untracked NaN into an accumulator contaminates every downstream result it touches.

### 2.2 Frame-level selection is not pixel weighting

Reject an entire frame when it fails an acquisition-quality rule: gross tracking
failure, clouds, focus failure, saturation fraction, incompatible exposure or
filter, calibration failure, or registration quality. Record the reason.

After those gates, retain continuous diagnostics such as background variance,
transparency, FWHM, ellipticity, and registration RMS. A continuous weight MUST not
be used to hide a frame that is scientifically invalid. Conversely, an arbitrary
“best 80%” cut discards exposure without a statistical justification.

Seeing-based weighting is objective-dependent. Scalar inverse-background-variance
weights are optimal for estimating a common surface-brightness sample when PSFs are
effectively equal. They are not universally optimal for point-source detection when
PSFs differ. Zackay and Ofek show that optimal point-source coaddition filters each
exposure by its own PSF before combining. Therefore Stage 5 SHOULD either:

- propagate an effective, possibly spatially varying coadd PSF;
- homogenize PSFs for a measurement product, accepting resolution loss; or
- expose a separate PSF-aware/proper-coadd product for quantitative detection and
  photometry.

Do not multiply an inverse-variance weight by an undocumented “sharpness score” and
continue to call the result inverse variance.

### 2.3 Photometric and background normalization

All samples combined at one output location MUST represent the same physical
quantity. For each frame/channel, fit

```text
x_i(p) = g_i d_i(p) + b_i(p).
```

`g_i > 0` matches transparency/exposure response. `b_i` matches an additive sky
pedestal; it can be a scalar or a deliberately fitted low-frequency surface.

The fit domain MUST exclude:

- invalid, saturated, and defect pixels;
- cosmic rays and known trails;
- low-support warped borders;
- clipped sensor values;
- regions absent from either member of a paired fit.

For ordinary same-field light frames, a robust paired fit is preferable to matching
only independent histograms:

1. Choose a reference exposure or a synthetic photometric reference.
2. Build a common valid domain in source-independent sky coordinates.
3. Stratify samples spatially so a dense star cluster or one nebular region cannot
   dominate the regression.
4. Estimate an initial gain from a robust scale ratio and an initial offset from
   medians.
5. Compute paired residuals `r_j = y_j - (g x_j + b)`.
6. Estimate their robust center and scale; keep a broad inlier window such as four
   robust sigmas.
7. Fit the slope with errors in both axes. Deming regression is appropriate when
   source and reference noise variances are known. For paired centered moments
   `S_xx`, `S_yy`, `S_xy` and noise-variance ratio
   `λ = variance_y / variance_x`, use the stable positive root

```text
delta = S_yy - λ S_xx
g = (delta + sqrt(delta² + 4 λ S_xy²)) / (2 S_xy)   when delta >= 0
g = 2 λ S_xy / (sqrt(delta² + 4 λ S_xy²) - delta) when delta < 0
b = median(y) - g median(x).
```

8. Validate `g`, the inlier count, residual structure, and extrapolation. Fall back
   explicitly or fail; never convert a failed fit to identity without reporting it.

For crowded or extended targets, star photometry can give a cleaner transparency
gain: robustly fit cataloged matched-star fluxes after local background subtraction,
then estimate the sky offset separately. A MAD ratio over the entire image can mix
transparency, noise, object structure, and gradient changes.

For mosaics, a single scalar offset per tile is often insufficient. Fit background
surfaces only from sky regions or solve overlap differences between images. Protect
real extended emission: an unconstrained polynomial can “correct” a nebula or galaxy
out of existence. Store the fitted surface and overlap graph in provenance.

The reference affects numerical scale, not attainable SNR. Prefer a well-exposed,
unsaturated, photometrically stable frame with broad overlap and a good PSF. “Lowest
MAD” alone tends to select a dark or transparency-reduced frame and is not a complete
reference criterion.

### 2.4 Propagate normalization into variance

An affine offset does not change random variance; a gain does:

```text
x_i = g_i d_i + b_i
Var(x_i) = g_i² Var(d_i).
```

Every inverse-variance weight and rejection residual MUST use the normalized
variance. This squared-gain factor is easy to omit and can heavily over-weight a
frame that was scaled upward.

If `g_i` or `b_i` has material fit uncertainty, quantitative products SHOULD also
propagate the induced common-mode covariance. Treating a fitted gain as exact is a
reasonable display-stack approximation, but it must be stated.

### 2.5 Input variance model

The preferred input is a calibrated per-pixel variance plane. In electrons, a basic
CCD/CMOS model is

```text
v = max(model_signal + model_sky, 0)
    + read_noise_e²
    + dark_current_variance
    + calibration_variance.
```

Convert to the science image's units with the square of the unit conversion. Include
flat-field uncertainty and any propagated calibration-master uncertainty when those
are significant.

Use a model signal or a leave-one-out/reference estimate for the Poisson term. Using
the noisy observed value itself in both numerator and inverse-variance weight makes
the estimate signal-dependent and biased: downward noise excursions receive larger
weight.

If no variance plane exists, a frame-level robust background sigma is a useful proxy
only in background-dominated regions. It is not a substitute for source Poisson
noise, saturation masks, channel-specific response, or spatial defects. Mark products
derived from this proxy as approximate.

### 2.6 Statistical weights

For independent unbiased samples with known variances, the minimum-variance linear
estimator uses

```text
q_i = 1 / v_i
mu = sum(q_i x_i) / sum(q_i)
Var(mu) = 1 / sum(q_i).
```

The general formula, valid for arbitrary nonnegative weights, is

```text
a_i = q_i / sum(q)
Var(mu) = sum(a_i² v_i).
```

It reduces to `1/sum(q)` only for true inverse-variance weights. Manual quality
weights MUST use the general formula.

Normalize weights only for numerical convenience; multiplying every `q_i` by the
same positive constant does not change the estimate. Preserve their physical scale
when an output inverse-variance map is promised.

### 2.7 Calibration-master special cases

Calibration stacks have different normalization rules from light frames.

- **Bias:** no additive or multiplicative matching. Combine frames from one stable
  acquisition regime. Use equal or measured inverse-variance weights and reject
  transient events.
- **Dark:** no arbitrary image-statistic normalization. Exposure/temperature/current
  scaling is a physical calibration model and belongs upstream. A hot pixel is real
  fixed structure, not an outlier merely because it is bright.
- **Flat:** after bias/dark correction, normalize each flat multiplicatively by a
  robust illumination level. Do not apply an additive sky-style correction. Reject
  only transient contamination; fixed dust shadows and pixel-response structure are
  the signal the master flat must retain.

---

## 3. Registered statistical stacking

### 3.1 Resample each source once

For the conventional path, inverse-warp calibrated sources onto the common output
grid with Stage 4's selected interpolation kernel. For each output/channel retain:

- the resampled science value;
- geometric support;
- the propagated variance, using squared interpolation coefficients;
- an indication of which original pixels contributed;
- any covariance description the resampler can provide.

If a resampled value is

```text
y_o = sum_j h_oj x_j / sum_j h_oj,
```

then, for independent source pixels,

```text
Var(y_o) = sum_j h_oj² Var(x_j) / (sum_j h_oj)².
```

Interpolation creates covariance between neighboring output pixels whenever they
share source samples. A scalar confidence map can describe the diagonal variance;
it cannot describe that covariance.

### 3.2 Per-output gather contract

At each output/channel, gather a compact array of records, not disconnected value
and weight arrays:

```text
Sample { value, variance, weight, frame_id, valid }
```

A sample is usable only if all relevant masks are good, geometric support passes the
configured threshold, and value/variance/weight are finite. Apply normalization
before rejection. Preserve `frame_id` through every sort, partition, and compaction.

When no sample survives, emit the configured invalid/fill value and zero validity,
coverage, weight, and effective count. Do not silently emit a valid zero.

### 3.3 Plain and weighted mean

For survivors `S`, compute in at least `f64` or with compensated accumulation:

```text
Q  = sum_{i in S} q_i
N  = sum_{i in S} q_i x_i
C2 = sum_{i in S} q_i²
mu = N / Q
N_eff = Q² / C2.
```

`N_eff` equals the count for equal weights and decreases as one frame dominates. It
is often more informative than raw coverage.

Also compute the actual modeled variance:

```text
V_num = sum_{i in S} q_i² v_i
V_out = V_num / Q².
```

When `q_i = 1/v_i`, `V_out = 1/Q`. Store the general result rather than assuming
that identity.

The familiar `sqrt(N)` SNR improvement follows only for independent equal-noise
frames. Correlation, unequal weights, source Poisson noise, and systematic errors
change it.

### 3.4 Median

Define the median convention exactly. Recommended behavior is:

- odd `k`: sorted element `z[k/2]`;
- even `k`: arithmetic mean of `z[k/2 - 1]` and `z[k/2]`.

An upper-middle-only median is a biased convention for even sample counts unless it
is explicitly required for compatibility.

The median is nonlinear. It has no fixed linear coefficient map, so a
`sum(weights²)/sum(weights)²` plane is not its variance. For equal independent
Gaussian samples, the large-`N` result is

```text
Var(median) ≈ (pi / 2) sigma² / N,
SNR_median ≈ sqrt(2/pi) sqrt(N) SNR_single ≈ 0.798 sqrt(N) SNR_single.
```

Finite-sample order-statistic formulas or a bootstrap are needed for a rigorous
median uncertainty. A weighted median is a different estimator and MUST be named as
such; otherwise requested frame weights should produce an error or an explicit
warning.

A median is useful for very small contaminated sets and for constructing a robust
model in a drizzle/blot rejection pass. A rejected weighted mean normally retains
more SNR for the final light stack.

### 3.5 Rejection and combination are separate operations

Every rejection algorithm produces a survivor mask over the original sample
records. The final estimator then combines those survivors. “Winsorized sigma
clipping,” for example, uses Winsorization to estimate a robust scale but normally
rejects original samples before the final mean; it does not necessarily average the
clamped values.

Rejection SHOULD operate on standardized residuals when variances differ:

```text
r_i = (x_i - model_i) / sqrt(v_i + v_model_i).
```

If the center/model uncertainty is ignored, document that approximation. Applying
one raw-value sigma threshold to heteroscedastic samples rejects noisy frames more
often even when their values are statistically consistent.

Asymmetric thresholds are useful because cosmic rays and trails are usually positive,
while interpolation ringing and calibration defects can be negative. Inclusive
bounds are recommended: keep a value exactly on the threshold.

Never permit rejection to reduce the sample below the estimator's declared minimum.
Stop before that removal and report that the pixel was underconstrained.

### 3.6 Iterative median/MAD sigma clipping

For a survivor set `S`:

1. Compute `m = median(x_i)`.
2. Compute `MAD = median(|x_i - m|)`.
3. Convert to a Gaussian-consistent scale with
   `sigma_MAD = 1.482602218505602 * MAD`.
4. With no per-sample variance, use
   `sigma = max(sigma_MAD, sigma_floor)`, where `sigma_floor` comes from a
   quantization/noise model, and compare raw deviations with `k_low*sigma` and
   `k_high*sigma`.
5. With per-sample variance, form standardized residuals

```text
r_i = (x_i - m) / sqrt(v_i + v_m).
```

   `v_m` is the center-estimate variance, preferably computed leave-one-out so
   sample `i` is not compared with a model containing itself. Estimate a robust
   residual-scale multiplier

```text
s_r = max(1, 1.482602218505602 * median(|r_i - median(r)|)).
```

   Then keep `-k_low*s_r <= r_i <= k_high*s_r`. The floor at one prevents a
   small sample fluctuation from claiming that a trusted noise model is too broad;
   an optional variance-rescaling mode may relax this only when explicitly
   configured.
6. Recompute center, center variance, and scales from survivors until no new
   rejection or `max_iterations` is reached.

`MAD = 0` MUST NOT be interpreted as “there are no outliers.” A stack containing
many exactly equal quantized values and one cosmic ray has zero MAD. The propagated
noise/quantization floor resolves this case; without one, any fallback is heuristic
and must be tested explicitly.

A range-based fast path is legal only if it proves that the exact algorithm cannot
reject anything. It must reproduce the zero-scale and boundary semantics of the
full path.

Fixed `k` does not give a fixed family-wise false-rejection rate as frame count
grows. Presets SHOULD become more conservative with large `N`, or offer a
multiplicity-aware mode.

### 3.7 Winsorized sigma clipping

The name is used for several variants. The configuration and documentation MUST
state the exact one. A Siril-style robust-scale/reject algorithm is:

```text
S = all valid samples
repeat outer:
    m = median(S)
    s = sample_standard_deviation(S)
    repeat inner:
        Y = clamp_each(S, m - 1.5*s, m + 1.5*s)
        s_new = 1.134 * sample_standard_deviation(Y)
        stop when abs(s_new - s) <= 0.0005*s
        s = s_new
    reject original samples outside [m - k_low*s, m + k_high*s]
until no change or minimum survivors reached
```

Set a finite inner-iteration cap and define behavior when `s = 0`. Keep the median
and all index associations exact through sorting. The `1.134` factor is tied to this
Huber/Winsorized estimator; it is not a universal correction for every clamping
scheme.

This method is robust at smaller `N` than ordinary mean/standard-deviation clipping,
but thresholds still need validation against representative sensor noise and
outlier populations. The raw-value form assumes comparable variances. For
heteroscedastic data, run the robust estimation on standardized model residuals and
apply the resulting survivor mask to the original samples.

### 3.8 Fixed-fraction trimming

Lumos's operation called percentile clipping is a fixed-count trimmed mean. Define
it without ambiguity:

```text
sort ascending with frame IDs
n_low  = floor(k * p_low  / 100)
n_high = floor(k * p_high / 100)
keep [n_low, k - n_high)
```

Require `0 <= p_low,p_high < 100` and at least one survivor; a stricter API SHOULD
require several survivors for a mean. Ties are retained or removed according to
their sorted positions, so this method always removes the configured counts even
from clean Gaussian data. Report the realized counts.

This is not Siril's historical “percentile clipping,” which compares fractional
deviation from the median. The two algorithms must not share serialized names
without a qualifier.

### 3.9 Rank-linear-fit clipping

The Siril/PixInsight-family method used by Lumos is an **order-statistic fit**, not a
fit against exposure time, frame number, a reference-image value, or spatial
position:

1. Optionally run one robust median/MAD seed rejection.
2. Sort survivors by value, preserving their frame IDs.
3. Fit `z_j = a + b j` to sorted rank `j = 0..k-1` by least squares.
4. Compute residuals `e_j = z_j - (a + b j)`.
5. Estimate residual scale using the explicitly selected statistic.
6. Reject ranks outside the asymmetric residual band.
7. Repeat the sort/fit/reject step to convergence or the iteration cap.

If the scale is `mean(abs(e_j))`, the thresholds are multiples of mean absolute
residual, **not Gaussian sigma**. Parameter names and UI labels must say so or apply
the appropriate consistency conversion.

Sorting removes temporal identity from the independent variable. Therefore this
method does not fit a time trend and does not remove a sky gradient. It can tolerate
a broad, smoothly ordered distribution of per-frame values, which is why it may
appear helpful when normalization residuals vary, but a real spatial background
mismatch should be fixed by normalization rather than hidden inside rejection.

A statistically cleaner order-distribution model MAY fit expected normal quantiles
instead of raw rank, but that is a different algorithm and needs its own tests and
name.

### 3.10 Generalized ESD (Rosner)

GESD tests up to `r` outliers in an approximately i.i.d. Gaussian sample. For the
live sample `S_i` of size `n_i = n - i + 1`:

```text
mean_i = mean(S_i)
s_i    = sample_standard_deviation(S_i)
R_i    = max_j |x_j - mean_i| / s_i
```

Remove the maximizing sample provisionally and repeat for `i = 1..r`. For a
two-sided test at significance `alpha`, compute

```text
p_i      = 1 - alpha / (2 n_i)
t_i      = StudentT_inverse_cdf(p_i, df = n_i - 2)
lambda_i = (n_i - 1) * t_i / sqrt(n_i * (n_i - 2 + t_i²)).
```

The number of outliers is the **largest** `i` for which `R_i > lambda_i`. Restore all
provisional removals after that index. Stopping at the first failed test is wrong
because masking can make a later statistic significant.

Use sample, not population, standard deviation; live sample size in both `p_i` and
degrees of freedom; an accurate Student-t inverse; and stable `f64` moments. Rosner's
critical-value approximation is very accurate for `n >= 25` and reasonably accurate
for `n >= 15`. Below that, use a declared fallback.

GESD's Gaussian i.i.d. assumptions do not hold for raw heteroscedastic samples.
Standardize them first or do not present `alpha` as a calibrated false-positive
probability. `r` is an upper bound on contamination, not a free “aggressiveness”
slider.

### 3.11 Small-N policy

The small-stack fallback is part of the configuration and provenance. Suggested
starting policy:

| Valid samples at a pixel | Suggested estimator |
|---:|---|
| 0 | invalid/fill |
| 1 | the sample, flagged `N_eff=1` |
| 2 | weighted mean only if both agree under the noise model; otherwise underconstrained |
| 3–4 | median or a carefully validated robust rule |
| 5–14 | Winsorized or median/MAD clipping with conservative thresholds |
| >= 15 | GESD becomes statistically defensible; sigma/Winsorized remain valid |

The decision SHOULD use the local valid count, not only the global number of
frames. Edges and masked regions can have a much smaller `N` than the center.

### 3.12 RGB and CFA policy

For raw mosaic data, calibration and transient rejection at the CFA sample level
avoid demosaic-induced correlations and colored cosmic-ray artifacts. Samples may
only be compared within the same CFA color/filter class.

For RGB data, choose and record one of two policies:

- **Per-channel rejection:** maximizes use of unaffected channels but can produce
  different survivor sets and color shifts.
- **Joint rejection:** detect an artifact from standardized multi-channel residuals
  and reject that frame in all channels; better for chromatic consistency and broad
  artifacts.

Whichever policy is selected, weight, variance, survivor count, and rejection maps
must have the same channel shape as the estimator. A shared scalar frame-noise
weight is only justified when channel variances have already been equalized.

### 3.13 Required statistical-stack outputs

A quantitative product SHOULD contain:

- linear science image;
- validity mask;
- geometric coverage count/fraction before rejection;
- survivor count after rejection;
- sum of actual combination weights;
- `N_eff`;
- propagated variance in science units;
- optional empirical scatter or reduced-chi-square map;
- low/high rejection count maps, preferably per channel;
- effective PSF/provenance and registration diagnostics.

The coefficient-only quantity

```text
L_o = sum(q_i²) / sum(q_i)² = 1/N_eff
```

is useful, but it is an actual variance only when every contributing input has the
same unit variance. Name it `linear_variance_factor`, not `variance`.

An optional model-check is

```text
chi2 = sum((x_i - mu)² / v_i)
nu   = max(k - 1, 1).
```

Large `chi2/nu` indicates underestimated variance, unmodeled normalization/PSF
differences, variability, or remaining artifacts. Rejection biases the conditional
scatter downward, so do not use post-clipping scatter alone as a fully calibrated
uncertainty.

### 3.14 Reference pseudocode

```text
for each output tile:
    load aligned science, variance, support, and masks for every frame
    for each output pixel and channel:
        samples = []
        for frame i:
            if mask_good(i) and support(i) > support_min:
                x = gain[i,c] * warped_value[i,c] + offset[i,c]
                v = gain[i,c]^2 * warped_variance[i,c]
                q = choose_weight(v, frame_quality, interpolation_model)
                if finite(x, v, q) and v >= 0 and q > 0:
                    samples.push({x, v, q, frame_id=i})

        geometric_count = len(samples)
        survivors = reject(samples, local_small_n_policy)

        if estimator is mean:
            Q    = compensated_sum(q)
            N    = compensated_sum(q*x)
            C2   = compensated_sum(q*q)
            Vnum = compensated_sum(q*q*v)
            output = N/Q
            variance = Vnum/(Q*Q)
            n_eff = Q*Q/C2
        else if estimator is median:
            output = defined_median(survivor values)
            variance = median_uncertainty_or_missing

        write science, validity, counts, Q, n_eff, variance, rejection diagnostics
```

---

## 4. Drizzle: geometric reconstruction

### 4.1 When drizzle is justified

Drizzle is useful only when all of these are substantially true:

- the scene is static and the data remain linear;
- the detector undersamples the optical PSF;
- exposures contain sufficiently accurate complementary sub-pixel phases;
- mappings include all relevant distortion;
- masks remove cosmic rays, trails, defects, saturation, and transient sources;
- output scale and `pixfrac` are chosen from sampling diagnostics rather than a
  desire for a larger file.

If the source PSF is already sampled by roughly 2–2.5 pixels across its FWHM, a
finer grid may aid display or alignment but contains little new information. If all
dithers share nearly the same fractional phase, increasing `S` mostly produces
holes or correlated interpolation.

Drizzle is linear reconstruction, not deconvolution. It cannot restore frequencies
removed by the optical PSF, detector response, focus, tracking, or registration
error.

### 4.2 Construct the output grid from pixel boundaries

Choose an output mode explicitly: reference footprint, intersection, union/mosaic,
or caller-provided WCS/grid. For each source, map the boundary of its valid pixel
footprint through `F_i`. With nonlinear distortion, sample boundary curves
adaptively; four image corners are insufficient when an edge is curved.

Let common-coordinate bounds be `[x_min, x_max) × [y_min, y_max)` and let `S = 1/s`.
Then

```text
width  = ceil(S * (x_max - x_min))
height = ceil(S * (y_max - y_min)).
```

Map a common coordinate to integer-center output coordinates with

```text
o_x = S * (r_x - x_min) - 0.5
o_y = S * (r_y - y_min) - 0.5.
```

The `-0.5` term aligns **boundaries**, not just centers. For a reference image whose
footprint is `[-0.5, width-0.5)`, `x_min=-0.5`; at `S=2`, source center `0` maps to
output coordinate `0.5`, so the scaled source boundary maps exactly onto the output
boundary. Simply computing `o = S*r` causes a half-output-footprint shift and clips
one side.

Derive output WCS, pixel scale, reference pixel, crop offset, and physical pixel area
from this same mapping. Metadata MUST describe the produced grid; copying the source
WCS unchanged or resetting metadata is wrong.

### 4.3 Square-drop geometry

For input pixel center `u=(x,y)`, create its `pixfrac`-shrunken square in source
coordinates:

```text
D_source = [x-p/2, x+p/2] × [y-p/2, y+p/2].
```

Map that footprint through `F_i` and the output-grid transform. For an affine or a
valid homography without a horizon crossing, mapped edges remain straight and the
four mapped corners define the footprint. For SIP or other nonlinear maps, subdivide
edges/cells until the maximum mapping chord error is below a configured fraction of
an output pixel; then deposit the resulting polygons.

Let `D_iu` be the mapped drop, `A_iu` its area in output-pixel units, and `P_o` the
unit-square footprint of output pixel `o`. Compute

```text
A_iuo = area(D_iu intersect P_o)
a_iuo = A_iuo / A_iu.
```

For a fully contained drop, `sum_o a_iuo = 1`. This division by mapped drop area is
the Jacobian/area normalization. Do **not** divide by another local Jacobian after
forming `a_iuo`.

Use robust polygon clipping or STScI's `boxer`/Green's-theorem method. Validate
orientation, finiteness, convexity where assumed, positive area, output bounds, and
the overlap-sum invariant. A projective denominator approaching zero is a transform
error, not a tiny-area pixel to skip silently.

### 4.4 F&H accumulation equations

For a positive square-drop coefficient and non-geometric statistical weight `q_iu`,
define

```text
c_iuo = m_iu * q_iu * a_iuo.
```

Accumulate per output/channel:

```text
N_o    += c_iuo * x_iuc
W_o    += c_iuo
Vnum_o += c_iuo² * v_iuc
C2_o   += c_iuo².
```

Finalize when `W_o > 0`:

```text
science_o                = N_o / W_o
variance_o               = Vnum_o / W_o²
linear_variance_factor_o = C2_o / W_o²
N_eff,o                  = W_o² / C2_o.
```

This is the batch form of Fruchter and Hook's iterative weighted update. It also
matches STScI's square kernel: overlap is divided by the mapped drop area exactly
once.

If a pixel contributes to several outputs, the outputs share that input noise.
`variance_o` is the correct diagonal term for independent source pixels, but the
off-diagonal covariance is nonzero:

```text
Cov(o, p) = sum_iu c_iuo c_iup v_iu / (W_o W_p).
```

### 4.5 Flux units versus surface-brightness units

The output-unit contract MUST be explicit.

If input samples represent **surface brightness** or count rate per angular area,
the weighted mean above preserves that quantity. A uniform field has the same
numeric value at every output scale.

If input samples represent **integrated flux per input pixel**, an output pixel has
area `s²` times an input pixel for equal square plate scales. Convert each input
sample to the desired output-pixel flux before accumulation:

```text
x_for_output = x_input * s²
v_for_output = v_input * s⁴.
```

Derive statistical weights after that unit conversion. In particular, an
inverse-variance weight changes by `1/s⁴`; using a pre-conversion weight with a
post-conversion variance makes both the estimator and its uncertainty inconsistent.

This is the `s²` science-numerator factor in Fruchter and Hook and the `iscale`
distinction in STScI `drizzle`. It does not belong in `W_o`.

With `S=2`, omitting `s²=1/4` preserves numeric surface brightness but makes the sum
over output pixels four times the input sum. That is correct only if the output is
declared a surface-brightness image. Never infer units from an accidental sum test.

For spatially varying WCS pixel area, use the appropriate local angular-area
conversion and document whether detector flat-fielding has already produced a
surface-brightness-like value.

### 4.6 Kernel definitions

#### Exact square

The exact mapped polygon from §4.3 is the scientific default. It handles rotation,
shear, affine scale, and valid projective geometry. It is the canonical drizzle
kernel.

#### Turbo

Turbo replaces the mapped quadrilateral with an axis-aligned rectangle centered at
the mapped input center. For a uniform scale it uses width `p/s = pS` in output
pixels and normalizes overlap by that rectangle's area. It is an approximation when
rotation, shear, anisotropic scale, or spatial distortion is non-negligible.

Turbo MUST NOT apply an additional Jacobian after its overlap coefficients already
sum to one. Select it only after a configured bound on its footprint error relative
to exact square, or expose it as an explicit speed/accuracy tradeoff.

#### Point

Point is the `p → 0` interlacing limit. Deposit at the output pixel containing the
mapped source center with coefficient one. It needs excellent phase coverage and
accurate transforms. It MUST NOT divide its unit coefficient by an extra Jacobian.
Allowing `pixfrac=0` for Point is clearer than requiring a positive ignored value.

#### Gaussian

A Gaussian kernel is not the canonical flux-conserving square drizzle drop. Define
its FWHM/support and normalization exactly. If

```text
K(dx,dy) = exp(-(dx²+dy²)/(2 sigma²)),
sigma = (p/s) / 2.354820045,
```

then compute all support taps, normalize by the full phase-dependent tap sum, and
discard taps outside the output grid without renormalizing the remainder. Renormalizing
only in-bounds taps changes the estimator at image edges. Variance uses squared
normalized coefficients.

STScI explicitly warns that its Gaussian and Lanczos drizzle kernels do not conserve
flux. A Lumos alternative with different normalization must not claim bit-for-bit or
photometric equivalence to STScI.

#### Lanczos

Lanczos-`a` is the separable signed interpolation kernel

```text
L_a(x) = sinc(x) sinc(x/a), |x| < a; 0 otherwise
K(dx,dy) = L_a(dx) L_a(dy).
```

Signed coefficients require special treatment:

- preserve negative science values; never clamp the final image to zero;
- accumulate variance with coefficient squares;
- do not use the signed denominator as a coverage map;
- track geometric/absolute support separately;
- guard against a near-zero signed coefficient sum;
- do not renormalize only the in-bounds signed taps at an edge.

Lanczos is interpolation-like rather than the square variable-pixel algorithm.
Restricting it to `s=1`, `p=1` is a defensible initial policy. It should not be the
default for quantitative drizzle reconstruction.

### 4.7 Masks and channel shape

The source validity mask MUST combine detector DQ, calibration defects, saturation,
cosmic-ray/trail masks, user masks, and non-finite handling before deposition.
Statistical weight zero is an acceptable internal representation of invalidity only
if a separate validity/context output remains available.

A per-pixel weight plane that is shared by RGB channels cannot represent a defect or
variance affecting only one channel. Support per-channel masks/variance or explicitly
use a joint-rejection policy.

Track at least:

- geometric contributor count or context;
- statistical `W_o`;
- `N_eff`;
- validity;
- actual variance;
- DQ bitwise combination according to a documented rule.

Do not define coverage as `W_o / max(W)`. That ratio changes globally when one pixel
has an unusually large weight, mixes geometry with exposure quality, and is unusable
with signed kernels. A `min_coverage` threshold should compare an explicit local
support metric with an expected exposure/support model.

### 4.8 Drizzle outlier rejection: median, blot, derivative, mask

Direct per-output sigma clipping is not sufficient for drizzle because one source
pixel contributes to multiple output pixels and input samples do not initially share
one grid. Use a source-plane masking pass modeled on Fruchter–Hook/DrizzlePac:

1. Drizzle each exposure separately onto the common grid, normally with `p=1` for a
   smooth, well-covered model.
2. Form a robust median/model from those aligned products.
3. **Blot** the common model back into each original source grid using the exact
   inverse geometry and consistent unit scaling.
4. Compute a derivative image `D_i` from the blotted model to tolerate small
   registration and model-blurring errors near real edges.
5. Compare original and blotted samples in source coordinates.

For data in ADU with gain `G` electrons/ADU and read noise `RN_e` electrons, a
consistent noise estimate is

```text
sigma_ADU = sqrt(G * max(B + sky, 0) + RN_e²) / G,
delta     = abs(I - B).
```

The first-pass good-pixel criterion is

```text
delta <= derivative_scale_1 * D + snr_1 * sigma_ADU.
```

6. Grow around first-pass bad pixels. A neighboring pixel failing a second, more
   sensitive criterion

```text
delta <= derivative_scale_2 * D + snr_2 * sigma_ADU
```

   is also marked bad. DrizzlePac implements the neighborhood condition with a 3×3
   convolution of the first-pass good mask.
7. Add detector DQ, saturation blooms, and spatially detected trail masks.
8. Run the final drizzle once from original calibrated inputs with the completed
   source masks and the selected final `p`/grid.

The two thresholds, derivative definition, growth radius, gain/units, and sky term
must be serialized in provenance. For satellite and aircraft trails, augment the
pixel test with connected-component/line detection and growth; a thin per-pixel
test alone can leave fragmented tracks.

### 4.9 Choosing output scale and `pixfrac`

Use measured sampling, not fixed folklore.

1. Measure source PSF FWHM and registration uncertainty across the field.
2. Plot fractional dither phases after the complete distortion map.
3. Make a Point-kernel phase-coverage image on candidate output grids.
4. Simulate the actual masks and weights; inspect minimum/percentile support and
   `N_eff`, not only mean coverage.
5. Choose an output scale that gives useful sampling of the narrowest reliable PSF.
   A practical start is about 2–2.5 output pixels across FWHM, then validate with
   stars; this is not a universal Nyquist proof for every PSF.
6. Start with `p` slightly larger than `s` so drops spill into adjacent output pixels,
   as STScI recommends, then reduce it only while coverage remains acceptably uniform.
7. Compare encircled energy, stellar FWHM/ellipticity, astrometry, aperture flux,
   blank-sky covariance, and holes across candidate settings.

For an ideal integer `S×S` interlace, at least `S²` well-distributed phases are
needed even before masks and rejected pixels. Random dithers generally need more.
More frames at the same phase improve SNR but not sampling rank.

Smaller `p` reduces the extra drop convolution and correlated noise but increases
coverage variation and holes. Larger `p` smooths coverage but broadens the PSF and
correlates neighbors. Increasing `S` without phase diversity only divides the same
information among more pixels.

### 4.10 Correlated noise

Square drizzle with `p>0` splits one noisy source pixel among neighboring outputs.
Their noise is correlated, so pixel-to-pixel RMS understates noise in an aperture or
smoothed measurement.

For many uniformly distributed dithers with equal weights, Fruchter and Hook define

```text
r = p/s = pS.
```

The asymptotic correlation ratio is

```text
R = 1 / (1 - r/3),             r <= 1
R = r / (1 - 1/(3r)),          r >= 1.
```

Sanity checks: `p→0` gives `R→1`; both branches give `R=1.5` at `r=1`; and larger
`p/s` increases correlation. The formula assumes a filled uniform dither pattern
and square drops. It is not valid unchanged for sparse/structured phases, spatially
varying distortion or weights, Gaussian kernels, or Lanczos.

For the real dataset, estimate covariance by one or more of:

- propagate a compact covariance stencil from shared source coefficients;
- drizzle independent unit-white-noise realizations through the exact transforms,
  masks, weights, and kernel;
- measure blank-sky apertures over several sizes after removing real structure.

For an aperture with coefficient vector `h`, use

```text
Var(aperture) = h^T C h,
```

not merely the sum of diagonal variance pixels. Store the kernel/settings needed to
reproduce the correction.

### 4.11 Drizzle reference pseudocode

```text
grid = build_output_grid_from_mapped_pixel_boundaries(frames, mode, S)
validate_grid_and_transforms(grid, frames)

if artifact masks are incomplete:
    per_frame_drizzles = drizzle_each_frame_for_model(p=1)
    robust_model = median(per_frame_drizzles)
    for each frame:
        blot = map_model_back_to_source(robust_model, frame.inverse_geometry)
        derivative = spatial_derivative(blot)
        frame.artifact_mask |= two_pass_noise_derivative_test(
            frame.image, blot, derivative, frame.variance
        )

initialize tiled N, W, Vnum, C2, contributor_count, context, dq
for frames in deterministic order:
    compute normalization and normalized variance
    for each valid source pixel u:
        drop = map_shrunken_source_footprint(u, p, frame.forward_map, grid)
        require finite positive drop area
        for output pixel o intersecting drop:
            a = overlap_area(drop, pixel(o)) / area(drop)
            c = statistical_weight(u) * a
            N[o,c]    += c * science_for_output_units(u,c)
            W[o,c]    += c
            Vnum[o,c] += c*c * variance_for_output_units(u,c)
            C2[o,c]   += c*c
            update contributor/context/DQ separately

for each output pixel/channel:
    valid = support_policy_passes and W > 0
    science = N/W if valid else fill
    variance = Vnum/(W*W) if valid else invalid
    n_eff = W*W/C2 if valid else 0
write grid metadata, normalization, transforms, masks, kernel, and diagnostics
```

---

## 5. Numerical, memory, and determinism requirements

### 5.1 Precision

Science storage may remain `f32`, but normalization fits, transform geometry,
polygon areas, moments, weight totals, and large accumulations SHOULD use `f64`.
Compensated or pairwise summation is required where a long stack or large dynamic
range can lose small contributions.

Reject non-finite intermediates with frame/pixel context. Do not repair them by
clamping. In particular, signed calibrated samples and signed Lanczos results are
valid; negative is not synonymous with invalid.

### 5.2 Tiling and streaming

Ordinary stacking SHOULD process output row/tile chunks sized from a declared memory
budget and support resident or memory-mapped source planes.

Drizzle can stream one source at a time, but the output accumulators can dominate
memory: for `C` channels, science numerator, weight, variance numerator, coefficient
squares, support, and context require several full output planes. A scalable design
uses output tiles:

1. map each source frame's valid footprint to output tile ranges;
2. process only source pixels whose drops intersect the active tile, including a
   kernel halo;
3. write finalized or mergeable accumulator tiles to the selected storage tier;
4. keep summation order deterministic.

Do not change numerical results when the memory tier changes. RAM and mmap/tiled
paths should be bit-identical when they use the same defined reduction order, or
within a documented tolerance if parallel reductions differ.

### 5.3 Cancellation and progress

Poll cancellation during loading, validation, normalization sampling, per-tile
combine, per-frame drizzle deposition, blotting, and finalization. Return no partial
science product as if it were complete. Progress phases should distinguish load,
normalization, model drizzle, blot/rejection, final accumulation, and output.

### 5.4 Provenance and metadata

Record at minimum:

- ordered input identities and rejection reasons;
- calibration/linear units and flux-vs-surface-brightness convention;
- reference and normalization coefficients/surfaces;
- variance model and weight semantics;
- transform direction, full coefficients/SIP, output WCS/grid/crop;
- estimator, rejection parameters, and local fallback policy;
- drizzle kernel, `s`, `S`, `p`, edge normalization, and covariance method;
- software version and deterministic/non-deterministic reduction mode.

Output metadata must be derived, not copied blindly from one input and not reset to
defaults.

---

## 6. Recommended starting policies

These are starting points to validate, not universal constants.

| Dataset | Reconstruction | Normalization | Weight | Rejection |
|---|---|---|---|---|
| Bias master | same-grid mean | none | equal or inverse variance | conservative Winsorized/transient mask |
| Dark master | same-grid mean | physical upstream scaling only | equal or inverse variance | conservative Winsorized/transient mask |
| Flat master | same-grid mean | multiplicative illumination level | propagated inverse variance | conservative clip of transients |
| Light, adequate sampling | registered weighted mean | robust gain + sky offset | per-pixel inverse variance | standardized asymmetric robust clip |
| Light, very small `N` | registered median/robust mean | same | explicit | local small-N policy |
| Undersampled, good dithers | exact Square drizzle | same | per-pixel inverse variance | source masks from median+blot+derivative |
| Mosaic | registered coadd or Square drizzle | overlap-constrained background + photometry | per-pixel inverse variance | masks plus robust combine/model |
| Point-source measurement | PSF-aware/proper coadd | photometric | model-derived | model/mask dependent |

Use exact Square as drizzle's correctness reference. Optimize to Turbo only after
cross-checking it over translations, rotations, shear, scale, and distortion.

---

## 7. Verification requirements

Tests must validate numeric answers and invariants, not merely successful execution.

### 7.1 Statistical-combine tests

1. **Equal mean:** `[1,2,3,4] → 2.5`; variance for four unit-variance samples is
   `1/4`; `N_eff=4`.
2. **Weighted mean:** values `[10,20]`, variances `[1,4]` give weights `[1,1/4]`,
   mean `12`, variance `0.8`, and `N_eff=25/17`.
3. **Manual-weight variance:** compare `sum(q²v)/sum(q)²` with a hand calculation;
   prove it does not incorrectly collapse to `1/sum(q)`.
4. **Gain propagation:** doubling a frame's science scale multiplies its variance by
   four and divides its inverse-variance weight by four.
5. **Normalization:** recover known gain/offset from paired synthetic structure with
   noise and injected outliers; test failed overlap and degenerate covariance.
6. **MAD zero:** many equal quantized samples plus one outlier must exercise the
   defined variance-floor behavior.
7. **Sigma boundaries:** values exactly on low/high thresholds survive; asymmetric
   sides differ as configured; iteration changes the result on a masking example.
8. **Winsorized:** hand-check clamping, `1.134` update, convergence, outer rejection,
   and minimum-survivor stop.
9. **Trimmed fraction:** hand-check floor counts, asymmetric trimming, ties, and the
   minimum survivor rule.
10. **Rank fit:** demonstrate that sorting preserves frame/weight association and
    that the scale is mean absolute residual if so configured.
11. **GESD:** reproduce Rosner/NIST example critical values and three-outlier result;
    test the largest-passing-index rule and `n=15/25` boundaries.
12. **Median:** test odd and even conventions and confirm no linear variance factor
    is emitted.
13. **Local small N:** masked borders trigger local fallback even when global frame
    count is large.
14. **RGB/CFA:** verify channel-specific and joint masks exactly; never compare
    unlike CFA colors.
15. **Precision:** compare compensated/SIMD results with an `f64` reference over
    large offsets plus small signals.
16. **Storage tiers:** RAM and mmap/tiled paths produce the declared identical or
    tolerance-bounded result.

### 7.2 Drizzle geometry and photometry tests

1. **Boundary mapping:** identity with `S=2` maps source center `0` to output `0.5`
   for a reference-footprint grid and maps both outer source boundaries exactly.
2. **Overlap conservation:** for interior drops, `sum_o A_overlap/A_drop = 1` over
   random phases, `p`, rotation, shear, affine scale, and safe homographies.
3. **Square reference:** compare polygon overlap and accumulated weights with pinned
   STScI `cdrizzle` fixtures.
4. **No double Jacobian:** Square, Turbo, Point, and normalized positive kernels
   preserve total per-input coefficient under uniform affine scale according to
   their defined semantics.
5. **Units:** a constant surface-brightness field keeps its numeric value at every
   `S`; a one-pixel integrated-flux impulse preserves total output flux only when
   the `s²` convention is applied.
6. **Translation/dither:** an ideal 2×2 half-pixel phase set at `S=2`, `p→0`
   interlaces into the expected four phases without holes.
7. **Rotation/shear:** exact Square matches a brute-force supersampled overlap
   reference; Turbo's measured error remains within its selection bound.
8. **Nonlinear mapping:** adaptive SIP subdivision converges as tolerance tightens.
9. **Masks:** a zero-weight cosmic-ray pixel contributes to no output numerator,
   variance, weight, context, or DQ-good count.
10. **Variance:** hand-compute `sum(c²v)/W²`; Monte Carlo white-noise trials match the
    predicted diagonal.
11. **Covariance:** two outputs sharing one source pixel have the analytically
    predicted nonzero covariance.
12. **Lanczos:** negative inputs/results remain signed, squared coefficients drive
    variance, and near-zero signed denominators are invalid rather than clamped.
13. **Edges:** kernels are not renormalized merely because support falls outside the
    requested crop; explicit crop loss is measurable.
14. **Coverage:** changing one frame's statistical weight does not alter geometric
    contributor count; an extreme central weight does not globally rescale edge
    validity.
15. **Metadata:** output WCS maps pixel centers/boundaries back to the same sky
    coordinates and records output pixel area/units.
16. **Blot rejection:** injected cosmic rays and trails are masked while shifted
    stellar cores survive derivative-aware thresholds.
17. **Cancellation and memory:** cancel every phase; enforce budget/overflow errors;
    compare streamed/tiled output with the all-resident reference.

### 7.3 End-to-end scientific tests

- inject stars with known flux, PSF, background, Poisson/read noise, dithers,
  distortion, bad pixels, cosmic rays, and a satellite trail;
- recover aperture flux, centroid, FWHM, ellipticity, background, variance, and
  rejection masks against hand-defined tolerances;
- compare ordinary registered stack and drizzle at equal output sampling;
- prove drizzle improves sampled resolution only for the phase-diverse undersampled
  set, not the repeated-phase control;
- compare blank-sky aperture noise with the propagated covariance/correlation model.

---

## 8. Common failure modes

- **Double resampling:** pre-warping and then drizzling.
- **Transform reversal:** passing registration's reference-to-source inverse warp to
  a source-to-output deposition API without inversion.
- **Lost distortion:** inverting only the linear transform and dropping SIP.
- **Half-pixel grid error:** scaling centers as `S*x` while sizing the grid from
  scaled pixel boundaries.
- **Histogram-only transparency fit:** confusing noise/structure MAD with
  photometric gain.
- **Unscaled variance:** applying gain `g` to data but not `g²` to variance.
- **Observed-value Poisson weights:** biasing the weighted mean toward downward
  fluctuations.
- **Raw-value clipping:** applying equal thresholds to unequal-variance samples.
- **Zero MAD early exit:** keeping an obvious isolated outlier in quantized data.
- **Misnamed algorithms:** treating fixed-count trimming as Siril percentile
  clipping, or rank fit as a temporal/spatial gradient fit.
- **Weight as coverage:** normalizing accumulated statistical weight by a global
  maximum and using it as geometric validity.
- **Second Jacobian:** dividing a drop coefficient by local area after it was already
  normalized by mapped drop area.
- **Signed-kernel clamp:** forcing negative Lanczos/science values to zero.
- **Crop renormalization:** renormalizing a kernel over only in-bounds taps and
  brightening edge contributions.
- **Diagonal-only uncertainty:** summing drizzle variance pixels as if neighbors were
  independent.
- **Drizzle without dithers:** creating more pixels, not more information.
- **Background overfit:** subtracting extended astrophysical emission as a mosaic
  “gradient.”

---

## 9. Open-source implementation comparison

### 9.1 STScI `drizzle`

The maintained STScI implementation is the primary geometric reference.

- `pixmap` explicitly maps input coordinates to output coordinates.
- the exact Square kernel maps the four shrunken corners, computes mapped drop area
  (`jaco`), and uses `overlap/jaco` once;
- Turbo uses an axis-aligned footprint normalized by `pixel_scale_ratio²/p²` without
  an additional local Jacobian;
- Point deposits a unit coefficient without a Jacobian;
- Gaussian and Lanczos use kernel coefficients and STScI warns that they do not
  conserve flux;
- `iscale` separates science-unit scaling from `pixel_scale_ratio`, which sizes
  kernels;
- `data2` is accumulated with squared weights for variance propagation;
- DQ and context outputs remain separate from the science/weight images.

These details are visible in `src/cdrizzlebox.c` and `drizzle/resample.py` at the
pinned revision in §12.

### 9.2 DrizzlePac

DrizzlePac supplies the rejection workflow missing from the linear drizzle kernel:
separate drizzles, median model, blot back to the source plane, derivative-aware
two-threshold cosmic-ray detection, neighborhood growth, and final masked drizzle.
Its current `drizCR.py` implements the threshold as the sum of a model-derivative
term and a gain/read-noise term.

### 9.3 Siril

Siril is a useful same-domain reference for astrophotography stacking:

- normalization distinguishes additive, multiplicative, and scale variants;
- noise weights include the square of the normalization scale;
- Winsorized clipping has an inner Huber clamp/scale loop and an outer reject/repeat
  loop;
- linear-fit clipping sorts the pixel stack and fits value against rank;
- its “percentile” method is fractional deviation from the median, not fixed-count
  trimming;
- GESD is exposed for larger stacks.

Source comparison is more reliable than copying old UI documentation; semantics
have changed across Siril releases.

### 9.4 CCDProc and Astropy

CCDProc cleanly separates clipping-generated masks from combination and offers a
memory-limited `combine` path, per-image/per-pixel weights, mean, median, extrema
clipping, and Astropy sigma clipping. Its code is a good API reference, but its
default uncertainty calculations are not a substitute for the general propagated
weighted-variance formula: source inspection shows the average-combine uncertainty
is based on sample deviation divided by survivor-count square root even when a
weighted mean is requested.

### 9.5 SWarp

SWarp demonstrates large-image resampling/mosaic coaddition, weight-map handling,
background modeling, and a clipped weighted mean. Its clipped path compares samples
with a median using an effective noise term that includes the input variance map and
signal/gain term, then inverse-variance combines survivors. It is a useful reference
for heteroscedastic rejection and tile-scale engineering, not for variable-pixel
drizzle geometry.

### 9.6 Proper coaddition

Zackay and Ofek's proper coadd is outside Lumos's current Stage 5 API but sets an
important boundary on claims of “optimal” stacking. A scalar weighted mean is
minimum-variance for a common pixel value under its assumptions. When exposures have
different PSFs and the goal is point-source detection/photometry, a PSF-aware Fourier
coadd can preserve more information and produce white noise. Stage 5 should reserve
room for that product rather than baking a seeing heuristic into statistical weight.

---

## 10. Current Lumos source audit — 2026-07-21

This section describes the repository at the audit date. “Implemented” does not mean
the normative design above should be weakened; “gap” does not mean code was changed
as part of this documentation work.

### 10.1 Statistical stack: working behavior

The current combine path has several strong foundations:

- `StackFrame::registered` preserves source-domain MAD statistics before
  interpolation and carries separate warp coverage/confidence planes.
- coverage gates sample inclusion; interpolation confidence multiplies the
  statistical weight exactly once.
- registered global normalization fits paired samples over a common valid domain,
  uses stratified sampling, robust residual clipping, and errors-in-variables Deming
  gain; multiplicative normalization uses common-domain statistics.
- noise weighting includes normalization gain squared through
  `1/(gain * sigma)²`; the older claim that gain was omitted is stale.
- value/index association is retained through rejection sorts and compaction.
- weighted mean uses precision-preserving compensated accumulation.
- GESD uses Rosner's two-sided mean/sample-standard-deviation statistic, accurate
  Student-t critical values, and the largest passing index; its preset falls back to
  median below 15 frames.
- mean products emit per-channel sums of surviving effective weights and
  `sum(w²)/sum(w)²`; median correctly emits no linear variance factor.
- RAM and mmap tiers share the chunked combine path, with validation, cancellation,
  and tests for tier equivalence.

### 10.2 Statistical stack: corrections and remaining gaps

| Priority | Finding | Consequence / required direction |
|---|---|---|
| P0 | No calibrated per-pixel variance plane enters combine. | Noise weight is a frame-level background-MAD proxy; the linear factor is not science variance. Implement §2.5 and `sum(w²v)/sum(w)²`. |
| P1 | Rejection uses raw normalized values and ignores heteroscedastic variances/weights. | Noisy samples have uncalibrated rejection probability. Standardize residuals. |
| P1 | `coverage` counts geometrically supported frames before rejection and ignores confidence. | It cannot answer how many samples survived. Add survivor/rejection/`N_eff` maps rather than changing coverage's meaning silently. |
| P1 | Global normalization requires support common to every registered frame. | Disjoint mosaic tiles return `NoCommonCoverage`; there is no overlap-graph or surface background solution. |
| P1 | Reference selection is lowest average source MAD. | It is robust and stable but not a full photometric/PSF/overlap reference criterion. |
| P1 | RGB noise weighting averages channel sigmas into one frame scalar. | It is not per-channel inverse variance; channel-shaped variance/weights are needed. |
| P1 | Median ignores requested frame weighting (with a warning). | Correct for an unweighted median, but the API should make weighted-median intent explicit. |
| P1 | Sigma clipping exits when MAD is zero; its `N>=10` fast path also treats constant trimmed data as no-rejection. | An isolated outlier on otherwise equal/quantized samples can survive. Add a propagated noise/quantization floor. |
| P1 | Current Winsorized mode starts with `1.134` times an RMS about the median, performs one robust-estimate/reject pass, and does not repeat Siril's outer reject/re-estimate loop. Siril starts its first clamp from an uncorrected ordinary sample deviation. The Lumos enum comment says “replace” although the final combine rejects originals. | Fix naming/docs or implement the declared variant exactly; do not claim source equivalence. |
| P1 | LinearFit's first pass is median/MAD; later passes fit sorted value against rank and scale by mean absolute residual. | Existing comments that it fits a reference relation or handles sky gradients are misleading; `sigma_*` parameter names are not statistically calibrated. |
| P1 | Percentile removes `floor(N*p/100)` values from each sorted tail. | This is a trimmed mean, not Siril percentile-deviation clipping. Rename serialized/UI semantics. |
| P2 | Preset fallbacks are based on global frame count. | Local edge/mask counts can be smaller; resolve fallback per output sample. |
| P2 | Final median averages the two middle values for even `N`, but hot-path rejection centers/MADs use an upper-middle order statistic. | The estimator and rejection center have different even-`N` conventions; make the distinction intentional and test asymmetric effects. |
| P2 | No explicit low/high rejection maps, survivor count, chi-square, or effective PSF are emitted. | Quantitative diagnosis and artifact auditing remain limited. |

### 10.3 Drizzle: working behavior

The independent public drizzle API currently provides:

- streaming of path inputs one frame at a time;
- exact Square polygon overlap ported from STScI `boxer` geometry;
- Turbo, Point, Gaussian, and Lanczos-3 alternatives;
- frame and nonnegative per-pixel scalar weight maps;
- a shared accumulated weight and coefficient-square linear factor;
- validation of `scale`, `pixfrac`, fill, coverage threshold, and the policy that
  Lanczos requires `scale=1`, `pixfrac=1`.

The Square kernel's `overlap / mapped_drop_area` coefficient agrees with the STScI
reference for supported linear/projective transforms.

### 10.4 Drizzle: correctness and integration gaps

| Priority | Finding | Consequence / required direction |
|---|---|---|
| P0 | Drizzle is a standalone API and is not wired into the alignment/stacking pipeline. | Normalization, source statistics, masks, registration diagnostics, and output provenance are not coordinated. |
| P0 | `DrizzleFrame.transform` is documented input→common, while registration's canonical `Transform`/`WarpTransform` maps reference→source for inverse warping. The same base type permits the wrong direction. | A caller can drizzle with a reversed transform. Use direction-specific types and derive the forward map. |
| P0 | Drizzle accepts only linear/projective `Transform`; registration's optional SIP lives in `WarpTransform` and has no forward inverse here. | Drizzle silently cannot reproduce Stage 4's full distortion correction. Implement a numerically invertible complete map. |
| P0 | Output centers use `scale * transformed_center` and dimensions use `ceil(input_size*scale)` without the boundary-derived half-pixel offset. | For `scale != 1`, the grid is shifted relative to its scaled source footprint, clipping one side and leaving asymmetric blank area. Implement §4.2. |
| P0 | Output grid is fixed from one input size; there is no union/intersection/crop offset or WCS derivation. Finalization constructs `AstroImage::from_planar_channels`, which resets metadata. | Mosaics, transformed extents, units, astrometry, and provenance are lost. |
| P0 | Turbo, Point, Gaussian, and Lanczos divide their already normalized per-input coefficients by `local_jacobian`; Square does not. STScI applies no such second division to those kernels. | Relative frame/pixel weights are distorted under scale or spatially varying transforms, and kernels disagree. Remove the extra factor or redefine/test coefficients from first principles. |
| P0 | Lanczos finalization clamps negative values to zero. | This breaks linearity, signed calibrated data, ringing symmetry, background statistics, and photometry. Never clamp science output. |
| P0 | Frame validation checks dimensions and weight maps but not transform validity or non-finite source samples. | A NaN or invalid homography can contaminate accumulators or produce nonsensical indices. |
| P1 | No input variance/DQ/channel mask enters drizzle; `weight_sq` assumes unit common input variance. | Output is a coefficient factor, not propagated uncertainty; artifacts require externally prepared scalar weights. |
| P1 | There is no separate-drizzle/median/blot/derivative rejection workflow. | Drizzle itself cannot reject cosmic rays or trails. |
| P1 | Coverage is accumulated signed/statistical weight divided by its global maximum; `min_coverage` uses the same global ratio. | Geometry, exposure quality, Jacobian, and signed kernels are conflated; one extreme pixel changes validity elsewhere. |
| P1 | Gaussian/Lanczos normalize over only in-bounds taps. | The kernel changes at crop edges. Lanczos also permits negative accumulated weights while coverage assumes a nonnegative weight field. |
| P1 | Turbo remains axis-aligned with nominal `p*scale` size under every transform. | Rotation/shear/anisotropic or spatial scale is only approximated; exact Square should be the correctness default. |
| P1 | Science omits F&H `s²` and therefore uses surface-brightness-like numeric semantics, but output metadata does not state this. | Summed output DN grows by `S²`; callers cannot reliably interpret flux units. |
| P1 | Output accumulators are resident `f32` sums with no compensated reduction or cancellation during deposition. | Very large outputs/stacks can exceed memory, lose small contributions, and cannot be interrupted promptly. |
| P2 | `pixfrac=0` is rejected even for Point, which ignores it. | The interlacing limit cannot be represented directly. |
| P2 | A single shared weight plane is used for all channels. | Per-channel variance/masks and chromatic rejection cannot be represented. |

### 10.5 Recommended implementation order

1. Introduce an explicit reconstruction-frame/grid/unit/variance contract and
   direction-specific full mappings, then integrate drizzle with the Stage 4 result.
2. Correct boundary-derived grid geometry, output WCS/metadata, transform validation,
   and Square reference behavior.
3. Remove the non-Square second-Jacobian error; eliminate Lanczos clamping; separate
   signed statistical weight from geometric coverage.
4. Add per-channel masks, DQ/context, actual variance propagation, survivor/support
   diagnostics, and tiled `f64`/compensated accumulators.
5. Implement the median+blot+derivative source-mask workflow.
6. Standardize statistical-stack rejection with per-pixel variance and make the
   rejection variants/names match their actual mathematics.
7. Add covariance/PSF diagnostics and, separately, a proper-coadd path for
   measurement-oriented products.

---

## 11. Definition of done

Stage 5 is complete only when:

- ordinary stack and drizzle consume one coherent frame record with explicit units,
  variance, masks, and transform directions;
- normalization and weights are measured in valid domains and propagated through
  variance;
- every rejection method has exact documented semantics, local small-N behavior,
  survivor diagnostics, and reference-vector tests;
- drizzle maps pixel boundaries onto a caller-visible output WCS, applies full
  distortion once, conserves the declared quantity, and never clamps linear science;
- masks are generated in the source plane for drizzle artifacts;
- output variance uses actual coefficients and covariance is either propagated or
  explicitly characterized;
- memory tier, parallelism, and cancellation do not change scientific semantics;
- synthetic end-to-end tests recover flux, astrometry, PSF, noise, and masks within
  stated tolerances.

---

## 12. Sources and pinned implementations

### Primary literature and statistical references

- A. S. Fruchter and R. N. Hook, “Drizzle: A Method for the Linear
  Reconstruction of Undersampled Images,” PASP 114, 144–152 (2002),
  [arXiv:astro-ph/9808087v2](https://arxiv.org/abs/astro-ph/9808087),
  [DOI 10.1086/338393](https://doi.org/10.1086/338393). Primary source for drop
  geometry, weighted accumulation, `s²`, masks/blot flow, and correlated noise.
- B. Rosner, “Percentage Points for a Generalized ESD Many-Outlier Procedure,”
  Technometrics 25(2), 165–172 (1983),
  [DOI 10.1080/00401706.1983.10487848](https://doi.org/10.1080/00401706.1983.10487848).
- NIST/SEMATECH, [Generalized Extreme Studentized Deviate Test for
  Outliers](https://www.itl.nist.gov/div898/handbook/eda/section3/eda35h3.htm).
  Formula and worked Rosner example.
- B. Zackay and E. O. Ofek, “How to coadd images? I. Optimal source detection and
  photometry using ensembles of images,”
  [arXiv:1512.06872](https://arxiv.org/abs/1512.06872).
- B. Zackay and E. O. Ofek, “How to coadd images? II. A coaddition image that is
  optimal for any purpose in the background dominated noise limit,”
  [arXiv:1512.06879](https://arxiv.org/abs/1512.06879).

### Open-source implementations and official documentation

- STScI `drizzle`, revision
  [`f9e6f52a9ee69ba82d53f8826535083781e956fa`](https://github.com/spacetelescope/drizzle/tree/f9e6f52a9ee69ba82d53f8826535083781e956fa):
  [`src/cdrizzlebox.c`](https://github.com/spacetelescope/drizzle/blob/f9e6f52a9ee69ba82d53f8826535083781e956fa/src/cdrizzlebox.c)
  and
  [`drizzle/resample.py`](https://github.com/spacetelescope/drizzle/blob/f9e6f52a9ee69ba82d53f8826535083781e956fa/drizzle/resample.py).
- DrizzlePac official source documentation,
  [`drizzlepac.drizCR`](https://drizzlepac.readthedocs.io/en/latest/_modules/drizzlepac/drizCR.html),
  for blot/derivative cosmic-ray thresholds and growth.
- STScI HST notebooks,
  [Optimizing the Image Sampling](https://spacetelescope.github.io/hst_notebooks/notebooks/DrizzlePac/optimize_image_sampling/optimize_image_sampling.html),
  for practical output-scale/`pixfrac` validation.
- Siril, revision
  [`8ce9baa37215ae9783de16fa9e0d7a610303588d`](https://gitlab.com/free-astro/siril/-/tree/8ce9baa37215ae9783de16fa9e0d7a610303588d/src/stacking):
  `normalization.c`, `median_and_mean.c`, and `rejection_float.c`.
- Astropy CCDProc, revision
  [`c80e1f00b9326882c6b67011ea53b5bfc3be5d4f`](https://github.com/astropy/ccdproc/tree/c80e1f00b9326882c6b67011ea53b5bfc3be5d4f),
  and its official guide,
  [Combining images and generating masks from clipping](https://ccdproc.readthedocs.io/en/2.5.0/image_combination.html).
- Astropy,
  [`sigma_clip`](https://docs.astropy.org/en/stable/api/astropy.stats.sigma_clipping.sigma_clip.html),
  for explicit iterative center/scale/mask semantics.
- SWarp, revision
  [`bf4f496f18c04a8d32022b45449ef8675ab9b3da`](https://github.com/astromatic/swarp/tree/bf4f496f18c04a8d32022b45449ef8675ab9b3da),
  especially `src/coadd.c` and `src/back.c` for weighted/clipped mosaic coaddition
  and background handling.

### Local Lumos source audited

- `stacking/combine/config.rs`
- `stacking/combine/normalization/mod.rs`
- `stacking/combine/rejection.rs`
- `stacking/combine/cache/mod.rs`
- `stacking/combine/stack.rs`
- `stacking/drizzle/{accumulator,config,geometry,stack}.rs`
- `stacking/registration/{transform,resample}/`
- `stacking/product.rs`
- `io/astro_image/mod.rs`
