# Stage 5 — Stacking & Drizzle: Best Practices & Algorithms

## Scope & Goal

Stage 5 combines a set of registered (warped) frames into a single deep image. There
are two fundamentally different reconstruction paradigms, and they are *not*
interchangeable:

1. **Statistical stacking** — every output pixel is an independent estimator built
   from the co-registered input samples at that location: a (possibly rejected,
   normalized, weighted) **mean or median**. The output grid is the same size as the
   inputs. This is what almost every deep-sky stack is, and it is the right default.
   It maximizes signal-to-noise ratio (SNR) and removes transient outliers (cosmic
   rays, satellites, planes, hot pixels) but cannot recover spatial resolution lost
   to undersampling.

2. **Drizzle** (Variable-Pixel Linear Reconstruction; Fruchter & Hook 2002) — a
   *resampling* method that maps each shrunken input pixel ("drop") onto a finer
   output grid, distributing flux by geometric overlap area. It can recover
   resolution from **dithered, undersampled** data and removes geometric-distortion
   photometric error, at the cost of introducing **correlated noise**. Drizzle is a
   linear coadd, not a robust estimator — it does *not* by itself reject outliers
   (rejection is done separately, e.g. the DrizzlePac median+blot+derivative CR
   scheme, before drizzling).

The goal of this document is to state, with the math and with citations into the
cloned reference implementations, *what each method assumes, when it helps, when it
hurts*, and to map that onto lumos's current implementation (`src/stacking/`,
`src/drizzle/`).

The governing principle: **maximize SNR while suppressing outliers and not corrupting
photometry.** Mean stacking is the maximum-likelihood estimator for Gaussian noise and
has the best SNR; everything else (median, rejection, robust estimators) trades a
little of that efficiency for robustness against the non-Gaussian tail.

---

## 1. Pre-combine conditioning

Before any pixels are combined, three things must be made consistent across frames:
the **photometric scale/offset** (normalization), the **per-frame weight**, and the
choice of a **reference frame**. Skipping these is the most common cause of bad
stacks (see §7).

### 1.1 Reference frame selection by noise

The normalization and weighting are defined *relative to a reference frame*. The
standard choice is the **lowest-noise frame** — it has the most stable background and
makes the most reliable target for matching everyone else's statistics.

- **lumos**: `select_reference_frame` (`src/stacking/stack.rs:165`) picks the frame
  with the lowest mean MAD (median absolute deviation) across channels. MAD is used
  rather than standard deviation because it is robust to the stars/objects in the
  frame — it measures background noise, not signal.
- **siril**: the reference image is user-selectable but defaults to the best-quality
  frame; normalization coefficients are computed relative to it
  (`src/stacking/normalization.c:142`, `compute normalization factors based on the
  reference image`). siril warns and aborts if the reference image is not in the
  selected set (`normalization.c:240`).

Why lowest noise and not highest signal? The reference defines the *target
background statistics*; you want that target to be as clean as possible so the
additive/multiplicative transform you derive for every other frame is well
determined.

### 1.2 Normalization: additive, multiplicative, and scaling

Frames taken across a night differ in sky background (light pollution gradient,
moon, airmass) and transparency (clouds, dew). Two corrections:

- **Additive (offset) normalization** removes a background pedestal difference. Used
  for **light frames** where the sky brightness changed but the signal scale did not:
  `out = in + (ref_location − frame_location)`. This is what you want when the only
  thing that changed is the sky pedestal.
- **Multiplicative (scaling) normalization** corrects a gain/transparency difference:
  `out = in × (ref_scale / frame_scale)`. Used for **flat fields** (and sometimes
  lights through thin cloud), because a flat's response is multiplicative.

siril separates *location* (additive) from *scale* (multiplicative) and offers four
modes — `ADDITIVE`, `ADDITIVE_SCALING`, `MULTIPLICATIVE`, `MULTIPLICATIVE_SCALING`
(`src/stacking/normalization.c:124-134`). In the robust "lite" path location is the
median and scale is `1.5·MAD` (an approximation of `sqrt(bwmv)`, the biweight
midvariance, itself an approximation of the IKSS scale estimator;
`normalization.c:117-122`). The full path uses the IKSS location/scale estimators.
The crucial subtlety in siril's additive transform: `poffset = pscale·offset −
offset0` (`normalization.c:167`) — the offset is applied *after* the scale, so the two
compose correctly.

- **lumos**: `compute_frame_norms` (`src/stacking/stack.rs:189`) implements two of
  these. `Normalization::Global` is full additive+multiplicative
  (`gain = ref_mad/frame_mad`, `offset = ref_median − frame_median·gain`), and
  `Normalization::Multiplicative` is scale-only (`gain = ref_median/frame_median`).
  Normalization is applied per channel.

**Background/sky matching** for mosaics and wide fields goes a step beyond a single
scalar offset: a low-order *surface* (plane or polynomial) is fit and subtracted so
that the seams between tiles match. SWarp does this with its `BACK_TYPE`/`BACK_SIZE`
mesh background subtraction and `reproject`'s coadd has an explicit background-matching
step (`reproject/mosaicking/background.py`) that solves for per-image additive levels
minimizing overlap differences. lumos does **not** do surface background matching —
its normalization is a single scalar offset per channel per frame, which is correct
for a uniform sky-pedestal shift but not for gradient mismatch between mosaic tiles.

### 1.3 Per-frame weighting

If frames have unequal SNR (varying transparency, seeing, exposure), an equal-weight
mean is suboptimal. The **statistically optimal linear weight is inverse-variance**:
for independent Gaussian samples `x_i` with variance `σ_i²`, the minimum-variance
unbiased estimator is

```
        Σ (x_i / σ_i²)
x̂  =  ────────────────       with   Var(x̂) = 1 / Σ(1/σ_i²)
          Σ (1 / σ_i²)
