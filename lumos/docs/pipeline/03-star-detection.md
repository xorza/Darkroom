# Stage 3 — Star Detection: Best Practices & Algorithms

## Scope & Goal

Stage 3 turns a calibrated, registered-or-not 2D frame into a catalog of point
sources, each with a sub-pixel position and a set of quality metrics (flux, SNR,
FWHM, ellipticity, roundness, sharpness, saturation/cosmic-ray flags). This
catalog feeds Stage 4 (registration: triangle matching + RANSAC needs clean,
well-localized stars) and any photometry. The detection problem decomposes into
six sub-problems, and the literature has converged on a canonical solution for
each:

1. **Background & noise estimation** — what is "sky" at every pixel, and what is σ.
2. **Detection** — which pixels are significantly above sky (matched-filter +
   threshold + connected-component labeling).
3. **Deblending** — splitting touching/overlapping sources into distinct objects.
4. **Centroiding** — locating each source to a small fraction of a pixel.
5. **Measured parameters** — flux, SNR, FWHM, eccentricity, DAOFIND
   sharpness/roundness, saturation, cosmic-ray discrimination.
6. **Filtering** — rejecting non-stellar detections.

The two reference implementations that define "best practice" are **Source
Extractor** (SExtractor; Bertin & Arnouts 1996) — re-implemented cleanly as the
C library **SEP** — and **DAOFIND/DAOPHOT** (Stetson 1987), re-implemented in
**photutils**. Where they disagree, this document says so. Goal of this section
of the doc: give the actual formulas and defaults, separate best practice from
anti-patterns, and map them onto what lumos does (§8).

The single most important architectural rule, stated up front because every
subsection depends on it: **detection, centroiding, and flux/SNR all operate on a
*background-subtracted* image, with a *spatially varying* noise map σ(x,y).** A
constant sky and a constant σ are the two assumptions that break first on real
astrophotography data (gradients, vignetting, light pollution, amp glow).

---

## 1. Background & noise estimation

### 1.1 The tiled-mesh approach

Sky is not constant. Gradients from light pollution, moonlight, vignetting,
amplifier glow, and large galaxies/nebulae mean that a single global sky value
is wrong almost everywhere. SExtractor's solution, adopted essentially
universally, is a **coarse mesh**: divide the image into tiles (SExtractor
`BACK_SIZE`, default 64×64 px; SEP `bw`,`bh`; photutils `Background2D`
`box_size`), estimate a robust sky level and σ per tile, then **interpolate** the
tile grid back up to a full-resolution per-pixel map.

The tile must be **large enough to contain mostly sky** (so the robust estimator
sees a sky-dominated histogram) but **small enough to track the gradient**.
64 px is the canonical default; lumos uses 64 (`config.rs:265`,
`tile_size`). For heavy gradients shrink it; for sparse fields you can enlarge.

### 1.2 The robust per-tile estimator: sigma-clipped mode

Within each tile, SExtractor estimates sky from the **clipped histogram of pixel
values**. The procedure (verified in SEP `background.c`):

1. Compute mean and σ of the tile (`backstat`, `background.c:277`).
2. Build a histogram with bins spanning ±`QUANTIF_NSIGMA`=5 σ
   (`background.c:382-386`).
3. **Iteratively clip** the histogram at ±3σ around its *median* until σ
   converges (changes <0.02%, i.e. `EPS=1e-4` on the σ ratio) or σ drops below
   0.1, max 100 iterations (`backguess`, `background.c:490-524`).
4. Choose the sky estimate by a **crowding criterion** (`background.c:525-531`):

   ```
   if |mean - median| / σ < 0.3:   sky = 2.5·median − 1.5·mean   (the "mode")
   else:                            sky = median
   ```

The `2.5·median − 1.5·mean` form is **Pearson's empirical mode estimator** for a
mildly skewed unimodal distribution; the coefficients come from SExtractor's
`BACK_PEARSON`=2.5 preference (medfac=2.5, meafac=BACK_PEARSON−1=1.5;
SExtractor `back.c:698-699`). Photutils exposes this exactly as
`SExtractorBackground`: *"(2.5 · median) − (1.5 · mean). If (mean − median)/std >
0.3 then the median is used instead"* (photutils `background/core.py:430-478`).
The closely related **MMM** estimator (DAOPHOT) uses `3·median − 2·mean`
(photutils `MMMBackground`, `core.py:388-425`).

**Why mode, not mean or median?** In an *uncrowded* tile the sky histogram is a
near-Gaussian with a faint positive tail from undetected sources. The mean is
pulled up by that tail; the median is more robust but still slightly biased; the
2.5·med−1.5·mean mode corrects the residual skew and recovers the true peak of
the sky distribution. When the tile is *crowded* (large `|mean−median|/σ`), the
mode formula is unreliable, so SExtractor falls back to the plain median. This
0.3 switch is the single most-cited subtlety of SExtractor background estimation
and is easy to omit (lumos does — see §8).

The per-tile **noise** σ is the σ of the *clipped* histogram (`backguess`
returns `*sigma = sig·qscale`, `background.c:533`). This becomes the σ(x,y) map.

> **Source disagreement / nuance.** photutils decouples the two: you pick a
> `bkg_estimator` (e.g. `SExtractorBackground`) and a *separate*
> `bkgrms_estimator` (e.g. `MADStdBackgroundRMS` = 1.4826·MAD, or
> `StdBackgroundRMS`). SExtractor/SEP tie them together (both from the same
> clipped histogram). MAD-based σ (`1.4826·MAD`) is more outlier-robust than the
> clipped standard deviation and is what lumos uses (`prepare.rs:74`,
> `estimate.rs` tiles; `MAD_TO_SIGMA = 1.4826022`).

### 1.3 Mesh filtering (remove bright-source contamination)

