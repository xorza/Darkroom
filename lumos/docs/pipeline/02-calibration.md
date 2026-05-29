# Stage 2 — Calibration: Best Practices & Algorithms

## Scope & Goal

Calibration is the second stage of the astrophotography pipeline (after decode/ingest,
before star detection / registration / stacking). Its job is to convert a *raw sensor
readout* — which carries the instrument's additive and multiplicative signatures on top
of the sky signal — into a *linear estimate of the light that actually fell on each
pixel*, with sensor artifacts removed. Concretely it must:

1. Remove **additive** instrument signal: bias/offset (read-out pedestal + fixed-pattern
   read noise) and dark current (thermally generated electrons + amp glow).
2. Remove **multiplicative** instrument signal: the flat field (vignetting, optical
   throughput variation, dust-mote shadows, pixel-to-pixel quantum-efficiency variation).
3. Repair **deterministic defects**: hot/cold/dead pixels and bad columns that no
   amount of averaging can fix because they carry no real signal.
4. Optionally reject **transient outliers** (cosmic rays / satellite/airplane streaks)
   that survive into a single frame.

The non-negotiable invariant across all of this is **linearity**: every operation must
keep pixel value proportional to incident photons (plus a known constant), because every
*later* stage — flux-weighted centroids, noise-weighted stacking, photometry, drizzle —
assumes a linear scale. Calibration that subtracts the wrong constant, divides by a
mis-normalized flat, or clamps negative pixels to zero quietly destroys that invariant
and biases everything downstream.

This document separates **best practice** (what the authoritative implementations do and
agree on) from **anti-patterns** (documented failure modes). Source code is cited by path
into the cloned reference repos under `.tmp/refs/`; online claims are cited by URL and
cross-checked against at least two independent sources.

---

## 1. The calibration equation

### 1.1 The canonical form

