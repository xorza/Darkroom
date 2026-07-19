# Background / gradient extraction

How deep-sky astrophotography models and removes the smoothly-varying unwanted background — light-
pollution gradients, sky glow, moon glow, residual vignetting, amp glow — *without* eroding real
large-scale signal (extended nebulosity, galaxy halos, integrated flux nebulae). The research /
reference for the (not-yet-built) `background_extraction` module, in the family of
`color_calibration/README.md`, `denoise/README.md`, and `docs/image-stretching.md`.

**Distinct from background *neutralization*** (`color_calibration::neutralize_background`, which only
shifts per-channel *offsets* to a common level). This models and removes a **spatial surface** — the
gradient across the frame.

## Confidence legend

- ✅ **Verified** — confirmed in a multi-source adversarial pass (≥2/3 votes) against **primary**
  sources: the SExtractor (Bertin & Arnouts) docs, the photutils source + docs, the Siril background
  docs, and Eberly's *Thin Plate Splines* derivation (Geometric Tools).
- 📘 **Established** — standard domain/numerics knowledge (SVD least squares, the
  pipeline-ordering rationale, the min-divisor guard) — high confidence, but outside the
  adversarially-verified set here.
- ⚠️ **Open** — flagged by the research as unconfirmed (PixInsight DBE/ABE & GraXpert internals).

---

## 0. TL;DR — what to build

A **three-step** operation on the linear master:

```
1. Estimate background   — robust sky value over a grid of tiles (or sample points), rejecting
                           stars AND bright real signal (sigma-clip + object masking)
2. Fit a smooth surface  — low-order polynomial (safe) OR regularized thin-plate spline (flexible),
                           OR just bicubic-interpolate the tile grid (SExtractor)
3. Remove it             — SUBTRACT for additive gradients (light pollution / moon / sky glow);
                           DIVIDE by a normalized model for multiplicative residuals (vignetting)
```

| Tier | Method | Notes |
|---|---|---|
| **Constrained / safest** ✅ **implemented** | **low-order 2D polynomial (degree 1–4) → subtract/divide** | `mod.rs` — the shipping core; least likely to eat nebulosity |
| Pragmatic core | **SExtractor tiled-mesh model → subtract** | lumos *already has* this estimator (see §8); not yet wired here |
| Flexible | **regularized thin-plate-spline (TPS) surface → subtract** | Siril's RBF model; λ controls smoothness |
| Accurate (heavy) | **AI/CNN background prediction** (GraXpert) | ⚠️ ML extension, not a from-scratch impl |

**Pipeline slot:** 📘 **linear domain, after stacking, before photometric color calibration and the
stretch.** A new `background_extraction` stage:
`stack → [background_extraction] → color-calibration → denoise → stretch`.

---

## 1. Where it belongs, and why linear

📘 Run it on **linear** data, after stacking, before colour calibration and stretch:

- The gradient (light pollution, sky glow) is **additive in linear flux**. Modeling and subtracting it
  is only correct while the data is still linear — after a non-linear stretch the gradient is no
  longer a simple additive surface.
- Colour calibration assumes a neutral background; a gradient biases the per-channel background
  estimates, so gradient removal **precedes** calibration.
- **Vs flat-fielding:** the master flat (calibration) removes the *optical* vignetting/dust. Background
  extraction's *division* mode is for **residual** multiplicative effects the flat missed (✅ Siril:
  "vignetting is ideally handled by master-flat correction"). Most of the time you **subtract** (the
  dominant term is additive light pollution), not divide.

⚠️ The research could not pin the exact ordering rationale to a primary source — it's the universal
practitioner convention (PixInsight, Siril, GraXpert all run it on the linear master), not a
formally-verified claim here.

---

## 2. Step 1 — robust background estimation (reject stars *and* signal)

The estimate must reflect the **sky**, not stars or nebulosity. Two equivalent framings:

**Tile/mesh (SExtractor / photutils Background2D — the canonical open reference) ✅:**
Divide the frame into a grid of boxes (`box_size`), compute one robust value per box, interpolate.
- ✅ **Box size 32–512 px.** Too small → "part of the flux of the most extended objects is absorbed
  into the background map" (it eats real signal); too large → "cannot reproduce small-scale
  variations." Rule: **larger than typical sources, smaller than the gradient's scale.** *(For
  gradient extraction specifically, lean LARGE — you want only the smooth component.)*
- ✅ **Per-box value = clip → mode:** iteratively ±3σ-clip the box pixels around their median
  (photutils default `sigma=3, maxiters=10`) to reject stars, *then* take the **SExtractor mode**:
  ```
  mode = 2.5·median − 1.5·mean          (over the clipped survivors)
  fallback → median   if |mean − median| / std ≥ 0.3   (skewed / crowded box)
  fallback → mean     if std == 0
  ```
  ✅ The order is **clip-then-mode-then-grid-median-filter** (a claim that median-filtering precedes
  the clip was **refuted** 0-3).
- ✅ **Median-filter the low-res grid** before interpolation, to stop a star-contaminated box from
  ringing into the spline.

**Sample-point (DBE / Siril-style) ✅:** place samples (manual or auto grid), estimate each as a
sigma-clipped median over a small box; **reject samples on bright regions** — ✅ Siril: "if areas are
brighter than the median by `tolerance × sigma`, no sample is placed there," and "the position of
each sample is optimized by seeking a local minimum median" (slide off a star onto true sky).

The tile and sample framings are the same idea; the tile grid is just automatic, uniform sampling.

---

## 3. Step 2 — fit a smooth surface

### (a) Low-order 2D polynomial — the *safe* default 📘✅

Basis = all `x^i·y^j` with `i+j ≤ d`; `k = (d+1)(d+2)/2` terms (d=1→3, d=2→6, d=3→10, d=4→15).
✅ **Cap degree at 4** — Siril: "beyond n=4 the model is generally unstable and gives poor results."
Low order is *why* a polynomial can't eat large nebulosity — it physically cannot represent
small-scale structure, only the broad gradient.

📘 Least squares: stack the `m` sample values `z` and the `m×k` design matrix `A`
(row `i` = the basis evaluated at `(xᵢ,yᵢ)`), then solve the original system for coefficients `c`:
```
A · c ≈ z              →     c = SVD(A).solve(z)
```
Normalize `x,y` to `[-1,1]` before building `A`. The SVD is checked for full column rank before
solving; rank-deficient sample geometry is reported instead of silently producing a zero surface.
Then evaluate the surface at every pixel.

### (b) Thin-plate spline (TPS / RBF) — the *flexible* option ✅

Siril's RBF model. Kernel and interpolant (2D):
```
φ(r) = r²·ln r                                    (✅ Siril;  Green's form G(r) = (1/8π)·r²·ln r)
f(x) = Σᵢ wᵢ · φ(‖x − xᵢ‖)  +  (b₀ + b₁x + b₂y)   (RBF terms + an affine polynomial)
```
✅ Solve the saddle-point system with `Mᵢⱼ = φ(‖xᵢ−xⱼ‖)` (`m×m`) and `N` rows `[1, xᵢ, yᵢ]` (`m×3`):
```
[ M   N ] [ w ]   [ z ]
[ Nᵀ  0 ] [ b ] = [ 0 ]      (the Nᵀw = 0 orthogonality constraint)
```
Closed form: `b = (NᵀM⁻¹N)⁻¹ NᵀM⁻¹ z`  (compute first), then `a = M⁻¹(z − Nb)`.

✅ **Regularization = the faint-signal lever.** Exact interpolation forces the surface through every
sample (over-fit → eats signal). Smoothed TPS minimizes `Σ|f(xᵢ)−zᵢ|² + λ∫|D²f|²` — just add `λI` to
the diagonal:
```
w = (M + λI)⁻¹ (z − Nb)        b = (Nᵀ(M+λI)⁻¹N)⁻¹ Nᵀ(M+λI)⁻¹ z
```
Larger `λ` → smoother, large-scale-only surface (✅ exactly Siril's `s·I` smoothing slider, default
50%). This is the single most important knob for not subtracting real nebulosity.