A tile centered on a bright star or galaxy gets a biased sky even after
clipping. SExtractor median-filters the *tile grid itself* with a small kernel
(`BACK_FILTERSIZE`, default 3×3) before interpolation: each node is replaced by
the median of its neighbors **only if** the change exceeds `BACK_FILTERTHRESH`
(`fthresh`) — SEP `filterback`, `background.c:540-659`, the test is
`fabs(med − back[i]) >= fthresh`. Bad tiles (too few good pixels,
`BACK_MINGOODFRAC`=0.5) are flagged `-BIG` and filled from the nearest valid
tile (`background.c:569-596`). This step is what makes the mesh robust to a few
contaminated tiles. lumos does **not** currently median-filter its tile grid
(§8 gap).

### 1.4 Interpolation: bicubic spline, not bilinear

The coarse grid is interpolated to per-pixel resolution with a **natural
bicubic spline** (C²-continuous), not bilinear. SEP precomputes the y-direction
second derivatives once (`makebackspline`, `background.c:682`) and evaluates a
1D natural cubic spline per row in x (`bkg_line_flt_internal`,
`background.c:790-900`). Bilinear interpolation (`sep_bkg_pix`,
`background.c:740`) is offered only for spot queries. lumos matches this:
natural bicubic spline, tridiagonal solve per row, SIMD segment evaluation
(`background/mod.rs:167-297`, `interpolate_row`). Bilinear creates visible
kinks at tile boundaries that imprint onto faint photometry; bicubic does not.

### 1.5 Object masking (iterative refinement)

For crowded fields the first sky estimate is still biased high because object
flux leaks into every tile. The fix is **iterate**: detect sources at the first
sky, **mask them** (dilated to cover the PSF wings), re-estimate sky on the
masked image, repeat. SExtractor calls this `BACK_TYPE`/2-pass detection;
photutils `Background2D` takes a `coverage_mask`/`mask` and a `SExtractorBackground`
estimator; lumos implements it as `BackgroundRefinement::Iterative{iterations}`
(`background/mod.rs:60-96`, `refine_background` → `create_object_mask` +
`dilate_mask`). Best practice: enable ≥1 refinement pass on dense fields; skip it
on sparse ones for speed.

### 1.6 Global map vs. local annulus

Two philosophies for the sky *under a given star*:

- **Global map** (SExtractor default): read σ and sky from the interpolated
  mesh. Fast, smooth, but a nearby bright neighbor or nebular filament can bias
  the local sky.
- **Local annulus** (DAOPHOT/aperture photometry): a sky ring around each star
  (e.g. inner = stamp radius, outer = 1.5×), robustly averaged (sigma-clipped
  median). More local, better in nebulosity, but noisier (fewer pixels) and can
  be contaminated by neighbors falling in the annulus.

lumos supports both via `LocalBackgroundMethod::{GlobalMap, LocalAnnulus}`
(`centroid/mod.rs:213-253`, `compute_annulus_background`, sigma-clipped median,
≥10 px required). Best practice: global mesh for detection thresholding; local
annulus for *measurement* of individual stars in nebulosity.

---

## 2. Detection

### 2.1 Matched-filter convolution (the optimal pre-filter)

The optimal linear statistic for detecting a known shape in additive noise is the
**matched filter**: cross-correlate the (background-subtracted) image with a
kernel shaped like the PSF. For point sources the kernel is a Gaussian matched to
the seeing FWHM. This is the well-known optimal statistic for detecting a single
source in white Gaussian noise (Zackay & Ofek 2017; SEP filter docs).

When noise is uniform across pixels, the matched filter reduces to plain
convolution with the kernel — exactly what SExtractor does
(`filter.c:convolve`, `filter.c:53`). The general form, for signal template `S`,
data `D`, noise covariance `C`, is (SEP docs):

```
SNR = (Sᵀ C⁻¹ D) / sqrt(Sᵀ C⁻¹ S)
```

With uniform σ this is `conv(D,K) / (σ · sqrt(Σ K²))`. **The kernel-energy
normalization `sqrt(Σ K²)` is essential**: it makes the filtered image's noise
equal the per-pixel σ again, so a fixed `n·σ` threshold has the same
false-alarm rate before and after filtering. lumos gets this right — `matched_filter`
divides by `noise_norm = sqrt(Σ K²)` so that `filtered > sigma·noise` is a valid
SNR cut (`convolution/mod.rs:46-117`). SEP goes further than SExtractor:
with a per-pixel variance map it does a *full* matched filter (deweighting noisy
pixels), where plain `conv` would fail (SEP filter docs).

**Kernel choice.** SExtractor ships Gaussian masks for FWHM 1.5–5 px; the
default `default.conv` is a 3×3 Gaussian (FWHM≈2 px). Matching the kernel to the
actual FWHM maximizes faint-source SNR; over-smoothing merges close pairs and
hurts deblending. lumos auto-estimates FWHM first (§5) then builds a
(possibly elliptical) Gaussian kernel (`config.psf_axis_ratio`, `psf_angle`).

> **Pitfall — convolution and centroids.** SExtractor and DAOFIND detect *and*
> compute roundness/sharpness on the *convolved* image, but measure flux and
> position on the *unconvolved* image (convolution shifts/biases moments and
> destroys flux linearity). lumos correctly thresholds on the filtered plane but
> measures flux/FWHM/centroid on the original plane (`detect.rs:98-110` vs
> `measure.rs`). It does, however, compute roundness/sharpness on the *unconvolved*
> stamp — DAOFIND uses the convolved stamp (§5.4, §8 gap).

### 2.2 Thresholding

