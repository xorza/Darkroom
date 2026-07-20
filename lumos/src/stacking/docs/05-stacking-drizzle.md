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

- **lumos**: `select_reference_frame` (`src/stacking/combine/normalization/mod.rs`) picks the frame
  with the lowest mean MAD (median absolute deviation) across channels. MAD is used
  rather than standard deviation because it is robust to the stars/objects in the
  frame — it measures background noise, not signal.
- **Registered-frame domain**: reference selection and inverse-variance weighting use MAD captured
  from each source before interpolation. Global normalization fits paired samples over the
  intersection of coverage-valid, confidence-positive pixels, while multiplicative normalization
  uses medians over that same domain. Warp fill and interpolation smoothing therefore cannot change
  source-noise weights or become a false gain. Frames with disjoint support can still combine
  without normalization using equal, manual, or source-noise weights; normalization returns
  `NoCommonCoverage`.
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

- **lumos**: `compute_frame_norms` (`src/stacking/combine/normalization/mod.rs`) implements two of
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
combined pixel; equal weighting is optimal only when all `σ_i` are equal. **Proof
sketch:** for a weighted mean `x̂ = Σw_i x_i / Σw_i`, `Var(x̂) = Σw_i²σ_i² / (Σw_i)²`.
Minimizing over `{w_i}` by Lagrange (∂/∂w_j = 0) gives `w_j ∝ 1/σ_j²`; substituting back
collapses the variance to `1/Σ(1/σ_i²)` — the Cramér-Rao bound for `N` Gaussian samples,
so no unbiased linear combiner can do better. This is the same statement F&H make about
drizzle's weighting being *"statistically optimum when inverse variance maps are used as
the input weights"* (§5.3). Note `w_i ∝ SNR_i²` for fixed signal (since `SNR ∝ 1/σ` at
fixed flux), which is why "SNR² weighting" and "inverse-variance weighting" are the same
prescription expressed in different units. **Subexposure-weighting corollary:** for
sky-noise-limited subs of equal length, `σ_i² ∝ sky_i`, so `w_i ∝ 1/sky_i` — the cloudy
/ light-polluted / high-airmass sub is automatically down-weighted, and a sub twice as
noisy contributes ¼ the weight, not ½.

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
efficient* estimator. **Derivation of the 0.80 factor:** for a sample of `N` draws from
a distribution with density `f` and standard deviation `σ`, the asymptotic variance of
the sample median is `1/(4N·f(m)²)` where `m` is the population median. For a Gaussian,
`f(m) = 1/(σ√(2π))`, so `Var(median) = (σ²·2π)/(4N) = (π/2)·σ²/N`, versus `Var(mean) =
σ²/N`. The ratio is exactly **`π/2 ≈ 1.5708`** in variance, so the median's standard
error is larger than the mean's by `√(π/2) ≈ 1.2533`. Equivalently the median's
statistical efficiency is `2/π ≈ 0.637` in *variance*, i.e. `√(2/π) ≈ 0.7979 ≈ 0.80` in
*standard deviation*. So:

```
SNR_median ≈ 0.80 · √N · SNR_single        (asymptotic floor; loss is *smaller* at small N — below)
```

A median stack throws away ~20% of the SNR you could have had — roughly equivalent to
discarding ~36% of your frames (`0.80² ≈ 0.637`, so you'd need `1/0.637 ≈ 1.57×` as
many frames to match a mean's SNR). **Correction (pass 3):** the prior version added
"the penalty is *worse* at small N (approached from below; ~0.74 at N=3)" — that is
**backwards**. Direct numerical integration of the order-statistic variance for a
Gaussian gives a *variance* efficiency `Var(mean)/Var(median)` of `0.743` at N=3,
`0.697` at N=5, `0.669` at N=9, *decreasing monotonically* to the asymptotic
`2/π ≈ 0.637` — i.e. the limit is approached **from above**. In *standard-deviation*
terms that is `0.862` (N=3) → `0.835` (N=5) → `0.798` (N→∞), so a 3-frame median actually
keeps ~86% of the mean's SNR, *more* than the ~80% asymptotic floor. The `~0.74` quoted
before is the *variance* efficiency at N=3 (which is **above** the `0.637` asymptotic
variance efficiency), not a worse SD efficiency than `0.80`. The real small-N caution
about median/clipping is about the **robustness of the σ estimate** (§4.4), not SNR
efficiency. Corroborated by standard order-statistics theory and astrophotography
references (Siril docs; jonrista; Akinshin, *median-vs-mean*).

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

### 2.3 Trimmed and Winsorized means (the efficiency middle ground)

Between the plain mean (best efficiency, no robustness) and the median (full
robustness, 0.80 efficiency) sit two compromise estimators, both of which the rejection
family reduces to:

- **Trimmed mean** — sort the `N` samples, *discard* the lowest and highest `α·N`, and
  average the rest. A 10% trimmed mean of a Gaussian has ~0.97 efficiency yet rejects
  one-sided contamination up to 10%. **Sigma/kappa-clipping is an adaptive trimmed
  mean**: instead of trimming a fixed *fraction*, it trims everything past `κσ`, so the
  trim count adapts to how heavy the tail actually is. The `SIGMEDIAN` /
  median-kappa-sigma variants (siril `rejection_float.c:210`, DSS
  `MedianKappaSigmaClip`) are the *replacing* cousin — rejected samples become the
  median rather than being dropped, keeping `N` constant.
- **Winsorized mean** — instead of discarding the tails, *clamp* them to the cutoff
  value, then average. Retains all `N` samples' worth of count but caps their leverage.
  This is the combination-operator analogue of the **σ-estimation** trick in §3.2;
  the rejection there Winsorizes only to compute a robust *spread*, then clips the
  original data, but the same clamp-don't-drop idea defines the Winsorized *mean*.

The practical upshot: a sigma-clipped mean is, statistically, an adaptive trimmed mean,
and that is *why* it keeps ~99% of the mean's efficiency while shedding outliers —
it trims only the genuine tail, not the Gaussian core. This is the formal justification
for the "clipped mean beats median for science stacks" rule of §2.1.

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
`rejection.rs:831`).

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
- lumos `SigmaClip` (`rejection.rs`, `SigmaClipConfig` at `rejection.rs:20`) — separate
  low/high sigma, iterative; median center + MAD spread, with a Welford mean+stddev
  early-exit that skips the median/MAD pass when no sample can possibly clip
  (`no_outliers_possible`, `rejection.rs:138`).

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
(because clamping the tails shrinks the variance). **Where 1.134 comes from:**
Winsorizing a Gaussian at Huber's `c = 1.5` (i.e. clamping everything beyond ±1.5σ to
exactly ±1.5σ) leaves a distribution whose variance is smaller than the parent σ². The
deflation factor for `c = 1.5` works out to ≈ `1/1.134² ≈ 0.778`, so multiplying the
Winsorized sd by `1.134 ≈ 1/√0.778` restores an (asymptotically) unbiased σ estimate.
The value is specific to `c = 1.5` — a different Winsorizing cutoff needs a different
factor. This `c = 1.5` / `1.134` pairing is the one PixInsight, siril, and lumos all
use; it is documented in PixInsight's Winsorized-sigma-clipping notes and Huber's robust
statistics. **Verified (pass 3) by direct numerical integration:** for the symmetric
Winsorized Gaussian at `c = 1.5`, `E[ψ_{1.5}(z)²] = ∫_{−1.5}^{1.5} z²φ(z) dz +
1.5²·2(1−Φ(1.5)) = 0.47783 + 0.30063 = 0.77846`, so the bias factor is exactly
`1/√0.77846 = 1.13339 ≈ 1.134` — siril/lumos's constant to four digits. (`ψ_c` is
Huber's clipped score, which is just the Winsorized variable `clamp(z, −c, c)`; its
second moment is the deflated variance.) lumos hard-codes exactly this:
`HUBER_C = 1.5` (`rejection.rs:201`), `WINSORIZED_CORRECTION = 1.134`
(`rejection.rs:203`), `WINSORIZE_CONVERGENCE = 0.0005` (`rejection.rs:205`) — bit-for-bit
the same constants as siril's `1.5f`, `1.134f`, `0.0005f` (`rejection_float.c:230-237`),
including the same iterate-to-stable-σ inner loop.

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
CDF**. The exact NIST/Rosner form (parsed from the NIST e-Handbook, pass 2) indexes by
`i = 1…r` (number already removed):

