# Stretching Stacked Astrophotography Images

A reference for implementing a best-in-class non-linear **stretch** (tone curve) stage in
`lumos`. Stretching is the step that turns a *linear* stacked frame — where the nebula sits a
few ADU above a near-black background and bright stars are 10⁴–10⁵× brighter — into a *display*
image with strong, balanced contrast across that enormous dynamic range, without crushing faint
signal, blowing out star cores, or amplifying noise into a wall.

This document surveys how the major tools (PixInsight, Siril, GraXpert, Gnuastro, Astropy, GHS,
StarTools) actually do it, gives the **exact math** for each method, and ends with a concrete
recommendation and Rust sketch wired to `lumos`'s real types.

## Confidence legend

Every non-trivial claim is tagged so you know how much to trust it:

- ✅ **Verified** — confirmed in a multi-source adversarial research pass (≥2 of 3 independent
  verifier votes) against **primary** sources: the Lupton et al. 2004 paper, official
  PixInsight/GHS/Gnuastro docs, and reference code (Astropy `make_lupton_rgb`, LCOGT
  `auto_stretch`, HumVI, GraXpert). Formulas were independently re-derived and numerically checked.
- 📘 **Established** — standard, well-documented image-processing knowledge (CLAHE, gamma,
  workflow ordering). Sources were gathered but this specific statement wasn't in the verified set.
  High confidence, flagged for honesty.
- ⚠️ **Approximate** — conceptual or reconstructed detail (e.g. exact MaskedStretch iteration
  counts) where the primary source was thin.

---

## 0. TL;DR — what to build

For a custom pipeline you want **two or three** complementary stretches, all operating on a
**combined intensity channel** with **ratio-scaling color preservation**:

| Priority | Method | Why | Cost |
|---|---|---|---|
| **1** | **MTF / STF AutoStretch** | Fully automatic black point + midtones from image statistics. The "screen stretch" every tool ships. Great default/preview. | Trivial |
| **2** | **Color-preserving normalized asinh** | One intuitive knob (β). Best-in-class star-color and core retention. Robust, SIMD-friendly. The permanent workhorse stretch. | Trivial |
| **3** | **Generalized Hyperbolic Stretch (GHS)** | Power-user control: place the stretch exactly where the faint signal is (SP) and protect shadows/highlights (LP/HP). Superset of log/asinh/exp. | Moderate |

**Golden rules** (✅/📘):

1. Stretch **last** among the brightness-altering steps. Background extraction and color
   calibration assume *linear* flux and **must** run before the stretch.
2. Stretch a **single intensity** `I = (r+g+b)/3`, then scale each channel by `f(I)/I`. Never
   stretch R, G, B independently for the final image — that is what burns star cores white and
   makes an object's hue depend on its brightness.
3. Don't hard-clip the black point. Leave the background floating slightly above 0 so faint
   signal and the noise floor aren't crushed into a hard wall.

---

## 1. The problem, precisely

A calibrated, stacked frame is **linear**: pixel value ∝ photons collected. Two facts make it
unviewable as-is:

- **Dynamic range.** Background sky ≈ a small fraction of full scale; a magnitude-6 star is
  ~10⁵× brighter. A linear map to a display shows only the stars on black.
- **Signal location.** The interesting structure (nebulosity, galaxy arms, faint outer halos)
  lives in a *thin slice just above the background*, typically the bottom 1–5 % of the range.