A pixel is "detected" if `(I − sky) > n·σ(x,y)` (or `filtered > n·σ` after
matched filtering). The threshold `n` (SExtractor `DETECT_THRESH`, default
1.5σ for analysis but commonly 3–5σ for clean catalogs; photutils
`detect_threshold` with `nsigma`; lumos `sigma_threshold`, default 4.0,
`config.rs:272`) trades completeness against false-alarm rate. For a Gaussian
noise field, the per-pixel false-positive rate at 5σ is ~3×10⁻⁷; with a
minimum-area requirement (below) the effective rate drops much further.

**Best practice:** threshold against the *local* σ map, never a global scalar.
A global threshold over-detects in noisy corners (vignette) and misses faint
stars in clean centers. lumos thresholds per-pixel against `stats.noise`
(`threshold_mask`).

### 2.3 Minimum area & connected-component labeling

Single hot pixels and read-noise spikes pass a per-pixel threshold; real sources
span several contiguous pixels. Require a **minimum number of connected pixels
above threshold** (SExtractor `DETECT_MINAREA`, typically 3–5; lumos
`min_area`, default 5, `config.rs:290`). Combined with a matched filter and an
n·σ cut this is the classic SExtractor detection criterion.

Grouping above-threshold pixels into objects is **connected-component labeling
(CCL)**. Two families:

- **Lutz one-pass algorithm** (Lutz 1980), used by SExtractor/SEP
  (`lutz.c`): a single raster scan maintaining a marker/segment state machine
  (`'S'`,`'s'`,`'f'`,`'F'` markers, a per-column status stack) that merges
  8-connected runs on the fly and emits objects when a feature completes. Memory
  ~one image row. This is also the engine reused for deblending (§3).
- **Union-find / two-pass (run-length)**: label runs, union adjacent runs across
  rows, flatten. Easier to parallelize. lumos uses **RLE + lock-free atomic
  union-find**, strip-parallel with boundary merges (`labeling/mod.rs`,
  `Connectivity::{Four, Eight}`, `runs_connected`).

**Connectivity:** 8-connectivity (diagonal neighbors count) is the astronomy
default — diagonally-touching PSF pixels belong to the same star. lumos defaults
to 8 across every preset (`config.rs:273` `connectivity: Connectivity::Eight`).
Use 4-connectivity only if you deliberately want to split diagonal bridges.

---

## 3. Deblending

Two stars whose PSFs overlap produce a single connected component above
threshold. Deblending decides whether that blob is one object or several, and
partitions its pixels.

### 3.1 SExtractor multi-threshold tree + contrast (the gold standard)

This is the algorithm to implement. From SEP `deblend.c` and Bertin & Arnouts
1996 §2.2:

1. For each detected object, re-threshold its pixels at **`DEBLEND_NTHRESH`
   levels** (default **32**) spaced **exponentially** (geometrically) between the
   detection threshold `t₀` and the object's peak. SEP:
   `thresh = t₀ · (peak/t₀)^(k/N)` (`deblend.c:121-122`). Exponential spacing
   puts more levels near the bright core where blends separate.
2. **Build a tree bottom-up.** At each rising level, re-run Lutz CCL
   (`deblend.c:130-141`) on the pixels above that level. A component that splits
   into two children at a higher threshold becomes a branch point. Each node
   records its integrated flux above its own threshold.
3. **Prune top-down with the contrast criterion.** A branch is accepted as a
   separate object only if its integrated flux exceeds **`DEBLEND_MINCONT`**
   (default **0.005**) **times the *total* flux of the original (root) object**:
   SEP `value0 = objlist[0].obj[0].fdflux · deblend_mincont` and the test
   `obj[j].fdflux − obj[j].thresh·obj[j].fdnpix > value0` (`deblend.c:116,179`).
   Only when **two or more** children clear this bar is the parent actually split
   (`if (m > 1)`, `deblend.c:184`); otherwise it stays a single object.
4. **Reassign leftover faint pixels.** Pixels below the split level are assigned
   to the most probable progenitor by a bivariate-Gaussian profile likelihood
   (`gatherup`, `analyse.c`/`deblend.c:274-391`), with a stochastic
   tie-break.

`DEBLEND_MINCONT` controls aggressiveness: **0 = maximal deblending** (every
local peak becomes an object), **1 = no deblending**. 0.005 is the SExtractor
default; lower it (e.g. 0.0001) for crowded fields, raise it to suppress
spurious splits of noisy bright stars.

photutils `deblend_sources` re-implements this with watershed:
`n_levels=32`, `contrast=0.001`, `mode ∈ {exponential, linear, sinh}`
(`segmentation/deblend.py:44-103`). It adds a **watershed segmentation** step on
the multi-threshold seeds and notes `contrast=0` → every local peak is an object;
`contrast=1` → no deblending. The `sinh` mode spaces levels independent of the
peak/min ratio.

### 3.2 Local-maxima / watershed (the fast alternative)

A simpler, faster approach: find local maxima in the component, keep those that
pass a prominence + separation test, and assign each pixel to the nearest peak
(Voronoi by peak) or via watershed flooding. This is essentially `contrast=0`
multi-threshold deblending without the tree. It is faster and adequate for
well-separated stars, but it **over-deblends** noisy bright stars (every bump in
the PSF wings becomes a "star") and lacks the flux-contrast safeguard. It is the
right default only when sources are sparse.

lumos offers both, selected by `deblend_n_thresholds` (`config.rs:224`,
default **0** → `deblend_local_maxima`; ≥1 → `deblend_multi_threshold`):

- `deblend/local_maxima/mod.rs`: local maxima + prominence + separation +
  nearest-peak Voronoi, ArrayVec for ≤`MAX_PEAKS` peaks.
- `deblend/multi_threshold/mod.rs`: exponential levels
  (`threshold = low·ratio^(level/N)`, `mod.rs:500-502`), a tree, and a contrast
  criterion (`collect_significant_leaves`, `mod.rs:795-834`).