```

so `w_i = 1/σ_i²`. This is the *only* weighting that minimizes the variance of the
combined pixel; equal weighting is optimal only when all `σ_i` are equal.

Reference implementations expose several practical proxies for `1/σ_i²`:

- **Noise weighting** (`w_i ∝ 1/σ_bg²`). siril's `compute_noise_weights`
  (`src/stacking/median_and_mean.c:1110`):
  `w = 1 / (pscale² · bgnoise²)` — inverse of the *scaled* background noise variance.
  Note the `pscale²` term: weights must be computed in the *normalized* frame, so the
  scaling coefficient enters squared. lumos's `Weighting::Noise`
  (`src/stacking/stack.rs:271`) computes `w = 1/σ²` with `σ = mad_to_sigma(MAD)`
  averaged across channels — the same idea, but it does **not** fold in the
  normalization scale factor (a minor gap; see §8).
- **FWHM / weighted-FWHM weighting** — better seeing → higher weight. siril's
  `compute_wfwhm_weights` (`median_and_mean.c:1136`) uses
  `w_i = (1/fwhm_i² − 1/fwhm_max²) / (1/fwhm_min² − 1/fwhm_max²)`, a normalized
  inverse-square-FWHM so the worst frame gets weight 0 and the best gets 1. This is a
  *quality* weight, not a noise weight, and is a heuristic.
- **Star-count weighting** — more detected stars → clearer frame. siril
  `compute_nbstars_weights` (`median_and_mean.c:1183`) uses a normalized squared
  star-count excess.
- **Exposure/sub-count weighting** (`NBSTACK_WEIGHT`) for already-partially-stacked
  inputs.

PixInsight's "Weighted BatchPreprocessing" derives a per-sub weight from an SNR
estimate (noise + star quality), which is the same inverse-variance idea wrapped in a
quality metric.

**Best practice:** for light frames of varying quality, inverse-variance (noise)
weighting is theoretically correct and should be the default; FWHM/star-count are
useful *additional* quality gates that down-weight bloated frames the noise estimate
alone wouldn't catch. For calibration frames (darks/bias/flats taken back-to-back
under identical conditions), equal weighting is correct — they have equal variance by
construction.

---

## 2. Combination operators

### 2.1 Mean vs median, and the √N law

For `N` independent frames each with per-pixel noise `σ`, the **mean** of the stack
has noise `σ/√N`, so

```
SNR_stack = √N · SNR_single
```

This is the central result of stacking: SNR grows as the square root of the number of
frames (or, since shot noise scales with √(signal) and signal scales with total
integration time `t`, SNR ∝ √t). Doubling SNR requires 4× the frames; 10× SNR
requires 100×.

The **median** is robust (a single huge outlier cannot move it) but is a *less
efficient* estimator. For Gaussian data, the standard error of the median is larger
than that of the mean by a factor of `√(π/2) ≈ 1.2533`. Equivalently the median's
statistical efficiency is `2/π ≈ 0.637` in *variance*, i.e. `√(2/π) ≈ 0.80` in
*standard deviation*. So:

```
SNR_median ≈ 0.80 · √N · SNR_single
```

A median stack throws away ~20% of the SNR you could have had — roughly equivalent to
discarding ~36% of your frames (`0.80² ≈ 0.64`). This is verified across multiple
astrophotography references (Siril docs; jonrista; medium/Sreeraman).

**When is median worth it?** Median is the right choice only when (a) you have few
frames and an unknown/heavy outlier population that rejection can't reliably model, or
(b) you specifically want a maximally robust reference (e.g. the DrizzlePac CR
median, §5.8, where the median image is a *model* not a science product). For a
science stack, **a sigma-clipped mean almost always beats a plain median** — it keeps
~99% of the mean's efficiency while removing outliers. siril, DSS, and PixInsight all
default to clipped-mean for the final light stack and reserve median for
robustness-critical intermediate steps.

### 2.2 Reference implementations

- **lumos**: `CombineMethod::Mean(Rejection)` and `CombineMethod::Median`
  (`src/stacking/config.rs:13`). Mean carries a `Rejection` policy; median is plain.
- **siril**: `median_and_mean.c` implements both, with the mean path running the full
  rejection family (`apply_rejection_float`) before averaging.
- **DSS**: distinguishes Average, Median, Kappa-Sigma (`nClip`/`KappaSigmaClip`),
  Median-Kappa-Sigma (`MediannClip`/`MedianKappaSigmaClip`), Auto-Adaptive Weighted
  Average, and Entropy-Weighted Average (`DeepSkyStackerKernel/DSSTools.h`,
  `MultiBitmapProcess.cpp`).

---

## 3. Pixel rejection algorithms

Rejection removes per-pixel outliers (cosmic rays, satellite/plane trails, hot/cold
pixels not caught by calibration, aircraft) from the sample set *before* the mean is
taken. Every method answers the same question — "is sample `x_i` too far from the
estimated center given the estimated spread?" — but differs in (a) the *center*
(mean / median / fitted line), (b) the *spread* (σ / MAD / Winsorized σ), (c)
*iteration*, and (d) *failure modes with small N*.

All clip methods share a hard floor: with very few samples you cannot tell signal
from outlier. siril enforces `N − r > 4` to keep at least 4 samples
(`rejection_float.c:188`); when fewer than ~3–4 frames survive it simply stops
rejecting. **The whole family is statistically meaningless below ~6–8 frames** because
the σ estimate is itself dominated by the outliers (see §4.4 and §7).

The lumos rejection family lives in `src/stacking/rejection.rs`
(`enum Rejection { None | SigmaClip | Winsorized | LinearFit | Percentile | Gesd }`,
`config.rs:831`).

### 3.1 Sigma clipping / kappa-sigma

The canonical method. Iterate:

1. Compute center `c` (median, robust) and spread `s` (σ or MAD) of the surviving
   samples.
2. Reject any `x_i` with `x_i − c > κ_high·s` (high) or `c − x_i > κ_low·s` (low).
3. Recompute `c`, `s`; repeat until no samples are rejected (or N too small).

"**Kappa-sigma**" is the same algorithm; "kappa" (κ) is just the symbol some packages
(DSS, PixInsight) use for the threshold instead of "sigma multiplier". Typical
`κ ≈ 3` high, often looser low (cosmic rays and trails are positive outliers; there
are few negative ones, so `κ_low` matters less — confirmed in the PixInsight
guidance: "the low parameter is not that important").

```
reject x_i  ⇔  (c − x_i > κ_low·s)  ∨  (x_i − c > κ_high·s)
```

- siril `sigma_clipping_float` (`rejection_float.c:49`) with the `SIGMA` case
  iterating `do … while (changed && N > 3)` (`:174-208`); uses median as center and
  `siril_stats_float_sd` as spread. The **MAD** variant (`case MAD`) is identical but
  substitutes `siril_stats_float_mad` for the spread (`:181`) — more robust because
  MAD is not inflated by the very outliers you're trying to reject.
- DSS `KappaSigmaClip` (`DSSTools.h:606`) iterates a *fixed* number of times
  (`lNrIterations`) using **mean ± κσ** (not median-centered), recomputing dynamic
  stats each pass. `MedianKappaSigmaClip` (`:677`) is the "median-kappa-sigma" variant
  that *replaces* rejected pixels with the median rather than dropping them — this
  keeps `N` constant so the final average is over a fixed count (DSS's
  `SIGMEDIAN`-like behavior; siril has the same idea in its `SIGMEDIAN` case,
  `rejection_float.c:210`).
- lumos `SigmaClip` (`rejection.rs`, `SigmaClipConfig` at `config.rs:20`) — separate
  low/high sigma, iterative.

**Assumptions:** Gaussian core, outliers in the tails, enough samples that the
center/spread are well estimated. **Failure mode:** with few frames, a single bright
cosmic ray inflates σ so much that *nothing* is rejected (masking); using a
median+MAD center/spread mitigates this but doesn't cure the small-N problem.

### 3.2 Winsorized sigma clipping

Winsorization computes the spread *robustly* by first **clipping the distribution's
tails to the ±1.5σ values** (rather than removing them), iterating to a stable σ, and
only *then* applying the σ-clip rejection on the original data. This gives a σ
estimate that is not blown up by the outliers, so clipping converges correctly even
with a fat tail.

siril `WINSORIZED` (`rejection_float.c:223`):

```
repeat:
    σ ← sd(stack);  m ← median(stack)
    w ← copy(stack)
    repeat:                                    # winsorize to ±1.5σ
        w[j] ← clamp(w[j], m − 1.5σ, m + 1.5σ)
        σ₀ ← σ;  σ ← 1.134 · sd(w)             # 1.134 corrects Winsorized-σ bias
    until |σ − σ₀| ≤ 0.0005·σ₀
    reject stack[i] with sigma_clipping(σ, m)  # clip ORIGINAL data with robust σ
