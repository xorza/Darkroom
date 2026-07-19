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
64 px is the canonical default; lumos uses 64 (`BackgroundConfig::tile_size`).
For heavy gradients shrink it; for sparse fields you can enlarge.

### 1.2 The robust per-tile estimator: sigma-clipped mode

Within each tile, SExtractor estimates sky from the **clipped histogram of pixel
values**. The procedure (verified in SEP `background.c`):

1. Compute mean and σ of the tile (`backstat`, `background.c:277`).
2. Build a histogram with bins spanning ±`QUANTIF_NSIGMA`=5 σ
   (`background.c:382-386`).
3. **Iteratively clip** the histogram at ±3σ around its *median* until σ
   converges (relative change ≤ `EPS=1e-4`, i.e. <0.01% on the σ ratio) or σ
   drops below 0.1, max 100 iterations (`backguess`, `background.c:490-524`;
   the loop guard is `fabs(sig/sig1 − 1.0) > EPS`).
4. Choose the sky estimate by a **crowding criterion** (`background.c:525-531`):

   ```
   if |mean - median| / σ < 0.3:   sky = 2.5·median − 1.5·mean   (the "mode")
   else:                            sky = median
   ```

The `2.5·median − 1.5·mean` form is **Pearson's empirical mode estimator** for a
mildly skewed unimodal distribution. **Verified verbatim (pass 2)** in both primary
sources:

- SExtractor `back.c:698-699,735-737`: `medfac = prefs.back_pearson` (=2.5),
  `meafac = prefs.back_pearson − 1.0` (=1.5), and the sky is
  `qzero + (medfac·med − meafac·mea)·qscale` when `fabs((mea−med)/sig) < 0.3`,
  else `qzero + med·qscale`. **`BACK_PEARSON` is a configurable parameter**
  (`preflist.h:264`, default 2.5), so SExtractor's mode is the *family*
  `α·med − (α−1)·mea`; 2.5/1.5 is just the default α.
- SEP `background.c:528-530` is byte-identical:
  `fabs((mea − med)/sig) < 0.3 ? qzero + (2.5·med − 1.5·mea)·qscale
  : qzero + med·qscale`.

**Nuance the prior pass missed: `med` and `mea` are *histogram-domain* quantities.**
Both implementations work on the per-tile histogram (bins of width `qscale` spanning
±5σ from `qzero`), iteratively clipping at `med ± 3σ`. `mea` is the histogram's
intensity-weighted mean *in bin units*; `med` is **not** a sorted median but a
**histogram-interpolated median** found by walking two pointers inward from the
clipped ends until the cumulative counts balance, then linearly interpolating within
the crossover bin (`background.c:508-511`). The final sky is `qzero + (…)·qscale` —
i.e. the bin-unit estimate scaled back to ADU. This is why a naive "sorted-median +
sample-mean" reimplementation will not bit-match SExtractor even with the right
coefficients.

Photutils exposes the formula (but on raw pixel arrays, not a histogram) as
`SExtractorBackground`: *"(2.5 · median) − (1.5 · mean). If (mean − median)/std >
0.3 then the median is used instead"* (photutils `background/core.py:430-478`).
The closely related **MMM** estimator (DAOPHOT) uses `3·median − 2·mean`
(photutils `MMMBackground`, `core.py:388-425`) — i.e. the same family with α=3.

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
> clipped standard deviation and is what lumos uses: the RGB-plane noise weights
> in `prepare.rs:74`, and the per-tile σ in `tile_grid.rs`
> (`compute_tile_stats` → `sigma_clipped_median_mad`, `:452`, returns the
> sigma-clipped **median** plus `mad_to_sigma(mad)`); `MAD_TO_SIGMA = 1.4826022`.

### 1.3 Mesh filtering (remove bright-source contamination)

A tile centered on a bright star or galaxy gets a biased sky even after
clipping. SExtractor median-filters the *tile grid itself* with a small kernel
(`BACK_FILTERSIZE`, default 3×3) before interpolation: each node is replaced by
the median of its neighbors **only if** the change exceeds `BACK_FILTERTHRESH`
(`fthresh`) — SEP `filterback`, `background.c:540-659`, the test is
`fabs(med − back[i]) >= fthresh`. The σ map is median-filtered alongside the sky
map (`background.c:627`). **The default `fthresh` is `0.0` in both tools**
(SExtractor `BACK_FILTTHRESH`, `preflist.h:62`, range `[0, BIG]`; SEP
`fthresh=0.0`, `sep.pyx:390`), so `|med − back| >= 0` is always true and the
**default behavior is unconditional replacement** with the 3×3 neighbor median.
The threshold only suppresses replacement once a user raises it above zero. Bad
tiles (too few good pixels, `BACK_MINGOODFRAC`=0.5) are flagged `-BIG` and
filled from the nearest valid tile (`background.c:569-596`). This step is what
makes the mesh robust to a few contaminated tiles.

lumos **does** median-filter its tile grid — `apply_median_filter`
(`tile_grid.rs:208-249`) runs on every `compute()` (`:87`), replacing each tile's
median *and* σ with the median of its 3×3 tile neighborhood (`:244-245`,
edge tiles use the available neighbors). Because the replacement is
**unconditional**, this matches SExtractor/SEP at their default `fthresh=0`.
Two deviations: lumos exposes no `fthresh` knob (it cannot replicate a non-zero
`BACK_FILTERTHRESH`), and it skips the filter entirely for grids smaller than
3×3 tiles (`:212`), whereas SExtractor/SEP still filter small grids with a
shrunk window.

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
SNR cut (`convolution/mod.rs:99-117`; separable path computes
`(Σk₁²)·(Σk₁²)` then sqrt).