> **Source disagreement — what flux the contrast is relative to.** SExtractor
> tests each branch against `MINCONT · root_total_flux` (a single global bar).
> lumos tests each child against `min_contrast · parent_node_flux` (a *recursive,
> relative* bar; `mod.rs:811-816`). These are *not* equivalent: lumos's recursive
> form can either over- or under-split relative to SExtractor depending on the
> blend hierarchy. If bit-compatibility with SExtractor is a goal this is a real
> behavioral gap (§8).

### 3.3 Deblending tradeoffs

| Failure mode | Cause | Symptom |
|---|---|---|
| **Over-deblending** | contrast too low, or local-maxima w/o contrast | one bright star → several spurious "stars" in its wings; corrupts registration |
| **Under-deblending** | contrast too high, too few thresholds | close double → one elongated blob; bad centroid, high eccentricity |
| **Saturation spikes** | bleed columns / diffraction spikes | spikes deblended into fake companions |

Best practice: multi-threshold + contrast (≥16 thresholds, contrast 0.001–0.005)
for science/crowded fields; local-maxima for speed on sparse frames; always
gate the final catalog on eccentricity and a duplicate-removal pass (§5, §6).

---

## 4. Centroiding

### 4.1 Intensity-weighted moments (first-moment / barycenter)

The cheapest centroid is the flux-weighted first moment over the
background-subtracted stamp:

```
x̄ = Σ (Iᵢ − sky)·xᵢ / Σ (Iᵢ − sky),   ȳ similarly
```

SExtractor's basic `X_IMAGE`/`Y_IMAGE` are exactly this isophotal barycenter
(`analyse.c:204-213`: `mx += cval·x`, then `xm = mx/rv`). It is unbiased *only*
if the stamp is symmetric about the true center and the threshold/window does not
clip the PSF asymmetrically. In practice the **isophotal** barycenter is biased:
the detection threshold chops the PSF at a level, and the surviving footprint is
not symmetric, so faint stars get pulled toward bright neighbors and toward the
noisier side. This is the classic centroid bias.

### 4.2 Windowed / Gaussian-weighted iterative centroid (best practice for moments)

The fix is a **Gaussian window** centered on the current estimate, iterated to
convergence. SExtractor `XWIN_IMAGE`/`YWIN_IMAGE`
(`doc/src/PositionWin.rst`):

```
w_i = exp(−r_i² / (2 s²)),   s = d₅₀ / sqrt(8 ln 2)   (d₅₀ = half-flux diameter)
x^(t+1) = x^(t) + 2 · [Σ w_i I_i (x_i − x^(t))] / [Σ w_i I_i]      (likewise y)
```

iterated until the shift < 2×10⁻⁴ px (typically 3–5 iterations). The **factor of
2** is a convergence accelerator derived from the Gaussian-window fixed-point
analysis. SExtractor states XWIN/YWIN accuracy is *"very close to that of
PSF-fitting on focused and properly sampled star images"* and *"close to the
theoretical limit set by image noise"* for isolated Gaussian-like sources. The
Gaussian window suppresses the contribution of distant/contaminating pixels and
removes the isophotal-threshold bias.

lumos's `WeightedMoments` is this iterative Gaussian-weighted centroid
(`centroid/mod.rs:437-503`, `refine_centroid`): weight
`= value · exp(−dist²/2σ²)` with σ from the expected FWHM (×0.8 for tighter
weighting), iterated ≤10 times to a 10⁻⁴ px threshold. **Difference from XWIN:**
lumos uses the plain weighted-mean update (`new = Σwx/Σw`) **without** the
SExtractor factor-of-2 acceleration. The plain form still converges to the
weighted centroid, just more slowly; with a generous iteration cap (10) this is
fine, but it is a deliberate deviation worth noting.

### 4.3 PSF fitting: Gaussian vs. Moffat (best precision)

For the best sub-pixel accuracy, fit a model PSF to the stamp by nonlinear least
squares:

- **2D Gaussian** (6 params: x₀,y₀, amplitude, σ_x, σ_y, background; or +θ for
  rotation). Good for space-based / well-corrected optics. Underestimates the
  wings of atmospheric seeing PSFs.
- **2D Moffat** `I(r) = I₀ · [1 + (r/α)²]^(−β)` (Moffat 1969). The β parameter
  controls the wing heaviness; β≈2.5–4.8 fits atmospheric seeing far better than
  a Gaussian, which is the β→∞ limit. FWHM = 2α·sqrt(2^(1/β) − 1). PSFEx
  (`psfex/src/`) models PSFs with Moffat/Gaussian basis and tracks variation
  across the field.

lumos implements both via a shared Levenberg–Marquardt optimizer:
`CentroidMethod::{WeightedMoments, GaussianFit, MoffatFit{beta}}`
(`config.rs:44`), `centroid/gaussian_fit/` (6-param, SIMD), `centroid/moffat_fit/`
(5–6 param, fixed or fit β), `lm_optimizer.rs` + `linear_solver.rs`. It seeds the
fit with 2 moment iterations then lets LM refine independently
(`measure.rs`/`centroid/mod.rs:304-401`). Documented accuracy: ~0.05 px (moments)
vs ~0.01 px (fit).

### 4.4 Marginal 1D Gaussian (DAOFIND's centroid)

DAOFIND fits separate 1D Gaussians to the **marginal** (row-summed and
column-summed) distributions of the convolved stamp — cheaper than a full 2D fit
and the basis of DAOFIND's GROUND roundness (§5.4). photutils
`daofinder.py:_marginal_*` (`daofinder.py:596-709`).

### 4.5 Accuracy limits and LM pitfalls