Stretching is a **monotonic point transform** `f: [0,1] → [0,1]` that is **steep near the
background** (expanding that thin slice into lots of display contrast) and **shallow in the
highlights** (compressing the huge stellar range so cores don't saturate). Every method below is
a different shape of that curve. The art is steepness placement: too steep near black amplifies
noise; too shallow leaves the target buried.

A useful quantitative lens: the **local contrast gain** at input value `x` is just `f'(x)`, and
the **noise amplification** at the background is `f'(x_bkg)`. Good stretches make `f'` large where
*signal* is and keep it bounded where only *noise* is.

---

## 2. MTF / STF AutoStretch (PixInsight & Siril "screen stretch")

This is the automatic stretch behind PixInsight's STF, Siril's auto-stretch, and GraXpert's
preview. It is a **two-stage** transform: a linear clip-rescale, then a non-linear midtones curve.

### 2.1 The Midtones Transfer Function (MTF)

✅ The core curve is **not a gamma curve** — a documented misconception. It is a **rational
(Möbius) interpolation** pinned to pass through exactly `(0,0)`, `(m, 0.5)`, `(1,1)`:

```
                (m − 1) · x
MTF(m, x) = ───────────────────────
            (2m − 1) · x − m
```

where `m ∈ (0,1)` is the **midtones balance**:

- `m = 0.5` → identity (`MTF = x`).
- `m < 0.5` → **brightens** midtones (the astro case).
- `m > 0.5` → darkens midtones.

Sanity checks (all verified numerically): `MTF(m,0)=0`, `MTF(m,m)=0.5`, `MTF(m,1)=1`, monotonic
increasing for all `m ∈ (0,1)`. At `x=0.25`: `m=0.25 → 0.50` (brighter), `m=0.75 → 0.10` (darker).

> PixInsight docs, verbatim: *"A common misunderstanding is the assumption that MTFs are gamma
> curves… rational interpolation allows for much more flexible, accurate and controllable MTF
> curves."* (✅, corroborated by Siril docs and LCOGT reference code.)

**Noise-gain insight (re-derived here):** the slope of the MTF at the black end is

```
MTF'(m, 0) = (1 − m) / m
```

So an aggressive auto-stretch with `m = 0.1` multiplies background noise by **9×**; `m = 0.25`
by 3×. This is exactly why a single hard STF looks noisy, and why you favor a gentler curve
(or asinh/GHS) for the *permanent* stretch.

### 2.2 The two-stage pipeline

✅ STF is:

```
1. Linear clip-rescale:   v = clamp( (x − c0) / (c1 − c0), 0, 1 )
2. Non-linear midtones:   out = MTF(m, v)
```

`c0` = shadows clip (black point), `c1` = highlights clip (default `1.0`), `m` = midtones
balance. Pixels below `c0` go to 0, above `c1` go to 1, and the rest is rescaled to `[0,1]`
before the MTF. Constraint: `c0 ≤ c1`.

### 2.3 Auto-parameters from image statistics

✅ This is the clever part — `c0` and `m` are derived from the image's own median and a
dispersion statistic:

```
M       = median(image)
avgDev  = mean(|x − M|)                          // mean absolute deviation from the median

c0      = clamp(M + shadows_clip · avgDev, 0, 1) // default shadows_clip = −1.25
x0      = (M − c0) / (c1 − c0)                    // median after the linear rescale
m       = MTF(target_bkg, x0)                     // default target_bkg = 0.25
```

So the **black point sits ~1.25 average-deviations below the median**, and the **midtones balance
is whatever maps the rescaled median onto a target background brightness of 0.25**.

> The `−2.8` value you may have seen is a *more aggressive, non-default* shadows-clipping setting,
> not a different unit. PixInsight's and LCOGT's default is `−1.25`. (✅, dissent on this point
> was framing-only and resolved against primary code.)

**Why `m = MTF(0.25, x0)` works (re-derived here):** the Möbius MTF satisfies the self-inverse
identity

```
MTF( MTF(t, x0), x0 ) = t
```

i.e. setting `m = MTF(t, x0)` is *exactly* the midtones balance that maps `x0 → t`. Worked
example: `x0 = 0.05`, `t = 0.25` → `m = MTF(0.25, 0.05) = 0.13636`, and indeed
`MTF(0.13636, 0.05) = 0.25`. ✅ (verified numerically). So the auto-stretch literally solves
"what curve puts my median background at 0.25?"

### 2.4 Linked vs unlinked (color!)

📘 PixInsight computes STF **per channel by default** ("unlinked"): each of R/G/B gets its own
`c0, m`. Because each channel's background is independently pushed to 0.25, this acts as an
*automatic gray-background neutralization* — convenient for a screen preview, but it **distorts
color** (it bakes a per-channel white balance into the tone curve).

For a stacked, color-calibrated image you want the **linked** stretch: compute one `c0, m` from
the **combined luminance/intensity** and apply it identically to all three channels, preserving
the color balance you established during calibration. **Use unlinked only for an on-screen preview;
use linked (or the asinh/GHS color-preserving form in §4) for anything permanent.**

---

## 3. Arcsinh (asinh) stretch — Lupton et al. 2004

✅ The asinh stretch is the gold standard for **color and star-core fidelity**. It originated for
SDSS true-color composites (Lupton, Blanton, et al. 2004, *PASP* 116:133, DOI 10.1086/382245) and
is implemented in Astropy (`make_lupton_rgb`), Gnuastro, HumVI, and many pipelines.

### 3.1 The curve

✅ The base function is the inverse hyperbolic sine:

```
F(x) = asinh(x / β)
```

with the beautiful dual behavior that makes it ideal for astro:

```
asinh(x) ≈ x          for x ≪ 1   → linear near black: faint features preserved, low noise gain
asinh(x) ≈ ln(2x)     for x ≫ 1   → logarithmic in highlights: spiral arms & star cores compressed
```

`β` (the *softening* parameter) sets the crossover between the linear and logarithmic regimes.

### 3.2 Normalized `[0,1]` form (recommended for a normalized pipeline)

Since `lumos` data is already in `[0,1]`, the cleanest implementable form pins `0→0` and `1→1`:

```
              asinh(x / β)
f(x) = ──────────────────────
              asinh(1 / β)
```

- **One knob, β.** Small β → aggressive, log-like (big faint boost). Large β → near-linear.
  As `β → ∞`, `f(x) → x`.
- Slope at black: `f'(0) = (1/β) / asinh(1/β)` — i.e. the noise gain is *finite and tunable*,
  and unlike the MTF it **decreases smoothly** toward the highlights.

📘 You can pick β automatically from statistics: choose β so the background median maps to a
target display level (e.g. `f(M) ≈ 0.15–0.25`), solved by a few Newton iterations or a bisection
on β — directly analogous to STF's `target_bkg`.

### 3.3 Astropy `LuptonAsinhStretch` (to match a known reference exactly)

✅ If you want bit-comparable output with Astropy:

```
soften = Q / stretch
slope  = 0.1 / asinh(0.1 · Q)
y = asinh(x · soften) · slope          // defaults: Q = 8, stretch = 5
```

This is the same asinh family with a different knob mapping (`Q` ≈ brightness of bright features,
`stretch` ≈ linear scale). The original paper's user-facing parameterization is
`f(x) = asinh(αQ(x − m)) / Q` with display max `M = m + sinh(Q)/(αQ)`. ✅

### 3.4 Gnuastro / HumVI variants

✅ Same family, different normalization constants:

- **Gnuastro** `color-faint-gray`: `f(I) = asinh(qbright · stretch · I) / qbright`.
- **HumVI** (MIT-licensed, portable): `I = r+g+b`; `factor = asinh(α·Q·I)/(Q·I)`; then
  `r' = factor·r`, etc. Tuning: set `α` at `Q=1` until noise is just visible, raise `Q` to
  brighten features.

All three differ only by a normalization constant; pick whichever knobs you want to expose.

---

## 4. Color preservation — the single most important idea

✅ **This is why asinh/GHS beat per-channel stretches, and it is independent of the curve shape.**
Apply the non-linear curve to **one combined intensity**, then scale all channels by the same
factor:

```
I      = (r + g + b) / 3                  // combined intensity
factor = f(I) / I                         // f = any monotonic stretch (asinh, GHS, MTF…)
R = r · factor
G = g · factor
B = b · factor
```

Because all three channels are multiplied by the **same scalar**, their ratios `r:g:b` — i.e. the
**hue and saturation** — are preserved exactly. Only the *intensity* is remapped.

> Lupton 2004, verbatim: *"a given color (i.e. value of g−b and r−g) is now mapped to a unique
> color… the intensity is clipped at unity, but the color is correct,"* and the warning about the
> alternative: *"for any non-linear function F, an object's color in the composite image depends
> upon its brightness."* ✅

**Per-channel stretching is the anti-pattern** ✅: when you stretch R, G, B independently, a bright
star saturates the red channel first, then green, then blue — so it marches *white* as it
brightens, and every object's displayed hue becomes a function of its brightness. The combined-
intensity method makes color **brightness-independent**.

### 4.1 Hue-preserving highlight handling

✅ When `factor` pushes a channel above 1 (a bright star where one channel dominates), **do not
clip channels independently** — that re-introduces the hue shift. Instead divide all three by
their max:

```
maxc = max(R, G, B)
if maxc > 1.0 {  R /= maxc;  G /= maxc;  B /= maxc  }   // proportional, ratio-preserving
```

This caps brightness at white-point while keeping the star's true color. (Astropy's
`make_lupton_rgb` does exactly this; LSST additionally smooths *detector-saturated* cores, which
is an optional refinement, not a contradiction.) ✅