> **What SExtractor actually does with the kernel (pass 2, primary source).**
> SExtractor's `getconv` (`filter.c:177-195`) computes **two** norms: `sum = Σ|K|`
> and `varnorm = sqrt(ΣK²)`. It divides the convolution mask by **`Σ|K|`** (so the
> *convolved sky stays unbiased* — a flat field convolves to itself), and keeps
> `varnorm = sqrt(ΣK²)` separately for the *significance* test. The two
> formulations — "normalize kernel by `Σ|K|`, then threshold the convolved value
> against `(varnorm/Σ|K|)·σ`" vs. lumos's "normalize the *output noise* by
> `sqrt(ΣK²)`, then threshold against `1·σ`" — are the **same n·σ cut up to a
> constant**. lumos's is the cleaner SNR-image form (it makes `filtered/σ` directly
> the per-pixel SNR). Note lumos does **not** zero-mean the kernel the way DAOFIND
> does (the "lowered Gaussian", §5.4), so lumos's convolution does not get DAOFIND's
> automatic sky-slope cancellation — it relies on prior background subtraction.

SEP goes further than SExtractor: with a per-pixel variance map it does a *full*
matched filter (deweighting noisy pixels), where plain `conv` would fail. The SEP
JOSS paper (Barbary 2016, `.tmp/papers/sep_barbary2016.txt`) lists *"Optimized
matched filter for variable noise in source extraction"* as one of the features
**added beyond** the original SExtractor — confirming this is a SEP extension, not a
SExtractor feature. Makovoz & Marleau 2005 (MOPEX, `.tmp/papers/makovoz2005.txt`)
note that when many sources are present the pixel distribution is *"highly
non-Gaussian … the linear filter becomes sub-optimal and the optimal filter is
non-linear"*, and that *"the filtered images are used for detection only"* — the
same convolve-to-detect / measure-on-the-raw-image discipline (§2.1 pitfall).

**Kernel choice.** SExtractor ships Gaussian masks for FWHM 1.5–5 px; the
default `default.conv` is a 3×3 Gaussian (FWHM≈2 px). Matching the kernel to the
actual FWHM maximizes faint-source SNR; over-smoothing merges close pairs and
hurts deblending. lumos auto-estimates FWHM first (§5) then builds a
(possibly elliptical) Gaussian kernel (`config.psf_axis_ratio`, `psf_angle`).

> **Pitfall — convolution and centroids.** SExtractor and DAOFIND detect on the
> *convolved* image and compute the **sharpness denominator and SROUND** there, but
> measure flux, position, and the GROUND marginals on the *unconvolved* image
> (convolution shifts/biases moments and destroys flux linearity). lumos correctly
> thresholds on the filtered plane but measures flux/FWHM/centroid on the original
> plane (`detect.rs:98-110` vs `measure.rs`). It computes **both** roundness indices
> and sharpness on the *unconvolved* stamp — diverging from DAOFIND/photutils for
> sharpness's `H` denominator and for SROUND (§5.4, §8 gap).

### 2.2 Thresholding

A pixel is "detected" if `(I − sky) > n·σ(x,y)` (or `filtered > n·σ` after
matched filtering). The threshold `n` (SExtractor `DETECT_THRESH`, default
1.5σ for analysis but commonly 3–5σ for clean catalogs; photutils
`detect_threshold` with `nsigma`; lumos `DetectionConfig::sigma_threshold`, default 4.0)
trades completeness against false-alarm rate. For a Gaussian
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
   detection threshold `t₀` and the object's peak. SEP verbatim (`deblend.c:120-122`):
   `thresh = thresh0 · pow(peak/thresh0, k/N)` where `peak = obj.fdpeak`,
   `thresh0 = obj.thresh`. Exponential spacing puts more levels near the bright
   core where blends separate.
2. **Build a tree bottom-up.** At each rising level, re-run Lutz CCL
   (`deblend.c:130-141`) on the pixels above that level. A component that splits
   into two children at a higher threshold becomes a branch point. Each node
   records its integrated flux above its own threshold.
3. **Prune top-down with the contrast criterion.** A branch is accepted as a
   separate object only if its integrated flux exceeds **`DEBLEND_MINCONT`**
   (default **0.005**) **times the detection-isophotal flux of the original (root)
   object** (`fdflux`): SEP `value0 = objlist[0].obj[0].fdflux · deblend_mincont`
   (`deblend.c:116`) and the test
   `obj[j].fdflux − obj[j].thresh·obj[j].fdnpix > value0` (`deblend.c:179`).
   Only when **two or more** children clear this bar is the parent actually split
   (`if (m > 1)`, `deblend.c:184`); otherwise it stays a single object.

   > **Correction (pass 2): "total flux" → root *detection-isophotal* flux.** The
   > prior pass said the bar is `MINCONT × the *total* flux of the root object`.
   > Primary-source `fdflux` is the flux summed **above the detection threshold
   > isophote** at deblend time (not an aperture or total/Kron flux), and the
   > per-branch quantity tested is `fdflux − thresh·fdnpix` (flux *above that
   > branch's own level*). Holwerda's *SExtractor for Dummies*
   > (`.tmp/papers/holwerda_dummies.txt:1182-1186`) states the criterion in words:
   > a branch is a separate object iff *"(1) the number of counts in the branch is
   > above a certain fraction of the total count in the entire 'island'"* and
   > *"(2) there is at least one other branch above the same level that is also
   > above this fraction"* — confirming both the **root/island-relative** bar and
   > the **≥2-children** rule.
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

lumos offers both, selected by `DetectionConfig::deblend_n_thresholds`,
default **0** → `deblend_local_maxima`; ≥2 → `deblend_multi_threshold`):

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