The Cramér–Rao floor for centroiding a Gaussian of FWHM `w` at SNR is roughly
`σ_pos ≈ w / (2.35 · SNR)` per axis (background-limited). At SNR=100, FWHM=3 px →
~0.013 px — which is why XWIN and PSF-fitting both land near 0.01 px. You cannot
beat this with a better algorithm; only more photons help.

**Levenberg–Marquardt pitfalls** (verified across the IMFIT and microscopy-PSF
literature):

- **Local minima.** LM is fast but *"highly susceptible to becoming trapped in
  local minima unless very good initial guesses are made."* Seed x₀,y₀ from the
  moment centroid, σ from the second moment (lumos: `estimate_sigma_from_moments`,
  `centroid/mod.rs:171-196`), amplitude from peak−sky.
- **Fitting an un-background-subtracted stamp.** If background is not removed (or
  not a free parameter), the fit amplitude and σ are biased and flux is wrong.
  lumos passes `local_bg` into the fit and subtracts it (`measure.rs`/
  `gaussian_fit`). Make background either pre-subtracted or a fit parameter —
  never ignored.
- **Too small a stamp** clips the wings → σ/β biased low, flux underestimated.
  Rule of thumb ≥1.5×FWHM radius; lumos uses 1.75×FWHM (≈99% of Gaussian flux),
  clamped 4–15 px (`centroid/mod.rs:44-90`).
- **Staged fitting** (fix rotation first, then free it) avoids divergence on
  near-circular sources where θ is ill-constrained.

---

## 5. Measured parameters & quality metrics

### 5.1 Flux

**Isophotal flux** = sum of background-subtracted pixels above threshold within
the object (SExtractor `FLUX_ISO`; `analyse.c` `rv += cval`). Threshold-dependent
(faint wings below threshold are lost). **Aperture** and **PSF-fit (PSF_FLUX)**
fluxes are less biased but need a model/aperture. For detection-stage catalogs,
isophotal or fixed-stamp summed flux is standard. lumos sums
`(I − sky).max(0)` over the measurement stamp (`compute_metrics`,
`centroid/mod.rs:574-577`). Note: clamping negatives to 0 biases faint-source
flux slightly *high* (it discards negative noise excursions) — acceptable for
detection, wrong for precise photometry.

### 5.2 SNR — the CCD equation

The signal-to-noise of a star measured over `npix` pixels, in electrons, is the
**CCD equation** (Howell; verified at Dhillon PHY217):

```
            S* · sqrt(t·g)                          (S* in ADU, g = e⁻/ADU)
SNR = ───────────────────────────────────────
       sqrt( S* + npix·( S_sky + S_dark/g + R²/(g·t) ) )
```

or, in electrons with total source counts `N*`:

```
SNR = N* / sqrt( N* + npix·( N_sky + N_dark + R² ) )
```

where `N*` = source electrons (its own **shot noise** = sqrt(N\*)), `N_sky` = sky
e⁻ per pixel, `N_dark` = dark e⁻ per pixel, `R` = read noise e⁻/pixel, `npix` =
pixels in the aperture. **Two regimes:**

- **Shot/sky-limited** (bright source or bright sky): SNR ∝ sqrt(N\*) ∝ sqrt(t).
- **Read-noise-limited** (faint source, short exposure): denominator ≈
  sqrt(npix)·R, so SNR ∝ N\* ∝ t (linear in exposure).

lumos implements exactly this (`compute_snr`, `centroid/mod.rs:721-750`):
with a `NoiseModel{gain, read_noise}` it uses
`total_var = flux/g + npix·(σ_sky² + R²/g²)` (the shot + sky + read terms);
without a noise model it falls back to the background-limited
`SNR = flux / (σ_sky·sqrt(npix))`. The full form requires the user to supply
gain and read noise (`config.noise_model`).

> **Pitfall — gain unknown / data normalized to [0,1].** lumos pixels are
> normalized to [0,1] (planar f32), so "flux" and σ are in normalized units, not
> electrons. The shot-noise term `flux/g` is only correct if `g` is expressed in
> the same normalized units; otherwise only the background-limited branch is
> meaningful. State the units assumption explicitly when configuring `NoiseModel`.

### 5.3 FWHM & eccentricity from second moments

From the flux-weighted second central moments
`x2 = Σw·(x−x̄)²/Σw`, `y2`, `xy` (SExtractor `analyse.c:228-230`):

- **Size:** for a Gaussian, `E[r²] = 2σ²`, so `σ = sqrt((x2+y2)/2)` and
  `FWHM = 2.3548·σ`. lumos: `sigma_sq = sum_r2/flux/2`,
  `fwhm = sigma_to_fwhm(sqrt)` (`centroid/mod.rs:612-613`).
- **Shape:** the moment matrix `[[x2,xy],[xy,y2]]` has eigenvalues λ₁≥λ₂ giving
  semi-axes `a=sqrt(λ₁)`, `b=sqrt(λ₂)` (SExtractor `A_IMAGE`,`B_IMAGE`,`THETA`,
  `analyse.c:273-296`). **Eccentricity** `e = sqrt(1 − (b/a)²) = sqrt(1 − λ₂/λ₁)`
  (lumos `centroid/mod.rs:616-630`); **ellipticity** `= 1 − b/a`. Circular → 0.
  When the source is unresolved (moment matrix near-singular, `x2·y2−xy² < 0.0694`)
  SExtractor adds 1/12 to the diagonal (the pixel-quantization variance) and flags
  `OBJ_SINGU` (`analyse.c:259-271`) — a subtlety lumos does not replicate.

Eccentricity is the single most useful non-stellar reject: galaxies, blends, and
trailed stars have high e; round PSFs have e≈0.

### 5.4 DAOFIND sharpness & roundness (Stetson 1987)

DAOFIND's three shape statistics, computed on the **convolved** image (photutils
`daofinder.py`, verified against docs):