```
              (n−i) · t_{p, n−i−1}                                α
λ_i  =  ──────────────────────────────── ,   p = 1 − ─────────────────
        √((n−i−1+t²)·(n−i+1))                          2·(n − i + 1)
```

where `t_{p,ν}` is the 100p-th percentile of the t-distribution with `ν` d.o.f. This is
**identical** to the per-step form `λ = (m−1)·t / √(m·(m−2+t²))` with `t = t_inv(1 −
α/(2m), m−2)` once you set `m = n − i + 1` (the live sample count at step `i`): then
`m−1 = n−i`, `m−2 = n−i−1`, `m = n−i+1`, and `p = 1 − α/(2m)` line up exactly. siril
codes the `m` (decrementing `size`) convention; the doc's earlier `(n−1)t/√(n(n−2+t²))`
is this same form at the first step (`m = n`). The number of outliers is the *largest*
`i` for which `G_i > λ_i`. Because the comparison is done backward (test for the most
outliers that still pass), GESD is robust to **masking** (one outlier hiding another) —
NIST's worked example flags **3** outliers at α=0.05 where sequential Grubbs *"would
stop at the first iteration and declare no outliers."* NIST also notes the t-critical
approximation is *"very accurate for n ≥ 25 and reasonably accurate for n ≥ 15"* — below
that, neither the t- nor the normal-approximation should be trusted much.

- siril `GESDT` (`rejection_float.c:301`) with `grubbs_stat` (`:82`) and the critical
  values precomputed via the exact t-inverse CDF
  `gsl_cdf_tdist_Pinv(1 − sig/(2·size), size−2)` (`median_and_mean.c:1481`) — this is
  *exactly* the λ formula above. The max number of outliers is
  `floor(N · sig[0])` (`:1480`), i.e. a fraction of the stack.
- lumos `Gesd` (`GesdConfig`, `rejection.rs:616`; `reject` at `:668`) implements the
  same two-sided mean/sample-sd statistic and backward scan as NIST. Critical values use
  an accurate Student-t inverse CDF and the live sample count, and are cached per rayon
  worker for a `(sample count, max outliers, α)` configuration. Automatic `max_outliers`
  targets `N/4`, capped at two below 25 samples and ten thereafter; explicit values remain
  available for independently validated contamination policies. The `StackConfig::gesd()`
  preset falls back to median below 15 frames.
  The implementation is checked against NIST's 54-value worked example and all ten
  tabulated statistics and critical values. Deterministic Gaussian Monte Carlo at
  `N = 15, 25, 50, 100` verifies the family-wise false-positive rate against `α = 0.05`.

**Assumptions:** approximately Gaussian core, outlier fraction below `r/N`. GESD needs
`n ≥ ~3` to even compute (siril guards `nb_frames < 3` at `median_and_mean.c:1281`)
and only makes statistical sense with `N ≥ 15`; the Lumos preset enforces that floor.

### 3.6 MAD-based clipping

Not a separate algorithm so much as a *robust spread choice* within sigma clipping:
use **MAD** (`MAD = median(|x_i − median(x)|)`) scaled to a σ-equivalent via
`σ ≈ 1.4826·MAD` instead of the sample standard deviation. Because the median and MAD
are not inflated by outliers, MAD-based clipping converges correctly even with a heavy
tail and is preferable to mean+sd clipping. siril exposes it as the `MAD` rejection
case (`rejection_float.c:175-181`), identical to sigma clipping but with
`siril_stats_float_mad` as the spread. lumos uses MAD throughout its frame statistics
(`FrameStats`, `frame_store/mod.rs`) and `mad_to_sigma` (`MAD_TO_SIGMA = 1.4826022`) in
`math/statistics`.

### 3.7 Min-frame requirements summary

| Method | Min sensible N | Best regime | Spread used |
|--------|----------------|-------------|-------------|
| Percentile | 3 | 3–5 frames | none (fraction of median) |
| Winsorized σ-clip | ~6 | 6–20 frames | Winsorized σ (×1.134) |
| Sigma / kappa-σ | ~8 | 8–20 frames | sd or MAD |
| Linear-fit | ~15 | 20+ frames | mean abs residual |
| GESD | 15 | 15+ frames, controlled FPR | sample sd, exact t-critical |

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
accept a distinct input-variance image for every frame. Both statistical stacking and drizzle emit
`StackProduct.weight`. Linear statistical combines and drizzle additionally emit
`StackProduct.linear_variance = Some(Σw_i²/(Σw_i)²)` from the actual contributing samples; median
output uses `None` because it has no linear coefficients. Statistical quality images are
channel-shaped because rejection can select different survivors per channel; drizzle's channels
are identical because its geometric weights are channel-independent.

### 4.2 Weight maps and coverage maps

Both paradigms benefit from carrying a per-pixel weight:

- In **stacking**, a per-pixel weight (e.g. inverse-variance, or a quality mask
  flagging bad columns/edges) lets you down-weight rather than hard-reject, and the
  output weight image records the surviving WHT after multiplying each frame weight by per-pixel
  confidence. Equal weighting therefore tracks survivor count where confidence is one;
  Noise/Manual frame weights are normalized before applying confidence.
  `StackProduct.coverage` stays a separate scalar fraction of frames with geometric support;
  zero confidence removes statistical weight without erasing that support.
- In **drizzle**, the **coverage/weight map is mandatory** — it records `W_xy = Σ
  a_xy·w_i` (the accumulated overlap-area × input-weight; **per F&H Eq. 4 there is no
  `s²` in the weight** — the `s²` lives only in the flux numerator, Eq. 5) and is what
  the flux is normalized by. Edge pixels and chip gaps get low coverage; `min_coverage`
  masks them. lumos returns this as `StackProduct.coverage` normalized to [0,1];
  STScI returns `output_counts` (`cdrizzlebox.c`,
  `p->output_counts`).