until no change
```

The magic constant **1.134** is the bias-correction factor that rescales the
Winsorized standard deviation back to an unbiased estimate of the true Gaussian σ
(because clamping the tails shrinks the variance). lumos hard-codes the same idea:
`WinsorizedClipConfig` (`rejection.rs:193`) plus an explicit "Bias correction factor
for Winsorized standard deviation" (`rejection.rs:202`) and a
`std-from-mean-not-MAD` helper (`rejection.rs:339`).

**Best practice:** Winsorized sigma clipping is the **recommended default for small-to-
moderate frame counts** (PixInsight community: "Winsorized when fewer than ~20 subs").
It is more robust than plain sigma clipping at the same threshold and rarely worse.

### 3.3 Linear-fit clipping

Sort the samples, fit a **straight line** `y = a·i + b` through the sorted values vs.
index, and reject points whose vertical distance to the line exceeds `κ·σ` where σ is
the mean absolute residual. The rationale: across a long sub-stack, the background of
a given pixel may *trend* (changing sky, gradient), so the "center" is not a constant
but a line; fitting the trend removes spurious rejections and catches true outliers
better with many frames.

siril `LINEARFIT` (`rejection_float.c:260`):

```
quicksort(stack)
fit line (a,b) over (index, value)            # siril_fit_linear
σ ← (1/N) Σ |stack[i] − (a·i + b)|            # mean absolute residual
reject stack[i] with line_clipping(a·i+b, σ)
iterate until no change
```

lumos `LinearFit` (`LinearFitClipConfig`, `rejection.rs:362`) implements the same
sorted-index least-squares + residual clip.

**Best practice:** linear-fit clipping is the **recommended method for large frame
counts** (PixInsight community: "25+ subs use linear fit"). With many frames it is the
most discriminating; with few frames the line is poorly constrained and it
under-performs Winsorized clipping.

### 3.4 Percentile clipping

A *non-iterative* method for **very small N** (3–5 frames) where σ cannot be
estimated. Reject samples whose fractional deviation from the median exceeds a
percentage:

```
reject low   ⇔  median − x_i > median · p_low
reject high  ⇔  x_i − median > median · p_high
```

(siril `percentile_clipping`, `rejection_float.c:31` — note the threshold is a
*fraction of the median value*, not a count-based percentile.) lumos
`PercentileClipConfig` (`rejection.rs:529`) takes `low`/`high` as percentages
(0–50). Because it does not need a σ estimate or iteration, it is the safe choice when
you have too few frames for sigma clipping. **Failure mode:** thresholds are absolute
fractions of the median, so it is sensitive to background level and does not adapt to
the actual noise.

### 3.5 Generalized ESD (GESD / Grubbs)

The most statistically principled method, designed to detect **up to `r` outliers**
with a controlled false-positive rate `α`. It repeatedly applies the Grubbs test:

```
Grubbs statistic at each step:   G = max_i |x_i − x̄| / s        (x̄ = mean, s = sd)
remove the most extreme x_i, recompute, repeat up to r times → G_1, G_2, …, G_r
```

Each `G_k` is compared against a critical value derived from the **Student-t inverse
CDF**:

```
              (n−1) · t                                  α
λ_k  =  ─────────────────────────     ,   t = t_inv(1 − ───── , n−2)
        √(n · (n − 2 + t²))                              2n