- **Sharpness** = (height of central pixel in *unconvolved* data − mean of the
  surrounding pixels) / (height of the best-fit Gaussian at that point)
  (`daofinder.py:580-594`). A point source ≈ the kernel → sharpness ≈
  intermediate; a single hot pixel/cosmic ray is far sharper than the kernel →
  sharpness near 1; a broad galaxy → low sharpness. Default accept range
  `(0.2, 1.0)`.
- **roundness1 = SROUND** — *symmetry* based: ratio of the object's 2-fold to
  4-fold symmetry computed from four quadrant sums of the **convolved** stamp
  (peak pixel zeroed), `2·sum2/sum4` (`daofinder.py:534-577`). Circular → 0.
- **roundness2 = GROUND** — *marginal-Gaussian* based: fit 1D Gaussians to the x
  and y marginals; `GROUND = 2·(hx − hy)/(hx + hy)` where hx,hy are the fit
  *heights* (`daofinder.py:964-976`). x-extended → negative, y-extended →
  positive. Default accept range `(−1.0, 1.0)`; kernel `sigma_radius=1.5`.

> **NAMING BUG / gap in lumos.** photutils (and DAOFIND) define **roundness1 =
> SROUND (symmetry)** and **roundness2 = GROUND (marginal Gaussian heights)**.
> lumos has them **swapped**: its `Star.roundness1` is documented and computed as
> GROUND `(hx−hy)/(hx+hy)` and `Star.roundness2` as a symmetry/asymmetry RMS
> (`star.rs:24-30`, `compute_roundness`, `centroid/mod.rs:666-694`). Functionally
> both are still computed and both are gated by `is_round`, so detection still
> works — but the field names are the reverse of the published convention, which
> will mislead anyone cross-checking against photutils/DAOFIND. Additionally,
> lumos computes them on the **unconvolved** stamp and its SROUND is an
> ad-hoc left/right + top/bottom asymmetry RMS, not Stetson's quadrant 2-fold/
> 4-fold ratio. These are approximations, not the canonical definitions.

### 5.5 Saturation & cosmic-ray flagging

- **Saturation:** flag stars whose peak exceeds a fraction of the ADC/well limit
  (SExtractor `SATUR_LEVEL` → `FLAGS` bit 2). Saturated cores have flat tops →
  biased centroids and meaningless flux. lumos: `Star::is_saturated(threshold)`,
  default 0.95 of normalized max (`star.rs:38`, applied in `filter.rs:46`).
- **Cosmic rays:** sharp, often single-/few-pixel spikes far narrower than the
  PSF. Two discriminators: (a) **sharpness** — CRs have sharpness ≫ a real star
  (lumos `is_cosmic_ray(max_sharpness)`, default 0.7, `star.rs:45`,
  `filter.rs:55`); (b) **Laplacian edge detection** (van Dokkum 2001,
  L.A.Cosmic): a CR's edges are sharper than any PSF-convolved source, so a
  Laplacian-filtered/SNR image flags CRs while *"reliably discriminating between
  poorly sampled point sources and cosmic rays"* — the right tool when the PSF is
  undersampled and a simple sharpness cut would reject real faint stars. The most
  robust CR rejection, though, is **multi-frame**: a CR appears in one sub only,
  so sigma-clipped stacking (Stage 5) removes it. For single-frame detection,
  sharpness is a cheap first pass; Laplacian is the rigorous upgrade.

---

## 6. Recommended best-practice implementation

A reference pipeline, in order:

1. **Reduce to one detection plane.** Grayscale → copy. Color → noise-weighted
   (inverse-variance) channel sum — the linear analogue of SExtractor's χ²
   detection image, kept *linear* so flux/centroid stay valid. (Do **not** use
   perceptual Rec.709 luminance: it crushes red/blue stars.) CFA → 3×3 median
   first to kill mosaic artifacts.
2. **Background:** 64-px tiled mesh; per tile, sigma-clipped histogram → mode
   `2.5·med−1.5·mean` (median if `|mean−med|/σ ≥ 0.3`); σ from clipped std or
   1.4826·MAD; **3×3 median-filter the tile grid** with a threshold;
   natural-bicubic interpolate to per-pixel sky and σ. On crowded fields, ≥1
   object-masked refinement pass.
3. **FWHM:** auto-estimate from a strict first-pass detection (high threshold,
   robust median/MAD of round bright stars).
4. **Detect:** matched-filter convolve with a Gaussian (or elliptical) kernel
   matched to FWHM, normalized by `sqrt(ΣK²)`; threshold at 3–5σ against the
   *local* σ map; CCL (Lutz or union-find, 8-connectivity); `DETECT_MINAREA` ≥3.
5. **Deblend:** multi-threshold (≥16–32 exponential levels) + contrast
   (0.001–0.005, relative to *root* flux); fall back to local-maxima only for
   sparse/speed cases.
6. **Centroid & measure on the *unconvolved* plane:** iterative Gaussian-windowed
   moments (XWIN-style, with the factor-of-2 acceleration) for speed, or 2D
   Gaussian/Moffat LM fit (seeded from moments, background subtracted, ≥1.5×FWHM
   stamp) for precision. Compute flux, CCD-equation SNR, FWHM and eccentricity
   from second moments, DAOFIND sharpness + SROUND + GROUND **on the convolved
   stamp**.
7. **Filter:** reject saturated, low-SNR, high-eccentricity, high-sharpness
   (CR), and non-round sources; MAD-clip FWHM outliers; remove duplicates; sort
   by flux.

---

## 7. Pitfalls & anti-patterns

- **Global background on a gradient field.** A single sky value (or a single σ)
  over a vignetted / light-polluted frame over-detects in the bright corners and
  misses faint stars in clean regions. → tiled mesh + per-pixel σ.
- **Fixed ADU threshold without a noise model.** "Detect everything above 1000
  counts" has a wildly varying false-alarm rate across the frame and across
  exposures. → threshold in units of local σ.