### 4.2 The three color strategies, ranked

| Strategy | What it does | Verdict |
|---|---|---|
| **Per-channel** (independent R/G/B) | Separate curve per channel | ❌ Hue depends on brightness; burns star cores white. Avoid for final. |
| **Luminance-only** | Stretch L of a Lab/YCbCr split, keep chroma | OK, but chroma noise untouched and saturation can feel flat after a big L boost |
| **Combined-intensity ratio scaling** (`f(I)/I`) | One curve, scale all channels | ✅ Best. Exact hue preservation, brightness-independent color, clean highlight handling |

---

## 5. Generalized Hyperbolic Stretch (GHS)

✅ GHS (Dave Payne / Mike Cranfield; in Siril natively and as a PixInsight plugin) is the most
*controllable* family. A single parameter `b` selects among five classical transforms, and a
piecewise construction adds a **symmetry point** and **shadow/highlight protection**.

### 5.1 The base transform family

✅ For the stretch factor `D` and local-intensity parameter `b`, the base curve `T(x)` is:

| `b` | Family | `T(x)` |
|---|---|---|
| `b = −1` | logarithmic | `ln(1 + D·x)` |
| `b = 0` | exponential | `1 − e^(−D·x)` |
| `b = 1` | harmonic | `1 − (1 + D·x)^(−1)` |
| `b > 0, b ≠ 1` | hyperbolic | `1 − (1 + b·D·x)^(−1/b)` |
| `b < 0, b ≠ −1` | integral | `(1 − (1 − b·D·x)^((b+1)/b)) / (D·(b+1))` |