```

(where `n` is the count at step `k`.) The number of outliers is the *largest* `k`
for which `G_k > λ_k`. Because the comparison is done backward (test for the most
outliers that still pass), GESD is robust to **masking** (one outlier hiding another)
in a way that single-pass sigma clipping is not.

- siril `GESDT` (`rejection_float.c:301`) with `grubbs_stat` (`:82`) and the critical
  values precomputed via the exact t-inverse CDF
  `gsl_cdf_tdist_Pinv(1 − sig/(2·size), size−2)` (`median_and_mean.c:1481`) — this is
  *exactly* the λ formula above. The max number of outliers is
  `floor(N · sig[0])` (`:1480`), i.e. a fraction of the stack.
- lumos `Gesd` (`GesdConfig`, `rejection.rs:616`; `reject` at `:716`) implements the
  same backward-scan Grubbs procedure (`floats_b` holds the `G_k`, the backward scan
  at `:736-749` finds the largest passing `k`). **Important difference:** lumos
  approximates the t-distribution critical value with an *inverse-normal*
  (Abramowitz-Stegun) approximation (`inverse_normal_approx`, `rejection.rs:759`)
  rather than the exact Student-t inverse. For small `N` (where `t` has heavy tails vs
  the normal) this **under-estimates λ and will over-reject** — a real correctness gap
  (see §8). `max_outliers` defaults to `N/4` (`rejection.rs:42`).

**Assumptions:** approximately Gaussian core, outlier fraction below `r/N`. GESD needs
`n ≥ ~3` to even compute (siril guards `nb_frames < 3` at `median_and_mean.c:1281`)
and only makes statistical sense with `N ≳ 10–15`.

### 3.6 MAD-based clipping

Not a separate algorithm so much as a *robust spread choice* within sigma clipping:
use **MAD** (`MAD = median(|x_i − median(x)|)`) scaled to a σ-equivalent via
`σ ≈ 1.4826·MAD` instead of the sample standard deviation. Because the median and MAD
are not inflated by outliers, MAD-based clipping converges correctly even with a heavy
tail and is preferable to mean+sd clipping. siril exposes it as the `MAD` rejection
case (`rejection_float.c:175-181`), identical to sigma clipping but with
`siril_stats_float_mad` as the spread. lumos uses MAD throughout its frame statistics
(`FrameStats`, `cache.rs:38`) and `mad_to_sigma` (`MAD_TO_SIGMA = 1.4826022`) in
`math/statistics`.

### 3.7 Min-frame requirements summary

| Method | Min sensible N | Best regime | Spread used |
|--------|----------------|-------------|-------------|
| Percentile | 3 | 3–5 frames | none (fraction of median) |
| Winsorized σ-clip | ~6 | 6–20 frames | Winsorized σ (×1.134) |
| Sigma / kappa-σ | ~8 | 8–20 frames | sd or MAD |
| Linear-fit | ~15 | 20+ frames | mean abs residual |
| GESD | ~10–15 | 15+ frames, controlled FPR | sd, t-critical |

---

## 4. Statistical correctness

### 4.1 Variance propagation

For a weighted mean `x̂ = Σw_i x_i / Σw_i`, the output variance is

```
Var(x̂) = Σ(w_i² σ_i²) / (Σw_i)²
```

which for the optimal `w_i = 1/σ_i²` collapses to `Var(x̂) = 1/Σ(1/σ_i²)`. After
rejecting `k` of `N` samples the effective `N` drops to `N−k`, so the output noise
grows by `√(N/(N−k))` — heavy rejection costs SNR. This is why over-aggressive
clipping (too small κ) is harmful: each rejected good pixel raises the noise.

The STScI drizzle core propagates variance explicitly: `update_data_var`
(`cdrizzlebox.c:91`) co-adds variance arrays using **squared weights**
(`v = (var·vc² + dow²·d2)/vc_plus_dow²`, `:135`) — the correct propagation for a
weighted average, since `Var(Σw x) = Σw² Var(x)`. lumos's stacker does **not**
currently produce an output variance/uncertainty map; its drizzle produces a coverage
map but not a variance map (see §8).

### 4.2 Weight maps and coverage maps

Both paradigms benefit from carrying a per-pixel weight:

- In **stacking**, a per-pixel weight (e.g. inverse-variance, or a quality mask
  flagging bad columns/edges) lets you down-weight rather than hard-reject, and the
  output weight map records how many good samples each pixel got.
- In **drizzle**, the **coverage/weight map is mandatory** — it records `W_xy = Σ
  a_xy·w_i·s²` (the accumulated overlap-area × input-weight) and is what the flux is
  normalized by. Edge pixels and chip gaps get low coverage; `min_coverage` masks
  them. lumos returns this as `DrizzleResult.coverage` normalized to [0,1]
  (`drizzle/mod.rs:680`); STScI returns `output_counts` (`cdrizzlebox.c`,
  `p->output_counts`).

### 4.3 Correlated noise from resampling

Any resampling — interpolation during registration warp, *or* drizzle with `pixfrac <
scale` — spreads one input pixel's value across several output pixels. Adjacent output
pixels then share input samples and their noise becomes **correlated**. The
consequence: the **pixel-to-pixel RMS measured in the output underestimates the true
noise on larger (aperture) scales**, because it misses the off-diagonal covariance
terms. This biases any noise-based weighting or SNR estimate computed *after*
resampling, and it makes faint extended features look smoother (better) than they
really are. (Quantified for drizzle in §5.6.) The practical rule: **compute noise
statistics and weights from the un-resampled frames whenever possible**, and treat
post-resample pixel RMS as a lower bound.

### 4.4 Bias of clipping with few frames

With small `N` the sample σ is a poor estimate of the true σ and is itself biased low
(for the sample sd) or dominated by the outlier (for sd that includes it). Two
failure modes:

- **Masking:** one extreme outlier inflates σ so that the threshold `κσ` is wide
  enough to keep the outlier itself — nothing is rejected. Robust center/spread
  (median + MAD, Winsorized σ, GESD's backward scan) mitigates this.
- **Swamping / over-rejection:** with `N ≈ 3–5`, random scatter can push a *good*
  sample past `κσ` of a poorly-estimated center, and you reject signal. siril's
  `N − r > 4` floor (`rejection_float.c:188`) and the `N > 3` loop guards exist
  precisely to stop this.

The honest conclusion (also PixInsight's documented guidance): **do not sigma-clip
below ~8 frames.** Use percentile clipping (3–5) or just a plain median for tiny
stacks; the rejection statistics are not trustworthy.

---

## 5. Drizzle (Fruchter & Hook 2002)

Drizzle is *Variable-Pixel Linear Reconstruction*. Reference implementation: the STScI
C core in `.tmp/refs/drizzle/src/cdrizzlebox.c` (`dobox` dispatcher,
`do_kernel_*` per kernel, `boxer`/`sgarea` polygon overlap, `update_data` flux
accumulation). lumos mirrors this in `src/drizzle/mod.rs`.

### 5.1 The footprint-mapping idea

Each input pixel is shrunk by **pixfrac** `p ∈ (0,1]` into a "drop", then its four
corners are mapped through the geometric transform onto a finer **output grid** whose
pixels are `s = scale` times smaller (output-to-input linear ratio). The drop is a
quadrilateral on the output grid; its overlap area with each output pixel determines
how much flux that output pixel receives.

- **Square kernel** (`do_kernel_square`, `cdrizzlebox.c:1982`): transforms all four
  corners (`interpolate_four_points`), computes the **exact polygon-pixel overlap**
  with `boxer()` (`:280`), which sums signed sub-areas under each edge via `sgarea()`
  (`:174`, Green's-theorem line-integral area). Correct under rotation/shear. lumos:
  `add_image_square` (`mod.rs:398`) with `compute_square_overlap`/`boxer`.
- **Turbo kernel** (`do_kernel_turbo`, `:1841`, lumos default): approximates the drop
  as an **axis-aligned rectangle** centered on the transformed pixel center, overlap
  via the simple `over()` rectangle intersection (`:460`). Fast; valid only when
  rotation between frames is small. lumos: `add_image_turbo` (`mod.rs:329`).

### 5.2 The Jacobian and flux conservation

When the transform changes the pixel scale (distortion, output `scale`), the drop's
area on the output grid is not constant. To **conserve flux per unit area**, the weight
is divided by the local **Jacobian** (the area magnification of the transform):

```
w_effective = w_input / |J|        (per input pixel)
```

STScI computes `jaco = ½·((x₁−x₃)(y₀−y₂) − (x₀−x₂)(y₁−y₃))` — the signed area of the
output quadrilateral via the diagonal cross product — and uses `w = weight/jaco`
(`cdrizzlebox.c:2076-2089`). lumos computes the same Jacobian in `add_image_square`
(`mod.rs:443`) and a `local_jacobian` for turbo (`mod.rs:356`), dividing the weight by
it. The data value is also multiplied by `iscale = s²`-type scaling in STScI
(`d = get_pixel(...) * p->iscale`) so surface brightness is preserved. This Jacobian
correction is what makes drizzle photometrically correct under distortion — the
headline F&H feature.

### 5.3 The flux & weight update equation

For each contribution of input pixel value `d` with effective weight `dow = dover·w`
(`dover` = overlap area, possibly kernel-weighted) to output pixel `(ii,jj)`, STScI's
`update_data` (`cdrizzlebox.c:32`) does a **running weighted average**:

```
if W == 0:   I_out      = d
else:        I_out      = (I_out · W + dow · d) / (W + dow)
in all cases: W_new     = W + dow
```

i.e. the Fruchter & Hook accumulation

```
        I_xy · W_xy + (a_xy · w_i · s²) · i_xy