- **Bilinear background interpolation.** Visible tile-boundary kinks imprint on
  faint photometry. → natural bicubic spline.
- **Skipping the 0.3 crowding switch / mesh median-filter.** A bright object in a
  tile biases sky high; without the median-filter or the mode/median switch the
  local sky is wrong and faint neighbors vanish.
- **Isophotal (threshold-clipped) centroid bias.** The detection threshold chops
  the PSF asymmetrically; the barycenter is pulled off-center, worst for faint
  stars near bright neighbors. → Gaussian-windowed iterative centroid or PSF fit.
- **Fitting an un-background-subtracted stamp.** Biases amplitude, σ, β, and
  flux; can prevent convergence. → subtract sky or fit it as a parameter.
- **Stamp too small for the PSF wings.** σ/β biased low, flux underestimated. →
  ≥1.5×FWHM radius.
- **LM without a good seed.** Lands in a local minimum (e.g. on a noise spike or
  a neighbor). → seed from moments; stage the rotation parameter.
- **Convolution before measuring flux/centroid.** Matched filtering is for
  *detection/thresholding* and for DAOFIND shape stats only; measure flux,
  position, and FWHM on the *unconvolved* image.
- **Over-deblending** (contrast too low / local-maxima without a contrast bar):
  one bright star explodes into spurious companions in its wings, poisoning
  registration. **Under-deblending** (contrast too high): close doubles merge
  into one elongated blob.
- **Counting cosmic rays / hot pixels as stars.** A per-pixel threshold + 1-pixel
  area passes them. → minimum area, sharpness cut, Laplacian (L.A.Cosmic), and
  ultimately multi-frame sigma-clipping.
- **Clamping negative (sky-subtracted) pixels to zero before summing flux.**
  Biases faint-source flux high by discarding negative noise. Fine for detection,
  wrong for photometry.
- **Trusting saturated-star centroids/FWHM.** Flat-topped cores give garbage
  shape and position. → flag and exclude from FWHM estimation and registration.

---

## 8. How lumos currently does it — and gaps/opportunities

**Pipeline (`detector/mod.rs:122-218`):** prepare → background (+optional
iterative refine) → FWHM estimate → detect (matched filter + threshold + CCL +
deblend) → measure (parallel centroid + metrics) → filter. This matches the
canonical six-stage flow and is well-structured.

**What lumos gets right (matches best practice):**

- Tiled (64 px) sigma-clipped background with **natural bicubic** interpolation
  and **per-pixel σ** map (`background/mod.rs`), MAD-based σ (1.4826·MAD).
- Optional **object-masked iterative refinement** for crowded fields
  (`refine_background`).
- **Matched-filter** detection with correct `sqrt(ΣK²)` kernel-energy
  normalization so the n·σ cut is a true SNR cut (`convolution/mod.rs:46-117`);
  per-pixel (local) σ thresholding.
- **Min-area + 8-connectivity CCL** via parallel RLE + lock-free union-find
  (`labeling/`).
- **Both deblenders:** multi-threshold tree + contrast (SExtractor-style) and
  fast local-maxima.
- **Iterative Gaussian-weighted centroid** plus **Gaussian and Moffat LM fits**,
  seeded from moments with background subtracted and a 1.75×FWHM stamp.
- **Full CCD-equation SNR** when a `NoiseModel` is supplied; sensible
  background-limited fallback.
- Eccentricity from the second-moment eigenvalues; saturation and sharpness-based
  CR flags; MAD-based FWHM outlier rejection; duplicate removal; noise-weighted
  RGB detection plane (better than Rec.709 for colored stars).

**Gaps / opportunities (ordered by impact):**

1. **roundness1/roundness2 are swapped vs. the DAOFIND/photutils convention** and
   computed on the *unconvolved* stamp with an ad-hoc SROUND. lumos roundness1 =
   GROUND-like `(hx−hy)/(hx+hy)`, roundness2 = an asymmetry-RMS, the reverse of
   photutils. Rename to match the convention, compute on the convolved stamp, and
   use Stetson's quadrant 2-fold/4-fold ratio for SROUND
   (`star.rs:24-30`, `centroid/mod.rs:666-694`).
2. **Background mode estimator is median/MAD only — no `2.5·med−1.5·mean` mode and
   no 0.3 crowding switch.** Adding the Pearson mode (with the median fallback)
   would reduce sky bias in uncrowded tiles, matching SExtractor/SEP exactly.
3. **No tile-grid median filter** (`BACK_FILTERSIZE`/`BACK_FILTERTHRESH`). A tile
   on a bright star biases sky locally; a 3×3 thresholded median filter on the
   grid (SEP `filterback`) is the standard fix and is cheap.
4. **Multi-threshold contrast is relative to the *parent node* flux, not the
   *root/total* flux** as in SExtractor (`mod.rs:811-816` vs SEP
   `deblend.c:116`). This changes split behavior; align with SExtractor's global
   `MINCONT·root_flux` bar if cross-tool consistency matters.
5. **Windowed centroid lacks the SExtractor factor-of-2 update acceleration**
   (`centroid/mod.rs:491`). Harmless to accuracy with 10 iterations, but adding it
   matches XWIN and converges faster.
6. **Sharpness is `peak/core_flux` (3×3)**, an approximation of DAOFIND's
   `(peak − mean_neighbors)/Gaussian_height`. The DAOFIND form is more
   discriminating for CRs vs. undersampled stars; consider it, or add a
   **Laplacian (L.A.Cosmic) CR test** for undersampled data where a sharpness cut
   would reject real faint stars.
7. **Flux clamps negatives to 0** (`centroid/mod.rs:574`) — fine for detection,
   biases photometry; expose an unclamped flux for measurement use.