iterated until the shift < 2×10⁻⁴ px. **Verified verbatim (pass 2)** from the
SExtractor manual `PositionWin.html`: the iteration *"is initialized"* with the
isophotal `x̄`,`ȳ`; the Gaussian window FWHM *"is the diameter of the disk that
contains half of the object flux (d₅₀)"* (so `s = d₅₀/√(8 ln 2)`); the update
carries the explicit **`+2·…`** prefactor; and *"the process stops when the change
in position between two iterations is less than 2×10⁻⁴ pixel, a condition which is
achieved in about 3 to 5 iterations in practice."* The **factor of 2** is a
fixed-point accelerator: for a Gaussian source under a matched Gaussian window the
naive weighted-mean update moves only ~half the way to the true centroid each step
(the window itself pulls the estimate back), so doubling the step makes the
iteration a near-Newton fixed point — which is why XWIN converges in 3-5 steps
where the un-accelerated form (lumos) needs more. SExtractor states XWIN/YWIN
accuracy is *"actually very close to that of PSF-fitting on focused and properly
sampled star images"* and, for *"isolated objects with Gaussian-like profiles … is
close to the theoretical limit set by image noise."* The Gaussian window suppresses
the contribution of distant/contaminating pixels and removes the isophotal-threshold
bias.

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
  a Gaussian, which is the β→∞ limit. FWHM = 2α·sqrt(2^(1/β) − 1).

**PSFEx and field-varying PSFs (clarified, pass 2).** For wide fields the PSF is
*not* constant — it broadens and elongates toward the corners (off-axis
aberrations, defocus, tracking). **PSFEx** models this: it samples bright,
unsaturated, isolated stars across the frame and fits a **pixel-basis** (or
Gauss-Laguerre "polar shapelet") PSF model whose coefficients **vary polynomially
with field position** (and optionally other "context" parameters). Reading the
PSFEx source (`.tmp/refs/psfex/src/diagnostic.c:184-225`), the **Moffat fit is a
*diagnostic*** — PSFEx fits a Moffat (and a "pixel-free" Moffat) to the
*reconstructed* PSF at a grid of field positions purely to *report* FWHM /
elongation / asymmetry (`moffat_fwhm_min/max` over the snapshot grid); it is **not**
the underlying PSF model. The takeaway for measurement: (1) a single global Gaussian
or Moffat is wrong across a wide field — interpolate the PSF, or at least fit each
star's own shape; (2) **aperture flux** (sum within a fixed radius) is model-free
but throws away SNR and needs an aperture correction, while **PSF flux** (amplitude
× model integral) is optimal but only as good as the PSF model — the right choice
depends on whether you trust your PSF more than your aperture-correction curve.

lumos implements per-star fits (not field-varying models) via a shared
Levenberg–Marquardt optimizer: `CentroidMethod::{WeightedMoments, GaussianFit,
MoffatFit{beta}}` (`config.rs:44`), `centroid/gaussian_fit/` (6-param, SIMD),
`centroid/moffat_fit/` (5–6 param, fixed or fit β), `lm_optimizer.rs` +
`linear_solver.rs`. It seeds the fit with 2 moment iterations then lets LM refine
independently (`measure.rs`/`centroid/mod.rs:304-401`). Documented accuracy:
~0.05 px (moments) vs ~0.01 px (fit). Because each star is fit independently, lumos
naturally accommodates a slowly field-varying PSF without an explicit interpolation
model — at the cost of more parameters per source and no shared regularization.

### 4.4 Marginal 1D Gaussian (DAOFIND's centroid)