I_xy ←  ─────────────────────────────────────── ,   W_xy ← W_xy + a_xy · w_i · s²
              W_xy + a_xy · w_i · s²
```

where `a_xy` is the fractional overlap area of the drop with output pixel `(x,y)`,
`w_i` the input pixel weight, `s²` the scale/Jacobian area factor, and `i_xy` the input
flux. lumos does this in two halves: `accumulate` (`mod.rs:603`) sums `d·dow` into the
flux buffer and `dow` into the weight buffer, then `finalize` (`mod.rs:625`) divides
`data/weights` once at the end (`val = data[idx]/w`, `mod.rs:667`) — algebraically the
same weighted average, just deferred. The variance-aware version `update_data_var`
(`:91`) additionally co-adds variance with squared weights (§4.1).

This is a **linear** combination — the output is `Σ(area·weight·flux)/Σ(area·weight)`.
Because it is linear and weight-conserving, total flux is preserved (a star's
integrated counts are unchanged), which is the property that justifies photometry on
drizzled images.

### 5.4 Kernels

| Kernel | Footprint model | Overlap function | Notes |
|--------|-----------------|------------------|-------|
| **Square** | shrunken quadrilateral, exact clip | `boxer`/`sgarea` polygon area | correct under rotation/shear; STScI default for accuracy |
| **Turbo** | axis-aligned rectangle | `over()` rect-rect | fast approximation; OK for small rotation; lumos default |
| **Point** | flux all at drop center | nearest output pixel | fastest, needs the best dithering; no Jacobian needed (`do_kernel_point`, `:1457`) |
| **Gaussian** | Gaussian droplet, FWHM = drop size | `exp(−r²·efac)` LUT-free | smooths/redistributes flux; `pfo` clamped ≥1.2/pscale so no holes (`:1577`) |
| **Lanczos2/3** | sinc-windowed sinc | precomputed LUT (`create_lanczos_lut`) | highest fidelity but **only valid at pixfrac=1, scale=1**; can ring (negative lobes) |

STScI dispatches via `kernel_handler_map` (`cdrizzlebox.c:2144`). lumos mirrors all
five (`DrizzleKernel`, `drizzle/mod.rs:63`); it explicitly forbids Lanczos when
`pixfrac≠1 ∨ scale≠1` ("Per STScI DrizzlePac: Lanczos should never be used for
pixfrac != 1.0", `mod.rs:274`) and applies a `val.max(0.0)` clamp on Lanczos output to
suppress negative ringing (`mod.rs:668`). STScI similarly warns and ignores pixfrac in
the Lanczos kernel (`cdrizzlebox.c:1702`).

### 5.5 pixfrac and output scale

- **pixfrac `p`** shrinks each input pixel before mapping. Limits:
  `p → 0` ≡ **interlacing** (point sampling — drops become points, requires perfect
  sub-pixel dither coverage or you get holes); `p = 1` ≡ **shift-and-add** (full pixel
  convolution, maximal overlap, least correlated noise but no resolution gain). STScI
  docs and F&H both state these limits.
- **scale `s`** sets the output pixel size = input/`s`. `s=2` doubles linear
  resolution of the grid. You can only *recover* the finer detail if the data are
  dithered and undersampled.

Typical optimal values from STScI/HST experience: **final pixfrac 0.7–0.9** for
well-dithered data, with the grid `scale` chosen so the output pixel is ~0.5–0.7× the
input. lumos defaults `scale=2.0, pixfrac=0.8` (`DrizzleConfig::default`,
`mod.rs:107`); the `x3` preset drops pixfrac to 0.7 (`mod.rs:137`).

### 5.6 Correlated-noise penalty (the cost of drizzle)

The price of resampling is correlated noise (§4.3). For a uniform dither filling the
output plane, STScI gives the noise-correlation ratio `R` — the factor by which the
*measured* pixel-to-pixel RMS underestimates the *true* noise — as a function of
`r = p/s`:

```
r ≤ 1 :   R = r / (1 − r/3)
r ≥ 1 :   R = r / (1 − 1/(3r))
```

Worked example from STScI: `p=0.6, s=0.5 → r=1.2 → R = 1.662`. So with those
parameters the apparent noise is ~40% lower than reality. **Implication:** the smaller
the pixfrac relative to scale, the *less* overlap between drops, the fewer effectively
independent samples per output pixel, and the *larger* `R` — i.e. small pixfrac buys
resolution but inflates correlated noise and degrades the *real* (aperture-scale) SNR.
This is the central drizzle tradeoff. Keeping `pixfrac` large (closer to `scale`)
keeps drops overlapping and `R` near 1.

### 5.7 Dithering requirement

Drizzle's resolution recovery **requires sub-pixel dithering**: the frames must sample
the scene at different sub-pixel phases so the finer output grid is filled. Without
dither, every frame samples the same phase, the finer grid has empty cells (or all
drops land identically), and drizzle reduces to a noisier interpolation that *adds*
correlated noise for *no* resolution gain. F&H developed the method specifically for
"undersampled, dithered data". **Drizzling un-dithered, well-sampled (Nyquist) data is
an anti-pattern** — you pay the correlated-noise cost and gain nothing; a plain
stacked mean is strictly better. lumos's drizzle takes per-frame `Transform`s
(`drizzle_stack`, `mod.rs:876`) and does not itself verify dither diversity — that is
the caller's responsibility.

### 5.8 Rejection in the drizzle workflow (DrizzlePac CR scheme)

Because drizzle is a *linear* coadd it cannot reject cosmic rays itself. The classic
DrizzlePac workflow rejects them *before* the final drizzle using **median + blot +
derivative**:

1. Drizzle all frames to a common grid and take the **median** → a clean, CR-free
   *model* of the scene.
2. **Blot** (inverse-drizzle) the median back into each input frame's distorted
   geometry.
3. Compare each input to its blotted model: flag a pixel as a CR if the absolute
   difference exceeds a noise-scaled threshold that also accounts for the local image
   **derivative** (so sharp real features near bright stars aren't falsely flagged).

The exact test from `drizzlepac/drizCR.py` (`:320-334`):

```
t1 = |input − blot|
ta = √(gain·|blot + sky| + readnoise²)               # expected noise (e⁻)
t2 = scale·blot_deriv + snr·ta/gain                  # threshold: noise + derivative term
cr_mask = (t1 ≤ t2)                                  # keep if within threshold
```

run twice with two (snr, scale) pairs (defaults `driz_cr_snr="3.5 3.0"`,
`driz_cr_scale="1.2 0.7"`, `drizCR.py:92-107`), with a 3×3 neighbor-growth step
(`tmp2 ≥ 9`) and radial/CTE-tail dilation of flagged pixels (`:346-373`). The flagged
CRs become zero-weight in the final drizzle. The derivative term `scale·blot_deriv` is
the key insight: it widens the tolerance where the image is steep (PSF cores, edges)
so undersampled structure isn't mistaken for CRs. siril's drizzle path similarly
rejects pixels with zero drizzle weight before combining (`rejection_float.c:117-126`).

lumos's drizzle has **no** built-in CR rejection or blot step — it accepts an optional
`pixel_weight_maps` parameter (`drizzle_stack`, `mod.rs:876`) through which a caller
can supply pre-computed rejection masks, but the median+blot+derivative scheme is not
implemented (see §8).

### 5.9 When drizzle is justified vs harmful

- **Justified:** undersampled data (PSF FWHM ≲ 2 px), **good sub-pixel dither**
  diversity, many frames, geometric distortion to correct, and you need either
  resolution recovery or distortion-free photometry. Keep `pixfrac ≈ scale·0.8` to
  bound correlated noise.
- **Harmful / pointless:** well-sampled (Nyquist or oversampled) data, no/poor dither,
  few frames, or when you only want SNR. In these cases drizzle adds correlated noise
  and resampling blur for no benefit — a robust stacked mean wins. Drizzle is *not* a
  general-purpose stacker.

---

## 6. Recommended best-practice implementation (decision guide)

**Always, before combining:** normalize (additive for lights, multiplicative for
flats), pick the lowest-noise reference, and weight by inverse-variance if frame
quality varies.

| Frame type | Combine | Rejection | Weighting | Normalization |
|------------|---------|-----------|-----------|---------------|
| **Bias** | mean | sigma/winsorized (if N≥8) else mean | equal | none (or additive) |
| **Dark** | mean | sigma/winsorized (N≥8) | equal | none |
| **Flat** | mean | winsorized / sigma | equal | **multiplicative** (per-color mean) |
| **Light, few (≤5)** | median *or* mean + **percentile** | percentile | equal/quality | additive |
| **Light, 6–20** | mean | **Winsorized sigma** (κ≈3) | inverse-variance | additive (+ scaling if transparency varies) |
| **Light, 20+** | mean | **Linear-fit** (or GESD) | inverse-variance + FWHM gate | additive + scaling |
| **Light, satellite/plane-heavy** | mean | Winsorized/GESD (robust) | inverse-variance | additive |

**Drizzle parameters:**
- Use drizzle *only* for dithered + undersampled data.
- `scale`: 1.5–2.0 typical; only as fine as the dither diversity supports.
- `pixfrac`: 0.7–0.9; keep `pixfrac/scale` close to 1 to limit correlated noise (§5.6).
  Reduce pixfrac only with many frames and excellent dither.
- `kernel`: **Square** for accuracy under rotation; **Turbo** for speed with small
  rotation (lumos default); **Point** only with superb dithering; **Lanczos** only at
  pixfrac=1, scale=1 (and accept ringing).
- `min_coverage`: ~0.1–0.5 to mask under-sampled edges/chip gaps.
- Reject CRs *before* drizzling (median+blot+derivative), never rely on drizzle to do
  it.

---

## 7. Pitfalls & anti-patterns

1. **Sigma clipping with < 8 frames.** The σ estimate is dominated by the very
   outliers you want to remove → masking or swamping. Use percentile clipping or a
   plain median for tiny stacks.
2. **Rejection without normalization.** If frames have different sky pedestals or
   gains, the spread across frames is dominated by the *offset*, not noise, so clipping
   rejects whole frames' worth of good pixels at the bright/faint ends. **Normalize
   first, always.**
3. **Plain mean (no rejection) on CR/satellite/plane data.** A single cosmic ray or
   trail leaks straight into the average as a bright streak. The mean is only safe when
   the data are genuinely outlier-free (e.g. already-rejected, or short clean subs).
4. **Drizzle without dithering.** Pays the correlated-noise + resampling-blur cost for
   zero resolution gain. Use a stacked mean instead.
5. **pixfrac too small for the coverage.** Small pixfrac with too few dither phases
   leaves empty/under-covered output pixels (holes) and blows up correlated noise
   (`R = r/(1−r/3)` grows as `r→0`). Increase pixfrac or add dither diversity.
6. **Ignoring correlated noise.** Measuring SNR or deriving inverse-variance weights
   from the *pixel-to-pixel RMS of a drizzled/resampled image* underestimates the true
   noise (by `R`, §5.6) and biases everything downstream. Compute noise on un-resampled
   frames.
7. **Equal weighting of unequal-SNR subs.** Throws away SNR; a cloudy/poor-seeing sub
   should be down-weighted (or dropped), not averaged in at full weight.
8. **Median as the default science combine.** Costs ~20% of SNR (`0.80·√N`) vs a
   clipped mean that keeps ~99%. Median is for robustness-critical *intermediate*
   products (e.g. the DrizzlePac CR model), not the final light stack.
9. **Lanczos drizzle at pixfrac≠1 / scale≠1.** Invalid; produces ringing and
   flux-conservation errors. (lumos guards this; some pipelines don't.)
10. **GESD/clipping critical values from a normal instead of Student-t.** For small N
    the t-distribution's heavy tails matter; using the normal under-estimates the
    threshold and over-rejects (lumos's current GESD approximation — §8).

---

## 8. How lumos currently does it — and gaps/opportunities

**What lumos has (correct and well-grounded):**

- Full rejection family `None | SigmaClip | Winsorized | LinearFit | Percentile |
  Gesd` (`src/stacking/rejection.rs`, `config.rs:831`), matching the siril set, with
  presets `sigma_clipped / winsorized / linear_fit / median / mean / gesd / percentile
  / weighted` (`config.rs:71`) and frame presets `light/flat/dark/bias`.
- Lowest-MAD reference selection (`stack.rs:165`), Global (additive+multiplicative)
  and Multiplicative normalization (`stack.rs:189`), inverse-variance noise weighting
  (`stack.rs:271`), equal/manual weighting.
- Winsorized σ with the correct 1.134 bias factor (`rejection.rs:202`); GESD with the
  backward-scan Grubbs procedure (`rejection.rs:716`); MAD-based robust spread
  throughout.
- Drizzle: all five kernels, exact `boxer` polygon clipping for Square, Jacobian flux
  conservation, deferred weighted-average `accumulate`/`finalize`, coverage map with
  `min_coverage` masking, Lanczos restricted to pixfrac=scale=1 with negative-lobe
  clamping (`drizzle/mod.rs`).
- Memory-tiered `ImageCache` (in-memory vs mmap, `cache.rs:153`) so large stacks don't
  OOM, with per-frame `FrameStats` (median + MAD) precomputed (`cache.rs:38`).

**Gaps / opportunities (ranked):**

1. **GESD critical values use an inverse-*normal* approximation, not Student-t.**
   `inverse_normal_approx` (`rejection.rs:759`) vs siril's exact
   `gsl_cdf_tdist_Pinv(...)` (`median_and_mean.c:1481`). For small N this
   over-rejects. Implement the proper t-inverse (or at least a t-correction) to match
   siril's `λ = (n−1)t / √(n(n−2+t²))` exactly.
2. **No output variance/uncertainty map.** STScI's `update_data_var`
   (`cdrizzlebox.c:91`) propagates variance with squared weights; lumos's stacker and
   drizzle return only the image (+ coverage). An inverse-variance-weighted output
   variance map would let downstream steps weight correctly and would expose the
   correlated-noise issue honestly.
3. **No drizzle CR rejection (median + blot + derivative).** lumos relies on
   caller-supplied `pixel_weight_maps`. Implementing the DrizzlePac `drizCR` scheme
   (`drizzlepac/drizCR.py`) — drizzle→median→blot→derivative-thresholded mask — would
   make the drizzle path self-contained. This also needs a **blot** (inverse-drizzle)
   operation, which lumos lacks (STScI: `cdrizzleblot.c`).
4. **Normalization is scalar-per-channel only; no surface/gradient background
   matching.** Fine for uniform pedestal shifts, insufficient for mosaics or strong
   gradients. A low-order plane/poly background match (cf. `reproject`'s
   `mosaicking/background.py`, SWarp's mesh background) would be needed for mosaics.
5. **Noise weighting omits the normalization scale factor.** siril uses `w = 1/(pscale²
   · bgnoise²)` (`median_and_mean.c:1122`) — the `pscale²` keeps weights consistent in
   the normalized frame. lumos's `Weighting::Noise` (`stack.rs:271`) uses `1/σ²`
   without the scaling term; if multiplicative normalization is active the weights are
   slightly inconsistent.
6. **No FWHM / star-count quality weighting.** lumos has only Equal/Noise/Manual;
   siril and PixInsight additionally weight by seeing/star-count, which catches bloated
   frames that the background-noise estimate alone would not.
7. **No explicit small-N rejection guard surfaced to the user.** lumos's rejection
   methods should refuse (or warn and fall back to percentile/median) below ~8 frames,
   the way siril's `N−r>4` floor and `nb_frames<3` GESD guard do; relying on the
   algorithm to "do nothing" silently is a footgun (§7 #1).
8. **Drizzle does not validate dither diversity.** It will happily drizzle un-dithered
   data and produce a correlated-noise-inflated result with no warning (§5.7).

---

## 9. References

### Source code (cloned references)

- STScI drizzle C core — `.tmp/refs/drizzle/src/cdrizzlebox.c`:
  `update_data` (:32), `update_data_var` (:91), `sgarea` (:174), `boxer` (:280),
  `over` (:460), `compute_pscale_ratio` (:504), `do_kernel_point` (:1457),
  `do_kernel_gaussian` (:1549), `do_kernel_lanczos` (:1688), `do_kernel_turbo`
  (:1841), `do_kernel_square` (:1982), `dobox` (:2160), kernel dispatch table (:2144).
  Blot: `.tmp/refs/drizzle/src/cdrizzleblot.c`.
- DrizzlePac CR rejection — `.tmp/refs/drizzlepac/drizzlepac/drizCR.py` (`_driz_cr`
  :220, the t1/ta/t2 threshold :320-334), `createMedian.py`, `ablot.py`.
- siril stacking — `.tmp/refs/siril/src/stacking/`: `rejection_float.c`
  (`percentile_clipping` :31, `sigma_clipping_float` :49, `line_clipping` :62,
  `grubbs_stat` :82, `apply_rejection_float` :100, WINSORIZED :223, LINEARFIT :260,
  GESDT :301); `median_and_mean.c` (`compute_noise_weights` :1110,
  `compute_wfwhm_weights` :1136, `compute_nbstars_weights` :1183, GESD t-critical
  :1481); `normalization.c` (modes :124-134, ref-relative transform :142-177).
- DeepSkyStacker — `.tmp/refs/DeepSkyStacker/DeepSkyStackerKernel/DSSTools.h`
  (`KappaSigmaClip` :606, `MedianKappaSigmaClip` :677); `MultiBitmapProcess.cpp`
  (Entropy / AutoAdapt method names).
- SWarp / reproject — `.tmp/refs/swarp/` (mesh background, weighted coadd),
  `.tmp/refs/reproject/reproject/mosaicking/background.py` (background matching).
- lumos — `src/stacking/{config.rs, stack.rs, rejection.rs, cache.rs}`,
  `src/drizzle/mod.rs`.

### Online sources

- Fruchter & Hook 2002, *Drizzle: A Method for the Linear Reconstruction of
  Undersampled Images*, PASP 114:144 — https://arxiv.org/abs/astro-ph/9808087 ,
  https://iopscience.iop.org/article/10.1086/338393
- STScI, *Dithering and the Drizzle Algorithm* —
  https://www.stsci.edu/ftp/science/hdf/combination/drizzle.html
- STScI HST docs, *Weight Maps and Correlated Noise* (the `R = r/(1−r/3)` formula) —
  https://hst-docs.stsci.edu/drizzpac/chapter-3-description-of-the-drizzle-algorithm/3-3-weight-maps-and-correlated-noise
- The DrizzlePac Handbook —
  https://www.stsci.edu/files/live/sites/www/files/home/scientific-community/software/drizzlepac/_documents/drizzlepac-handbook-v1.pdf
- drizzle package user docs (pixfrac/scale/kernels) —
  https://spacetelescope-drizzle.readthedocs.io/en/stable/drizzle/user.html
- NIST Engineering Statistics Handbook, *Generalized ESD Test for Outliers* (Grubbs
  statistic, λ critical value) —
  https://www.itl.nist.gov/div898/handbook/eda/section3/eda35h3.htm
- PixInsight ImageIntegration rejection guidance (Winsorized < ~20 subs, Linear-fit
  25+) — https://www.cloudynights.com/forums/topic/697077-question-about-rejection-algorithms-in-pixinsight/ ,
  https://dslr-astrophotography.com/detailed-pixel-rejection-methods/
- Siril stacking docs (rejection, median 0.8√N efficiency) —
  https://siril.readthedocs.io/en/stable/preprocessing/stacking.html
- Median vs mean efficiency (`√(π/2)`, `0.80`) — corroborated across
  https://jonrista.com/the-astrophotographers-guide/astrophotography-basics/snr/ and
  https://medium.com/@rupesh.rupeshs/image-stacking-and-signal-quality-a3b7d310df70