8. **No singular-moment 1/12 regularization / `OBJ_SINGU` flag** for unresolved
   sources (SExtractor `analyse.c:259-271`) — FWHM/eccentricity of 1–2 px blobs
   can be ill-defined.

None of these block correct detection; #1 (naming) and #2/#3 (background fidelity)
are the highest-value alignments with the reference implementations.

---

## 9. References

### Cloned source code (read directly)

- **SEP** (clean SExtractor C library) — `$HOME/Projects/Scenarium/lumos/.tmp/refs/sep/src/`:
  - `background.c` — `backstat` (clipped mean/σ, :277), `backhisto` (:398),
    `backguess` (mode estimator + 0.3 switch, :466-536), `filterback` (mesh
    median filter, :540), `makebackspline`/`bkg_line_flt_internal` (bicubic,
    :682, :790), `sep_bkg_pix` (bilinear, :740).
  - `lutz.c` — Lutz one-pass 8-connected extraction (`lutz`, :101).
  - `deblend.c` — multi-threshold tree + contrast (`deblend`, :63; exponential
    levels :121; `value0=fdflux·mincont` :116; split test :179-198;
    `gatherup` :274).
  - `analyse.c` — `preanalyse` (peak/bbox, :100), `analyse` (moments, flux, a/b/θ,
    cxx/cyy/cxy, singular-moment handling, :168), `analysemthresh` (CLEAN, :39).
- **SExtractor** (canonical) — `$HOME/Projects/Scenarium/lumos/.tmp/refs/sextractor/src/`:
  - `back.c` — mode estimator with `medfac`/`meafac` from `BACK_PEARSON`
    (:698-738), `filterback` (:751).
  - `filter.c` — `convolve` (matched-filter scan-line, :53), `getconv`
    (kernel normalization by Σ|K| or ΣK², :115-200).
  - `scan.c` — detection thresholding / variable threshold (:373-375), `analyse`.
- **photutils** — `$HOME/Projects/Scenarium/lumos/.tmp/refs/photutils/photutils/`:
  - `background/core.py` — `SExtractorBackground` (2.5·med−1.5·mean + 0.3 switch,
    :430-478), `MMMBackground` (3·med−2·mean, :388), `MADStdBackgroundRMS` (:604).
  - `detection/daofinder.py` — `sharpness` (:580), `roundness1`=SROUND (quadrant
    symmetry on convolved data, :534-577), `roundness2`=GROUND (marginal Gaussian
    heights, :964-976), marginal fits (:596-709).
  - `segmentation/deblend.py` — `deblend_sources` (n_levels=32, contrast=0.001,
    exponential/linear/sinh + watershed, :44-103).
- **PSFEx** — `$HOME/Projects/Scenarium/lumos/.tmp/refs/psfex/src/`
  (`psf.c`, `sample.c`, `diagnostic.c`) — Moffat/Gaussian PSF modeling and
  field variation.

### lumos source (grounding)

`$HOME/Projects/Scenarium/lumos/src/star_detection/`:
`detector/mod.rs` (pipeline), `detector/stages/{prepare,background→,fwhm,detect,
measure,filter}.rs`, `background/{mod,estimate,tile_grid}.rs`, `convolution/mod.rs`
(matched filter), `labeling/mod.rs` (CCL), `deblend/{local_maxima,multi_threshold}/`,
`centroid/{mod,gaussian_fit,moffat_fit,lm_optimizer}.rs`, `star.rs`, `config.rs`.

### Online sources (verified ≥2 where noted)

- Bertin & Arnouts 1996, *SExtractor: Software for source extraction*, A&AS 117,
  393 — https://aas.aanda.org/articles/aas/pdf/1996/08/ds1060.pdf and ADS
  https://ui.adsabs.harvard.edu/abs/1996A%26AS..117..393B/abstract
- *Source Extractor for Dummies* (Holwerda) — https://arxiv.org/pdf/astro-ph/0512139
- SExtractor manual — background, detection, deblending defaults:
  https://sextractor.readthedocs.io/en/latest/  (Background, Position,
  PositionWin pages)
- SExtractor windowed positions (XWIN/YWIN iterative formula, factor of 2,
  accuracy claim) — https://sextractor.readthedocs.io/en/latest/PositionWin.html
- SEP matched-filter docs (`SNR = SᵀC⁻¹D / sqrt(SᵀC⁻¹S)`, uniform-noise
  reduction to convolution) — https://sep.readthedocs.io/en/stable/filter.html
- photutils `SExtractorBackground` / `MMMBackground` / DAOStarFinder docs
  (sharpness/SROUND/GROUND definitions, defaults) —
  https://photutils.readthedocs.io/en/stable/api/photutils.detection.DAOStarFinder.html
- Stetson 1987, *DAOPHOT*, PASP 99, 191 — DAOFIND sharpness/roundness origin.
- CCD equation / SNR — Dhillon PHY217:
  https://vikdhillon.staff.shef.ac.uk/teaching/phy217/detectors/phy217_det_ccdeqn.html
  and ESO https://www.eso.org/~ohainaut/ccd/sn.html (Howell, *Handbook of CCD
  Astronomy*, is the textbook reference).
- Zackay & Ofek 2017, *How to COAAD Images I* (optimal matched-filter detection)
  — https://iopscience.iop.org/article/10.3847/1538-4357/836/2/187
- van Dokkum 2001, *Cosmic-Ray Rejection by Laplacian Edge Detection*, PASP 113,
  1420 — https://arxiv.org/abs/astro-ph/0108003
- Moffat 1969, *A theoretical investigation of focal stellar images* (Moffat
  profile) — A&A 3, 455.
- IMFIT (Erwin 2015) on LM local-minima sensitivity —
  https://iopscience.iop.org/article/10.1088/0004-637X/799/2/226