### (c) Bicubic spline over the tile grid — *what SExtractor does* ✅

Skip an explicit surface fit: ✅ "the final background map is a (natural) bicubic-spline interpolation
between the meshes of the grid" (photutils `BkgZoomInterpolator`, order-3 zoom). Simplest, and the
mesh size *is* the smoothness control. This is the path lumos can take almost for free (§8).

**Trade-off:** polynomial = globally smooth, safest, can't fit a complex multi-lobe gradient; TPS =
flexible, fits complex gradients, but needs `λ` tuned or it eats signal; bicubic-mesh = in between,
controlled by box size.

---

## 4. Step 3 — subtract vs divide

✅ Two modes (Siril):
- **Subtract** (default): `out = image − model`. For **additive** gradients — light pollution, moon
  glow, sky glow. Preserves the noise (subtracting a smooth, noiseless surface adds no noise) and the
  relative flux of real signal.
- **Divide**: `out = image / norm_model`, where `norm_model = model / mean(model)`. For
  **multiplicative** residuals — vignetting, differential atmospheric absorption.

📘 **Zero-point:** after subtraction the background sits at ≈0 (slightly negative on noise — correct,
see `stretching/real_data_tests`). Optionally add back a small constant pedestal so later steps don't
clip. Don't clamp negatives.

📘 **Division noise guard:** dividing by a near-zero model amplifies noise (the same hazard as
flat-fielding — see `cfa.rs` `MIN_NORMALIZED_FLAT`). Floor the normalized divisor at a small positive
fraction (e.g. `norm_model.max(0.1)`), bounding amplification to `1/floor`×.

