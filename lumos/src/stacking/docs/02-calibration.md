# Stage 2 — Sensor calibration and transient rejection

This document specifies calibration of linear, undemosaiced sensor samples. It is a
normative implementation contract, not an acquisition tutorial. “MUST”, “SHOULD”, and
“MAY” have their usual normative meanings.

Calibration estimates the celestial signal by removing additive instrument structure,
dividing out multiplicative response, recording samples that cannot be trusted, and
optionally identifying single-exposure transients. It does not remove photon noise,
read noise, or dark-current shot noise. In fact, noisy calibration masters increase the
variance of the result.

The required per-light order is:

~~~text
decode to one linear CFA domain
→ validate the complete calibration contract
→ subtract additive calibration
→ divide by a prepared flat response
→ repair or mask persistent defects
→ optionally detect cosmic rays
→ demosaic
→ register and combine
~~~

All sensor calibration happens before demosaicing. A demosaicer spreads one defective
photosite into several color samples; registration spreads it spatially. Calibration
after either operation is no longer the same measurement problem.

The adjacent [load/decode specification](01-load-decode.md) owns container parsing,
RAW linearization, active-area selection, black-model subtraction, and normalization.
This stage assumes those operations succeeded and must not repeat them.

## 1. Input and output contract

### 1.1 The scientific data domain

Every light and calibration frame in one set MUST be represented by the same affine
decoder transform:

~~~text
x = (linearized_code - decoder_black(y, x, phase)) / decoder_scale
~~~

The transform is fixed by acquisition metadata, not by the observed minimum or maximum
of a frame. In particular:

- camera white balance is unity;
- no transfer function, tone curve, denoising, sharpening, or color matrix was applied;
- samples are not clipped to [0, 1];
- the active rectangle and its CFA origin are identical;
- decoder black semantics and the normalization denominator are identical;
- finite negative values and values above one are legal.

This point changes the usual textbook notation. In conventional raw ADU equations, a
bias frame contains the camera pedestal. Stage 1 has already subtracted the decoder's
best black model. A Lumos bias master therefore estimates the residual repeatable
offset left after decoding, not necessarily the original positive pedestal. A matched
dark still estimates every repeatable additive term remaining in the decoded domain:
residual bias, dark current, amplifier glow, and stable readout pattern.

Mixing a master decoded with one black model or scale with a light decoded with another
is invalid even when both arrays have the same dimensions. No scalar rescaling can
generally repair a different per-phase or spatial black model.

### 1.2 Required products

A complete calibration bundle SHOULD contain:

1. an additive master for lights:
   - a matched raw master dark, or
   - a master residual bias plus a bias-free scalable dark-current master, or
   - a residual bias alone;
2. a prepared normalized flat divisor;
3. a persistent-defect mask with class bits;
4. variance or uncertainty for every master and prepared divisor;
5. acquisition/decode compatibility metadata;
6. construction provenance and quality measurements.

The per-light result SHOULD contain:

- calibrated CFA samples;
- a bit mask distinguishing source-invalid, saturated, persistent-defect,
  cosmic-ray, interpolated, and flat-invalid samples;
- a variance plane in the same sample units squared;
- immutable provenance identifying every master and operation;
- a calibrated-state marker that prevents accidental second application.

An implementation without mask and variance planes can produce a displayable image,
but cannot claim the full scientific contract in this document.

### 1.3 Global invariants

Before mutating a light, the implementation MUST validate all of the following:

1. width, height, active rectangle, orientation, and CFA phase are identical;
2. every sample and required scalar is finite;
3. the light has not already been calibrated;
4. every master was built in the same decoder domain;
5. acquisition settings meet the role-specific compatibility rules in section 2;
6. every divisor used for arithmetic is positive and valid;
7. masks and uncertainty planes have exactly the sensor dimensions;
8. a requested scaled-dark path has a bias-free dark and a valid scale;
9. a requested flat has a valid normalization for every CFA color present;
10. cancellation or validation failure leaves the light unchanged.

Set the calibrated marker only after validation and successful publication of the
complete result. An assertion or error halfway through calibration must not leave an
apparently calibrated partial frame.

### 1.4 Mask semantics

Use a bit mask, not a single Boolean, because several facts can apply at once:

| Bit | Meaning | Arithmetic policy |
|---|---|---|
| SOURCE_INVALID | undefined or non-finite source sample | never use as data |
| SATURATED | source or calibration sample reached the sensor's non-linear/saturation region | exclude from estimation |
| HOT | persistent high dark-current photosite | repair for demosaic; zero stack weight |
| COLD | persistent low-response photosite | repair for demosaic; zero stack weight |
| UNSTABLE | RTS/fading/intermittent photosite | zero stack weight |
| BAD_COLUMN | column or segment defect | zero stack weight; optional interpolation |
| FLAT_INVALID | response is non-positive, too uncertain, or below the permitted response | zero stack weight |
| COSMIC_RAY | transient detected in this exposure | zero stack weight |
| INTERPOLATED | value was synthesized for downstream geometry/demosaic | never treat as an independent measurement |