Every reference reduces to the same per-pixel equation. With `L` the raw light frame,
`B` master bias, `D` master dark, `F` master flat, `FD` flat-dark (a dark taken at the
flat's exposure), and `m` a scalar normalization of the flat:

```
                 L − D                              L − (B + k·Dc)
calibrated  =  ───────── · m       (full form:  ─────────────────── · m)
                 F − FD                              (F − FD)
```

The DeepSkyStacker theory page states it as
`Light = (RawLight − Offset − DarkFactor·(Dark − Offset)) / (Flat − Offset)`
where `Offset` is the bias, `Dark − Offset` is the bias-free dark current, and
`DarkFactor` is the dark-scaling coefficient (see §2.4). ccdproc's `ccd_process`
(`$HOME/Projects/Scenarium/lumos/.tmp/refs/ccdproc/ccdproc/core.py:138`) applies
the identical sequence as discrete steps: overscan → trim → gain → bias → dark → flat
(lines 294–375). Siril's `preprocess()`
(`$HOME/Projects/Scenarium/lumos/.tmp/refs/siril/src/core/preprocess.c:124`)
does bias-subtract → dark-subtract → flat-divide in that exact order (lines 127–156).

The decomposition into bias `B` and scaled dark current `k·Dc` only matters when you
*scale* the dark (§2.4). When the dark is used 1:1 (matched exposure/temperature/gain),
the dark already contains the bias, and you subtract `D` in a single step — subtracting
bias *and* an unscaled `D` would remove the bias twice. ccdproc's flat-calibration guide
makes this explicit: *"If the combined dark frame needs to be scaled to a different
exposure time, then bias and dark must be handled separately; otherwise, the dark and
bias can be removed in a single step because dark frames also include bias."*
(https://www.astropy.org/ccd-reduction-and-photometry-guide/v/dev/notebooks/05-03-Calibrating-the-flats.html)

### 1.2 Why this order

The order is forced by the *physics of how the signals combine*. The sensor readout is

```
L = (sky·QE·throughput·vignette  +  dark_current·t  +  bias)   [+ shot/read noise]
```

Bias and dark are **additive** and must be removed by **subtraction**; the flat is
**multiplicative** and must be removed by **division**. You cannot divide before
subtracting, because the additive pedestal is *not* scaled by the flat — dividing first
would spread the bias/dark pedestal through the (non-flat) flat field and imprint
vignetting onto what should be a constant offset. Subtract the additive terms first to
get a clean `sky·(multiplicative)`, *then* divide out the multiplicative term. This is
the "additive-before-multiplicative" rule and every reference enforces it
(opticalmechanics, celestron, astropy guide — three independent sources agree).

The same logic applies *inside* the flat: the flat frame itself contains bias + (small)
dark current, which are additive and must be removed from `F` **before** `F` is used as a
divisor — hence `F − FD` (or `F − B`). If you divide lights by a flat that still has its
own pedestal in it, you inject the flat's bias signal into your data
(opticalmechanics; astropy guide). This is exactly why lumos passes a `flat_sub`
(`master_flat_dark` with priority over `master_bias`) into `divide_by_normalized`
(`$HOME/Projects/Scenarium/lumos/src/calibration_masters/mod.rs:179`).

### 1.3 Linearity and the pedestal problem

After dark subtraction, background pixels straddle zero: dark-frame shot noise means a
sky-free pixel is `0 ± σ`, so roughly half its noise excursions are **negative**. These
negative values are *real information*. Clamping them to zero biases the mean upward and
corrupts later noise-weighted statistics. lumos handles this correctly: `CfaImage::subtract`
deliberately keeps negatives — *"Clamping to zero would introduce a positive bias in the
stacked result"* (`$HOME/Projects/Scenarium/lumos/src/astro_image/cfa.rs:142`).

The classic workaround in integer pipelines (MaxIm DL, some CCD software) is a **pedestal**:
add a fixed constant (commonly 100 ADU) after subtraction so values stay non-negative,
and account for it at every later step
(verified: grokipedia "Dark-frame subtraction"; AAVSO DSLR photometry guide). A float
pipeline like lumos (`f32`, normalized `[0,1]`) needs no pedestal because it represents
negatives natively — the better solution. The pedestal is only an integer-storage hack.

---

## 2. Master frame creation

A master frame exists to make calibration **noise-free relative to the lights**. If your
master adds its own noise, you are trading one noise source for another. The widely cited
rule of thumb: with too few subs you can *triple* the post-calibration noise versus the
uncalibrated light, requiring ~9× more lights to recover — so masters must be built from
enough subs that their noise is negligible
(opticalmechanics "Mastering Calibration Frames"; corroborated by practicalastrophotography).

### 2.1 How many subs, and which combine method

Consensus across sources (opticalmechanics, celestron, practicalastrophotography,
chaoticnebula):

| Frame   | Typical count | Combine method                         | Notes |
|---------|---------------|----------------------------------------|-------|
| Bias    | 50–100 (100+) | average + outlier rejection, or median | cheap (0 s); take many |
| Dark    | 20–50         | average + sigma/winsorized clip, or median | must NOT clip amp-glow region as outlier |
| Flat    | 20–50 per filter | average + sigma clip after per-frame normalization | normalize each sub before combining |
| Flat-dark | 20–50       | same as dark                           | matched to flat exposure |

Sigma-clip threshold for *combining calibration subs* is typically **κ ≈ 3.0** high
(starfieldview "Sigma Clipping"; opticalmechanics). **Winsorized** sigma clipping (replace
outliers with the clip boundary rather than discarding) is recommended because it rejects
outliers without throwing away the pixel's statistical weight — useful for the modest sub
counts typical of calibration. DSS additionally offers median, kappa-sigma, auto-adaptive
weighted average, and entropy-weighted average
(http://deepskystacker.free.fr/english/technical.htm via search).

**Why average-with-rejection beats plain median for masters:** the median of N frames has
~1.25× the variance of the mean (for Gaussian noise), so a sigma-clipped/winsorized *mean*
keeps more SNR while still rejecting cosmic rays and the occasional satellite. Median is the
robust fallback when N is too small for rejection statistics to be reliable. lumos encodes
exactly this heuristic: `stack_cfa_frames` uses the frame-type preset (winsorized mean for
darks/bias at σ=3.0, sigma-clip mean for flats at σ=2.5) when N ≥ 8, and falls back to
median below 8 frames
(`$HOME/Projects/Scenarium/lumos/src/calibration_masters/mod.rs:92`;
presets at `$HOME/Projects/Scenarium/lumos/src/stacking/config.rs:163`).

ccdproc's `Combiner` exposes the same toolbox: `average_combine`, `median_combine`,
`sum_combine`, plus `sigma_clipping`, `minmax_clipping`, and `clip_extrema` masking, and
supports per-frame `weights`
(`$HOME/Projects/Scenarium/lumos/.tmp/refs/ccdproc/ccdproc/combiner.py:298–740`).

### 2.2 Bias vs superbias

A **master bias** is the average of many zero-(or minimum-)exposure frames; it captures the
read-out pedestal and the fixed-pattern read noise. Because bias subs are free to take,
take 100+ — the master bias should be far quieter than anything it calibrates. A
**"superbias"** (PixInsight term) is a *modeled/smoothed* master bias: the master bias is
decomposed (e.g. multiscale/wavelet) and the random component discarded, keeping only the
deterministic fixed pattern. This yields an essentially noise-free bias model. lumos has no
superbias modeling — its bias master is a straight winsorized mean (a reasonable v1).

### 2.3 When each frame type is actually needed

- **Bias**: needed if (a) you scale darks (§2.4) — then bias must be removed separately, or
  (b) you calibrate flats with bias instead of flat-darks, or (c) you synthesize a master
  dark for a different exposure. **Caveat:** many modern CMOS sensors (e.g. ASI294,
  IMX571/455 families) *cannot produce a reliable bias frame* — their shortest exposures are
  non-linear / the bias drifts, so a "bias" frame makes calibration *worse*
  (britastro CMOS bias thread; astroworldcreations Part 3; cloudynights). For these, **skip
  bias entirely** and keep the bias signal inside matched darks and flat-darks.
- **Dark**: removes dark current and (critically) **amp glow**. Needed for any exposure long
  enough that thermal signal or amp glow is significant. Must match the lights' **exposure,
  gain/ISO, and temperature** (sensor temperature for CMOS, ideally set-point cooled).
- **Flat**: removes vignetting, dust shadows, and pixel-QE variation. Needed essentially
  always for deep-sky; it is the frame that most visibly improves images (flat field is what
  makes a gradient-free, dust-free background possible).
- **Flat-dark** (a.k.a. dark-flat): a dark matched to the **flat's** exposure time. The
  modern best practice for CMOS — subtract a flat-dark from the flat instead of a bias,
  because the flat-dark captures the (non-linear, drifting) short-exposure pedestal that a
  bias frame cannot. Switching bias→flat-dark is the documented fix for **residual rings**
  after flat correction on CMOS with unstable bias
  (astroworldcreations Part 3; cloudynights; mypetstars). lumos models this directly:
  `flat_darks` is its own role and takes **priority over bias** when normalizing the flat
  (`$HOME/Projects/Scenarium/lumos/src/calibration_masters/mod.rs:181`).

### 2.4 Dark scaling / dark optimization — and when it fails

**Dark scaling** multiplies the bias-subtracted master dark by a factor `k` before
subtraction, to compensate when the dark's exposure or temperature does not exactly match
the light. Two ways to compute `k`:

1. **Exposure ratio** (the linear, physically-motivated factor): `k = t_light / t_dark`.
   Siril's `darkOptimization` uses this when `use_exposure` is set (and warns if the dark is
   *shorter* than the lights)
   (`$HOME/Projects/Scenarium/lumos/.tmp/refs/siril/src/core/preprocess.c:178–198`).
   ccdproc's `subtract_dark(scale=True)` computes
   `scale_factor = data_exposure / dark_exposure` and multiplies the master before
   subtracting
   (`$HOME/Projects/Scenarium/lumos/.tmp/refs/ccdproc/ccdproc/core.py:843–856`).

2. **Noise/entropy minimization** (the empirical "dark optimization"): search `k` to
   minimize the noise (or entropy) of the *calibrated* background. Siril does a
   golden-section search over `k ∈ [0, 2]` minimizing background noise
   (`preprocess.c:88–122, 195–198`). DeepSkyStacker searches `k ∈ [0, 5]` in 0.01 steps
   minimizing either Shannon **entropy** (`ComputeMinimumEntropyFactor`) or RMS
   (`ComputeMinimumRMSFactor`), per color channel for OSC
   (`$HOME/Projects/Scenarium/lumos/.tmp/refs/DeepSkyStacker/DeepSkyStackerKernel/DarkFrame.cpp:127–265, 574–791`).

**Mandatory precondition:** dark scaling is only valid on the **bias-free** dark
(`Dark − Offset`). The bias pedestal does *not* scale with exposure; scaling a dark that
still contains bias multiplies the pedestal by `k` and leaves a residual offset. This is why
ccdproc *forces* separate bias handling when `scale=True`, and why DSS works on `Dark − Offset`.

**When dark scaling fails (anti-pattern territory):**
- **Amp glow is non-linear.** Amp-glow brightness does not scale linearly with exposure or
  temperature, so a single multiplicative `k` cannot simultaneously match the dark-current
  pedestal *and* the glow. Scaling either over- or under-corrects the glow, leaving a
  residual glow gradient. *"amp glow does not behave linearly so it's always important to not
  use dark scaling if your sensor has significant amp glow"*
  (telescope.live "Learning About Amp Glow"; cloudynights "Darks - why can't I correct amp
  glow"; astropixelprocessor forum — three sources).
- **CMOS pattern/telegraph noise** doesn't scale cleanly either (cloudynights IMX455/571).
- **Over-correction** from scaling produces dark mottle/holes where the scaled dark exceeded
  the light's true dark signal (cloudynights "Darks over-correcting").

**Best practice:** match darks to lights in exposure/gain/temperature and use `k = 1` (no
scaling). Reserve scaling for well-behaved, glow-free CCDs where exact matching is
impractical. **Disable dark optimization when the sensor has amp glow** (universal advice).
lumos currently does **not** scale darks at all (always `k = 1`), which is the safe default
but means it cannot reuse a dark library across exposures — see §7.

### 2.5 Flat illumination & normalization

**Illumination requirements.** A flat must be (a) **uniform** — sky flats (twilight), an EL
panel, or a T-shirt over the OTA — and (b) exposed into the sensor's **linear regime**:
roughly 1/3–1/2 of full well (ADU ~20k–35k for a 16-bit sensor), never near saturation and
never so dim that read noise dominates. PixInsight notes that with *"sufficiently exposed
flat frames, no clipping will arise, so the application of an output pedestal is never needed
in the calibration of flat frames"*
(https://www.pixinsight.com/tutorials/master-frames/ via search). Each flat must use the
**same optical train, focus, and camera rotation** as the lights (dust shadows move
otherwise), and a fresh flat set is required per filter.

**Normalization — divide by the mean.** Before a flat can be a divisor it is normalized to
unity mean (or median): `F_norm = (F − FD) / mean(F − FD)`. ccdproc's `flat_correct`
normalizes the (optionally min-clipped) flat by its mean (or a supplied `norm_value`) and
then divides
(`$HOME/Projects/Scenarium/lumos/.tmp/refs/ccdproc/ccdproc/core.py:927–1013`).
Siril computes the normalization as the **mean of a central region** of the flat
(`startx = width/3 … `, to avoid the vignetted edges biasing the mean) and divides by it via
`siril_fdiv`
(`$HOME/Projects/Scenarium/lumos/.tmp/refs/siril/src/core/preprocess.c:332–366`).
Dividing by the mean keeps the calibrated light at the same overall flux scale (the flat
only *reshapes*, it does not rescale brightness). ccdproc also exposes `min_value` to floor
the flat and prevent division by near-zero in deeply vignetted corners (core.py:970–975) —
lumos guards the same case by only dividing where `norm_flat > f32::EPSILON`
(`$HOME/Projects/Scenarium/lumos/src/astro_image/cfa.rs:232,243`).

**Per-channel normalization for OSC / CFA flats — the color-shift fix.** A flat taken with a
non-white source (LED panel, twilight) has *different mean levels in R, G, and B*. If you
normalize an OSC flat by a single global mean, division rescales the channels unequally and
**shifts the color** of every calibrated light. The fix is to normalize **each CFA color
channel by its own mean** (equivalently, compute per-channel scaling factors). This is now
standard:
- **PixInsight** added *"Separate CFA flat scaling factors"* in ImageCalibration 1.8.8-6,
  computing 3 CFA scaling factors so *"the color shift that occurred with flat field
  correction in previous versions is avoided"*
  (Landmann ImageCalibration guide; pixinsight.com — verified).
- **Siril** `compute_grey_flat` computes per-color channel means over the CFA pattern and
  applies `coeff1 = R̄/Ḡ`, `coeff2 = B̄/Ḡ` to equalize the flat before division
  (`$HOME/Projects/Scenarium/lumos/.tmp/refs/siril/src/core/siril.c:453–514`),
  gated by the `equalize_cfa` option (`preprocess.c:329`).
- **DeepSkyStacker** computes per-channel (R/G/B) dark factors and flat handling.
- **lumos** does this natively: `divide_by_normalized_cfa` accumulates independent R/G/B
  sums and divides each color by its own mean
  (`$HOME/Projects/Scenarium/lumos/src/astro_image/cfa.rs:253–289`), with the
  rationale documented at lines 166–168.

Note the subtle policy difference: lumos/PixInsight normalize each channel to its **own**
mean (color balance deferred to a later white-balance/PCC step), whereas Siril's
`compute_grey_flat` equalizes the channels **relative to green** (preserving the flat's
green-referenced balance). Both avoid the global-mean color shift; they differ only in
which neutral point they pick.

### 2.6 Overscan / bias drift

True CCDs have a physical **overscan** region (non-illuminated readout columns) used to track
the bias level *per frame*, absorbing slow bias drift between exposures. ccdproc supports
this with `subtract_overscan` (median or modeled per row/column) and `trim_image`
(`$HOME/Projects/Scenarium/lumos/.tmp/refs/ccdproc/ccdproc/core.py:486` and
`ccd_process` lines 294–315). Consumer CMOS/DSLR sensors expose no overscan; the bias-drift
problem there is handled either by frequent bias re-takes or — better — by folding bias into
matched darks/flat-darks (§2.3). lumos has no overscan support (irrelevant for its RAW/CMOS
target audience, but a gap for scientific CCD data).

---

## 3. Defect / hot-pixel maps

Hot, cold, and dead pixels and bad columns are **deterministic** sensor defects: they read a
wrong value regardless of incident light, so averaging cannot fix them and they must be
*detected* and *replaced*. The master dark is the natural detector (hot pixels are obvious in
a dark), though a defect map can also be built from a master flat (dead pixels) or supplied
externally.

### 3.1 Detection: robust thresholds via MAD

The robust recipe — used by lumos and structurally identical to Siril's cosmetic
correction — is:

1. Compute the **median** of the (per-color) pixel population.
2. Compute **MAD** (median absolute deviation): `MAD = median(|xᵢ − median|)`.
3. Convert to a robust sigma: `σ ≈ 1.4826 · MAD` (the constant is `1/Φ⁻¹(0.75)`; for a
   normal distribution MAD ≈ 0.6745·σ).
4. Flag **hot** if `x > median + k·σ`, **cold** if `x < median − k·σ`.

**Why MAD, not standard deviation:** std is itself inflated by the very outliers you are
hunting (a few 60000-ADU hot pixels balloon σ and hide the merely-warm ones). MAD has a 50%
breakdown point — up to half the pixels can be outliers and the median/MAD stay valid. lumos
documents this rationale precisely
(`$HOME/Projects/Scenarium/lumos/src/calibration_masters/defect_map.rs:7–17`).
Note: Siril's `find_deviant_pixels` uses **mean ± k·σ** with ordinary statistics
(`$HOME/Projects/Scenarium/lumos/.tmp/refs/siril/src/filters/cosmetic_correction.c:200–214`)
and DSS uses **median + 16·σ** for hot pixels (a deliberately loose threshold so only true
hot pixels are caught; per DSS docs via search). So the *threshold statistic* varies by
implementation — lumos's MAD-based estimator is the more robust choice; the *threshold value*
(σ multiplier) is a tuning knob.

**Per-CFA-color statistics.** On raw CFA data, the green pixels (50% of a Bayer frame) sit at
a different baseline than red/blue. Computing one global median/MAD lets green dominate and
masks defects in red/blue. lumos computes **per-color** thresholds so a hot red pixel is
tested only against other red pixels
(`defect_map.rs:60–92, 163–206`; tests `test_per_channel_detection_bayer` and
`test_per_channel_detection_cold_in_blue` verify a hot R / dead B pixel is caught when a
global threshold would miss it).

**Sigma-floor guard.** A pristine, near-uniform dark has MAD ≈ 0, which would make *any*
slightly-warm pixel exceed `k·σ` and flag 3%+ of the sensor. lumos floors σ with
`σ = max(1.4826·MAD, median·0.1, 5e-4)` — the absolute floor (`5e-4` ≈ 33 ADU in 16-bit
space) prevents flagging the continuous warm tail of clean CMOS darks
(`defect_map.rs:186–195`). This is a real, non-obvious robustness fix worth keeping.

**Adaptive sampling.** Exact median over a 60-megapixel channel is slow; lumos subsamples to
100K pixels per color when the channel exceeds 200K, citing <0.5% median error with >99%
confidence by the order-statistics CLT (`defect_map.rs:24–27, 145–146, 211–250`). Sound for
detection thresholds (you only need an accurate *distribution estimate*, not every pixel).

### 3.2 Correction: same-CFA-color neighbor median

A defect is replaced by a **robust local estimate**, never by zero or by a raw neighbor of the
wrong color:

- **Mono:** median of the 8-connected neighbors (lumos `median_of_neighbors_raw`,
  `defect_map.rs:253–285`).
- **Bayer:** median of **same-color** neighbors at **stride 2** (the nearest pixels behind the
  *same* color filter). Replacing a hot red pixel with adjacent green/blue values would inject
  a false color into the demosaic. lumos `bayer_same_color_median` uses the 8 stride-2
  neighbors (`defect_map.rs:307–334`); Siril's cosmetic median uses `step = 2` for CFA data,
  same idea (`cosmetic_correction.c:46, 73–74, 131–132`).
- **X-Trans:** lumos searches a radius-6 (one full 6×6 period) window for same-color pixels,
  sorts by Manhattan distance, and medians the closest 24 to avoid directional bias
  (`defect_map.rs:340–383`). (Siril explicitly refuses cosmetic correction on X-Trans —
  `preprocess.c:388–390` — so lumos is *more* capable here.)

Using the **median** (not mean) of neighbors is important: a defect often clusters with other
defects or sits next to a star, and the median resists those.

### 3.3 Cold / dead pixels and bad columns

Cold/dead pixels (stuck low or zero) are detected by the *lower* threshold (`median − k·σ`,
floored at 0) and corrected identically (lumos `cold_indices`, tested at
`test_cold_pixel_detection` / `test_cold_pixel_correction`). **Bad columns** are a special
case neither lumos nor Siril's dark-map path handles directly: a full bad column has no good
same-color *horizontal* neighbors nearby, so a per-pixel neighbor median degrades (it pulls
from the adjacent bad column). DSS specifically offers *"detect and remove hot pixels **and
bad columns**"* as a stacking option (DSS technical docs via search), and the robust general
remedy is **dithering** (§3.4) + outlier rejection at the stack: shifting the column across
the sky between frames turns a fixed bad column into a rejectable per-frame outlier.
**Gap:** lumos has no explicit bad-column model.

### 3.4 Dithering as a complement to defect maps

Defect maps fix *known, static* defects. **Dithering** — nudging the mount a few pixels
between exposures — is the complementary technique for everything else: residual hot pixels,
walking noise, fixed-pattern noise, and bad columns become *non-coincident* across frames in
sky coordinates, so the stacking rejection (sigma/winsorized clip) removes them and they
average down (opticalmechanics; multiple guides). The practical doctrine: **dither + robust
stack first, defect map as cleanup** — the defect map then only has to handle the few pixels
too consistently bad to reject. Calibration and acquisition strategy are coupled here.

---

## 4. Cosmic ray rejection

Cosmic rays (and satellite/airplane streaks) are sharp, localized, single-frame events. Two
families of methods:

### 4.1 Multi-frame rejection (sigma clipping at the stack)

When you have many registered lights, **per-pixel sigma/winsorized clipping during stacking**
removes any value far from the stack median — cosmic rays included. This is the cheapest and
most reliable CR removal *when you have enough frames* and is what lumos relies on (its light
preset uses σ=2.5 sigma-clip,
`$HOME/Projects/Scenarium/lumos/src/stacking/config.rs:191`). Caveats: it needs
enough frames for robust statistics, and stacks with very different PSF widths force a
lenient-vs-strict tradeoff (cambridge.org PASA "Evaluation of CR rejection"; swarp clipped-mean paper).

### 4.2 Single-frame rejection: L.A.Cosmic (van Dokkum 2001)

**L.A.Cosmic** identifies cosmic rays in a *single* image by Laplacian edge detection: CRs
have sharper edges than the (PSF-broadened) stars, so a Laplacian-of-the-image, compared to a
noise model and a "fine-structure" image, distinguishes CRs from real point sources of
arbitrary shape/size (van Dokkum 2001, IOP 10.1086/323894; McCully astroscrappy
implementation). ccdproc wraps it as `cosmicray_lacosmic` with the canonical defaults
`sigclip=4.5, sigfrac=0.3, objlim=5.0, niter=4`
(`$HOME/Projects/Scenarium/lumos/.tmp/refs/ccdproc/ccdproc/core.py:1541–1560`). It
**works in electrons**, so it needs `gain` and `readnoise` (lines 1621–1627) — i.e. it must
run on linear, calibrated data. ccdproc also offers a simpler `cosmicray_median` (subtract an
11×11 median, threshold at `k·σ`, optionally grow and replace)
(`core.py:1949`).

### 4.3 Where CR rejection belongs in the pipeline

The important, well-sourced nuance: **single-frame CR rejection (L.A.Cosmic) belongs before
registration/warping**, because *"stacking often requires spatial interpolation of the input
images, and any cosmic rays not previously identified can be spread over many pixels by
spatially extended interpolation kernels"* (van Dokkum 2001 / arxiv astro-ph 0108003;
CSST CR paper arxiv 2511.01524 — two sources). A CR smeared by a Lanczos kernel during warp
becomes a low-amplitude blob that sigma clipping can no longer reject. So:

- **Few frames / single frame** → run L.A.Cosmic on each *calibrated, pre-registration* frame.
- **Many frames** → dither + sigma/winsorized clip at the stack catches most CRs, but running
  L.A.Cosmic first still protects against interpolation smearing of the brightest hits.

L.A.Cosmic is itself **not** part of master-frame creation (you reject CRs in the *masters*
via the same combine-time clipping). It is a *light-frame* operation, logically after
calibration (it needs linear data + gain/read-noise) and before warp. **Gap:** lumos has no
single-frame CR rejection; it relies entirely on stack-time clipping.

---

## 5. Recommended best-practice implementation

An opinionated, concrete target for a CFA-aware float pipeline like lumos.

### 5.1 Process raw CFA *before* demosaic

Do **all** calibration on the un-demosaiced single-channel CFA frame, then demosaic. Two hard
reasons: (1) defect correction must use **same-color** neighbors, which is only well-defined
on the mosaic — demosaicing first smears a hot pixel across a 2–3 px color neighborhood and
makes correct repair impossible; (2) dark/bias/flat are sensor-domain quantities that pair
1:1 with mosaic pixels. Siril (`debayer` is the *last* step in `prepro_image_hook`,
`preprocess.c:440`) and lumos (`calibrate` operates on `CfaImage`, README + `mod.rs:165`)
both do this. **Anti-pattern:** demosaic → calibrate (§6).

### 5.2 Canonical order (per light frame)

```
1. (optional) overscan-subtract + trim          [CCD only]
2. additive removal:
      if matched dark:        L −= D            (D contains bias)
      elif scaled dark:       L −= B; L −= k·(D − B)   (k = t_L/t_D; CCD, glow-free only)
      elif bias only:         L −= B
3. multiplicative removal:    L /= normalize(F − FD_or_B)   [per-CFA-channel mean]
4. defect correction:         replace hot/cold via same-color neighbor median
5. (optional) L.A.Cosmic on the calibrated frame, if few subs / before warp
   → then hand off to registration & stacking
```

lumos's `calibrate` implements steps 2–4 exactly (`mod.rs:171–189`). Note: do defect
correction **after** flat division (Siril order, `preprocess.c`) so the neighbor medians are
computed on fully-calibrated values; lumos matches this.

### 5.3 Master-frame parameters

- **Bias:** 100+ subs, winsorized mean κ=3, no normalization. Consider a smoothed
  superbias model. *Skip entirely on CMOS with unreliable short exposures.*
- **Dark:** 20–50 subs matched in exposure/gain/temp, winsorized mean κ=3, **no scaling by
  default**; expose scaling only as an opt-in for glow-free CCDs, and force separate bias when
  scaling.
- **Flat:** 20–50 per filter, exposed to ~1/2 well, sigma-clip mean κ≈2.5 *after per-frame
  normalization*; central-region mean for the normalization constant; **per-CFA-channel**
  scaling for OSC; floor the flat (`min_value`) to avoid div-by-near-zero.
- **Flat-dark:** prefer over bias for flat calibration on CMOS.

### 5.4 Numerical care

- Keep everything in **f32 (or accumulate in f64)** and **preserve negatives** through
  subtraction; never clamp to 0. lumos accumulates flat means in f64
  (`cfa.rs:207–217, 263–289`) — correct.
- Guard flat division: skip/floor pixels where the normalized flat ≤ ε (lumos: `> f32::EPSILON`).
- Per-color means must use **exact pixel counts** per CFA color (lumos asserts `counts[c] > 0`).
- Defect σ via **MAD** with a **floor** to avoid over-detection on clean darks.

### 5.5 CFA-aware defect correction

Stride-2 same-color median for Bayer; full-period same-color search for X-Trans; 8-neighbor
median for mono. (lumos already does all three.)

---

## 6. Pitfalls & anti-patterns

1. **Wrong operation order — dividing before subtracting.** Imprints vignetting onto the
   additive pedestal. Always additive (subtract) before multiplicative (divide). (§1.2)
2. **Scaling a dark that still contains bias.** `k·(D)` scales the bias pedestal too,
   leaving a residual offset. Scale only `D − B`, and handle bias separately when scaling
   (ccdproc forces this). (§2.4)
3. **Dark scaling / dark optimization on amp-glow sensors.** Amp glow is non-linear; a single
   `k` can't match both glow and dark current → residual glow gradient or over-correction.
   *Disable dark optimization when amp glow is present* (three independent sources). (§2.4)
4. **White-balancing / globally normalizing OSC flats.** A single global mean on a non-white
   flat rescales R/G/B unequally → permanent **color shift** in every calibrated light. Use
   per-CFA-channel scaling. (§2.5)
5. **Clamping negative pixels to zero after dark subtraction.** Half of a sky-free pixel's
   noise is legitimately negative; clamping injects a **positive bias** and corrupts
   noise-weighted stacking. Preserve negatives (use float / a pedestal). (§1.3)
6. **Demosaicing before calibrating.** Destroys the 1:1 sensor mapping and makes same-color
   defect repair impossible; smears hot pixels. Calibrate on the raw CFA, demosaic last. (§5.1)
7. **Darks not matched in temperature / exposure / gain.** Mismatched darks leave dark-current
   residuals or, worse, over-subtract → dark mottle. Match all three (CMOS temperature is the
   sensor set-point). (§2.3)
8. **Using bias to calibrate flats on CMOS with unstable bias.** Causes residual rings; switch
   to **flat-darks**. (§2.3)
9. **Too few calibration subs.** A noisy master *adds* noise — can triple post-calibration
   noise. Use the recommended counts. (§2)
10. **Flats clipped/saturated or too dim.** Saturated flat pixels are non-linear (bad
    divisor); too-dim flats are read-noise-dominated. Aim ~1/2 well. (§2.5)
11. **Dividing by an un-floored flat.** Deeply vignetted corners → near-zero divisor →
    exploding values / NaN. Floor the flat (`min_value`) or skip ε-small pixels. (§2.5)
12. **Replacing a defect with a wrong-color or zero value.** Injects false color / dark holes.
    Use same-CFA-color neighbor median. (§3.2)
13. **Standard deviation instead of MAD for defect thresholds.** σ is inflated by the very
    outliers you're hunting; use MAD (50% breakdown). (§3.1)
14. **Rejecting cosmic rays only after warping.** Interpolation smears CRs across pixels,
    defeating sigma clipping; run single-frame CR rejection *before* registration. (§4.3)
15. **Treating defect maps as a substitute for dithering** (or vice-versa). They're
    complementary — dither + robust stack handles transients & bad columns; the defect map
    handles persistent hot/cold pixels. (§3.4)

---

## 7. How lumos currently does it — and gaps/opportunities

**What lumos gets right (matches authoritative practice):**

- **Order is correct:** dark (or bias) subtract → flat divide → defect correct, all on raw
  CFA before demosaic (`calibration_masters/mod.rs:171–189`). Matches Siril/ccdproc.
- **Flat-dark over bias** for flat calibration — the modern CMOS best practice
  (`mod.rs:181`).
- **Per-CFA-channel flat normalization** — avoids OSC color shift; aligns with PixInsight
  1.8.8-6 "separate CFA flat scaling factors" and Siril's `compute_grey_flat`
  (`astro_image/cfa.rs:253–289`).
- **Preserves negative pixels** through subtraction (`cfa.rs:142`) — correct for linearity.
- **Robust MAD-based, per-color defect thresholds with a sigma floor and adaptive sampling**
  (`defect_map.rs`) — more robust than Siril's mean±σ; more capable than Siril on X-Trans.
- **Same-CFA-color neighbor median** correction for Bayer/X-Trans/mono — mathematically correct.
- **Sensible combine presets:** winsorized-mean darks/bias (κ=3), sigma-clip flats (κ=2.5),
  with median fallback below 8 frames (`stacking/config.rs:163–197`; `mod.rs:92`).

**Gaps and opportunities (in rough priority):**

1. **No dark scaling / optimization.** Always `k = 1`. Safe (avoids the amp-glow failure
   mode), but cannot reuse a dark library across exposures. *Opportunity:* add an opt-in
   exposure-ratio scaling (`k = t_L/t_D`) that **requires** separate bias subtraction (like
   ccdproc `scale=True`), explicitly gated off when amp glow is detected; optionally a Siril-
   style noise-minimizing or DSS-style entropy-minimizing search for glow-free CCDs.
2. **No single-frame cosmic-ray rejection.** Relies entirely on stack-time sigma clipping —
   fine for many dithered frames, weak for short sequences and vulnerable to warp-smearing.
   *Opportunity:* an L.A.Cosmic pass on calibrated frames before registration (needs
   gain/read-noise, which lumos already models in `NoiseModel`).
3. **No bad-column handling.** Per-pixel neighbor median degrades on full columns. *Opportunity:*
   detect persistent columns from the master dark and repair from cross-column same-color
   neighbors, or document reliance on dither + clip.
4. **No superbias / no bias modeling, no overscan.** Bias is a plain winsorized mean; no
   overscan support for scientific CCDs. Lower priority for the RAW/CMOS target.
5. **Defect map only from the master dark.** Dead pixels that read *normal* in a dark but fail
   under illumination are invisible. *Opportunity:* also derive cold/dead pixels from the
   master flat.
6. **No defect-map persistence / external bad-pixel-map import** (Siril supports
   `apply_cosme_to_image` from a file, `preprocess.c:436`). *Opportunity:* serialize the
   `DefectMap` so it can be reused / hand-edited.
7. **Flat normalization uses the whole-frame mean**, not Siril's central-region mean. For
   heavily vignetted optics the edge pixels pull the mean down slightly; a central-region (or
   median) normalization would be marginally more robust. Minor.

---

## 8. References

### Source code (cloned, under `$HOME/Projects/Scenarium/lumos/.tmp/refs/`)

- **ccdproc** (Astropy CCD reduction — most authoritative):
  - `ccdproc/core.py:138` `ccd_process` (full order: overscan→trim→gain→bias→dark→flat).
  - `ccdproc/core.py:702` `subtract_bias`; `:741` `subtract_dark` (scaling at lines 843–856,
    `scale_factor = data_exposure/dark_exposure`).
  - `ccdproc/core.py:874` `gain_correct`; `:927` `flat_correct` (mean normalization +
    `min_value` floor, lines 970–1013).
  - `ccdproc/core.py:1541` `cosmicray_lacosmic` (defaults sigclip 4.5 / objlim 5.0 / niter 4,
    works in electrons); `:1949` `cosmicray_median`.
  - `ccdproc/combiner.py:298–740` `clip_extrema` / `minmax_clipping` / `sigma_clipping` /
    `median_combine` / `average_combine` / `sum_combine` + weights.
  - `ccdproc/core.py:386` `create_deviation` (uncertainty/Poisson+read-noise model).
- **Siril** (`src/`):
  - `core/preprocess.c:124` `preprocess` (order: bias→dark→flat); `:161` `darkOptimization`
    (golden-section noise min over k∈[0,2], or k=t_L/t_D); `:304` `prepro_prepare_hook`
    (flat normalization via central-region mean, `equalize_cfa`, cosmetic-from-dark); `:409`
    `prepro_image_hook` (dark-optim → calibrate → cosmetic → debayer).
  - `core/siril.c:453` `compute_grey_flat` (per-CFA-channel means, coeff1=R̄/Ḡ, coeff2=B̄/Ḡ).
  - `filters/cosmetic_correction.c:182` `find_deviant_pixels` (mean±k·σ thresholds); `:43`
    CFA-aware median (`step = is_cfa ? 2 : 1`).
- **DeepSkyStacker** (`DeepSkyStackerKernel/`):
  - `DarkFrame.cpp:127` `ComputeMinimumEntropyFactor` (dark factor k∈[0,5] minimizing entropy);
    `:190` `ComputeMinimumRMSFactor`; `:574` `ComputeDarkFactor` (per-R/G/B for OSC);
    hot-pixel removal lines 473–486.
- **lumos** (grounding):
  - `src/calibration_masters/mod.rs` (`calibrate` order, flat-dark priority, presets/fallback).
  - `src/calibration_masters/defect_map.rs` (MAD per-color thresholds, sigma floor, CFA-color
    neighbor median, X-Trans handling).
  - `src/astro_image/cfa.rs:140` `subtract` (preserves negatives); `:169`
    `divide_by_normalized` (per-CFA-channel means).
  - `src/stacking/config.rs:163` frame-type combine presets.

### Online (cross-verified, ≥2 sources per major claim)

- Astropy CCD Reduction & Photometry Guide — calibrating flats / bias-dark separation when scaling:
  https://www.astropy.org/ccd-reduction-and-photometry-guide/v/dev/notebooks/05-03-Calibrating-the-flats.html
- ccdproc `cosmicray_lacosmic` API & defaults:
  https://ccdproc.readthedocs.io/en/latest/api/ccdproc.cosmicray_lacosmic.html
- van Dokkum 2001, *Cosmic-Ray Rejection by Laplacian Edge Detection* (L.A.Cosmic):
  https://iopscience.iop.org/article/10.1086/323894 ; arXiv: https://arxiv.org/pdf/astro-ph/0108003
- CR rejection before interpolation / single vs multi-frame:
  https://arxiv.org/html/2511.01524 ; https://www.cambridge.org/core/journals/publications-of-the-astronomical-society-of-australia/article/evaluation-of-cosmic-ray-rejection-algorithms-on-singleshot-exposures/F36D800232478BE7D3B5A29543B0D4CF
- Calibration counts / combine methods / sigma κ≈3:
  https://www.opticalmechanics.com/mastering-calibration-frames-for-deep-sky-astrophotography/ ;
  https://practicalastrophotography.com/a-brief-guide-to-calibration-frames/ ;
  https://www.celestron.com/blogs/knowledgebase/the-ultimate-guide-to-calibration-frames-for-astrophotography ;
  https://starfieldview.com/imaging-and-processing/sigma-clipping-outlier-rejection-image-stacking/
- Dark scaling fails on amp glow (non-linear); disable dark optimization:
  https://telescope.live/blog/learning-about-amp-glow ;
  https://www.cloudynights.com/topic/606815-darks-why-cant-i-correct-amp-glow/ ;
  https://www.astropixelprocessor.com/community/main-forum/amp-glow-not-corrected-with-darks/ ;
  https://www.cloudynights.com/topic/758187-darks-over-correcting-new-problem/
- CMOS bias unreliable → use flat-darks; residual rings fix:
  https://britastro.org/forums/topic/bias-frames-for-cmos ;
  https://www.astroworldcreations.com/blog/understanding-flats-part3-conclusions ;
  https://www.cloudynights.com/topic/801195-dark-scaling-with-latest-cmos-imx455imx571/
- Per-CFA-channel flat scaling avoids OSC color shift (PixInsight 1.8.8-6):
  https://www.pixinsight.com/tutorials/master-frames/ ;
  https://www.cloudynights.com/topic/667524-preparing-color-balanced-osc-flats-in-pixinsight/ ;
  https://sh-cosmiccanvas.s3.us-west-2.amazonaws.com/Resources/20200902_GuideToPIsImageCalibration.pdf
- DSS dark optimization / hot-pixel & bad-column detection / entropy-weighted combine:
  http://deepskystacker.free.fr/english/technical.htm ;
  http://deepskystacker.free.fr/english/theory.htm (ECONNREFUSED at fetch time; corroborated via search + DSS source `DarkFrame.cpp`)
- Pedestal / negative-pixel handling & linearity:
  https://grokipedia.com/page/Dark-frame_subtraction ;
  https://www.aavso.org/calibration-dslr-images-photometry
- Dithering complements defect maps:
  https://www.opticalmechanics.com/mastering-flats-darks-and-bias-for-clean-deep-sky-images/

### Notes on source disagreements / unverifiable items

- **Defect threshold statistic differs by tool:** lumos uses median+MAD (robust); Siril uses
  mean+σ; DSS uses median+16σ. All three are defensible; the σ-multiplier is a tuning knob,
  not a correctness issue.
- **OSC flat neutral point differs:** lumos/PixInsight normalize each channel to its own mean;
  Siril equalizes to green. Both avoid the global-mean color shift.
- The DSS *theory.htm* page refused a direct fetch (`ECONNREFUSED`); its equation and
  hot-pixel rule (median + 16σ) were verified instead via the DSS source code
  (`DarkFrame.cpp`) and the technical-docs search snippet — treat the exact "16σ" figure as
  search-snippet-sourced rather than primary-fetched.