### 4.3 Correlated noise from resampling

Any resampling — interpolation during registration warp, *or* drizzle whenever drops
overlap (i.e. `pixfrac > 0`, increasing with `r = p/s`; in lumos terms `r = pixfrac·scale`, §5.6) — spreads one
input pixel's value across several output pixels. Adjacent output pixels then share
input samples and their noise becomes **correlated**. The
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
pixels are `scale`× smaller in linear size (lumos's `scale`, the super-resolution
factor; equivalently F&H's output/input pixel-size ratio `s = 1/scale` — see §5.5 for
this reciprocal, which is easy to get wrong). The drop is a quadrilateral on the output
grid; its overlap area with each output pixel determines how much flux that output pixel
receives.

- **Square kernel** (`do_kernel_square`, `cdrizzlebox.c:1982`): transforms all four
  corners (`interpolate_four_points`), computes the **exact polygon-pixel overlap**
  with `boxer()` (`:280`), which sums signed sub-areas under each edge via `sgarea()`
  (`:174`, Green's-theorem line-integral area). Correct under rotation/shear. lumos:
  `add_image_square` (`drizzle/accumulator.rs`) with `boxer`.
- **Turbo kernel** (`do_kernel_turbo`, `:1841`, lumos default): approximates the drop
  as an **axis-aligned rectangle** centered on the transformed pixel center, overlap
  via the simple `over()` rectangle intersection (`:460`). Fast; valid only when
  rotation between frames is small. lumos: `add_image_turbo` (`drizzle/accumulator.rs`) using
  `math::rect::Rect::overlap_area`.

### 5.2 The Jacobian and flux conservation

When the transform changes the pixel scale (distortion, output `scale`), the drop's
area on the output grid is not constant. To **conserve flux per unit area**, the weight
is divided by the local **Jacobian** (the area magnification of the transform):

```
w_effective = w_input / |J|        (per input pixel)
```

STScI computes `jaco = ½·((x₁−x₃)(y₀−y₂) − (x₀−x₂)(y₁−y₃))` (`cdrizzlebox.c:1376`) —
the signed area of the output quadrilateral via the diagonal cross product — and uses
`w = get_pixel(weights)·weight_scale / jaco` (`:1393`; comment: *"Scale the weighting
mask … inversely by the Jacobian to ensure conservation of weight in the output"*). The
sign is irrelevant because, as the STScI comment notes, anticlockwise corners make both
`jaco` and the `boxer` areas negative, so it cancels on division. lumos computes the
same Jacobian in `add_image_square` (`mod.rs:443`) and a `local_jacobian` for turbo
(`mod.rs:356`), dividing the weight by it. The data value is also multiplied by
`iscale` in STScI (`d = get_pixel(...) * p->iscale`, `:640`/`:1380`) so surface
brightness is preserved.

This Jacobian correction is the **headline F&H feature**. F&H §5 (Photometry) states it
plainly: camera distortion means *"pixels at the corner of each CCD subtend less area
on the sky than those near the center … point sources near the corners of the chip are
artificially brightened compared to those in the center. By scaling the weights of the
input pixels by their areal overlap with the output pixel, and by moving input points
to their corrected geometric positions, Drizzle largely removes this effect. In the
case of pixfrac = 1, this correction is exact."* Their quantitative test: a 4% edge
brightening from WF distortion is reduced to **0.004 mag RMS** after drizzling at
`scale=0.5, pixfrac=0.6` (§5 of the paper), and ≤0.015 mag with realistic random
dithers plus CR masks — the photometric-fidelity numbers that justify aperture
photometry on drizzled images.

### 5.3 The flux & weight update equation

**Correction/confirmation (pass 2):** the F&H paper is now read in full; here are the
*verbatim* equations (their §2, Eqs. 2–5). When input pixel `(xᵢ,yᵢ)` with data
`d_{xᵢyᵢ}` and weight `w_{xᵢyᵢ}` is added to output pixel `(xₒ,yₒ)` with running value
`I`, running weight `W`, and fractional overlap `0 < a_{xᵢyᵢxₒyₒ} < 1`, the *iterative*
update (Eqs. 2, 3) is

```
W'  =  a·w  +  W                                         (F&H Eq. 2)

       d · a · w · s²  +  I · W
I'  =  ───────────────────────────                       (F&H Eq. 3)
                W'
```

"where a factor of `s²` is introduced to conserve surface intensity" (`s = scale`,
output-to-input pixel-size ratio). After all inputs, the closed form (Eqs. 4, 5, with
Einstein summation over all input pixels of all images) is

```
W_{xₒyₒ}  =  Σ a · w                                      (F&H Eq. 4)

             Σ d · a · w · s²
I_{xₒyₒ}  =  ───────────────────                          (F&H Eq. 5)
                W_{xₒyₒ}
```

The earlier (pass 1) rendering of this doc had the algebra right — the only refinement
is that the `s²` factor multiplies the **flux numerator** (surface-intensity
conservation), and the **weight** `W` accumulates the bare `a·w` (Eq. 4 has *no* `s²`).
STScI's `update_data` (`cdrizzlebox.c:32`) implements Eq. 3 as a **running weighted
average** with `dow = a·w` and the `s²` folded into `d` upstream via `d = data·iscale`
(`cdrizzlebox.c:640`, `iscale = s²`-type factor):

```
if W == 0:   I_out  = d
else:        I_out  = (I_out · W + dow · d) / (W + dow)
in all cases: W_new = W + dow                            # dow = a·w
```

F&H also state the deep design property directly: *"the linear weighting scheme is
statistically optimum when inverse variance maps are used as the input weights"* — i.e.
`w_i = 1/σ_i²` makes Eq. 5 the minimum-variance estimator (§1.3). lumos does this in two
halves: `accumulate` (`mod.rs:603`) sums `flux·pixel_weight` into the flux buffer and
`pixel_weight` into the weight buffer, then `finalize` (`mod.rs:625`) divides
`data/weights` once at the end (`val = data[idx]/w`, `mod.rs:667`) — algebraically the
same weighted average as Eqs. 4–5 **except for the global `s²` factor, which lumos
omits** (see the closing paragraph of this subsection), just deferred to one final
division. The
variance-aware variant `update_data_var` (`:91`) additionally co-adds variance arrays
with **squared** weights `v = (var·vc² + dow²·d2)/vc_plus_dow²` (`:135`) — the correct
`Var(Σwx)=Σw²Var(x)` propagation (§4.1), and exactly the `w²s⁴σ²` form that reappears in
the F&H correlated-noise derivation (§5.6).

This is a **linear** combination — lumos's output is exactly
`Σ(a·w·flux)/Σ(a·w)`, i.e. F&H Eqs. 4–5 **without the `s²` factor**.
**Correction (pass 3):** the prior version called this total-flux-conserving; it is not,
quite. F&H put `s²` (`= s_F&H² = 1/scale_lumos²`) in the flux numerator (Eq. 5), and
STScI folds it into the data via `d = data·iscale` with `iscale = s²`
(`cdrizzlebox.c:640`; `iscale = scale·scale` at `cdrizzleapi.c:324`, where STScI's
`scale` is F&H's `s`, *not* lumos's super-resolution `scale`); that factor is what makes
the output conserve surface intensity / total integrated flux across the pixel-size
change. lumos drops it — the `scale²` in its Jacobian cancels between the flux buffer and
the weight buffer, so a flat field of value `F` drizzles back to `F`. lumos
therefore preserves the **input per-pixel DN scale**, not F&H's flux normalization, and
its integrated counts grow by `scale²`. Because `s²` is a *single global constant*, this
leaves SNR, linearity, and *relative* aperture photometry on one drizzled frame intact
(it cancels in any ratio) and arguably makes lumos's drizzle output directly comparable
to its ordinary mean-stack output; it only shifts the absolute level. But it is **not**
the F&H/STScI flux convention — relevant if you mix drizzled and undrizzled frames or do
cross-pipeline absolute photometry (§8).

### 5.4 Kernels

| Kernel | Footprint model | Overlap function | Notes |
|--------|-----------------|------------------|-------|
| **Square** | shrunken quadrilateral, exact clip | `boxer`/`sgarea` polygon area | correct under rotation/shear; STScI default for accuracy |
| **Turbo** | axis-aligned rectangle | `over()` rect-rect | fast approximation; OK for small rotation; lumos default |
| **Point** | flux all at drop center | nearest output pixel | fastest, needs the best dithering; no Jacobian needed (`do_kernel_point`, `:1457`) |
| **Gaussian** | Gaussian droplet, FWHM = drop size | `exp(−r²·efac)` LUT-free | smooths/redistributes flux; `pfo` clamped ≥1.2/pscale so no holes (`:1577`) |
| **Lanczos2/3** | sinc-windowed sinc | precomputed LUT (`create_lanczos_lut`) | highest fidelity but **only valid at pixfrac=1, scale=1**; can ring (negative lobes) |

STScI dispatches via `kernel_handler_map` (`cdrizzlebox.c:2144`). lumos mirrors all
five (`DrizzleKernel`, `drizzle/mod.rs:63`). **Correction (pass 3):** lumos does *not*
"forbid" Lanczos off `pixfrac=scale=1` — it only emits a `tracing::warn!` and then runs
the kernel anyway (`mod.rs:271-281`: "Lanczos kernel should only be used with
pixfrac=1.0 and scale=1.0"), applying a `val.max(0.0)` clamp on the output to suppress
negative ringing (`mod.rs:669`). STScI behaves the same — it warns and *ignores* pixfrac
in the Lanczos kernel rather than erroring (`cdrizzlebox.c:1702`). Three further lumos
specifics worth noting: (a) its Lanczos is **Lanczos-3 only** (`a = 3`, separable
`lanczos_kernel(dx)·lanczos_kernel(dy)`, `mod.rs:312-323`; STScI also offers Lanczos-2);
(b) its Gaussian and Lanczos paths **normalise the kernel to sum 1** per input pixel
before accumulating (two-pass `add_image_radial`, `mod.rs:520-599`), so the drop's total
weight is conserved regardless of kernel shape; and (c) lumos's **Point** kernel *does*
apply the Jacobian (`weight·pw/jaco`, `mod.rs:508`), so the table's "no Jacobian needed"
note describes STScI's `do_kernel_point`, not lumos's.

### 5.5 pixfrac and output scale

- **pixfrac `p`** is, in F&H's exact words, *"the ratio of the linear size of the drop
  to the input pixel"*. Limits, stated verbatim in F&H §2: *"interlacing is equivalent
  to Drizzle in the limit of pixfrac → 0.0, while shift-and-add is equivalent to
  pixfrac = 1.0."* So `p → 0` ≡ **interlacing** (drops become points, requires perfect
  sub-pixel dither coverage or you get holes — "if the drop size is sufficiently small
  not all output pixels have data added to them from each input image"); `p = 1` ≡
  **shift-and-add** (full pixel convolution, maximal overlap). F&H's image-quality
  argument (their Eq. 1, `I = T⊗O⊗E⊗P⊗G`): drizzle *replaces the convolution by the
  physical pixel `P` with a convolution by the smaller kernel `p`*, and "as convolutions
  add as the sum of squares, the effect of this replacement is often quite
  significant" — this is *why* a smaller pixfrac sharpens the image.
- **scale `s`** is *"the ratio of the linear size of an output pixel to an input
  pixel"* (F&H §2) — so **`s < 1` means a *finer* output grid**: `s = 0.5` makes each
  output pixel ½ the input pixel and doubles the linear sampling. F&H note the sweet
  spot: when the dithers map onto output-grid centers and *"pixfrac and scale are chosen
  so that p is only slightly greater than s, one obtains the full advantages of
  interlacing"* — the convolutions with both `p` and the output grid `G` effectively
  drop away while the small drop overlap still fills missing data. In the `r = p/s`
  parameterization of §5.6 that sweet spot is `r` slightly above 1. You can only
  *recover* finer detail if the data are dithered and undersampled.

  **Correction (pass 3) — lumos's `scale` is the *reciprocal* of F&H's `s`.** lumos's
  `scale` is the linear *super-resolution factor* (`output_width = scale·input_width`,
  `mod.rs:189`): the output grid is `scale`× **finer**, so each output pixel is `1/scale`
  the size of an input pixel. Hence `s_F&H = 1/scale_lumos`, and the F&H ratio is
  `r = p/s = pixfrac · scale_lumos` — equivalently, **`r` is just lumos's
  `drop_size = pixfrac·scale` measured in output pixels** (`mod.rs:269`). The prior
  version's "`s=2` (output pixel = ½ input)" was self-contradictory: under F&H's
  definition `s=2` makes the output pixel *twice* the input (coarser); 2× finer sampling
  is `s = 0.5`, i.e. `scale_lumos = 2`. When applying the §5.6 formula to lumos
  parameters, use `r = pixfrac·scale`, **not** `pixfrac/scale`.

Typical optimal values from STScI/HST experience: **final pixfrac 0.7–0.9** for
well-dithered data, with the grid `scale` chosen so the output pixel is ~0.5–0.7× the
input. lumos defaults `scale=2.0, pixfrac=0.8` (`DrizzleConfig::default`,
`mod.rs:107`); the `x3` preset drops pixfrac to 0.7 (`mod.rs:137`).

**lumos's default `r` is therefore high, not low (pass 3).** Using `r = pixfrac·scale`
(above): the default `scale=2.0, pixfrac=0.8` gives `r = 1.6` (drop = 1.6 output pixels)
→ `R = r/(1−1/(3r)) ≈ 2.0`; the `x3` preset (`scale=3, pixfrac=0.7`) gives `r = 2.1` →
`R ≈ 2.5`. Both sit well **above** the F&H/Casertano sweet spot `r ≈ 1.2–1.25`
(`R ≈ 1.6`): lumos's defaults favour uniform coverage and a smooth result at the cost of
substantial correlated noise — the *opposite* end of the tradeoff from the "keep `r`
modestly below 1" advice in §5.9 (which is the low-correlated-noise choice). To reach
`r < 1` in lumos you need `pixfrac < 1/scale` (e.g. `pixfrac ≤ 0.45` at `scale=2`).

**Drizzle in practice — the HDF-S numbers (Casertano et al. 2000, parsed pass 2).** The
canonical worked example of choosing these parameters: for the final HDF-S WFPC2 combine
Casertano used *"a pixel scale of 0.4 WF pixels, and a footprint of 0.5 WF pixels"* —
i.e. output pixel = 0.4× input (`scale ≈ 2.5`), drop = 0.5× input (`pixfrac = 0.5`), so
`r = p/s = 0.5/0.4 = 1.25`. They also ran a *coarser intermediate* combine at 0.6 WF
pixels (`60 mas`) with footprint 1.0 specifically because *"60 mas [is] more than
adequate for a proper cosmic ray rejection, but not as demanding in terms of number of
input images and pointing quality as the scale of 40 mas/pixel desired for"* the final
science image. This is the standard two-tier pattern: a forgiving large-pixel combine
for the CR median, a fine-pixel combine for the science product. The PC chip (smaller
pixels) got `pixfrac = 0.8` — *footprint and pixfrac are tuned per detector sampling*,
not globally. Casertano's weighting was strictly *background-noise* inverse-variance
(*"weighting each input exposure in inverse proportion to the square of the noise per
pixel … weights that reflect only the noise due to the background, both sky and
detector, without considering the local signal due to individual objects"*) — using the
measured *signal* to set per-pixel noise *"can produce biased results"*, the same trap
flagged in §4.3. The output weight map yields the **equivalent single-pixel noise**
`σ̄_i = 1/√W_i` (Casertano Eq. 1), the practical handle on the correlated-noise problem:
on large scales the true area noise is the quadrature sum of `σ̄_i`, *not* of the
(correlation-suppressed) measured pixel RMS.

### 5.6 Correlated-noise penalty (the cost of drizzle)

The price of resampling is correlated noise (§4.3). The F&H derivation (now read in
full from the paper — see *Primary sources parsed (pass 2)*) defines the noise
correlation ratio `R = σ_c/σ_p` (Eq. 8): the ratio of the *true* block-summed noise
`σ_c` (the variance you'd get if drops only landed on the pixel under their center) to
the *measured* per-output-pixel noise `σ_p`. `R > 1` always, because pixel-to-pixel RMS
on a drizzled image **misses the off-diagonal covariance** and so underestimates the
true large-scale noise by the factor `R`. F&H derive `σ_c²` and `σ_p²` explicitly
(their Eqs. 6–7, weights enter as `w²·s⁴·σ²`, the same squared-weight propagation as
`update_data_var`):

```
σ_c² = Σ_C w²s⁴σ² / (Σ_C w)²          (drops only on center pixel)
σ_p² = Σ_P a²w²s⁴σ² / (Σ_P aw)²        (all drops overlapping the pixel)
R    = σ_c / σ_p                       (Eq. 8)
```

For a uniform dither continuously filling the output plane (`r = p/s`, with `s` the F&H
output/input pixel-size ratio; **in lumos's parameters `r = pixfrac·scale`** since
`s = 1/scale_lumos`, §5.5), the sums become integrals and F&H give the closed form
(their Eqs. 9–10):

```
r ≥ 1 :   R = r / (1 − 1/(3r))         (F&H Eq. 9)
r ≤ 1 :   R = 1 / (1 − r/3)            (F&H Eq. 10)
```

**Correction (pass 2):** the prior version of this doc wrote the `r ≤ 1` case as
`R = r/(1 − r/3)`. That is the form printed in the **DrizzlePac Handbook §3.3** (and
parroted by many secondary sources), but it is a **typo in the Handbook** — the
original F&H 2002 paper (Eq. 10, read directly from the PDF) has numerator **1**, not
`r`. The F&H form is the physically correct one: as `r → 0` (interlacing limit),
`R → 1/(1−0) = 1` (no correlated noise, exactly as F&H state — "Consider then the
situation when pixfrac, p, is set to zero. There is then no correlated noise in the
output image"), whereas the Handbook's `r/(1−r/3)` wrongly gives `R → 0`, which is
impossible since `R ≥ 1` by construction. Both forms coincide at `r = 1` (`R = 1.5`),
which is why the error is easy to miss. Use the F&H form.

Worked example from F&H (Eq. 9, `r ≥ 1` branch): `p=0.6, s=0.5 → r=1.2 →
R = 1.2/(1 − 1/3.6) = 1.2/0.7222 = 1.662`. So with those parameters the apparent
pixel-to-pixel noise is ~40% *lower* than the real aperture-scale noise.

**Correction (pass 2): the monotonicity is the opposite of what pass 1 stated.** `R(r)`
is *monotonically increasing* in `r = p/s`: it is **minimal** (`R → 1`, no correlated
noise) at `r → 0` (interlacing limit), `R = 1.5` at `r = 1`, and grows without bound for
`r > 1`. So **larger** pixfrac relative to scale (larger `r`) means *more* overlap
between drops and *more* correlated noise, not less. The pass-1 claim that "smaller
pixfrac inflates correlated noise" was backwards.

The actual drizzle tradeoff is therefore a three-way tension, not a single axis:

- **Small `r` (small pixfrac / large scale):** drops barely overlap → low correlated
  noise (`R → 1`) and the sharpest result, *but* poor coverage — output pixels receive
  few samples, the *uncorrelated* per-pixel variance is high, and with insufficient
  dither you get **holes**. This is the regime where coverage, not correlation, limits
  you.
- **Large `r` (large pixfrac / fine scale):** heavy drop overlap → uniform coverage and
  smooth-looking output, *but* high correlated noise (`R` large), so the pixel-to-pixel
  RMS badly understates the true aperture-scale noise and resolution is lost to the
  pixel re-convolution.
- The sweet spot F&H and Casertano land on is `r` slightly above 1 (`R ≈ 1.5–1.7`):
  enough overlap to fill coverage robustly, while keeping the correlated-noise penalty
  bounded and most of the resolution. Note `R` never reaches 1 for a coverage-adequate
  pixfrac — even good parameters carry `R ≈ 1.5` of correlated-noise penalty under a
  filled uniform dither, which is *why* §4.3's rule (compute noise on un-resampled
  frames) matters.

### 5.7 Dithering requirement

Drizzle's resolution recovery **requires sub-pixel dithering**: the frames must sample
the scene at different sub-pixel phases so the finer output grid is filled. F&H frame
this physically (§2): a dither is *"offset samples from the same convolved image"* —
the detector samples `Id = T⊗O⊗E` at different sub-pixel positions, and drizzle
reassembles those offset samples on the fine grid. Without dither, every frame samples
the same phase, the finer grid has empty cells (or all drops land identically), and
drizzle reduces to a noisier interpolation that *adds* correlated noise for *no*
resolution gain. F&H's title and abstract scope the method explicitly to *"undersampled,
dithered data"*, and F&H §2 cautions the drop must be *"small enough to avoid degrading
the image, but large enough so that after all images are drizzled the coverage is
reasonably uniform"* — the dither-vs-pixfrac coverage tradeoff. Crucially, F&H §1 also
warn what drizzle is **not**: it *"does not attempt to improve upon the final image
resolution by enhancing the high frequency components"* (that is *image restoration* —
Richardson-Lucy, max-entropy — which trades S/N for resolution). Drizzle is *image
reconstruction*: it recovers only the information lost to the **pixel** convolution
`P`, not information lost to the optics `O`. **Drizzling un-dithered, well-sampled
(Nyquist or oversampled) data is therefore an anti-pattern** — there is no undersampling
to undo, so you pay the correlated-noise cost and gain nothing; a plain stacked mean is
strictly better. lumos's drizzle takes per-frame `Transform`s (`drizzle_stack`,
`mod.rs:937`) and does not itself verify dither diversity — that is the caller's
responsibility.

### 5.8 Rejection in the drizzle workflow (DrizzlePac CR scheme)

Because drizzle is a *linear* coadd it cannot reject cosmic rays itself. The
median+blot+derivative scheme **originates in the F&H paper itself** (§3, "Cosmic Ray
Detection") — DrizzlePac's `drizCR` is the modern implementation. F&H's exact 7-step
recipe (paraphrasing their numbered list, §3):

1. Drizzle each image to a separate sub-sampled output using **pixfrac = 1.0**.
2. Take the **median** of the aligned drizzled images → *"a first estimate of an image
   free of cosmic-rays."*
3. **Blot** (the F&H program literally named "Blot") — map the median back to each
   input's distorted plane by interpolation.
4. Take the **spatial derivative** of each blotted image — used *"to estimate the
   degree to which errors in the computed image shift or the blurring effect of taking
   the median could have distorted the value of the blotted estimate."*
5. Compare each original to its blot: *"Where the difference is larger than can be
   explained by noise statistics, the flattening effect of taking the median or an
   error in the shift, the suspect pixel is masked."*
6. Repeat on **pixels adjacent** to already-masked ones, *"using a more stringent
   comparison criterion"* (neighbor growth).
7. Finally, drizzle the inputs onto a single output using the masks, *"For this final
   combination a smaller pixfrac than in step 1) will usually be used in order to
   maximize the resolution."*

The key insight (step 4, the derivative term) is F&H's: the threshold widens where the
blotted model is *steep* (PSF cores, edges), because median-blurring and small shift
errors there produce large legitimate differences that would otherwise be misflagged as
CRs. The DrizzlePac implementation makes this concrete:

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

lumos's drizzle has **no** built-in CR rejection or blot step — each `DrizzleFrame`
accepts an optional `pixel_weight_map` through which a caller can supply pre-computed
rejection masks, but the median+blot+derivative scheme is not
implemented (see §8).

### 5.9 When drizzle is justified vs harmful

- **Justified:** undersampled data (PSF FWHM ≲ 2 px), **good sub-pixel dither**
  diversity, many frames, geometric distortion to correct, and you need either
  resolution recovery or distortion-free photometry. In lumos's parameters the F&H ratio
  is `r = pixfrac·scale` (§5.5), and there are two defensible targets (§5.6): F&H and
  Casertano themselves used `r ≈ 1.2–1.25` (`R ≈ 1.6`) to *guarantee* uniform coverage;
  if instead you want to **minimise correlated noise** and have enough dither/frames to
  keep coverage, push `r` modestly *below* 1 (`r ≈ 0.8 → R ≈ 1.36`, which at `scale=2`
  means `pixfrac ≈ 0.4`). Either way `r` is `pixfrac·scale`, not `pixfrac/scale`.
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
- `pixfrac`: 0.7–0.9. In lumos's parameters the F&H ratio is `r = pixfrac·scale`
  (§5.5), so lumos's defaults (`scale=2, pixfrac=0.8`) already give `r = 1.6`, `R ≈ 2.0`
  — coverage-safe but with substantial correlated noise. Smaller `r` means *less*
  correlated noise (`R → 1` as `r → 0`) but worse coverage; don't push `r` so low that
  output pixels under-sample / form holes (§5.6 corrected monotonicity). F&H/Casertano
  targeted `r ≈ 1.2–1.25` (`R ≈ 1.6`); go lower only with many frames and excellent
  dither.
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
   leaves empty/under-covered output pixels (**holes**) — the dominant failure of small
   pixfrac. **Correction (pass 2):** the prior wording claimed correlated noise "blows
   up as `r → 0`"; that is backwards. By F&H Eq. 10, `R = 1/(1 − r/3) → 1` as `r → 0`
   (interlacing has the *least* correlated noise). The real cost of small pixfrac is
   *coverage*, not correlation: too few independent samples land on each output pixel, so
   it's the *uncorrelated* per-pixel variance (and holes) that hurt, while `R` actually
   *decreases* toward 1. Increase pixfrac (toward `r ≈ 1`, `R ≈ 1.5`) or add dither
   diversity. (Note: `R` is **monotonically increasing in `r`** — minimal at `r → 0`
   where `R → 1`, `R = 1.5` at `r = 1`, and growing without bound for `r > 1`.)
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
    threshold and over-rejects.

---

## 8. How lumos currently does it — and gaps/opportunities

**What lumos has (correct and well-grounded):**

- Full rejection family `None | SigmaClip | Winsorized | LinearFit | Percentile |
  Gesd` (`src/stacking/rejection.rs:831`), matching the siril set, with
  presets `sigma_clipped / winsorized / linear_fit / median / mean / gesd / percentile
  / weighted` (`config.rs:98`) and frame presets `light/flat/dark/bias` (`config.rs:164`).
- Pre-warp lowest-MAD reference selection, paired Global (additive+multiplicative) and
  common-domain Multiplicative normalization, source-noise × interpolation-confidence
  inverse-variance weighting, and equal/manual weighting.
- Winsorized σ with the correct 1.134 bias factor (`rejection.rs:203`); textbook
  two-sided GESD with mean/sample-sd statistics, accurate Student-t critical values,
  and a backward scan (`rejection.rs`); MAD-based robust spread in the clipping and
  frame-statistics paths.
- Drizzle: all five kernels, exact `boxer` polygon clipping for Square, Jacobian
  weight conservation, deferred weighted-average `accumulate`/`finalize`, coverage map
  with `min_coverage` masking, Lanczos **warned** (not blocked — it still runs) off
  pixfrac=scale=1 with negative-lobe clamping (`drizzle/mod.rs:271`, `:669`).
- Memory-tiered stacking caches (in-memory vs mmap, `combine/cache/loader/mod.rs`) so large stacks
  don't OOM. `LightCache` computes registered-frame `FrameStats` (median + MAD) sequentially with
  one reused scratch buffer over the shared valid-support mask, identically for RAM and mmap planes.

**Gaps / opportunities (ranked):**

1. **No data-dependent variance propagation.** STScI's `update_data_var`
   (`cdrizzlebox.c:91`) propagates input variance with squared weights. lumos returns
   a conditional linear factor `Σwᵢ²/(Σwᵢ)²`, but does not accept a distinct variance model for
   each input frame or pixel. Median output has no linear factor.
2. **No drizzle CR rejection (median + blot + derivative).** lumos relies on
   caller-supplied `DrizzleFrame::pixel_weight_map` values. Implementing the DrizzlePac `drizCR` scheme
   (`drizzlepac/drizCR.py`) — drizzle→median→blot→derivative-thresholded mask — would
   make the drizzle path self-contained. This also needs a **blot** (inverse-drizzle)
   operation, which lumos lacks (STScI: `cdrizzleblot.c`).
3. **Normalization is scalar-per-channel only; no surface/gradient background
   matching.** Fine for uniform pedestal shifts, insufficient for mosaics or strong
   gradients. A low-order plane/poly background match (cf. `reproject`'s
   `mosaicking/background.py`, SWarp's mesh background) would be needed for mosaics.
4. **No FWHM / star-count quality weighting.** lumos has only Equal/Noise/Manual;
   siril and PixInsight additionally weight by seeing/star-count, which catches bloated
   frames that the background-noise estimate alone would not.
5. **Drizzle does not validate dither diversity.** It will happily drizzle un-dithered
   data and produce a correlated-noise-inflated result with no warning (§5.7).
6. **Drizzle output omits F&H's `s²` flux factor (pass 3).** `accumulate`/`finalize`
   compute `Σ(a·w·flux)/Σ(a·w)` with no `s²` term (`mod.rs:603`/`:667`), so the output
   preserves the input per-pixel DN scale (flat field DN `F` → `F`) rather than F&H
   Eq. 5's surface-intensity normalization (STScI folds `s² = scale²` into the flux via
   `iscale`, `cdrizzlebox.c:640`). `s²` is a single global constant, so SNR and
   *relative* photometry are unaffected and the choice arguably makes drizzle output
   comparable to the ordinary stack; but absolute integrated counts scale by `scale²`
   and the output is **not** in F&H/STScI flux units — note it for cross-pipeline
   photometry (§5.3). This is a documented behavioural difference, not necessarily a bug.

---

## Primary sources parsed (pass 2)

PDFs/pages read directly this pass (saved under `.tmp/papers/`), with the load-bearing
takeaway each resolved:

- **Fruchter & Hook 2002, *Drizzle* (PASP 114:144)** —
  `.tmp/papers/fruchter_hook_2002_drizzle.txt` (the previously-garbled PDF, now fully
  legible). **Resolved:** the verbatim accumulation equations (Eqs. 2–5: `W' = a·w + W`;
  `I' = (d·a·w·s² + I·W)/W'`; with `s²` only in the **flux** numerator, never in the
  weight), the correlated-noise derivation (Eqs. 6–8, weights enter as `w²s⁴σ²`;
  `R = σ_c/σ_p`), and the closed-form `R` (Eqs. 9–10). **Correction it forced:** the
  `r ≤ 1` branch is `R = 1/(1 − r/3)` (numerator 1), *not* `r/(1 − r/3)` — see §5.6. Also
  resolved: the interlacing↔shift-and-add limits, the photometry numbers (0.004 / ≤0.015
  mag), and the 7-step median+blot+derivative CR recipe (which *originates here*, not in
  DrizzlePac) — §§5.2, 5.3, 5.5, 5.7, 5.8.
- **Casertano et al. 2000, *WFPC2 Observations of the HDF-S* (AJ 120:2747)** —
  arXiv `astro-ph/0010245` → `.tmp/papers/casertano_2000_hdfs.txt`. **Added:** real-world
  drizzle parameters (`scale = 0.4` WF px, `pixfrac = 0.5` WF px → `r = 1.25`; PC chip
  `pixfrac = 0.8`; coarse 60 mas intermediate for CR, fine 40 mas for science),
  background-only inverse-variance weighting (signal-based weighting "can produce biased
  results"), and the equivalent single-pixel noise `σ̄ = 1/√W` from the output weight map
  — §5.5. (The first download under the task's `astro-ph/0004178` ID was the *wrong*
  paper — a CMB `Pseudo-Cl` paper; the correct HDF-S preprint was found via the arXiv API
  and is the one cited here.)
- **DrizzlePac Handbook §3.3, *Weight Maps and Correlated Noise*** (HST docs, web) →
  `.tmp/papers/drizzlepac_handbook_3_3.html`. Confirms the `R = σ_c/σ_p` framing and the
  `r=1.2 → R=1.662` example, **but** prints the `r ≤ 1` branch as `r/(1 − r/3)` — the
  typo this doc previously inherited; F&H's original is authoritative (§5.6).
- **NIST e-Handbook §1.3.5.17.3, *Generalized ESD Test*** (web) →
  `.tmp/papers/nist_gesd.html`. **Resolved** the exact λ critical-value formula
  `λ_i = (n−i)·t_{p,n−i−1} / √((n−i−1+t²)(n−i+1))`, `p = 1 − α/(2(n−i+1))`, and reconciled
  it index-for-index with siril's decrementing-`size` form (§3.5). Confirms masking
  robustness (3 outliers vs Grubbs' 0) and the `n ≥ 15`/`n ≥ 25` accuracy bounds.

**Re-verified in cloned source (cite file:line):** siril `rejection_float.c`
Winsorized `1.5f / 1.134f / 0.0005f` (`:230-237`), `median_and_mean.c` GESD t-critical
`gsl_cdf_tdist_Pinv(1 − sig/(2·size), size−2)` then `λ = (size−1)t/√(size(size−2+t²))`
(`:1481-1484`), noise weights `1/(pscale²·bgnoise²)` (`:1122`), wFWHM/nbstars weights
(`:1136`/`:1183`); cdrizzle `update_data` running weighted average (`:32`),
`update_data_var` squared-weight variance `(var·vc²+dow²·d2)/vc_plus_dow²` (`:135`),
`boxer`/`sgarea` (`:280`/`:174`), `over()` rect-intersect (`:460`), square-kernel
Jacobian `0.5((x₁−x₃)(y₀−y₂)−(x₀−x₂)(y₁−y₃))` with `w = weights·weight_scale/jaco`
(`:1376`/`:1393`), Gaussian `pfo` clamp `≥1.2/pscale_ratio` (`:1576`), Lanczos pixfrac
warning (`:1701`); DrizzlePac `drizCR.py` threshold `t2 = mult·blot_deriv + snr·ta/gain`,
defaults `3.5 3.0` / `1.2 0.7`, `tmp2 ≥ 9` (`:320-336`). **lumos confirmed:**
the GESD statistic, live-count λ formula, and exact Student-t quantile now match the
NIST/Siril contract; Winsorized constants
`HUBER_C=1.5 / WINSORIZED_CORRECTION=1.134` (`rejection.rs:201-203`); lowest-mean-MAD
reference; paired registered-frame normalization; source-domain inverse-variance noise weighting
with interpolation confidence applied once;
drizzle `accumulate`/`finalize` deferred weighted average (`mod.rs`).

**Resolved in pass 3 (were "open"):** the **1.134** factor is now nailed by direct
numerical integration — `E[ψ_{1.5}(z)²] = 0.47783 + 0.30063 = 0.77846`, so
`1/√0.77846 = 1.13339 ≈ 1.134`, exactly siril/lumos's constant (§3.2); no statistics
reference printing the literal "1.134" is needed, the integral pins it. The small-N
median efficiency is likewise computed exactly: *variance* efficiency `0.743` at N=3
(SD `0.862`), `0.697`/`0.835` at N=5, *decreasing* to the asymptotic `2/π = 0.637`
(SD `0.798`) — which **overturned** the pass-1/2 claim that the penalty is "worse at
small N, approached from below" (§2.1; the limit is approached from *above*, small N is
*more* efficient).

## Verification & corrections (pass 3)

A third pass re-checked every load-bearing claim manually against lumos source and the
cloned references, and ran the two numerical integrations above. New corrections
(marked `**Correction (pass 3):**` inline):

| § | Pass-1/2 said | Pass-3 corrected to |
|---|---------------|---------------------|
| 2.1 | median penalty "worse at small N (~0.74), approached from below" | RE approached **from above**; small N is *more* efficient (N=3: var 0.743 / SD 0.862 vs asymptotic 0.637 / 0.798); the `0.74` is the *variance* efficiency at N=3 |
| 3.5 / 8 | lumos GESD differed from siril only by normal-vs-t | It also used `n − 2i` instead of the live count `n − i` and a median+MAD statistic. All three divergences are now resolved by the textbook implementation. |
| 5.1 / 5.5 / 5.6 | "`s = scale`", "`s=2` (output pixel = ½ input)", "`r = pixfrac/scale`" | lumos's `scale = 1/s_F&H`; for lumos parameters **`r = pixfrac·scale`** (= `drop_size` in output px). `s=0.5` (not 2) gives a half-size output pixel |
| 5.5 / 6 | (implicit) lumos defaults are coverage-/noise-balanced | lumos defaults `scale=2, pixfrac=0.8` give `r = 1.6`, `R ≈ 2.0` (x3 → `r=2.1`, `R ≈ 2.5`) — **high** correlated noise, above the F&H/Casertano `r ≈ 1.2` sweet spot |
| 5.3 / 8 | drizzle "total flux preserved … same as Eqs. 4–5" | lumos **omits F&H's global `s²` flux factor** (no `iscale`), preserving the input per-pixel DN scale instead; integrated counts scale by `scale²` (benign: a global constant) |
| 5.4 / 8 | lumos "explicitly forbids" Lanczos off pixfrac=scale=1 | lumos only **warns** and runs anyway (`mod.rs:271-281`); also Lanczos-3-only, kernel normalised to sum 1, and lumos's Point kernel *does* apply the Jacobian |

Citation fixes (line numbers drifted or pointed at the wrong file): `enum Rejection`
`config.rs:831` → `rejection.rs:831`; `SigmaClipConfig` `config.rs:20` →
`rejection.rs:20`; GESD `reject` `:716` → `:668`; `max_outliers` `N/4` `rejection.rs:42`
→ `:657`; Winsorized const `rejection.rs:202` → `:203`; `drizzle_stack` `mod.rs:876` →
`:937` (twice); presets `config.rs:71` → `:98`.

**Re-verified as correct (no change needed):** the headline pass-2 correlated-noise
correction (F&H Eq. 10 numerator is **1**; the DrizzlePac Handbook §3.3 prints
`r/(1−r/3)` — confirmed *both* against `fruchter_hook_2002_drizzle.txt` line 560 and
`drizzlepac_handbook_3_3.html` line 791); F&H Eqs. 2–5 with `s²` only in the flux
numerator (paper lines 154-176); the `R(r)` monotonicity and `r=1.2 → R=1.662` worked
example; the 0.004 / ≤0.015 mag photometry numbers (lines 314-344); the 7-step
median+blot+derivative CR recipe (lines 233-253); Casertano's HDF-S parameters
(`scale 0.4`/`pixfrac 0.5` → `r=1.25`, PC `0.8`, 60 mas CR / 40 mas science) and
background-only inverse-variance weighting; siril's normalization modes / `poffset =
pscale·offset − offset0` / noise & wFWHM & nbstars weights / Winsorized
`1.5f/1.134f/0.0005f` / GESD `gsl_cdf_tdist_Pinv` λ; DSS `KappaSigmaClip` /
`MedianKappaSigmaClip`; DrizzlePac `drizCR.py` `t1/ta/t2` thresholds and `3.5 3.0` /
`1.2 0.7` defaults; STScI `update_data` / `update_data_var` / `jaco` / `boxer` / `sgarea`
/ `iscale = scale²`.

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
- lumos — `src/stacking/combine/{config.rs, stack.rs, rejection.rs, cache/}`,
  `src/drizzle/mod.rs`.

### Online sources

- Fruchter & Hook 2002, *Drizzle: A Method for the Linear Reconstruction of
  Undersampled Images*, PASP 114:144 — https://arxiv.org/abs/astro-ph/9808087 ,
  https://iopscience.iop.org/article/10.1086/338393 . **Parsed in full this pass** →
  `.tmp/papers/fruchter_hook_2002_drizzle.txt` (Eqs. 2–10 quoted in §§5.2–5.8).
- Casertano et al. 2000, *WFPC2 Observations of the Hubble Deep Field-South*, AJ 120:2747
  — https://arxiv.org/abs/astro-ph/0010245 . **Parsed this pass** →
  `.tmp/papers/casertano_2000_hdfs.txt` (real-world drizzle parameters, background-noise
  weighting, `σ̄ = 1/√W`; §5.5).
- STScI, *Dithering and the Drizzle Algorithm* —
  https://www.stsci.edu/ftp/science/hdf/combination/drizzle.html
- STScI HST docs, *Weight Maps and Correlated Noise* (the `R` formula — note its `r ≤ 1`
  branch `r/(1−r/3)` is a typo; F&H's `1/(1−r/3)` is correct, §5.6) —
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
- Finite-sample (small-N) median efficiency approached *from above* (var-eff `0.743`
  at N=3 ↓ `2/π`) — A. Akinshin, *Understanding the pitfalls of preferring the median*
  https://aakinshin.net/posts/median-vs-mean/ , cross-checked by direct numerical
  integration of the order-statistic variance (pass 3, §2.1).
- PixInsight rejection-by-frame-count guidance (Linear Fit >25, Winsorized 15–25,
  Averaged Sigma 9–15, Percentile 5–9) —
  https://www.astropixelprocessor.com/community/faq/when-to-use-which-outlier-rejection-filter/ ,
  https://chaoticnebula.com/pixinsight-image-integration/