✅ Notably, **`b ≈ −1.4` closely approximates the asinh stretch** — so GHS is a strict superset
of §3, with extra control. `b → ` more negative = gentler/more log-like; `b` large positive =
harder hyperbolic.

### 5.2 The stretch factor `D`

✅ `D` is not entered directly. The control value `x_D` maps through

```
D = e^(x_D) − 1
```

(PixInsight uses `x_D ∈ [0, 20]`; Siril exposes a direct strength in `[0, 10]` — same math,
different UI). So effective stretch strength grows **exponentially** with the slider.

### 5.3 The full piecewise transform (SP, LP, HP)

✅ The base `T` is wrapped into a four-segment, C¹-continuous curve built around a **symmetry
point** `SP` with a **shadow-protect point** `LP` (`0 ≤ LP ≤ SP`) and **highlight-protect point**
`HP` (`SP ≤ HP ≤ 1`):

```
            ┌ T1(x) = T2'(LP)·(x − LP) + T2(LP)      for  0  ≤ x < LP   (linear: protect shadows)
            │ T2(x) = −T(SP − x)                     for  LP ≤ x < SP   (180° rotation about SP)
q(x)  =     │ T3(x) =  T(x − SP)                     for  SP ≤ x < HP   (the stretch itself)
            └ T4(x) = T3'(HP)·(x − HP) + T3(HP)      for  HP ≤ x ≤ 1    (linear: protect highlights)

out(x) = (q(x) − q(0)) / (q(1) − q(0))              // normalize to [0,1]
```

The slopes `T2'(LP)` and `T3'(HP)` at the join points make it slope-continuous (no kinks). The
intuition for the knobs:

```
   noise floor        faint nebula          star cores
   │                  │ │                    │
   ▼                  ▼ ▼                    ▼
 ──┴──────────────────┴─┴────────────────────┴────────►  input
   0        LP        SP                     HP        1
   └─ linear ─┘  └── full hyperbolic ──┘  └─ linear ─┘
   (protect       (max contrast here)    (protect cores
    shadows)                              from blowout)
```

- **SP** — put it just above the background, on the faint signal you want to expand. The curve is
  steepest here.
- **LP** — below it, the transform is linear, so **background noise is not amplified**.
- **HP** — above it, linear again, so **star cores keep their gradient** instead of saturating.

This LP/SP/HP control is precisely what lets GHS maximize mid-tone contrast *without* the noise
and core-blowout costs of a global MTF.

### 5.4 GHS color preservation

✅ Identical ratio-scaling principle as §4: compute a weighted average `z` of R/G/B, transform to
`T(z)`, scale every channel by `T(z)/z`. Default weights are equal (straight average); optionally
the D50 luminance coefficients of the working space. Channel ratios — hence color and saturation —
are preserved.

---

## 6. Histogram-based methods (use sparingly)

### 6.1 Global histogram equalization — why it's *bad* for astro

📘 Histogram equalization sets the transfer function to the **normalized CDF** of the histogram,
producing a (near-)uniform output histogram:

```
T(x) = CDF(x) = (1/N) · Σ_{k ≤ x} hist(k)
```

For a natural photo this maximizes global contrast. For an astro frame it is **actively harmful**:
the histogram is dominated by a giant spike of background-sky pixels near black. Equalization
allocates output dynamic range *in proportion to pixel population*, so it spends almost the entire
output range stretching apart the **background noise** (the most populous bin) and crushes the
sparse, faint nebula and stars into a few output levels. It inverts the priority you want. **Never
use global HE as the primary stretch.**