DAOFIND fits separate 1D Gaussians to the **marginal** (row-summed and
column-summed) distributions of the **unconvolved** stamp (the marginal *shape* is
taken from the kernel; the data summed is the original image — see §5.4 correction)
— cheaper than a full 2D fit, and the basis of both the DAOFIND sub-pixel shift
(`x_centroid = x_max + dx`) and GROUND roundness (§5.4). photutils
`daofind_marginal_fit` / `_marginal_*` (`daofinder.py:596-709,862-896`).

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
`(I − sky).max(0)` over the measurement stamp (`compute_star`,
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
pixels in the aperture. Each noise term enters as a **variance** (they add in
quadrature because the underlying processes are independent): source shot noise
`N*`, sky shot noise `npix·N_sky`, dark shot noise `npix·N_dark`, read noise
`npix·R²`. A fifth term, **digitization/quantization noise** `npix·(g²/12)` (the
±½-ADU rounding of the ADC, gain `g` in e⁻/ADU), is usually folded into the
effective read noise and omitted; it matters only at very low read noise. **Two
regimes:**

- **Shot/sky-limited** (bright source or bright sky): SNR ∝ sqrt(N\*) ∝ sqrt(t).
- **Read-noise-limited** (faint source, short exposure): denominator ≈
  sqrt(npix)·R, so SNR ∝ N\* ∝ t (linear in exposure).

A subtle bias the CCD equation hides: `npix` is the *aperture* pixel count, treated
as exact, but the sky level subtracted under the source is itself estimated and
carries error `npix²·σ_sky²/n_sky` (n_sky = sky-annulus pixel count). Photometry
codes that use a small sky annulus add this term; lumos's global-mesh σ makes it
negligible. The equation also assumes Poisson ≈ Gaussian, valid for `N ≳ 10` e⁻.

lumos implements the normalized-domain equivalent in `compute_snr`. A
`NoiseModel` supplies `G`, electrons represented by one normalized pixel unit,
and read noise `R` in electrons:
`total_var = flux/G + npix·(σ_sky² + (R/G)²)` (shot + sky + read). Without a
noise model it uses the background-limited `total_var = npix·σ_sky²`, i.e.
`SNR = flux / (σ_sky·sqrt(npix))`. Two implementation details worth stating:
`npix` is the **full square stamp** `(2r+1)²`, not an
above-threshold/aperture pixel count — flux is summed over the same full stamp,
so signal and the `npix`-scaled noise terms are consistent but both span a
larger region than a tight aperture would; and `σ_sky` (`avg_noise`) is the mean
of the per-pixel noise map over the stamp's **outer ring** (`r² > (r−2)²`), a
local sky-noise estimate rather than a global scalar. Configure the model with
`G = electrons_per_adu × adu_per_normalized_unit`; passing the physical
electrons-per-ADU value directly is dimensionally wrong.

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
  When the source is unresolved (moment matrix near-singular,
  `x2·y2−xy² < 0.00694`) SExtractor adds `1/12 = 0.0833333` to each diagonal
  moment (the pixel-quantization variance of a uniform 1-px-wide distribution)
  and flags `OBJ_SINGU` (SEP `analyse.c:259-263`: `if (xm2·ym2 − xym·xym <
  0.00694) { xm2 += 0.0833333; ym2 += 0.0833333; … |= SEP_OBJ_SINGU; }`) — a
  subtlety lumos does not replicate. (Without it, an unresolved 1–2 px blob can
  drive `λ₂→0` and force `e→1`, spuriously failing the eccentricity gate.)

Eccentricity is the single most useful non-stellar reject: galaxies, blends, and
trailed stars have high e; round PSFs have e≈0.

### 5.4 DAOFIND sharpness & roundness (Stetson 1987)

**What Stetson 1987 actually defines** (parsed from the original PASP paper, §II.A,
`.tmp/papers/stetson1987.txt`). DAOFIND first builds `H`, the array of best-fit
Gaussian *central heights* — equivalent to convolving the data `D` with a
**"lowered" (zero-integral) truncated Gaussian** `G' = G − ⟨G⟩` so that
`Σ H = 0` (the constant + sloping sky cancels): `H = Σ(GD)−(ΣG)(ΣD)/n) /
(Σ(G²)−(ΣG)²/n)` with `σ = FWHM/2.355`. Detections are local maxima of `H` above
`Hmin` (the n·σ threshold). Stetson then gives **two** secondary indices:

- **sharp** = `D_central / ⟨D_neighbors⟩`-style ratio. Verbatim:
  `d_{i0j0} = D_{i0j0} − ⟨D⟩` (observed difference between the central pixel and
  the *mean of the remaining pixels used in the fit*), and
  **`sharp = d / H`** — the central intensity excess divided by the best-fit
  Gaussian height. *"For a very narrow profile, such as that caused by a cosmic-ray
  event, all of the intensity will be contained in the central pixel … hence sharp >
  1. … moderately-peaked objects, such as star images … should scatter about a value
  significantly less than unity and greater than zero."* Accept `0.2 < sharp < 1.0`.
- **round** (Stetson defines only ONE): *"compares the peakedness of the enhancement
  in the x-direction with that in the y-direction: the height of the best-fitting
  one-dimensional Gaussian function of x, hx, is compared to the height … of y, hy."*
  Charge-overflow columns give `hx ≫ 0, hy ≈ 0`; *"a roundness criterion readily
  distinguishes stars (round ≈ 0) from bad rows and columns (round ≈ ±2)."* The
  ±2 range fixes the normalization as **`round = 2·(hx − hy)/(hx + hy)`**. Accept
  `−1.0 < round < 1.0`. **This is what photutils later named GROUND.** Stetson's
  "round" has **no** symmetry/quadrant variant — the SROUND statistic was added by
  the IRAF DAOFIND reimplementation and inherited by photutils.

**What photutils computes** (verified line-by-line in `daofinder.py`):

- **sharpness** (`daofinder.py:580-594`) = `(peak − data_mean) / convdata_peak`,
  where `peak` and `data_mean` (mean over the kernel-masked footprint excluding the
  peak) come from the **unconvolved** cutout and `convdata_peak` is the peak of the
  **convolved** cutout = Stetson's `H`. So sharpness ≈ Stetson's `d/H`. Accept
  `(0.2, 1.0)`.
- **roundness1 = SROUND** (`daofinder.py:534-577`) — *symmetry* index. Zero the
  central pixel of the **convolved** cutout, split into four quadrants, and compute
  `sum2 = −q1 + q2 − q3 + q4`, `sum4 = Σ|cutout_conv|`, then
  **`roundness1 = 2·sum2/sum4`**. Circular → 0.
- **roundness2 = GROUND** (`daofinder.py:964-976`) — *marginal-Gaussian* index,
  **`roundness2 = 2·(hx − hy)/(hx + hy)`**, where hx,hy are least-squares
  amplitudes of 1D Gaussians (shaped from the *kernel's* marginal) fit to the
  marginal x/y distributions of the **unconvolved** image (docstring of
  `daofind_marginal_fit`: *"to the marginal x/y distributions of the original
  (unconvolved) image"*). x-extended → negative, y-extended → positive. Accept
  `(−1.0, 1.0)`; kernel `sigma_radius=1.5`. **This is Stetson's original `round`.**

> **FINAL VERDICT (pass 2): the "swapped" claim is CONFIRMED, with two extra
> caveats.** photutils + IRAF DAOFIND fix the convention as **roundness1 = SROUND
> (quadrant 2-fold/4-fold symmetry, on the convolved stamp)** and **roundness2 =
> GROUND (marginal-Gaussian height ratio, on the unconvolved stamp)** — and
> Stetson's *single* original `round` is exactly GROUND. lumos reverses both names
> (`star.rs:24-30`, `centroid/mod.rs:671-694`):
>
> - lumos `roundness1` is documented and computed as **GROUND-like**:
>   `safe_ratio(hx − hy, hx + hy)` (`centroid/mod.rs:673-675`) → matches photutils
>   **`roundness2`**, the *reverse* name. Two further deviations: lumos **drops the
>   factor of 2** photutils carries, and lumos takes `hx`/`hy` as the **max of the
>   raw marginal-sum profile** (`marginal_x.iter().fold(0, f64::max)`), *not* a
>   least-squares 1D-Gaussian amplitude — so it is a marginal-peak ratio, not a
>   marginal-Gaussian-fit ratio.
> - lumos `roundness2` is computed as **SROUND-like**: `hypot(asym_x, asym_y)` of
>   the left/right and top/bottom marginal asymmetries (`centroid/mod.rs:681-688`)
>   → fills the role of photutils **`roundness1`**, the *reverse* name. But it is an
>   *ad-hoc 1D-marginal asymmetry RMS*, not Stetson/IRAF's quadrant `2·sum2/sum4`
>   ratio. It is clamped to `[0, 1]`, so it can never go negative and cannot encode
>   the directional sign that SROUND carries.
> - **Both are computed on the *unconvolved* stamp** — `compute_star` is handed
>   the raw `pixels` plane (`centroid/mod.rs:528-535`, called from `measure.rs`),
>   never the matched-filter output. photutils computes sharpness and SROUND on the
>   **convolved** cutout (only GROUND uses the unconvolved image), so lumos diverges
>   on the image *and* the formula for its SROUND analogue.
>
> Net effect: both indices are still computed and both are gated by `is_round`
> (`star.rs:52-54`), so circular vs. elongated discrimination still works; but the
> field names are the inverse of every published reference, the directional sign of
> SROUND is lost, and neither index is bit-comparable to photutils/DAOFIND. The fix
> is purely cosmetic for detection but important for anyone cross-checking: swap the
> names, restore the factor of 2 on GROUND, fit (not max) the marginal Gaussians,
> compute the symmetry index as `2·sum2/sum4` over convolved quadrants, and feed the
> convolved stamp.

**Correction (pass 2) — GROUND is on the *unconvolved* image, not convolved.** The
prior pass said all three DAOFIND stats are "computed on the convolved image." That
is right for sharpness's denominator and for SROUND, but photutils explicitly fits
the GROUND marginals to the **original (unconvolved)** data (`daofind_marginal_fit`
docstring). Stetson's text agrees: `round` compares 1D-Gaussian heights `hx`,`hy`
fit "by a formula involving sums of the image-brightness values identical in form to
equation (1)" — i.e. of the *image* `D`, not of `H`.

### 5.5 Saturation, bleed, spikes & cosmic-ray flagging

- **Saturation:** flag stars whose peak exceeds a fraction of the ADC/well limit
  (SExtractor `SATUR_LEVEL` → `FLAGS` bit 2). Saturated cores have flat tops →
  biased centroids and meaningless flux. lumos: `Star::is_saturated(threshold)`,
  default 0.95 of normalized max (`star.rs:38`, applied in `filter.rs:46`).
- **Bleed columns / charge overflow:** a grossly overexposed star spills charge
  along the readout column (vertical bleed) or row, producing a long thin
  detection. This is exactly the case Stetson's `round` was *designed* to catch:
  *"false detections arise in the charge overflow columns and rows from grossly
  overexposed objects … much more elongated in x or y than star images … hx ≫ 0
  and hy ≈ 0 … round ≈ ±2"* (Stetson 1987, `.tmp/papers/stetson1987.txt:662-783`).
  So a hard eccentricity cut *plus* an axis-aligned roundness (GROUND) cut rejects
  bleed. SExtractor flags the saturated parent (`FLAGS` bit 2) and the deblend can
  shatter a bleed trail into fake companions — see §3.3.
- **Diffraction spikes:** four (or six) symmetric rays from a bright star
  (secondary-mirror spider or aperture edges). They are highly elongated but
  **inclined** to the axes, so an axis-aligned roundness misses them (Stetson notes
  `round` *"does not select against objects which are elongated in a direction
  inclined to the rows and columns"*); the eccentricity-from-eigenvalues cut (§5.3)
  is the rotation-invariant discriminator that does. lumos has no dedicated
  spike model; it relies on the eccentricity gate.
- **Cosmic rays:** sharp, often single-/few-pixel spikes far narrower than the
  PSF. Two discriminators: (a) **sharpness** — CRs have sharpness ≫ a real star
  (Stetson: a CR concentrates *"all of the intensity … in the central pixel … hence
  sharp > 1"*; lumos `is_cosmic_ray(max_sharpness)`, default 0.7, `star.rs:45`,
  `filter.rs:55`); (b) **Laplacian edge detection** (van Dokkum 2001,
  L.A.Cosmic): a CR's edges are sharper than any PSF-convolved source, so a
  Laplacian-filtered/SNR image flags CRs while *"reliably discriminating between
  poorly sampled point sources and cosmic rays"* — the right tool when the PSF is
  undersampled and a simple sharpness cut would reject real faint stars. The most
  robust CR rejection, though, is **multi-frame**: a CR appears in one sub only,
  so sigma-clipped stacking (Stage 5) removes it. For single-frame detection,
  sharpness is a cheap first pass; Laplacian is the rigorous upgrade.

### 5.6 Star–galaxy separation (CLASS_STAR / neural net)

For science catalogs SExtractor adds a dedicated **stellarity** index
`CLASS_STAR ∈ [0, 1]` (0 = galaxy/non-star, 1 = star), output of a small
pre-trained **neural network** (`default.nnw`). Per Holwerda's tutorial
(`.tmp/papers/holwerda_dummies.txt:2674-2979`), the net is fed the object's **7
isophotal areas** (`ISO0..ISO7`, areas above evenly-spaced levels), the **peak
intensity**, and the **`SEEING_FWHM`** — the seeing scale is essential because
"compact" is defined relative to the PSF. The output separates point sources
(stars: their isophotal areas shrink fast with level, like the PSF) from extended
sources (galaxies: shallow area-vs-level profile). CLASS_STAR degrades at faint
magnitudes and in crowding; modern pipelines replace it with `SPREAD_MODEL` (a
linear discriminant between the local PSF and a slightly broadened PSF) or a CNN.

For lumos, whose targets are stars and whose downstream consumer is *registration*
(which wants clean point sources), full star–galaxy classification is overkill: the
eccentricity + FWHM-outlier + sharpness gates already reject the obvious extended /
elongated objects. The seeing-relative idea is the transferable lesson — lumos's
auto-FWHM (§3 fwhm stage) plays the role of `SEEING_FWHM`, and a future
"compactness vs. measured FWHM" cut would be the cheap analogue of `SPREAD_MODEL`.

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
   1.4826·MAD; **3×3 median-filter the tile grid** (sky *and* σ; replacement
   gated by `BACK_FILTERTHRESH`, default 0 = unconditional);
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
   from second moments, and DAOFIND shape stats: **sharpness and SROUND on the
   convolved stamp, GROUND (marginal-Gaussian heights) on the unconvolved stamp**
   (§5.4).
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

**Pipeline (`detector/mod.rs:122-222`, `StarDetector::detect`):** prepare → background (+optional
iterative refine) → FWHM estimate → detect (matched filter + threshold + CCL +
deblend) → measure (parallel centroid + metrics) → filter. This matches the
canonical six-stage flow and is well-structured.

**What lumos gets right (matches best practice):**

- Tiled (64 px) sigma-clipped background with **natural bicubic** interpolation
  and **per-pixel σ** map (`background/mod.rs`), MAD-based σ (1.4826·MAD).
- **3×3 tile-grid median filter** on both the sky and σ grids
  (`tile_grid.rs:208-249`, run on every `compute()`) — matches SExtractor/SEP
  `filterback` at its default `BACK_FILTERTHRESH=0` (unconditional replacement).
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

1. **roundness1/roundness2 are swapped vs. the DAOFIND/photutils convention**
   (CONFIRMED pass 2 against Stetson 1987 + photutils source — see §5.4 verdict).
   lumos `roundness1` = GROUND-like `(hx−hy)/(hx+hy)` (= photutils `roundness2`),
   lumos `roundness2` = an asymmetry-RMS (≈ photutils `roundness1`), reversed names.
   Extra deviations: GROUND drops the factor of 2 and uses the marginal-sum *max*
   rather than a 1D-Gaussian *fit amplitude*; SROUND is a 1D-marginal asymmetry RMS
   clamped to `[0,1]` (loses the directional sign), not Stetson/IRAF's quadrant
   `2·sum2/sum4`; both are computed on the *unconvolved* stamp (photutils does
   sharpness + SROUND on the *convolved* stamp, GROUND on the unconvolved). Fix:
   swap the names, restore the ×2 on GROUND, least-squares-fit the marginals,
   compute SROUND as `2·sum2/sum4` over convolved quadrants
   (`star.rs:24-30`, `centroid/mod.rs:671-694`).
2. **Background mode estimator is median/MAD only — no `2.5·med−1.5·mean` mode and
   no 0.3 crowding switch.** lumos's per-tile statistic is the sigma-clipped
   *median* with `σ = 1.4826·MAD` (`tile_grid.rs:452`); SExtractor/SEP use the
   Pearson mode `2.5·med−1.5·mean` (falling back to median when `|mean−med|/σ ≥
   0.3`). Adding the mode (with the median fallback) would shave the residual
   positive-tail bias in uncrowded tiles, matching SExtractor/SEP exactly. This
   is now the single highest-value background-fidelity gap (the tile-grid median
   filter, previously flagged here, is in fact present — see §1.3).
3. **Multi-threshold contrast is relative to the *parent node* flux, not the
   *root/total* flux** as in SExtractor (`mod.rs:811-816` vs SEP
   `deblend.c:116`, `value0 = root.fdflux · mincont`). This changes split
   behavior; align with SExtractor's global `MINCONT·root_flux` bar if cross-tool
   consistency matters.
4. **Windowed centroid lacks the SExtractor factor-of-2 update acceleration**
   (`centroid/mod.rs:491` is the plain `Σwx/Σw`). Harmless to accuracy with 10
   iterations, but adding it matches XWIN and converges in 3–5 steps.
5. **Sharpness is `peak/core_flux` (3×3)** (`centroid/mod.rs:644-649`), an
   approximation of DAOFIND's `(peak − mean_neighbors)/Gaussian_height`. The
   DAOFIND form is more discriminating for CRs vs. undersampled stars; consider
   it, or add a **Laplacian (L.A.Cosmic) CR test** for undersampled data where a
   sharpness cut would reject real faint stars.
6. **Flux clamps negatives to 0** (`centroid/mod.rs:574`) — fine for detection,
   biases photometry; expose an unclamped flux for measurement use. SNR's
   `npix` is also the full square stamp `(2r+1)²`, not an aperture count (§5.2).
7. **No singular-moment 1/12 regularization / `OBJ_SINGU` flag** for unresolved
   sources (SEP `analyse.c:259-263`, threshold `x2·y2−xy² < 0.00694`) —
   FWHM/eccentricity of 1–2 px blobs can be ill-defined and over-reject.

None of these block correct detection; #1 (roundness naming) and #2 (background
mode) are the highest-value alignments with the reference implementations.

---

## Primary sources parsed (pass 2)

PDF/HTML fetched and read this pass (saved under `.tmp/papers/`, parsed with
`pdftotext`). Each line: source → takeaway → local file.

- **Stetson 1987, DAOPHOT/FIND, PASP 99 191** (`.tmp/papers/stetson1987.txt`, 32 pp,
  text layer clean). *The decisive source for the roundness verdict.* Settled:
  Stetson defines a **single** roundness, `round = 2·(hx−hy)/(hx+hy)` from marginal
  1D-Gaussian heights (= photutils GROUND, accept `−1<round<1`, bad rows/cols → ±2);
  `sharp = d/H` = (central pixel − neighbor mean) / best-fit Gaussian height (accept
  `0.2<sharp<1`); the detection image `H` is the convolution with a **lowered
  (zero-integral) Gaussian** so sky slope cancels. SROUND is **not** Stetson's — it
  is a later IRAF/photutils addition.
- **Bertin & Arnouts 1996, SExtractor, A&AS 117 393** (`.tmp/papers/bertin1996.pdf`,
  12 pp, downloaded from ADS — **scanned image, no text layer**, `pdftotext` yields
  only the bibcode). Could not quote directly; substituted the SExtractor C source
  (`.tmp/refs/sextractor`), the SExtractor readthedocs manual, Holwerda's tutorial,
  and SEP source — all of which trace to this paper.
- **Barbary 2016, SEP, JOSS 1(6) 58** (`.tmp/papers/sep_barbary2016.txt`). Confirmed
  the *"optimized matched filter for variable noise"* is a SEP feature **added beyond**
  SExtractor; SEP aims for SExtractor-compatible results from in-memory arrays.
- **Makovoz & Marleau 2005, MOPEX point-source extraction** (`.tmp/papers/makovoz2005.txt`,
  34 pp). Linear matched filter is optimal only for a single source / Gaussian noise;
  with many sources the optimal filter is non-linear; *"filtered images are used for
  detection only"* (corroborates the convolve-to-detect, measure-on-raw discipline);
  multi-threshold "passive deblending" by progressively raising the segmentation
  threshold.
- **Holwerda, *Source Extractor for Dummies*** (`.tmp/papers/holwerda_dummies.txt`,
  87 pp). Deblend criterion in words: branch counts > fraction (`MINCONT`) of **total
  island count** AND ≥1 other branch passes — corroborates the root-flux bar and the
  ≥2-children rule. CLASS_STAR neural net fed by 7 isophotal areas + peak + SEEING_FWHM.
  *Caveat:* its prose has the mode/median switch **backwards** ("mean in non-crowded,
  `2.5·med−1.5·mean` in crowded") — the C source is authoritative (mode when *not*
  crowded). Bicubic-spline background interpolation + `BACK_FILTERSIZE` confirmed.
- **SExtractor manual, PositionWin** (`.tmp/papers/positionwin.html`). XWIN/YWIN
  verbatim: window FWHM = `d₅₀`; explicit `+2·…` update; converges in 3-5 iters at
  `<2×10⁻⁴` px; accuracy *"very close to … PSF-fitting"* and *"close to the
  theoretical limit set by image noise."*

**Re-read cloned source this pass (cite file:line):** SEP `background.c:478-535`
(mode + 0.3 switch + histogram-interpolated median), `deblend.c:116,120-122,179,184`
(root-flux `value0`, exponential `pow(peak/thresh0,k/N)`, split test, ≥2-children);
SExtractor `back.c:698-699,735-737` + `preflist.h:264` (`BACK_PEARSON`=2.5 →
medfac/meafac), `filter.c:177-195` (`Σ|K|` normalization + `varnorm=√ΣK²`); photutils
`daofinder.py:534-577` (SROUND `2·sum2/sum4`), `:580-594` (sharpness), `:862-976`
(GROUND marginal-Gaussian fit on *unconvolved* image, `2·(hx−hy)/(hx+hy)`); PSFEx
`diagnostic.c:184-225` (Moffat as a *diagnostic*, not the PSF model); lumos
`star.rs:24-30`, `centroid/mod.rs:671-694` (roundness), `:491` (no XWIN ×2 —
plain `Σwx/Σw`), `convolution/mod.rs:99-117` (`√ΣK²`), `multi_threshold/mod.rs`
(parent-relative contrast), `config.rs:265,272,273,286,287,290` (defaults).

**Still unverifiable / could not parse:** Bertin & Arnouts 1996 itself (scanned
image — secondary sources cover all cited claims); Zackay & Ofek 2017 not
re-fetched this pass (carried from pass 1 with its published claims).

---

## Re-verification (pass 3)

Every claim above was re-checked against the cloned reference source and the
lumos tree, plus targeted online re-fetches. Corrections applied this pass:

- **lumos *does* median-filter its tile grid** (the largest fix). `tile_grid.rs`
  `apply_median_filter` (`:208-249`) runs on every `compute()` (`:87`), replacing
  each tile's median **and** σ with the 3×3 tile-neighborhood median (`:244-245`).
  SEP/SExtractor default `fthresh = 0.0` (`sep.pyx:390`; `preflist.h:62`,
  `BACK_FILTTHRESH 0.0`), so their `filterback` replacement is *also unconditional
  by default* — lumos matches it. The prior "no tile-grid median filter" gap was
  wrong; §1.3, the §8 "gets right" list, and the §8 gap list are corrected. Only
  residual deviations: lumos has no `fthresh` knob and skips grids < 3×3 tiles.
- **Singular-moment threshold is `< 0.00694`, not `0.0694`** — SEP
  `analyse.c:259` (`if (xm2·ym2 − xym·xym < 0.00694)`), then `+= 0.0833333`
  (= 1/12) and `SEP_OBJ_SINGU` (`:260-263`). Fixed in §5.3 and §8.
- **Background convergence is `<0.01%`, not `<0.02%`** — `EPS=1e-4` with guard
  `fabs(sig/sig1 − 1.0) > EPS` (SEP `background.c:467,490`). Fixed in §1.2.
- **§5.2 expanded:** lumos's `compute_snr` has *three* branches (gain+read,
  gain-only, neither), and `npix` is the full square stamp `(2r+1)²`, with
  `σ_sky` taken from the stamp's outer ring — now stated explicitly.

Re-confirmed unchanged (primary source re-read this pass, cite file:line):
SEP `background.c:528-530` (mode `2.5·med−1.5·mea` + 0.3 switch), `:508-511`
(histogram-interpolated median), `:625` (filterback `|med−back|>=fthresh`);
`deblend.c:116,120-122,179,184` (root `value0`, `pow(fdpeak/thresh0,k/xn)`,
branch test, `m>1`); SExtractor `filter.c:178-195` (`Σ|K|` norm + `varnorm=√ΣK²`,
zero-sum→variance fallback); SEP `deblend_nthresh=32,cont=0.005` (`sep.pyx:610`),
SExtractor `DEBLEND_NTHRESH 32 / DEBLEND_MINCONT 0.005` (`preflist.h:207-208`),
photutils `n_levels=32,contrast=0.001` watershed (`deblend.py:45,582`); photutils
`daofinder.py:533-577` (SROUND quadrants `2·sum2/sum4`, convolved), `:579-594`
(sharpness `(peak−data_mean)/convdata_peak`), `:862-976` (GROUND
`2·(hx−hy)/(hx+hy)`, lstsq marginal Gaussians on the **unconvolved** image),
`background/core.py:436,478` (`2.5·med−1.5·mean`+0.3), `:388-394` (MMM
`3·med−2·mean`), `sigma_radius=1.5` (`:211`); PSFEx `diagnostic.c:182-184`
(`psf_diagnostic` "by fitting Moffat models" — diagnostic only). Online: CCD
equation re-fetched (Dhillon PHY217 — `S·t·g / √(S·t·g + Ssky·t·g·npix +
Sdark·t·npix + R²·npix)`, read noise un-rooted; lumos's rearrangement is
algebraically equivalent); Moffat `FWHM = 2α√(2^(1/β)−1)`, Gaussian = β→∞ limit,
β=1 = Lorentzian (photutils/MNRAS). lumos source re-read in full this pass:
`star.rs`, `config.rs`, `centroid/mod.rs`, `convolution/mod.rs`,
`detector/mod.rs`, `detector/stages/{prepare,detect,measure,filter,fwhm}.rs`,
`background/{mod,estimate}.rs`, `tile_grid.rs`, `math/statistics/mod.rs` — all
file:line citations confirmed current.

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