Masks from the source, masters, and calibration arithmetic combine by bitwise OR. A
filled value may be useful to a demosaicer, but its mask remains set so stacking does
not give synthetic data normal statistical weight. This is the approach used by
scientific open-source ISR systems such as
[Rubin Science Pipelines](https://pipelines.lsst.io/modules/lsst.ip.isr/index.html),
which carry image, variance, and mask planes together.

Determine saturation from the original linearized sensor code and the instrument's
validated linearity/saturation threshold, before black or master subtraction changes
the numeric value. Preserve that bit throughout calibration. When blooming or amplifier
cross-talk makes neighboring samples unreliable, the instrument profile also defines
the directional mask growth; a generic circular dilation is not a substitute for the
readout geometry.

Master construction is the one place where input masks do not simply OR into the
output. A masked contributor is excluded at that coordinate. The master coordinate is
invalid only if no acceptable contributor survives or a persistent condition is being
deliberately encoded. Store rejected/masked contributor counts so this exception is
auditable.

## 2. Calibration compatibility and selection

### 2.1 Compatibility key

Do not group frames by filename or frame-type label alone. Build a typed compatibility
key from decoded facts. Missing metadata is not a wildcard: either an explicit
instrument profile supplies the value, or the set is rejected as unverifiable.

All roles MUST match:

- camera/sensor identity and, where relevant, amplifier/readout channel;
- active width, height, crop, orientation, binning, and pixel aspect;
- Mono/Bayer/X-Trans pattern anchored to active coordinate (0, 0);
- decoder name/version/profile, linearization semantics, black model, and scale;
- gain/ISO, camera offset, ADC/readout mode, bit depth, and hardware conversion mode;
- on-camera processing state and RAW compression mode;
- exposure unit and metadata interpretation.

Role-specific rules are:

| Relation | Required match |
|---|---|
| raw dark ↔ light | exposure time, gain/ISO, offset, readout mode, and normally sensor temperature |
| bias ↔ any consumer | gain/ISO, offset, readout mode, minimum-exposure mode; temperature policy is instrument-specific |
| flat ↔ light | camera geometry/settings plus filter, aperture, focus, optical train, camera rotation, and dust configuration |
| flat-dark ↔ flat | exposure time, gain/ISO, offset, readout mode, and temperature |
| scalable dark ↔ light | same sensor mode; bias-free; exposure/temperature scaling explicitly supported and validated |

No universal temperature tolerance is correct. A cooled sensor profile may require the
same set point. An uncooled camera may group a narrow measured-temperature interval only
after real dark data show the residual is acceptable. Record both the requested set
point and measured sensor temperature when available.

Flat exposure need not equal light exposure, but every flat sample used must be inside
the detector's measured linear range. “Half the histogram” is only an acquisition hint,
not a substitute for a camera-specific saturation and linearity limit. The
[Astropy CCD Reduction Guide](https://www.astropy.org/ccd-reduction-and-photometry-guide/v/dev/notebooks/05-01-about-flat-corrections.html)
recommends high-count, unsaturated flats and per-frame normalization; the implementation
must enforce the actual numeric limits carried by the instrument profile.

### 2.2 Overscan and trim consistency

If a format exposes overscan, overscan correction occurs before master combination and
before the arrays are trimmed to their common active rectangle. Apply the identical
geometry and estimator to lights, bias, darks, flat-darks, and flats.

For an amplifier with overscan samples o(r, j) in row r:

1. mask invalid and saturated overscan samples;
2. reject isolated outliers robustly;
3. estimate b(r) by a median, trimmed mean, or a validated low-order fit;
4. subtract b(r) from every science pixel in that amplifier row;
5. record estimator, fit order, region, residual RMS, and rejected count;
6. trim overscan from every frame using the same section.

A per-row median is a safe default when enough overscan columns exist. A polynomial or
spline is allowed only when residual tests justify it; a high-order fit can turn
overscan noise into structured signal. ccdproc exposes row/column median and model-fit
overscan paths in its
[reduction toolbox](https://ccdproc.readthedocs.io/en/latest/reduction_toolbox.html#subtract-overscan-and-trim-images).

Camera RAW decoders commonly expose only the active area. In that case Stage 1's black
model is the only offset correction available and this stage records “no overscan,”
rather than inventing one.

### 2.3 Master selection

Choose exactly one additive path:

1. Prefer a fully matched raw dark and subtract it once.
2. Otherwise, if validated scaling is enabled, use a residual bias plus a bias-free
   dark-current master.
3. Otherwise use a residual bias alone.
4. Otherwise leave additive structure uncorrected and report that fact.

Never subtract a bias and then subtract a matched raw dark that still contains the same
residual bias. Never scale a raw dark that still contains bias. These are both
double-subtraction errors, not harmless alternative workflows.

For a flat, prefer a matched flat-dark. Use a residual bias only if dark signal during
the flat exposure is demonstrably negligible or already represented by the bias model.
If neither exists, a flat may be used only through an explicit “uncalibrated flat”
policy with a prominent quality warning; its additive pedestal will bias the response.

## 3. Forward model and calibration equations

### 3.1 Per-pixel model

For decoded CFA pixel p of color c:

~~~text
L_p = R_p S_p + A_L,p + n_L,p
F_j,p = R_p I_j,c + A_F,j,p + n_F,j,p
~~~

where:

- S is celestial photons expressed in the decoded linear scale;
- R is relative optical/pixel response, including vignetting, dust, and PRNU;
- A_L is repeatable additive light-frame structure;
- I is flat-source illumination for flat exposure j;
- A_F is repeatable additive structure during the flat;
- n terms are random shot, read, digitization, and temporal noise.

Calibration estimates S up to an arbitrary normalization constant. It does not estimate
or subtract sky background; sky is celestial signal for this stage.

### 3.2 Matched-dark path

Let D_L be a raw master dark made with the light's exposure and sensor settings, and
D_F a flat-dark matched to the flat:

~~~text
X_p = L_p - D_L,p
G_p = master_response(F_j,p - D_F,p)
m_c = robust normalization of G over valid pixels of color c
Q_p = G_p / m_c
C_p = X_p / Q_p
~~~

D_L contains every residual additive term left by Stage 1. Do not subtract B separately.
The flat-dark likewise removes both residual bias and flat-exposure dark signal.

### 3.3 Bias-only path

If dark current/glow is negligible:

~~~text
X_p = L_p - B_p
G_p = master_response(F_j,p - B_p)
Q_p = G_p / m_c
C_p = X_p / Q_p
~~~

The same bias master may occur in numerator and denominator. That shared dependency
matters to uncertainty propagation in section 9.

### 3.4 Scaled-dark path

Let D_raw be a raw dark master, B a residual-bias master, and
D_c = D_raw - B the bias-free dark-current template:

~~~text
X_p = L_p - B_p - k D_c,p
    = L_p - k D_raw,p + (k - 1) B_p
~~~

Then prepare and apply the flat as above. The expanded form is useful both for detecting
double subtraction and for propagating uncertainty. The scale k is not automatically
valid merely because both exposure times exist; section 6 defines its preconditions.

### 3.5 Why operation order is fixed

Additive structure is not multiplied by the optical flat response. Dividing first gives

~~~text
(R S + A) / Q = S · normalization + A / Q
~~~

and turns a constant or structured additive residual into a vignetted artifact. The
only valid order is additive subtraction followed by multiplicative division.

Prepare a flat in the same order: subtract its additive calibration before estimating
illumination normalization or using it as a divisor. Normalizing raw flats that still
contain bias is wrong because the scale estimate becomes a function of the additive
pedestal.

### 3.6 Numeric policy

All arithmetic uses at least f32 storage and f64 reductions for sums, variances, and
normalization statistics. Preserve finite negative values. A negative calibrated
background sample is an ordinary noise realization; clamping it to zero creates a
positive bias in means, centroids, and photometry.

Do not add an integer-storage pedestal internally. If an integer export requires a
pedestal, export is a separate reversible encoding step and records the constant.

Saturated samples remain saturated/masked. Subtracting a master cannot recover charge
lost to clipping, and flat division must not turn a saturated code into an apparently
valid sample.

## 4. Building calibration masters

### 4.1 Common construction pipeline

For each role:

~~~text
validate acquisition/decode compatibility
→ apply the same overscan and active-area policy
→ reject frames with invalid global quality
→ prepare each role-specific input
→ combine corresponding sensor coordinates
→ compute uncertainty and survivor diagnostics
→ validate the master
→ publish data + mask + variance + provenance atomically
~~~

Validation happens before loading the entire set when headers suffice, then again after
decode. The master inherits no arbitrary “first frame wins” metadata. Instead, common
metadata are checked for equality and master-specific values are derived:

- exposure is the common exposure or an explicit normalized unit;
- date/time becomes the covered interval;
- temperature records median, range, and individual values;
- NCOMBINE is the input count, with per-pixel survivor count stored separately;
- filter/readout/decode fields must be common;
- provenance contains source identities and hashes.

Reject a set if any coordinate has no surviving valid sample. Do not silently write zero.

### 4.2 Frame-level quality rejection

Before pixel rejection, compute per-frame diagnostics on valid samples and per CFA color:

- robust location and MAD scale;
- saturated, invalid, and masked fractions;
- row/column residual statistics;
- exposure, gain/ISO, offset, temperature, and filter;
- for flats, illumination level and large-scale gradient;
- for darks, broad-pattern amplitude and hot-pixel population.

Reject a frame, with a reason, when metadata mismatch, saturation, light leak, gross
gradient, readout failure, or distributional discontinuity makes it a different
acquisition. Pixel clipping cannot repair an entire bad frame.

Automatic frame rejection MUST use documented thresholds from an instrument/profile,
not a hidden generic percentage. Always preserve the measurements so a caller can audit
the decision.

### 4.3 Pixel combination defaults

The statistically efficient estimator for independent Gaussian noise is a mean.
A median is robust but has asymptotic variance:

~~~text
Var(median) ≈ π σ² / (2N)
Var(mean)   = σ² / N
~~~

Thus the median has about 1.57 times the variance, or 1.253 times the standard error,
of the mean in the clean Gaussian case. Prefer a robustly rejected mean when the sample
count supports outlier identification. The
[Astropy combination guide](https://www.astropy.org/ccd-reduction-and-photometry-guide/v/dev/notebooks/01-06-Image-combination.html)
reaches the same practical result: average to reduce noise, but clip transient extremes.

Lumos's current role presets are:

| Role | Per-frame normalization | Combine |
|---|---|---|
| bias | none | Winsorized-estimate 3σ rejected mean |
| raw dark | none | Winsorized-estimate 3σ rejected mean |
| flat-dark | none | same as dark |
| flat | multiplicative | 3σ iterative clipped mean at N ≥ 8; median below 8 |

There is no dark/bias median fallback in the current code. At N ≤ 2, its Winsorized
rejection retains all values and reduces to a mean. A quality report must say that no
outlier can be identified at those counts.

#### Winsorized-estimate rejected mean

For values v_i at one sensor coordinate:

1. Sort a working copy and initialize center to element floor(N/2).
2. Compute sample standard deviation about that center.
3. Multiply σ by 1.134.
4. Clamp the working values to center ± 1.5σ.
5. Recompute the middle-element center and corrected σ.
6. Repeat steps 4–5 until |σ_new - σ_old| ≤ 0.0005 σ_old or 50 iterations.
7. On the original values, retain
   center - k_low σ ≤ v_i ≤ center + k_high σ.
8. Return the f64-accumulated mean of survivors.

The default has k_low = k_high = 3. This algorithm Winsorizes only to obtain a robust
center/scale; its final output is the mean of original surviving samples, not the mean
of clamped values.

#### Iterative MAD sigma-clipped mean

For the flat default:

1. Start with every valid sample.
2. Select the upper sample median center.
3. Compute MAD = median(|v_i - center|) and σ = 1.4826022185 · MAD.
4. Retain center - 3σ ≤ v_i ≤ center + 3σ.
5. Repeat at most three times or until the survivor set is unchanged.
6. Return the f64-accumulated mean of survivors.

If σ is numerically zero, retain the current set. If fewer than eight flat frames are
present, the current preset returns their coordinate-wise median instead. An
implementation may expose other methods, but master metadata must record the exact
algorithm, constants, and per-pixel survivor count.

#### Master variance and survivor diagnostics

For each coordinate, retain the original count, valid count, low/high rejection counts,
final survivor count n, and estimator used. For a weighted surviving mean with values
v_i, independent input variances V_i, nonnegative weights w_i, and W = Σw_i:

~~~text
M = Σ(w_i v_i) / W
Var_pred(M) = Σ(w_i² V_i) / W²
N_eff = W² / Σw_i²
~~~

Require W > 0. Weights derived from uncertainty use w_i = 1/V_i only when every V_i is
finite and positive. With inverse-variance weights, test whether the observed scatter
exceeds the model:

~~~text
χ²_red = Σ((v_i - M)² / V_i) / (n - 1)
Var(M) = Var_pred(M) · max(1, χ²_red)      for n ≥ 2
~~~

The inflation is conservative evidence that the input noise model missed temporal or
frame-to-frame variation; never shrink below Var_pred. With equal weights and no input
variance model, use the ordinary survivor estimate:

~~~text
s² = Σ(v_i - M)² / (n - 1)
Var(M) = s² / n                              for n ≥ 2
~~~

At n = 1 the variance is unknown, not zero; require an input variance or mark the
uncertainty unavailable. Rejection makes an empirical survivor variance optimistic, so
prefer propagated input variances and use split-half/held-out residuals to detect the
missing component. For a median, use a whole-frame bootstrap when enough inputs exist;
the Gaussian π/2 approximation in section 4.3 is only an explicitly labeled fallback.

Include the Stage 1 quantization variance in every input V_i rather than allowing a
zero-noise input; combination then reduces that independent contribution normally. Do
not floor the combined master at one frame's quantization variance. For flats, per-frame
normalization correlates all coordinates, so a scalar coordinate-wise variance is not
the complete result; section 9.4 specifies the required rebuild bootstrap.

### 4.4 Master bias

A bias exposure uses the shortest supported exposure in the same readout mode. After
Stage 1 black subtraction it may be centered near zero; fixed spatial residuals remain
scientifically useful.

Build it without frame normalization. Combining enough independent bias frames reduces
random read noise while preserving repeatable row, column, amplifier, and pixel
structure. Overscan correction, if selected, is applied to every bias before combination.

A synthetic constant bias is valid only after measurements show the residual bias has
no material spatial structure and its level can be predicted from metadata. Record that
it is synthetic and its uncertainty; do not silently replace a structured master with
its median.

A “superbias” is a model of repeatable structure, not merely a smoothed bias. It is safe
only when cross-validation proves that the model predicts held-out bias frames better
than the ordinary master. A complete implementation:

1. split bias frames into training and validation sets;
2. form independent masters;
3. fit the proposed low-rank/multiscale row-column model to training data;
4. evaluate residual mean, row/column power, and RMS on validation data;
5. accept only if fixed pattern decreases without attenuating reproducible structure;
6. store model uncertainty and the training/validation identities.

No fixed wavelet-layer count is universally correct. Lumos currently has no superbias
model.

### 4.5 Master dark and flat-dark

Raw matched darks and flat-darks are combined without normalization: their absolute
level is part of the calibration. Normalizing a dark erases the exposure/temperature
signal it is supposed to measure.

For a scalable dark:

1. build raw D_raw without normalization;
2. build B from independent bias frames;
3. validate identical readout/decoder settings;
4. compute D_c = D_raw - B in float;
5. propagate mask and variance;
6. store D_c explicitly as “bias-free dark current,” never as an ambiguous “dark.”

Keep broad amplifier glow. It is signal, not an outlier. Pixel rejection operates across
frames at a fixed coordinate and therefore does not reject a stable glow structure.
A light leak or changing glow pattern is a frame/set mismatch and must be diagnosed at
frame level.

### 4.6 Master flat: required order

The correct order for variable-illumination flats is:

~~~text
calibrate every flat's additive signal
→ estimate every flat's illumination per CFA color
→ normalize each calibrated flat
→ reject/combine corresponding pixels
→ normalize the combined response per CFA color
~~~

For flat j and color c:

~~~text
G_j,p = F_j,p - P_j,p
a_j,c = robust_location({G_j,p | color(p)=c and valid(p)})
H_j,p = G_j,p · target_c / a_j,c
G_p = rejected_mean_j(H_j,p)
m_c = robust_location({G_p | color(p)=c and valid(p)})
Q_p = G_p / m_c
~~~

P is the matched flat-dark or residual bias. target_c may be the location of a chosen
reference flat or one; its absolute value cancels in final normalization.

Use a robust median or a sigma-clipped mean for a_j,c and m_c. Exclude:

- source-invalid and saturated samples;
- known defects and non-linear samples;
- configurable border/overscan regions;
- samples outside the instrument's valid flat range.

Compute each CFA color independently. This cancels changes in the flat source's spectrum
and prevents an LED/twilight color balance from becoming a white-balance operation.
Color calibration belongs later. For Mono there is one color.

Subtracting one master P after already normalizing raw F_j is not equivalent:

~~~text
normalize(F_j) - P ≠ normalize(F_j - P)
~~~

except in special degenerate cases. A flat builder must therefore have access to the
flat subtractor before its per-frame multiplicative normalization.

### 4.7 How many calibration frames

There is no scientifically universal count such as “20 flats” or “50 biases.” Stop when
the measured master uncertainty meets a declared budget.

For a mean with survivor weights w_i:

~~~text
N_eff = (Σw_i)² / Σ(w_i²)
Var(master) ≈ Σ[w_i² Var(v_i)] / (Σw_i)²
~~~

For equal independent samples this becomes Var(v)/N. For a Gaussian median, use the
π/2 efficiency penalty above. Rejection makes N_eff coordinate-dependent.

Let V_L,p be expected variance of one light before master subtraction and V_A,p the
additive master variance. If calibration is allowed to increase standard deviation by
at most fraction δ:

~~~text
sqrt(V_L,p + V_A,p) / sqrt(V_L,p) ≤ 1 + δ
V_A,p / V_L,p ≤ (1 + δ)² - 1
~~~

Evaluate this over a declared percentile of ordinary pixels and separately over broad
glow/amp regions. For the flat, require the relative divisor error sqrt(V_Q,p)/Q_p to
meet the bright-signal budget derived in section 9. Report failing pixels instead of
hiding them behind a frame-count rule.

Practical minimums still exist for estimator behavior: one or two frames cannot identify
an outlier, and the current flat sigma-clip preset requires eight. These are algorithmic
floors, not evidence that the resulting master is quiet enough.

### 4.8 Master validation

Before publication, verify:

- all output data/variance are finite and dimensions match;
- expected additive masters have no unexplained light leak or clipped region;
- the flat has a positive normalization for every CFA color;
- flat response and relative uncertainty distributions meet policy;
- survivor counts are nonzero and rejection rates are plausible;
- row/column/amp residuals do not reveal a mismatched mode;
- split-half masters agree within predicted uncertainty;
- a held-out light/flat calibration reduces the target structure;
- source identities, settings, algorithm parameters, and software version are stored.

Split-half agreement is especially valuable: independently combine alternating frames
into M_A and M_B, then inspect M_A - M_B for additive masters and M_A/M_B - 1 for flats.
Coherent residual structure is a stability or modeling problem that adding more frames
will not necessarily solve.

## 5. Prepared flat construction and application

### 5.1 Divisor validity

For each p, Q_p must be finite and positive. Also define a maximum permitted correction
gain g_max. A response is valid only if:

~~~text
Q_p ≥ 1 / g_max
relative_uncertainty(Q_p) ≤ configured_limit
sample was linear and had enough survivors
~~~

Do not silently replace a physically valid Q_p = 0.05 with 0.1 and call the result
calibrated. Flooring changes the inferred response by a factor of two. The scientific
policy is:

1. set FLAT_INVALID;
2. place a finite placeholder divisor, normally one, for safe arithmetic;
3. exclude the sample from scientific weighting;
4. synthesize a value only if demosaic requires one, retaining INTERPOLATED.

A display-only policy MAY clamp the divisor, but must record the clamp and may not
present the sample as scientifically valid.

### 5.2 Application

Given additive-corrected numerator X:

~~~text
if valid(Q_p) and valid(X_p):
    C_p = X_p / Q_p
else:
    C_p = finite placeholder
    mask_p |= relevant bits
~~~

Use one parallel pass, but keep validation separate and complete before mutation.
Division must preserve negative X. A normalized divisor near one preserves the
convenient overall numeric scale; normalization does not make the flat noiseless.

### 5.3 Current Lumos prepared flat

Current Lumos subtracts a master flat-dark, otherwise a bias, from the already-combined
flat. It then computes an arithmetic mean per CFA color (or one global mean for Mono),
divides each response by its color mean, and clamps every divisor to at least 0.1.
Application divides every light sample by that stored divisor.

That behavior is finite and CFA-aware, but it is not the full contract above:

- raw flat subs are multiplicatively normalized before their additive master is
  subtracted;
- arithmetic normalization includes saturated/defective samples;
- 0.1 is an unconditional hidden maximum gain of 10;
- there is no flat-invalid mask or divisor uncertainty;
- no acquisition compatibility beyond dimensions/CFA is established.

These are implementation gaps, not recommended behavior.

## 6. Dark scaling and optimization

### 6.1 Default policy

The safe default is k = 1 with a dark matched to exposure, gain/ISO, offset, readout
mode, and temperature. Scaling is an exception that requires evidence about a particular
sensor mode. A scalar can reproduce only a pattern whose amplitude changes uniformly;
it cannot repair a changed spatial shape.

Scaling MUST be disabled when any of these holds:

- the dark includes residual bias;
- the light and dark use different gain, offset, readout, crop, binning, or decoder domain;
- the dark template has insufficient spatial variation to estimate k;
- amplifier glow changes shape or scales differently from the pixel dark current;
- a material population of pixels has RTS, fading, or other non-linear behavior;
- the required scale extrapolates beyond a validated exposure/temperature interval;
- residual diagnostics fail;
- scaling a shorter dark upward would inject more master noise than policy allows.

The Astropy guide explicitly separates bias whenever darks are scaled and warns against
scaling short darks upward; see
[calibration choices](https://www.astropy.org/ccd-reduction-and-photometry-guide/v/2.0.1/notebooks/01-09-Calibration-choices-you-need-to-make.html)
and
[handling darks](https://www.astropy.org/ccd-reduction-and-photometry-guide/notebooks/03-04-Handling-overscan-and-bias-for-dark-frames.html).

### 6.2 Exposure-ratio scaling

If measurements establish that the bias-free dark signal is linear in exposure at fixed
temperature and sensor mode:

~~~text
k_t = t_light / t_dark
X = L - B - k_t D_c
~~~

Validate t_light > 0 and t_dark > 0 in the same units. Prefer t_dark ≥ t_light so
k_t ≤ 1. Multiplying D_c by k also multiplies its master uncertainty by |k|;
scaling up a noisy short dark can be worse than omitting it.

Linearity validation uses actual dark libraries:

1. acquire dark sets at at least three exposures spanning the intended range;
2. build a bias-free master rate R_t = D_c(t)/t for each exposure;
3. mask unstable/hot/saturated pixels for the bulk comparison;
4. compare rate maps, amplifier regions, row/column profiles, and glow regions;
5. fit residual versus exposure at every diagnostic region;
6. approve an interval only when systematic residuals and uncertainty meet policy;
7. retain the test data and approved interval in the instrument profile.

A high global correlation is insufficient if an amp-glow corner fails.

### 6.3 Temperature libraries

Do not encode a generic “dark current doubles every N degrees” rule. Devices and
readout glow differ. Build measured, bias-free dark-rate masters at stable temperature
set points.

For a light temperature T bracketed by T0 and T1, an implementation MAY interpolate:

~~~text
α = (T - T0) / (T1 - T0)
R(T) = (1 - α) R(T0) + α R(T1)
D_c(T, t) = t R(T)
~~~

only after held-out darks demonstrate that this interpolation meets the residual budget.
Interpolate amplifier/glow components separately if their measured law differs.
Do not extrapolate beyond the library. For an uncooled camera whose temperature changes
during exposure, header temperature may not represent the photosite history; empirical
validation is mandatory.

### 6.4 Empirical optimization

Empirical “dark optimization” estimates k from pattern agreement rather than assuming
time linearity. It is experimental because stars, sky structure, gradients, and flat
errors can mimic or obscure a dark pattern.

A robust implementation works on bias-subtracted, source-masked data:

1. Form Y = L - B and use bias-free D_c.
2. Build a fit mask excluding saturation, source-invalid samples, known unstable
   defects, cosmic rays, and detected astronomical sources grown beyond their PSF.
3. Partition by amplifier and CFA color. Require a minimum valid sample count and
   nonzero robust variance in D_c.
4. Remove only a low-order sky model from Y. Apply the identical linear high-pass
   operator to D_c, so the fitted variables are y_i and d_i in the same spatial band.
5. Fit y_i = a + k d_i with an intercept. The intercept absorbs residual sky level.
6. Initialize from centered covariance:

   ~~~text
   k0 = Σ w_i(d_i - d̄)(y_i - ȳ) / Σ w_i(d_i - d̄)²
   a0 = ȳ - k0 d̄
   ~~~

7. Iteratively reweight residuals r_i = y_i - a - k d_i with a Huber loss:

   ~~~text
   q_i = r_i - median(r)
   s = max(1.4826022185 · median(|q_i|), s_floor)
   u_i = 1                         if |q_i| ≤ 1.345s
         1.345s / |q_i|            otherwise
   ω_i = base_weight_i · u_i
   ~~~

   s_floor comes from propagated read/quantization noise and must be finite and positive.
   A base weight is the inverse predicted variance of y_i - kd_i when a variance model
   exists, otherwise one; recompute it when k changes. Given the current ω_i, form:

   ~~~text
   W  = Σω_i          D  = Σω_i d_i       Y  = Σω_i y_i
   DD = Σω_i d_i²     DY = Σω_i d_i y_i     Δ = W·DD - D²

   a = (Y·DD - D·DY) / Δ
   k = (W·DY - D·Y) / Δ
   ~~~

   Require Δ to exceed a scale-aware positive floor; otherwise the dark pattern is
   unidentifiable. Recompute residuals and stop when
   |k_new - k_old| ≤ 10⁻⁴ max(1, |k_old|), or after 25 iterations.
8. Enforce a configured physical interval, normally including k ≥ 0. Reject a solution
   that lands on a bound.
9. Estimate uncertainty from a spatial block bootstrap over the fit region. In ordinary
   weighted least squares, the k diagonal of (XᵀΩX)⁻¹ is W/Δ; multiply it by an estimated
   residual scale only when Ω contains relative rather than physical inverse-variance
   weights. Huber reweighting and coherent dark/glow residuals require a robust or block
   covariance, so the matrix result is a diagnostic, not the acceptance uncertainty.
   Require k/σ_k and template explanatory power to exceed configured thresholds.
10. Apply k only if residual spatial power, amp profiles, and held-out regions improve
    without creating negative glow-shaped structure.

Fit regions must be independent of the regions used for final validation. A single
central box can miss edge glow.

For comparison, current Siril source performs either exposure-ratio scaling or a
golden-section search over k ∈ [0, 2], minimizing summed normalized σ in a central
512 × 512 area. That is a useful open-source heuristic, not a general physical proof;
see pinned
[Siril preprocess.c](https://gitlab.com/free-astro/siril/-/blob/8ce9baa37215ae9783de16fa9e0d7a610303588d/src/core/preprocess.c).

### 6.5 Residual acceptance test

For candidate k, compute R = Y - kD_c. On source-free validation pixels report:

- robust RMS before and after;
- correlation of R with D_c;
- row, column, amplifier, and radial/glow profiles;
- fraction made implausibly negative;
- spatial power spectrum in low/mid/high bands;
- result for k ± σ_k;
- predicted injected master variance k²V_D.

Reject optimization if it merely minimizes random-looking variance while leaving a
coherent dark-shaped residual. Record k per light, its uncertainty, method, fit mask,
and diagnostics.

Lumos currently implements only k = 1 and stores raw matched darks. This is safe but
requires darks matched to the light exposure and sensor conditions.

## 7. Persistent defects

### 7.1 Defect taxonomy

Persistent detector problems are not one class:

- hot/warm: excess stable dark current;
- cold/dead: abnormally low illuminated response;
- unstable/RTS: switches between discrete dark-current levels;
- fading/non-linear: response depends on exposure history or signal;
- bad row/column/segment: coherent readout or response defect;
- CFA-specific phase-detect/autofocus sites;
- source-invalid or permanently saturated sites.

A dark reveals high dark-current and instability. An illuminated flat reveals low
response. Multiple exposure levels reveal non-linearity. A master value alone cannot
classify temporal stability.

Keep class bits even if repair is identical. Stable hot pixels can sometimes be
dark-corrected with increased variance; unstable pixels must remain excluded. HST's
open calibration documentation similarly distinguishes stable hot pixels from RTS and
fading pixels and propagates data-quality flags; see
[ACS pixel stability](https://hst-docs.stsci.edu/acsdhb/chapter-4-acs-data-processing-considerations/4-3-dark-current-hot-pixels-and-cosmic-rays#id-4.3DarkCurrent,HotPixels,andCosmicRays-4.3.3PixelStability).

### 7.2 Lumos hot-pixel detector

The current Lumos detector is substantially more robust than a global
median-plus-standard-deviation threshold. Its exact algorithm is:

1. Split the master dark into approximately 64 × 64 tiles:

   ~~~text
   tiles_x = ceil(width / 64)
   x0 = tx · width / tiles_x
   x1 = (tx + 1) · width / tiles_x
   ~~~

   and likewise for y. This distributes remainder pixels evenly.
2. For each tile and CFA color, compute the sample median. Mono has one color, Bayer
   and X-Trans have three.
3. Place tile samples at their geometric centers and bilinearly interpolate/extrapolate
   per color to obtain broad background H_p. A global per-color median fills a missing
   tile color on very small images.
4. Form residual r_p = D_p - H_p.
5. Collect at most 100,000 residuals per color. Allocate the quota proportionally
   across the color's CFA phases, stratify over rows and columns, then sort indices.
   This avoids row-major stride aliases.
6. Compute:

   ~~~text
   μ_c = median(r)
   MAD_c = median(|r - μ_c|)
   σ_MAD = 1.4826022185 · MAD_c
   σ_tail = P99(|r - μ_c|) / Φ⁻¹(0.995)
          = 0.38822448 · P99(|r - μ_c|)
   σ_c = max(σ_MAD, σ_tail, σ_resolution)
   ~~~

   The two-sided absolute 99th percentile corresponds to normal quantile 0.995.
   σ_tail is enabled only with at least 500 samples.
7. σ_resolution is the master CFA quantization σ when available. Otherwise it is
   max(|D|) times f32 epsilon.
8. Clamp the requested threshold to k ≥ 1 and flag:

   ~~~text
   HOT_p iff r_p > μ_color(p) + k σ_color(p)
   ~~~

   The default is k = 5.

Only the positive tail is a hot-pixel candidate. The broad tiled model prevents smooth
amp glow from being labeled as millions of hot pixels; the upper-bulk scale protects
against column/model residuals inflating the sparse-defect tail.

Quality requirements missing from the current implementation are:

- exclude known invalid/saturated samples from tile and residual statistics;
- validate finite, positive k rather than turning NaN into surprising comparisons;
- measure stability across individual dark frames, not only the master;
- represent clusters/columns separately;
- use a union count when one coordinate has more than one class.

### 7.3 Cold/dead response detector

Detect cold pixels from the additive-corrected flat response before normalization or
flooring. A global lower threshold fails under vignetting, so use a local same-color
reference.

For valid response G_p:

~~~text
M_p = median({G_q | q is a valid same-color neighbor of p})
ratio_p = G_p / M_p
COLD_p iff M_p > 0 and ratio_p < f_dead
~~~

The current f_dead is 0.5. Neighborhoods are:

- Mono: the valid 8-connected neighbors;
- Bayer: offsets (±2, 0), (0, ±2), and (±2, ±2);
- X-Trans: nearest 24 same-color sites within Chebyshev radius 6, ordered by Manhattan
  distance.

Skip already masked neighbors. Require a configured minimum neighbor count; do not
classify when no positive local reference exists. Dust shadows and vignetting normally
affect a neighborhood together, leaving the local ratio near one, while a dead
photosite is isolated and near zero.

The current Lumos code uses the same neighborhoods but does not explicitly require
M_p > 0 or a minimum count.

### 7.4 Unstable and RTS pixels

Use the individual calibrated dark frames, not only their combined master. For each
pixel p with N observations d_i and predicted random variance e_i²:

~~~text
observed = sample_variance(d_i)
expected = mean(e_i²)
excess = max(0, observed - expected)
~~~

Flag instability when excess is inconsistent with the instrument's validated random
model and the time series shows state changes or exposure-dependent fading. A robust
implementation combines:

1. a χ² or variance-excess test with multiple-testing control;
2. a two-state mixture fit compared with a one-state model;
3. split-time and exposure-length consistency tests;
4. a minimum amplitude relevant to science data.

Thresholds must be trained on the camera mode. HST ACS, for example, uses a
camera-specific variance/ERR stability ratio and time-dependent thresholds; its numbers
must not be copied into a different detector.

Update defect libraries over time and store their validity interval. A pixel can age,
anneal, or change state.

### 7.5 Rows, columns, and clusters

After isolated detection, analyze the response residual and dark residual for coherent
segments. An IRAF/ccdmask-derived procedure is:

1. Compute local residual E = image - moving_median(image), per CFA color/phase.
2. The median box must span more than twice the widest defect to be detected; 7 × 7
   spans a single bad column in ordinary mono data.
3. Estimate local σ from a window containing roughly 100 or more valid values:

   ~~~text
   σ_local = (P69.1(E) - P30.9(E)) / 2
   ~~~

4. Flag individual values outside [-k_low σ_local, k_high σ_local], normally with a
   conservative k of at least six.
5. For each candidate readout column and contiguous segment, sum unflagged residuals.
   Compare the sum to k σ_local sqrt(n), so a weak but coherent column can be detected.
6. Fill gaps shorter than configured ngood between flagged portions of one column.
7. Repeat on rows only if the detector/readout model makes rows a meaningful special
   direction.
8. Merge connected components and classify isolated, compact cluster, column/row
   segment, or large invalid region.

ccdproc implements this open IRAF-derived method, including percentile σ, column sums,
and gap filling; see
[ccdmask](https://ccdproc.readthedocs.io/en/latest/api/ccdproc.ccdmask.html).
For a flat, also compare independently built split-half response masters or two
illumination levels; stable low response and non-linear response are different defects.

Lumos currently has no explicit row/column/cluster detector.

### 7.6 CFA-aware repair

Create the union defect mask before repairing any pixel. A repaired value may never be
used as if it were an original good neighbor during the same pass.

For an isolated defect:

1. gather valid, unmasked same-color neighbors using the layouts in section 7.3;
2. require enough spatial directions, not merely enough samples on one side;
3. take their median;
4. write the finite fill value;
5. retain defect and INTERPOLATED bits.

Current Lumos uses exactly the Mono/Bayer neighborhoods above and the nearest 24
X-Trans neighbors in radius 6. It masks all hot and cold positions first, so repair
order does not contaminate the median.

For a column or large cluster, a small local median can copy one side across an
astronomical edge. Prefer leaving zero scientific weight and use a fill only for
demosaicing. A fallback fill SHOULD fit a robust local plane to unmasked same-color
samples surrounding the component, require samples on opposing sides, and reject the
fit when geometry is underconstrained. Dithered multi-frame combination supplies the
scientific value later.

## 8. Cosmic rays and exposure-specific transients

### 8.1 Prefer multi-frame rejection

Persistent defects repeat at the same sensor coordinate; cosmic rays, satellites, and
aircraft are exposure-specific. With multiple registered, preferably dithered lights,
reject transients at stack time using masks and robust per-output-pixel statistics.
That compares independent observations of the same sky and does not need to decide
whether a sharp single-frame feature is a star.

Calibration masters should remove cosmic rays by rejected-mean combination. Running
single-image L.A.Cosmic on every bias/dark/flat is unnecessary and can alter valid
calibration structure; the
[Astropy cosmic-ray guide](https://www.astropy.org/ccd-reduction-and-photometry-guide/v/2.0.0/notebooks/08-03-Cosmic-ray-removal.html)
recommends proper combination for calibration images.

Use single-frame detection only when too few independent exposures exist, a transient
mask is needed before interpolation, or the science case requires it.

### 8.2 L.A.Cosmic prerequisites

The original [van Dokkum 2001 algorithm](https://arxiv.org/abs/astro-ph/0108003)
assumes a sampled image and an accurate noise model. Before detection:

- subtract bias/dark and apply the flat;
- retain a nonnegative pre-cosmic background estimate for Poisson noise estimation;
- if a sky model is subtracted from the detection image, supply that model separately
  to the noise calculation and restore the intended output background afterward;
- express image and noise in the same units;
- mask known defects, saturation, and invalid pixels;
- do not demosaic, resample, sharpen, or permanently sky-subtract;
- validate parameters and sensor sampling with injected-source tests.

Astro-SCRAPPY, the current open implementation wrapped by ccdproc, documents the same
requirements and defaults; see pinned
[astroscrappy.pyx](https://github.com/astropy/astroscrappy/blob/023554aa49d17ecf8c32aff4ae7a77396ec462a6/astroscrappy/astroscrappy.pyx).

### 8.3 Dense mono L.A.Cosmic

Let I be a calibrated dense monochrome plane of width W and height H.

#### Step L1 — subsampled positive Laplacian

Replicate every source pixel to a 2 × 2 block, producing I². Convolve with:

~~~text
K = [ 0 -1  0
     -1  4 -1
      0 -1  0 ]
~~~

Clip negative convolution values to zero, then average each 2 × 2 block back to native
resolution:

~~~text
L⁺ = block_mean_2x2(max(0, K * I²))
~~~

Use one explicitly tested border rule. Edge replication is acceptable. The subsampling
prevents negative Laplacian crosses from adjacent cosmic pixels suppressing one another.

#### Step L2 — noise map

Obtain a cosmic-free signal estimate M by a 5 × 5 median of I. In normalized Lumos units,
with gain g electrons/ADU, full_scale ADU per normalized unit, and read noise R_e:

~~~text
E_unit = g · full_scale
signal_e = max(M, 0) · E_unit
N = sqrt(signal_e + R_e²) / E_unit
~~~

If a propagated variance plane V exists, use N = sqrt(median_filter(V)) after adding
any separately supplied background variance. Floor N at a positive numeric minimum.
The variance must describe what the pixel noise would have been without a cosmic ray.

Lumos also offers an empirical model. With robust background b and σ_b:

~~~text
slope = σ_b² / max(b, σ_b)
N(M) = sqrt(σ_b² + max(M - b, 0) · slope)
~~~

This is a pragmatic sky-anchored approximation, not the canonical Poisson/read model.
It can overestimate bright-region noise when the background is read-noise dominated.

#### Step L3 — significance

~~~text
S = L⁺ / (2N)
S' = S - median_5x5(S)
~~~

The factor two is the subsampling factor. Subtracting the local median removes smooth
sampling structure.

#### Step L4 — fine structure

~~~text
M3 = median_3x3(I)
F = max(M3 - median_7x7(M3), ε_F)
Fσ = max(F / N, 0.01)
~~~

F remains large in sampled stellar cores but approaches zero for a sharp cosmic spike
erased by M3.

#### Step L5 — primary and neighbor masks

With defaults sigclip = 4.5, objlim = 5.0, sigfrac = 0.3:

~~~text
primary = good
          and S' > sigclip
          and S' / Fσ > objlim

near1 = dilate_8(primary) and good and S' > sigclip
near2 = dilate_8(near1) and good and S' > sigfrac · sigclip
new_cosmic = near2
~~~

Astro-SCRAPPY deliberately relaxes the fine-structure condition for neighbors after a
primary detection. A different growth rule is a different algorithm and needs its own
tests.

#### Step L6 — clean and iterate

OR new_cosmic into the cumulative mask. For detection iterations, replace masked
samples in a working copy with a statistic of unmasked 5 × 5 neighbors. A masked
median is robust; Astro-SCRAPPY defaults to a masked mean and also offers inverse-distance
weighting. Keep the original data and output mask separately when possible.

Repeat detection and working-copy cleaning up to niter = 4, stopping when no new pixel
is found. The cumulative mask, not the cleaned pixels, is the scientific result.

### 8.4 Bayer mosaics

L.A.Cosmic was defined for a dense sampled intensity image, not a color mosaic. Lumos
deinterleaves the four (x mod 2, y mod 2) Bayer phases, runs the dense mono algorithm on
each half-resolution phase plane, and writes the cleaned planes back.

This preserves same-color comparisons but halves spatial sampling in both axes:

~~~text
FWHM_phase ≈ FWHM_mosaic / 2
~~~

A perfectly real 2.4-pixel-FWHM mosaic star becomes roughly 1.2 pixels wide in a phase
plane and can resemble a cosmic ray. Therefore Bayer single-frame rejection MUST be
off by default unless camera/optics-specific injection tests establish acceptable
completeness and false-positive rates across:

- stellar FWHM and subpixel phase;
- brightness through saturation;
- red, green, and blue source colors;
- crowded fields and emission knots;
- trails and multi-pixel cosmic events.

For ordinary dithered OSC sets, prefer stack-time rejection.

### 8.5 X-Trans Lumos extension

X-Trans has no dense rectangular same-color sublattice, so current Lumos uses a
median-stencil extension rather than the canonical Laplacian:

1. Within Chebyshev radius 6, gather same-color, unmasked neighbors.
2. Sort by Manhattan distance and keep at most 24.
3. Let M_small be the median of the nearest up to 8 and M_large the median of all
   gathered values.
4. Compute:

   ~~~text
   L⁺ = max(I_p - M_small, 0)
   F = max(M_small - M_large, 10⁻⁶)
   signal = M_small
   S = L⁺ / N
   ~~~

5. Estimate empirical background/MAD noise separately for R, G, and B, or use the
   parametric noise model.
6. Apply the section 8.3 contrast/growth test to S and F, without S' because L⁺ is
   already a local high-pass.
7. Replace flagged working samples with the median of the nearest 12 unmasked same-color
   neighbors, and iterate.

This is a Lumos-specific heuristic, not an implementation of van Dokkum's Laplacian
derivation. It requires independent validation and must be named accordingly in
provenance.

### 8.6 Current Lumos differences

Current defaults match Astro-SCRAPPY's 4.5/5.0/0.3/four-iteration parameters and place
the operation after calibration and before demosaic. Current dense Mono fine structure
uses full 3 × 3 and nested 7 × 7 medians, replacement uses an unmasked-neighbor 5 × 5
median, and masks accumulate across iterations.

The current growth step performs one 8-neighbor pass and requires both the lowered
significance and the same object-contrast test for grown pixels. That is more
conservative than Astro-SCRAPPY's two-stage growth and is not identical to the
algorithm specified in section 8.3. Current code also lacks an input bad/saturation
mask and uses destructive in-painting because the pipeline has no transient mask plane.

## 9. Uncertainty propagation

### 9.1 General ratio

Let:

~~~text
X = additive-corrected light
G = unnormalized calibrated flat response
m = flat normalization
C = mX/G
~~~

The first derivatives are:

~~~text
∂C/∂X = m/G
∂C/∂G = -mX/G² = -C/G
∂C/∂m = X/G = C/m
~~~

If X, G, and m are independent:

~~~text
Var(C) =
    (m/G)² Var(X)
  + (C/G)² Var(G)
  + (C/m)² Var(m)
~~~

They are not always independent. The normalization m is estimated from G and creates
Cov(G, m); a shared bias can occur in X and G. The general implementation uses the
Jacobian J over primitive independent inputs and computes:

~~~text
Var(C) = J Σ Jᵀ
~~~

Do not add the same master's variance twice as though two uses were independent.

### 9.2 Additive numerator paths

For independent light and matched raw dark:

~~~text
X = L - D_L
Var(X) = Var(L) + Var(D_L)
~~~

For independent L, D_raw, B and fixed k:

~~~text
X = L - kD_raw + (k - 1)B
Var(X) = Var(L) + k²Var(D_raw) + (k - 1)²Var(B)
~~~

If k has uncertainty and is independent of the pixels, add:

~~~text
(D_raw - B)² Var(k)
~~~

plus covariance terms if k was fitted from the same light pixels. A fit-derived k is
usually correlated with L, so either carry that covariance or conservatively validate
with k ± σ_k.

For bias only:

~~~text
X = L - B
Var(X) = Var(L) + Var(B)
~~~

Subtracting the mean dark signal does not remove the light exposure's own dark-current
shot noise. That stochastic term remains in Var(L), just as photon and read noise do.

### 9.3 Shared bias in numerator and flat

If the same B is used in C = m(L - B)/(F - B), treat B as one primitive variable:

~~~text
∂C/∂L = m/G
∂C/∂F = -mX/G²
∂C/∂B = m(X - G)/G²
~~~

where X = L - B and G = F - B.

For the scalable-dark numerator with the same bias-subtracted flat:

~~~text
X = L - kD_raw + (k - 1)B
G = F - B
∂C/∂B = m((k - 1)G + X)/G²
~~~

These derivatives automatically account for the shared master. Expanding Var(X) and
Var(G) independently and then adding them would overcount or mis-sign the bias
contribution. They are conditional on a fixed normalization m. A full analytic
derivative also includes (X/G)(∂m/∂B); equivalently, carry the covariance of m with G
and B. The hierarchical whole-pipeline bootstrap below captures both dependencies and
is safer for a robust normalization estimator.

### 9.4 Prepared divisor uncertainty

For Q = G/m:

~~~text
Var(Q) =
    Var(G)/m²
  + G² Var(m)/m⁴
  - 2G Cov(G,m)/m³
~~~

With C = X/Q and independent X,Q:

~~~text
Var(C) = Var(X)/Q² + X² Var(Q)/Q⁴
~~~

Estimate Var(m) and Cov(G,m) by an analytic robust-estimator approximation or bootstrap
whole flat frames, not individual pixels: resample input flat exposures, rebuild the
complete normalized master, and measure the distribution of Q. Whole-frame bootstrap
preserves the correlation introduced by shared illumination normalization and additive
masters. To include master-bias or flat-dark uncertainty, use a hierarchical rebuild:
independently resample the source additive-calibration exposures, rebuild that master,
then calibrate the resampled flat exposures with it on every bootstrap replicate.

### 9.5 Initial light variance

When physical parameters are known, in normalized units:

~~~text
E_unit = electrons per normalized unit
signal_e = max(expected pre-read signal, 0) · E_unit
Var(L) = (signal_e + read_noise_e²) / E_unit² + Var(extra terms)
~~~

Use an expected signal estimate that is not itself inflated by a cosmic ray. Add
quantization variance from Stage 1 and any validated readout/model terms. A negative
sample does not imply negative Poisson variance; use the pre-subtraction expected
electron components or a nonnegative robust signal estimate.

The primary reference for the fact that calibration-frame noise propagates into the
result is Newberry, 1991,
[Signal-to-noise considerations for sky-subtracted CCD data](https://adsabs.harvard.edu/pdf/1991PASP..103..122N).
ccdproc provides a practical open-source comparison: CCDData arithmetic propagates
uncertainties through bias, dark, and flat operations.

### 9.6 Weights and synthetic pixels

The stack weight for an ordinary calibrated sample is proportional to 1/Var(C), modified
by geometric coverage. A masked or interpolated sample has zero scientific weight.
Do not assign interpolated pixels the variance of their neighbors; that would count
correlated synthetic information as a new measurement.

Store per-pixel survivor count and effective sample size for masters. A single global
quantization σ is useful as a numerical floor but is not a replacement for a master
variance plane.

## 10. Required implementation sequence

### 10.1 Build masters

~~~text
build_calibration_bundle(frame_groups, policy):
    headers = inspect_all_headers(frame_groups)
    keys = derive_typed_compatibility_keys(headers, policy.instrument_profile)
    reject_missing_or_mismatched_keys(keys)

    bias = build_additive_master(groups.bias, normalization=none)
    raw_dark = build_additive_master(groups.dark, normalization=none)
    flat_dark = build_additive_master(groups.flat_dark, normalization=none)

    if policy.scaled_dark:
        require(raw_dark and bias)
        scalable_dark = subtract_with_variance(raw_dark, bias)
    else:
        scalable_dark = none

    prepared_flats = []
    for flat_group matching one light optical key:
        subtractor = matching_flat_dark_or_bias(flat_group)
        calibrated_subs = []
        for flat in flat_group:
            g = subtract_with_variance(flat, subtractor)
            mask_invalid_saturated_and_nonlinear(g)
            levels = robust_level_per_cfa_color(g)
            require_positive_well_sampled(levels)
            calibrated_subs.push(normalize_per_color(g, levels))

        response = rejected_mean_or_small_n_median(calibrated_subs)
        divisor = normalize_master_per_color(response)
        classify_flat_invalid(divisor, policy)
        prepared_flats.push(divisor)

    defects = union(
        detect_hot_and_broad_dark_defects(raw_dark, source_darks),
        detect_cold_response_defects(prepared_flats),
        detect_unstable_pixels(source_darks),
        detect_rows_columns_clusters(raw_dark, prepared_flats)
    )

    validate_split_half_and_held_out_products(...)
    publish_atomically(data, masks, variance, compatibility, provenance)
~~~

The current Lumos API stacks all four roles independently and only later subtracts the
flat's master subtractor. Meeting this sequence requires restructuring that boundary so
flat subs are calibrated before multiplicative normalization.

### 10.2 Calibrate one light

~~~text
calibrate(light, bundle, policy):
    validate_complete_contract_without_mutation(light, bundle, policy)
    out = allocate_or_clone_transactional_output(light)

    if matched_raw_dark:
        X = subtract(out, matched_raw_dark)
    else if scalable_dark:
        k = validated_dark_scale(out, scalable_dark, policy)
        X = out - bias - k * scalable_dark
    else if bias:
        X = subtract(out, bias)
    else:
        X = out

    merge_source_and_master_masks()
    propagate_additive_variance()

    if flat:
        divide_only_valid_samples(X, flat.divisor)
        propagate_ratio_variance()

    repair_values_needed_by_demosaic(defect_union, keep_masks=true)

    if single_frame_cosmic_enabled:
        require_validated_sensor_specific_config()
        cosmic_mask = detect_on_working_copy(calibrated_cfa, variance, masks)
        merge_mask(cosmic_mask)
        fill_only_for_demosaic(keep_masks=true)

    verify_all_published_samples_finite()
    mark_calibrated_with_provenance()
    return out
~~~

The operation either returns the complete output or the original remains untouched.

### 10.3 Performance and memory

- Decode calibration roles in bounded parallel batches.
- Keep master roles concurrent only when their combined resident and transient memory
  fits the declared budget; otherwise run roles sequentially with the full budget.
- Combine in row-aligned chunks so resident and mmap tiers are bit-equivalent.
- Use f64 accumulators for normalization, sums, covariance, and variance.
- Precompute CFA phase/color lookup and X-Trans neighbor offsets.
- Poll cancellation between decode, tile/statistic, chunk, and iteration boundaries.
- Never publish a partial cache. Include cache schema, decoder-domain identity, and
  algorithm version in the cache key/header.

## 11. Current Lumos state and gap audit

This section describes the source tree as inspected for this document. It is informative;
sections 1–10 are the target contract.

### 11.1 Implemented well

- CfaImage uses unclipped f32 and preserves negative values during subtraction.
- Calibration runs on CFA data before demosaic and prevents a normal second application.
- A raw dark takes priority over bias, avoiding double subtraction in that path.
- Flat-dark takes priority over bias for flat preparation.
- Prepared flats normalize independently per R/G/B CFA color.
- Role stack presets use no normalization for bias/dark, multiplicative normalization
  for flat, robust rejection, memory tiering, finite-sample checks, and propagated
  quantization floors.
- Hot detection removes a robust per-color tiled broad-dark model and combines MAD,
  upper-bulk, and source-resolution scale estimates.
- Cold detection uses local same-color response ratios before the flat floor.
- Defect repair masks the whole defect set and uses same-color neighbors for Mono,
  Bayer, and X-Trans.
- Optional cosmic rejection is correctly placed after calibration and before demosaic,
  with explicit Mono/Bayer/X-Trans dispatch.
- Calibration bundles are atomically saved through a versioned cache and contain the
  prepared flat and coherent defect map.

Relevant sources:

- [calibration_masters/mod.rs](../calibration_masters/mod.rs)
- [prepared_flat/mod.rs](../calibration_masters/prepared_flat/mod.rs)
- [defect_map/mod.rs](../calibration_masters/defect_map/mod.rs)
- [cosmic_ray.rs](../calibration_masters/cosmic_ray.rs)
- [combine/config.rs](../combine/config.rs)
- [pipeline/streaming.rs](../pipeline/streaming.rs)
- [CfaImage](../../io/astro_image/cfa.rs)

### 11.2 Priority correctness gaps

| Priority | Gap | Consequence |
|---|---|---|
| P0 | Flat subs are globally multiplicatively normalized while still raw; master bias/flat-dark is subtracted only after combination | variable flat illumination is normalized with an additive pedestal in the statistic |
| P0 | Within-role loading validates dimensions and finite samples, but not exposure/gain/offset/filter/temperature/readout/decode compatibility | unrelated frames can be silently combined |
| P0 | Application validates CFA pattern only; dimensions fail later by assertion and other acquisition fields are ignored | a mismatch can panic after the calibrated flag is set or produce plausible corruption |
| P0 | Stage 1 decoder black model/scale provenance is not represented in AstroImageMetadata | masters from incompatible numeric domains cannot be rejected |
| P0 | No validity/saturation/defect/flat-invalid mask plane | invalid and synthesized samples receive ordinary downstream weight |
| P0 | No per-pixel master/divisor variance | flat and shared-bias uncertainty cannot be propagated; stack weights omit calibration noise |
| P0 | Prepared-flat arithmetic mean includes every sample and silently floors response at 0.1 | saturation/defects bias normalization; deep response is changed instead of masked |
| P1 | No overscan correction in the calibration architecture | frame-to-frame bias drift cannot be removed when overscan exists |
| P1 | No dark scaling or dark-rate library | every dark must be matched 1:1 |
| P1 | No RTS/fading stability classification or validity interval | intermittent pixels can evade a master threshold |
| P1 | No explicit bad row/column/cluster detection | coherent defects are treated as isolated pixels |
| P1 | Defect summary adds hot and cold counts rather than reporting the coordinate union | overlap can overstate defective percentage |
| P1 | Single-frame cosmic rejection has no input bad/saturation mask and destructively replaces pixels | false candidates and synthesized values cannot be downweighted later |
| P1 | Bayer phase-plane L.A.Cosmic is exposed without a sampling gate | tight real stars can be removed |
| P1 | Defect/cosmic thresholds and physical noise inputs are not uniformly checked for finite positive values | invalid configuration can silently disable detection or create non-finite arithmetic |
| P2 | Convenience from_files uses non-cancellable role stacks | long master construction cannot be stopped through that API |
| P2 | Cache provenance lacks full source list, compatibility key, quality report, and algorithm schema | a cache can be coherent internally but scientifically stale |

### 11.3 Open-source comparison

- ccdproc checks shape and physical units, supports overscan/trim, exposure dark
  scaling, flat minimum policy, masks, and uncertainty propagation. Its Combiner
  exposes mean/median, scaling, weights, masks, clipping, and master uncertainty.
- Astro-SCRAPPY supplies the maintained L.A.Cosmic reference behavior, including
  saturation/bad masks, Poisson/read noise, two-stage growth, multiple cleaning
  strategies, and optional input variance/background.
- Siril confirms the practical bias → dark → flat order, per-CFA flat equalization,
  master-dark cosmetic correction, exposure or empirical dark optimization, and
  calibration-before-demosaic workflow.
- Rubin's ip_isr demonstrates the larger scientific contract: input validation,
  amplifier overscan, bias/dark/flat, defects, and image+mask+variance products.

Lumos's tiled CFA defect detection is more sensor-layout-aware than Siril's current
global master-dark threshold, but the absence of masks/variance and full compatibility
metadata is the larger architectural limitation.

## 12. Verification specification

All algorithms require analytic/synthetic fixtures and real instrument regression sets.
Tests assert exact masks, values, and uncertainty—not merely that processing completes.

### 12.1 Algebra and domains

1. Matched raw dark removes residual bias+dark exactly and does not subtract bias twice.
2. Bias-only path recovers the known signal.
3. Scaled path verifies
   L - B - k(D-B) = L - kD + (k-1)B exactly.
4. A flat with known response removes vignetting up to the declared normalization.
5. Flat calibration before normalization recovers variable-illumination inputs; the
   intentionally wrong reverse order must fail the expected value.
6. Negative background samples remain negative.
7. Saturated and invalid inputs remain masked and never become valid after subtraction.
8. Applying calibration twice fails before mutation.

### 12.2 Compatibility

Table-drive one mismatch at a time:

- dimensions/crop/orientation/CFA origin;
- Mono/Bayer/X-Trans pattern;
- decoder black model and scale;
- gain/ISO, offset, bit depth, readout mode, binning;
- dark/flat-dark exposure;
- dark temperature policy;
- flat filter, optical key, and linearity/saturation state;
- missing required metadata.

For every failure, assert pixels, masks, variance, and calibrated marker are unchanged.
Test mixed-role source groups before master combination, not only master versus light.

### 12.3 Master estimators

- Hand-compute mean, median, sigma-clip, and Winsorized-estimate survivors for small
  vectors, including tied values, zero MAD, asymmetric outliers, and N = 1/2.
- Assert f64 sums on adversarial f32 magnitudes.
- Verify per-pixel survivor counts and N_eff.
- Hand-compute predicted and empirical master variance, including n = 1 and excess-χ²
  behavior.
- Inject one cosmic ray into one calibration sub and recover the clean expected master.
- Inject a whole bad exposure/light leak and verify frame-level rejection rather than
  millions of independent pixel clips.
- Force RAM and mmap tiers and assert bit-identical master, mask, variance, and metadata.

### 12.4 Flats

- Mono, all four Bayer patterns, and multiple valid X-Trans patterns.
- Strongly unequal R/G/B flat-source spectra: output response shape must be unchanged
  and no color gain introduced.
- Frame-to-frame illumination and color drift with a nonzero bias/flat-dark.
- Vignetting, dust shadows, saturated center, near-zero corners, and dead pixels.
- Exact maximum-gain boundary: just above remains valid; just below is FLAT_INVALID,
  not silently clamped.
- Split-half flat ratio agrees with predicted uncertainty.
- Per-filter and optical-key mismatch fails.

### 12.5 Dark scaling

- Exact linear exposure series recovers the known k.
- A shorter dark scaled upward reports the predicted k² variance penalty.
- Bias left in a scaled dark produces the analytically expected wrong pedestal and is
  rejected by type/metadata.
- Separate dark-current and amp-glow scaling laws cause scalar validation to fail.
- Robust fit ignores masked stars and recovers k with injected outliers.
- A featureless dark template is rejected as unidentifiable.
- Bound-hitting, extrapolated temperature, RTS, and low-significance fits fail.

### 12.6 Defects

- Broad gradients and amp glow yield no false hot map.
- Per-color noise differences do not hide red/blue defects.
- Quantized zero-MAD masters use the resolution floor.
- Sample cap covers sensor extent and every CFA phase.
- Hot clusters remain detectable under the tiled, not local, background.
- Cold detection catches exact sub-0.5 local response and preserves the boundary.
- Smooth vignette/dust does not trigger cold detection.
- RTS time series is unstable despite an ordinary-looking master mean.
- Bad columns with short gaps become one classified segment.
- Repair draws only from original good same-color neighbors; order cannot change output.
- Underconstrained large clusters remain masked rather than receiving a false valid fill.

### 12.7 Cosmic rays

For Mono, Bayer, and X-Trans separately:

- single and multi-pixel cosmic events at center, border, and near masks;
- exact primary, first-growth, and lowered-growth masks on a small hand-computed array;
- parametric noise in electrons versus normalized units;
- supplied variance versus internal noise model;
- saturated stars and known defects excluded;
- well-sampled stars over brightness/subpixel sweeps survive;
- undersampled Bayer stars demonstrate the safety gate;
- emission knots, diffraction spikes, trails, and crowded fields;
- iteration stops and cumulative masks are deterministic;
- NaN, infinite, zero, and negative thresholds/noise parameters fail before mutation;
- cleaned fill stays masked and has zero stack weight;
- multi-frame rejection recovers the true sky without single-frame cleaning.

### 12.8 Uncertainty

- Compare analytic Jacobians with finite differences.
- Verify matched-dark, bias-only, and expanded scalable-dark variance by Monte Carlo.
- Reusing one bias in numerator/denominator matches the shared-variable derivative and
  differs from the intentionally wrong independent treatment.
- Whole-flat bootstrap matches the prepared-divisor variance within statistical error.
- Calibrated residuals normalized by predicted σ have unit robust scale on synthetic
  noise.
- Interpolated/masked values never contribute effective sample size.

### 12.9 Real-data acceptance

Maintain versioned data sets for at least one Mono, Bayer, and X-Trans camera. For every
set report:

- master split-half residuals;
- calibrated background row/column/radial profiles;
- defect counts by class and stability;
- flat over/under-correction versus field position;
- noise before/after with predicted calibration contribution;
- cosmic completeness/false-positive rate from injected events;
- reproducibility across memory tiers and thread counts.

Do not approve an algorithm from a visually stretched image alone.

## 13. Primary and implementation references

### Scientific and observatory references

- Michael V. Newberry, 1991,
  [Signal-to-noise considerations for sky-subtracted CCD data](https://adsabs.harvard.edu/pdf/1991PASP..103..122N).
- Pieter G. van Dokkum, 2001,
  [Cosmic-Ray Rejection by Laplacian Edge Detection](https://arxiv.org/abs/astro-ph/0108003).
- Plazas Malagón et al., 2024,
  [Instrument Signature Removal and Calibration Products for the Rubin LSST](https://arxiv.org/abs/2404.14516).
- [Astropy CCD Data Reduction and Photometry Guide](https://www.astropy.org/ccd-reduction-and-photometry-guide/):
  calibration overview, image combination, dark/bias choices, flat construction, and
  cosmic-ray removal.
- STScI,
  [ACS dark current, hot pixels, RTS stability, sink pixels, and cosmic rays](https://hst-docs.stsci.edu/acsdhb/chapter-4-acs-data-processing-considerations/4-3-dark-current-hot-pixels-and-cosmic-rays).

### Open-source implementations inspected

The source links are commit-pinned so future upstream changes do not silently change
the comparison:

- ccdproc commit c80e1f00:
  [core calibration operations](https://github.com/astropy/ccdproc/blob/c80e1f00b9326882c6b67011ea53b5bfc3be5d4f/ccdproc/core.py) and
  [Combiner](https://github.com/astropy/ccdproc/blob/c80e1f00b9326882c6b67011ea53b5bfc3be5d4f/ccdproc/combiner.py).
- Astro-SCRAPPY commit 023554aa:
  [L.A.Cosmic implementation](https://github.com/astropy/astroscrappy/blob/023554aa49d17ecf8c32aff4ae7a77396ec462a6/astroscrappy/astroscrappy.pyx).
- Siril commit 8ce9baa3:
  [preprocessing/dark optimization](https://gitlab.com/free-astro/siril/-/blob/8ce9baa37215ae9783de16fa9e0d7a610303588d/src/core/preprocess.c),
  [cosmetic correction](https://gitlab.com/free-astro/siril/-/blob/8ce9baa37215ae9783de16fa9e0d7a610303588d/src/filters/cosmetic_correction.c), and
  [CFA flat equalization](https://gitlab.com/free-astro/siril/-/blob/8ce9baa37215ae9783de16fa9e0d7a610303588d/src/core/siril.c).
- [Rubin ip_isr](https://github.com/lsst/ip_isr), open image+mask+variance instrument
  signature removal.

### Interpretation notes

- ccdproc's dark scale multiplies the supplied master; the caller is responsible for
  ensuring it is bias-free. This document makes that state a distinct type/product.
- Siril's empirical optimizer and Lumos's X-Trans cosmic detector are valuable
  implementation evidence but are heuristics beyond the cited physical derivations.
- Instrument-specific HST/Rubin thresholds are evidence for carrying stability,
  amplifier, mask, and variance information—not universal constants for consumer
  cameras.
- Acquisition folklore and proprietary-tool defaults were deliberately excluded where
  a primary paper, observatory guide, or inspectable open-source implementation supplied
  the claim.