📘 **Per-channel vs luminance:** estimate and remove the model **per channel** — light pollution is
coloured (sodium/LED skies are strongly non-grey), so a single luminance model leaves a colour
gradient. (Per-channel removal also does much of `neutralize_background`'s job as a by-product.)

---

## 5. Robust fitting (don't let stars/signal bias the surface) ✅📘

Whichever surface model: fit **iteratively**.
1. Estimate background at samples/tiles (§2, already star-rejected).
2. Fit the surface (§3).
3. ✅📘 Compute residuals `zᵢ − f(xᵢ)`; **reject samples** with `|residual| > κ·σ` (κ≈2.5–3, σ from
   MAD of residuals) — these sit on nebulosity/unrejected stars. Refit. Repeat 2–4×.

This is the same sigma-clipped-fit lumos already uses for SIP distortion and the tiled sky.

---

## 6. The central pitfall — over-subtracting real signal ⚠️📘

The defining failure mode: **a too-flexible or too-fine background model treats faint extended signal
(large nebulae, galaxy outskirts, IFN) as "background" and subtracts it.** Mitigations, in order of
importance:

1. **Keep the model low-order / large-scale.** Low polynomial degree (1–3), large TPS `λ`, or large
   mesh `box_size`. The model must be *unable* to represent the structure you want to keep.
2. **Exclude bright/real-signal regions from the samples** — don't place samples on the nebula
   (Siril's tolerance test; manual DBE sample placement; an object mask). lumos can build this mask
   from its own star detector + a brightness threshold.
3. **Big mesh / few samples** beats many — fewer degrees of freedom can't chase nebulosity.
4. **Subtract conservatively** — better to leave a faint residual gradient than to gouge a dark bowl
   around a galaxy ("central halo" artifact). When unsure, under-correct.

⚠️ The specific tuning controls (DBE *tolerance*/*smoothing*, ABE *function degree*, GraXpert
*smoothing*) weren't verified to primary sources here; the principle above is the verified-by-design
common thread (SExtractor box-size guidance, Siril degree cap & `λ`).

---

## 7. AI / ML background extraction (the heavy extension) ⚠️

GraXpert's AI mode is a **CNN trained to predict the background/gradient** directly (distributed as an
ONNX model; Siril ships a GraXpert interface). It generalizes to complex, non-analytic gradients that
a polynomial/TPS can't fit, and learns to leave real nebulosity alone. It's the **accurate-but-heavy**
tier — a trained model + an inference runtime — out of scope for a from-scratch classical core, but
the obvious later add (ONNX backend) once the classical path exists, mirroring the denoise AI tier.
⚠️ GraXpert's exact internals weren't pinned to primary sources in this pass.

---

## 8. Implementation plan for `lumos`

> **Status (implemented).** `mod.rs` ships the **safe-default core**: the **shared
> `background_mesh::TileGrid`** SExtractor sky estimator (per-tile ±σ-clip → Pearson mode `2.5·median
> − 1.5·mean`, grid median filter **off** here so it can't bias a real gradient's boundary tiles) →
> tile-centre samples → **low-order 2D polynomial** surface fit by least squares with **iterative
> residual sigma-clipping** (§5) → **subtract or divide**, **per channel**. Public API:
> `extract_background(&mut AstroImage, &BackgroundConfig)` with `BackgroundMode::{Subtract, Divide}`
> (defaults: `tile_size 128`, `degree 2`, 3 reject passes, `divide_floor 0.1`). Verified by tests:
> a pure linear gradient → ≈0; a pedestal+stars → background ≈0 while stars survive; a quadratic
> vignette → flat under `Divide`; degree-3 fits a cubic where degree-1 can't; independent per-channel
> gradients each removed. **Still open:** the **TPS/RBF** surface (§3b), an explicit **object mask**
> from the star detector (§6.2 — `TileGrid::compute` already takes the mask, just not built yet), the
> full-res tiled-**mesh** model as an alternative surface, and **wiring into the pipeline stage**
> (`stack → [background_extraction] → colour-cal → stretch`).

**The robust sky estimator is now a shared foundation module.** `background_mesh::TileGrid` (promoted
out of `stacking::star_detection`) is *exactly* the SExtractor/photutils Background2D mesh: tiled,
per-tile **Pearson mode `2.5·median − 1.5·mean`** with the median fallback on skew, MAD σ, an optional
**3×3 tile median filter**, and **natural bicubic-spline** coefficients. Star detection consumes it for
its full-res `BackgroundEstimate`; this module consumes the same tile samples for the surface fit — so
both see one robust sky. That is the verified §2–§3c estimator, already written and SIMD-optimized.

So the pragmatic core is mostly **wiring, not new math**:

```
src/image_ops/background_extraction/
├── mod.rs        // extract_background(&mut AstroImage, BackgroundConfig) + Mode{Subtract,Divide}
└── tests.rs
```

**Build order:**
1. **Reuse the tiled estimator → subtract.** Run `star_detection::background` per channel with a
   **large tile size** (gradient-scale, e.g. 256–512 px, not the 64 px used for detection) and **object
   masking on**, take its `background` plane as the model, and `image -= model` per channel. This
   alone is a working DBE-class background extraction. *Build first.*
2. **Division mode** for residual vignetting — `image /= (model/mean(model)).max(FLOOR)`.
3. **Low-order polynomial surface** (§3a) as the *constrained* alternative — fit `Ac≈z` to the tile
   values (or sample points) with rank-checked SVD, degree 1–4, iterative residual sigma-clip (§5).
   Safer on big-nebula targets.
4. **Regularized TPS** (§3b) — the flexible option; needs an `m×m` solve (samples are few — hundreds —
   so dense LU is fine).
5. **AI/ONNX** — far later.

**Reuse already in the crate:**
- `stacking::star_detection::background` — the entire tiled-mode + bicubic-spline background model.
- `stacking::star_detection` (star list) + a brightness threshold → the **object mask** for §6.2.
- `core::math::statistics` — sigma-clipped median/MAD for sample estimation and residual rejection.
- planar `Buffer2<f32>` + `par_map_pixels` — per-channel subtract/divide.
- `cfa.rs::MIN_NORMALIZED_FLAT` precedent — the division-mode noise floor.

**Value-range note:** operates on **linear** data (background ≈ 0, bright tail > 1). Output background
sits at ≈0; preserve negatives (don't clamp), optionally add a small pedestal.

### Rust sketch — reuse-the-estimator core

```rust
pub enum Mode { Subtract, Divide }

pub struct BackgroundConfig {
    pub tile_size: usize,   // gradient-scale, e.g. 256–512 (NOT detection's 64)
    pub mode: Mode,
    pub mask_objects: bool, // exclude detected stars / bright signal from the sky estimate
    // optional: surface model (TileMesh | Polynomial{degree} | Tps{lambda})
}

pub fn extract_background(image: &mut AstroImage, cfg: BackgroundConfig) {
    for c in 0..image.channels() {
        let model = estimate_tiled_background(image.channel(c), cfg.tile_size, cfg.mask_objects);
        let plane = image.channel_mut(c);
        match cfg.mode {
            Mode::Subtract => plane.zip_sub(&model),                 // additive gradient
            Mode::Divide   => { let m = mean(&model); plane.zip_div_floored(&model, m, 0.1); }
        }
    }
}
```
(`estimate_tiled_background` = the existing `star_detection::background` mesh estimator at a large tile
size; the Polynomial/Tps variants fit §3a/§3b to the tile centroids instead of bicubic-interpolating.)

---

## 9. Pitfalls

- **Over-subtraction of nebulosity** — §6. The #1 risk; favor large scale + object masking.
- **Tile size too small** — absorbs extended-object flux into the "background" (✅ SExtractor). Use
  gradient-scale tiles, far larger than detection uses.
- **Star contamination of a tile** — caught by per-tile sigma-clip + grid median filter (✅); object
  masking for bright nebulosity the clip won't reject.
- **Division blow-up** — floor the normalized divisor (📘, like flat-fielding).
- **Luminance-only model** — leaves a colour gradient; go per-channel (📘).
- **Clamping negatives** — biases the background upward; keep it signed.

---

## 10. References

Primary (✅ verified against these):

- **SExtractor** — Background estimation: `astromatic.github.io/sextractor/Background.html` (mesh,
  mode `2.5·median−1.5·mean`, 30% skew fallback, ±3σ clip, bicubic-spline map) + Bertin & Arnouts 1996
  (`arxiv.org/abs/astro-ph/0512139` mirror).
- **photutils** — `Background2D` / `SExtractorBackground` source + docs
  (`photutils.readthedocs.io/.../background.html`, `background_2d.py`) — box grid, `sigma=3 maxiters=10`,
  `BkgZoomInterpolator` order-3.
- **Siril** — background extraction docs (`siril.readthedocs.io/en/latest/processing/background.html`)
  — polynomial degree ≤ 4, TPS kernel `r²·ln r` + `s·I` smoothing, sample tolerance, subtract/divide.
- **Eberly, *Thin Plate Splines*** (`geometrictools.com/Documentation/ThinPlateSplines.pdf`) — exact
  2D Green's function, saddle-point system, and `λI` smoothed form.

Supporting (📘): GraXpert (`github.com/Steffenhir/GraXpert`, Siril GraXpert interface); jonrista DBE
guide; astropy robust-stats docs.

⚠️ **Open / not verified here:** PixInsight DBE/ABE & GraXpert exact internals; the formal
pipeline-ordering source; the division min-divisor guard (sensible domain practice, not adversarially
sourced).

---

*Research method: 6 search angles → 22 sources fetched → 90 claims extracted → 25 adversarially
verified (3-vote, ≥2/3 to confirm), 24 confirmed / 1 killed. Core algorithm (SExtractor mesh, mode
estimator, TPS system) rests on primary sources; items tagged 📘/⚠️ are standard practice or were
flagged unconfirmed.*