### 6.2 CLAHE — Contrast-Limited Adaptive Histogram Equalization

📘 CLAHE (Zuiderveld 1994; in Siril, OpenCV) fixes the two failures of HE for *local contrast
enhancement* — used **after** the main stretch, on nebulosity, not as the global stretch:

1. **Adaptive (tiled):** compute a separate equalization per tile (e.g. 8×8 grid), then
   **bilinearly interpolate** the per-tile transfer functions across pixels to avoid block seams.
2. **Contrast-limited:** before building each tile's CDF, **clip** the histogram at a *clip limit*
   and redistribute the clipped excess uniformly. This caps the transfer-function slope, which
   **caps noise amplification** — the single most important parameter for astro.

```
for each tile:
    h = histogram(tile)
    excess = Σ max(h[k] − clip_limit, 0)
    h[k] = min(h[k], clip_limit) + excess / num_bins   // redistribute
    cdf  = normalize(cumsum(h))
output(pixel) = bilinear_interp(cdf_tiles_around(pixel))(pixel_value)
```

Keep the clip limit **low** for astro (high limits reintroduce HE's noise blowup). Treat CLAHE as
a local-contrast finisher, not a stretch.

---

## 7. Classic point curves (building blocks)

📘 These are the textbook primitives; useful as components, manual tweaks, and `f` candidates for
the ratio-scaling color machinery.

```
Levels (linear):   f(x) = clamp((x − black) / (white − black), 0, 1)
Gamma:             f(x) = x^(1/γ)              // γ > 1 brightens midtones (display-encode convention)
Logarithmic:       f(x) = ln(1 + μ·x) / ln(1 + μ)    // μ controls strength; 0→0, 1→1
Power-law:         f(x) = x^p                  // p < 1 brightens, p > 1 darkens
```

Notes:
- **Gamma** is the crude classic stretch (`γ ≈ 2.2–4` for a first push). It works but its slope is
  *unbounded at 0* (`f'(0) → ∞` for `γ > 1`), so it amplifies the deepest shadows/noise hardest —
  asinh and GHS are strictly better behaved there.
- **Levels** (the black/white-point linear map) is the front half of STF (§2.2) and the right tool
  for **black-point placement** independent of the curve.
- **Curves** in editors are just an interactive spline `f`; for color, run the spline through the
  §4 ratio machinery rather than per channel.

---

## 8. Masked & iterative/statistical stretches

⚠️ **MaskedStretch (PixInsight)** applies many *small* MTF stretches iteratively toward a target
background, using the image itself as a **mask** so that already-bright pixels (stars) receive
*less* stretch each pass. The result keeps star cores tight and small instead of bloating them —
at the cost of slightly flatter global contrast and a tendency to darken/desaturate. The exact
iteration count and target-background logic weren't pinned to a primary source in this research;
treat the mechanism as conceptually correct but the constants as approximate.

📘 **Statistical / iterative stretch** (the family STF belongs to) = "measure median + dispersion,
pick parameters that drive the background to a target level, apply." GHS and asinh can be driven
the same way (solve for β or D so `f(median) ≈ target`). This auto-parameterization is what makes
a stretch *headless-pipeline friendly* — exactly what `lumos` wants.

The modern alternative to masking for protecting cores is simply **GHS with an HP highlight-protect
point** (§5.3) or the **asinh ratio-scaling highlight guard** (§4.1) — both cleaner than iterative
masking.

---

## 9. Where the stretch sits in the pipeline

📘 This ordering is near-universal across PixInsight, Siril, and APP. The hard constraint:
**everything that assumes linear flux must run before the stretch.**

```
LINEAR DOMAIN (flux-proportional — order matters):
  1. Calibrate         (dark / flat / bias)              ← lumos: calibration_masters
  2. Register          (align frames)                    ← lumos: registration
  3. Integrate/Stack   (combine, reject outliers)        ← lumos: stacking / drizzle
  4. Crop              (trim stacking edge artifacts)
  5. Background / gradient extraction  (DBE/ABE/GraXpert) ── MUST be linear: fits a flux model
  6. Color calibration (SPCC / PCC, white balance)       ── MUST be linear: ratios of fluxes
  7. (optional) Deconvolution, linear noise reduction

  ────────────────  ★ STRETCH HERE ★  ────────────────   ← lumos: NEW post_processing module

NON-LINEAR DOMAIN (display-referred):
  8. Curves / fine contrast
  9. Local contrast (CLAHE / LHE / HDR multiscale)        ← §6.2, §10
 10. Saturation / color boost
 11. Star reduction / star removal
 12. Final sharpening, denoise, export
```

Why the constraints (📘):
- **Background extraction** fits a smooth model to the sky and subtracts it; that model is only
  valid while gradients are *additive and linear*. Stretch first and the subtraction is wrong.
- **Color calibration** equalizes channels by *ratios of fluxes*. A non-linear curve destroys those
  ratios, so calibration must precede the stretch (this is also why the §4 method preserves the
  ratios you calibrated).
- **Star reduction** works best on stretched (non-linear) data where stars are compact.

For `lumos` today the pipeline ends at stack/drizzle; the stretch is a **new stage slotting in at
★**, consuming the combined `AstroImage` before export.

---

## 10. Maximizing contrast without wrecking the image

📘/✅ Practical levers, in priority order:

1. **Black-point placement — don't over-clip.** Set the black point *just left of* the histogram's
   background peak, leaving a small gap so the noise floor isn't crushed into a hard, flat-black
   wall (which destroys faint outer signal and looks artificial). STF's `−1.25·avgDev` default
   (≈ 1σ below median, §2.3) is a good automatic starting point; back it off (smaller magnitude)
   if faint halos disappear.
2. **Stretch gently, possibly in two passes.** One violent stretch maximizes noise gain (`f'(0)`
   is largest for the most aggressive curve). A gentle asinh/GHS, optionally repeated, reaches the
   same brightness with less noise because each pass's slope at black is smaller.
3. **Protect star cores.** Use the §4 color-preserving form with the highlight guard, or GHS `HP`.
   Never per-channel clip.
4. **Local contrast for the finish.** After the global stretch, recover *texture* in nebulosity
   with **HDR multiscale transform** (decompose into dyadic wavelet scales, compress the
   large-scale/low-frequency component while preserving fine detail — this tames bright cores so
   you can push faint structure) and/or **low-clip CLAHE / local histogram equalization**. These
   add *local* contrast that a global point curve mathematically cannot.
5. **Denoise around the stretch, not instead of it.** Linear-domain noise reduction (or modern
   AI denoise) before the stretch, gentle chroma denoise after — because the stretch's steep
   region multiplies background noise the most.
6. **Quantify it.** Track the background noise gain `f'(x_bkg)` and the highlight compression
   `f'(x_star)`. A good stretch keeps the first modest (≲ a few ×) while the second is small
   (cores compressed). The MTF gain `(1−m)/m` and asinh's bounded, monotically-decreasing slope
   are why asinh/GHS win on this metric.

---

## 11. Recommendation & Rust implementation for `lumos`

### 11.1 What to build

A new `lumos/src/post_processing/` (or `display/`) module exposing a `stretch` submodule with:

1. `MidtonesTransfer` + `auto_stf_params` — the §2 STF for an automatic baseline/preview and for
   deriving an automatic black point. **Build first** (it's ~20 lines and reuses your stats).
2. `asinh_stretch` (mono) + `asinh_stretch_color` (color-preserving, §3.2/§4) — the **permanent
   workhorse**.
3. `GhsParams` + `ghs_stretch` — the §5 power-user curve. **Build last.**

All operate in-place on `AstroImage`'s planar `Buffer2<f32>` channels in `[0,1]`. Reuse the
existing `math::statistics` helpers (`median_f32_mut`, `mad_f32_with_scratch`, `mad_to_sigma`,
`sigma_clipped_median_mad`).

**One adaptation note (✅, re-derived):** the canonical STF uses *mean* absolute deviation
(`avgDev`), but `lumos` has *median* absolute deviation (MAD). For a normal distribution
`avgDev ≈ 0.798·σ` and `MAD ≈ 0.675·σ`, so the default black point `−1.25·avgDev ≈ −1.0·σ`. Since
`mad_to_sigma(mad) = 1.4826·mad = σ`, just express the black point as **"~1σ below the median"**
using your existing helper — same result, no new statistic needed.

### 11.2 Core curves (drop-in)

```rust
/// Midtones Transfer Function — the rational (Möbius) STF curve. `m` is the midtones
/// balance in (0,1): m = 0.5 is identity, m < 0.5 brightens. Verified against PixInsight/Siril.
#[inline]
fn mtf(m: f32, x: f32) -> f32 {
    // (m-1)x / ((2m-1)x - m); guard the m≈0.5 / x∈{0,1} fixed points implicitly hold.
    if x <= 0.0 { return 0.0; }
    if x >= 1.0 { return 1.0; }
    ((m - 1.0) * x) / ((2.0 * m - 1.0) * x - m)
}

/// Normalized asinh stretch f: [0,1] -> [0,1], pinned 0->0, 1->1.
/// `beta` is the softening knob: small = aggressive/log-like, large = near-linear.
#[inline]
fn asinh_norm(beta: f32, x: f32) -> f32 {
    let denom = (1.0 / beta).asinh(); // precompute per-image in practice
    (x / beta).asinh() / denom
}
```

### 11.3 Automatic STF parameters (reusing `lumos` stats)

```rust
#[derive(Debug, Clone, Copy)]
pub struct StfParams {
    pub black: f32,    // c0
    pub white: f32,    // c1 (default 1.0)
    pub midtones: f32, // m
}

/// Compute STF parameters from a channel's pixels. `samples` is scratch the caller owns
/// (it gets partially sorted). `shadow_sigmas` ~ 1.0 reproduces PixInsight's default black
/// point; `target_bkg` ~ 0.25 is the target background brightness.
pub fn auto_stf_params(samples: &mut [f32], scratch: &mut Vec<f32>,
                       shadow_sigmas: f32, target_bkg: f32) -> StfParams {
    let median = median_f32_mut(samples);
    let mad = mad_f32_with_scratch(samples, median, scratch);
    let sigma = mad_to_sigma(mad);                       // = 1.4826 * mad

    let black = (median - shadow_sigmas * sigma).clamp(0.0, 1.0);
    let white = 1.0_f32;
    let x0 = (median - black) / (white - black);         // rescaled median
    let midtones = mtf(target_bkg, x0);                  // self-inverse identity: maps x0 -> target_bkg
    StfParams { black, white, midtones }
}

pub fn apply_stf(channel: &mut Buffer2<f32>, p: &StfParams) {
    let inv = 1.0 / (p.white - p.black);
    for v in channel.pixels_mut() {
        let lin = (( *v - p.black) * inv).clamp(0.0, 1.0);
        *v = mtf(p.midtones, lin);
    }
}
```

### 11.4 Color-preserving asinh (the recommended permanent stretch)

```rust
/// Color-preserving stretch: apply `f` to combined intensity, scale all channels by f(I)/I,
/// then a hue-preserving highlight guard. `f` is any monotonic [0,1]->[0,1] curve.
pub fn stretch_color_preserving(img: &mut AstroImage, f: impl Fn(f32) -> f32) {
    match img.pixel_data_mut() {
        PixelData::L(buf) => {
            for v in buf.pixels_mut() { *v = f(*v); }     // mono: direct
        }
        PixelData::Rgb([r, g, b]) => {
            let (rp, gp, bp) = (r.pixels_mut(), g.pixels_mut(), b.pixels_mut());
            for i in 0..rp.len() {
                let (ri, gi, bi) = (rp[i], gp[i], bp[i]);
                let intensity = (ri + gi + bi) * (1.0 / 3.0);
                if intensity <= 0.0 { rp[i] = 0.0; gp[i] = 0.0; bp[i] = 0.0; continue; }
                let factor = f(intensity) / intensity;     // same scalar for all channels
                let (mut ro, mut go, mut bo) = (ri * factor, gi * factor, bi * factor);
                let maxc = ro.max(go).max(bo);             // hue-preserving highlight guard
                if maxc > 1.0 { let s = 1.0 / maxc; ro *= s; go *= s; bo *= s; }
                rp[i] = ro; gp[i] = go; bp[i] = bo;
            }
        }
    }
}

// Usage: pick beta from stats (e.g. so f(median) ≈ 0.2), then:
// let denom = (1.0 / beta).asinh();
// stretch_color_preserving(&mut img, |x| (x / beta).asinh() / denom);
```

(`pixel_data_mut` / the exact accessor names follow `astro_image/mod.rs`; the recon found
`channel_mut(c)` and `PixelData::{L,Rgb}` — adapt to whichever is public.)

### 11.5 GHS (when you add power-user control)

Implement the §5.1 base `T(x; b, D)` as a `match` on `b`'s regime, wrap it in the §5.3 four-segment
piecewise with `SP/LP/HP`, normalize by `(q(1)−q(0))`, and drive color through the **same**
`stretch_color_preserving` machinery (pass the GHS curve as `f`). Default `b ≈ −1.4` gives you an
asinh-equivalent with SP/LP/HP control on top.

### 11.6 Tests (per `lumos` conventions)

- **MTF fixed points:** `mtf(m,0)=0`, `mtf(m,m)=0.5`, `mtf(m,1)=1` for several `m`; assert
  monotonicity on a sweep.
- **Self-inverse identity:** `mtf(mtf(t,x0), x0) ≈ t` for a grid of `(t,x0)` — proves §2.3.
- **asinh limits:** `asinh_norm(β,0)=0`, `=1` at 1; `β→∞` ⇒ ≈ identity; small `β` ⇒ strong boost
  (assert `f(0.02) > 0.2` for a chosen aggressive β — hand-computed).
- **Color preservation:** synth RGB pixel with known ratio (e.g. `2:1:1`); after
  `stretch_color_preserving`, assert the *ratio* is unchanged below the highlight guard, and that a
  super-unity case divides by max (hue ratio still preserved). This is the key correctness test.
- **Parameter sensitivity:** assert aggressive vs gentle β produce different outputs at the same
  input, and that aggressive maps the background brighter (`A != B`, `A > B`).
- **Auto-STF:** on a synthetic frame with known median/MAD, hand-compute `black`, `x0`, `midtones`
  and assert exact values.

---

## 12. Method comparison at a glance

| Method | Curve | Auto? | Color-safe | Core-safe | Noise behavior | Use for |
|---|---|---|---|---|---|---|
| **STF / MTF** | rational Möbius | ✅ (median+MAD) | linked only | weak | gain `(1−m)/m` at black | auto preview, baseline |
| **asinh (norm.)** | `asinh(x/β)` | ✅ (solve β) | ✅ ratio | ✅ log highlights | bounded, decreasing slope | **permanent workhorse** |
| **GHS** | hyperbolic family | semi (SP/D) | ✅ ratio | ✅ HP protect | tunable via LP | power-user final stretch |
| **Gamma** | `x^(1/γ)` | no | per-channel ❌ | poor | `f'(0)→∞` | quick/legacy only |
| **Hist. EQ** | CDF | ✅ | ❌ | ❌ | amplifies background ❌ | **don't** (global) |
| **CLAHE** | tiled clipped CDF | semi | n/a (luminance) | n/a | clip-limited | local contrast finish |

---

## 13. Sources

Primary (✅ verified against these):

- Lupton, Blanton, et al. 2004, *PASP* 116:133 — "Preparing Red-Green-Blue Images from CCD Data."
  DOI 10.1086/382245 · `astro.princeton.edu/~rhl/Papers/truecolor.pdf` · arXiv astro-ph/0312483
- PixInsight — *HistogramTransformation / MTF* reference docs (`pixinsight.com/doc/tools/HistogramTransformation`)
- Siril — *Histogram Transformation* & *GHS* docs (`siril.readthedocs.io`, `siril.org/tutorials/ghs`)
- GHS reference — `ghsastro.co.uk/doc/tools/GeneralizedHyperbolicStretch` (Payne/Cranfield);
  code: `github.com/mikec1485/GHS`, `GHS-Module`
- Astropy `make_lupton_rgb` / `LuptonAsinhStretch` (`docs.astropy.org`, source on GitHub)
- LCOGT `auto_stretch` (`github.com/LCOGT/auto_stretch`) — reference STF implementation
- HumVI (`github.com/drphilmarshall/HumVI`, MIT) — Lupton+Hogg&Wherry composite
- Gnuastro `astscript-color-faint-gray` (`gnu.org/software/gnuastro`); Infante-Sainz & Akhlaghi
  2024, arXiv 2401.03814
- GraXpert `stretch.py` (`github.com/Steffenhir/GraXpert`)

Supporting (📘 standard knowledge):

- Zuiderveld 1994, "Contrast Limited Adaptive Histogram Equalization," *Graphics Gems IV*
  (`github.com/erich666/GraphicsGems/blob/master/gemsiv/clahe.c`); Siril CLAHE docs;
  Wikipedia "Adaptive histogram equalization"
- PixInsight HDR/HDRMultiscaleTransform and Masked Stretch tutorials (`pixinsight.com/tutorials`)

---

*Research method: 6 search angles → 26 sources fetched → 123 claims extracted → 25 adversarially
verified (3-vote, ≥2/3 to confirm), 25/25 confirmed, 0 refuted. Items tagged 📘/⚠️ were gathered
but fell outside the 25-claim verification budget; they reflect standard practice, not run-verified
claims. All ✅ formulas were independently re-derived and numerically checked.*
