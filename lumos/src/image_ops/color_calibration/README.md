# Color Calibration & Green-Cast Removal

How to **normalize color** — make the sky background neutral and kill the green tint — on a linear
OSC/DSLR stack, the way the major astro software does it, with the exact math to implement it in
`lumos`. This is the research/reference for the (not-yet-built) `color_calibration` module; it is the
color counterpart to `docs/image-stretching.md`.

The green tint you see on a freshly-stacked OSC frame (and on our `stacked_light.tiff` stretch test)
is **uncalibrated color**, not a bug. Fixing it is a real, named set of operations.

## Confidence legend

- ✅ **Verified** — confirmed in a multi-source adversarial research pass (≥2/3 verifier votes)
  against **primary** sources: the PixInsight SCNR (legacy) and SPCC reference docs, the Siril docs,
  and — most decisively — the **Siril C source** `src/filters/scnr.c`. Formulas were cross-checked
  across PixInsight ↔ Siril docs ↔ Siril code.
- 📘 **Established** — standard practice, sources gathered but not in the 25-claim verified set.
- ⚠️ **Approximate / open** — thin or unresolved in the sources; flagged for honesty.

---

## 0. TL;DR — what to build

The green tint is removed by a **linear color stage** plus a **post-stretch green kill**:

| # | Operation | Domain | What it does | Cost |
|---|---|---|---|---|
| 1 | **Background neutralization** | linear | per-channel robust background → additive shift so the sky is neutral gray (R=G=B) | trivial |
| 2 | **Color calibration** | linear | set the overall channel balance (star/galaxy color). Cheap white-reference, or accurate **PCC/SPCC** (Gaia catalog) | trivial → heavy |
| 3 | **SCNR** (green removal) | post-stretch | `G' = min(G, (R+B)/2)` — clamp residual green; it's noise on a calibrated image | trivial |

**Pragmatic core for `lumos`:** **background neutralization** (linear) + **Average-Neutral SCNR**
(post-stretch). That alone removes the green tint. **PCC/SPCC** is the *accurate-color* extension
(needs plate-solving + a Gaia spectra extract — much heavier; defer).

**Golden rule (✅):** these are *linear-domain* operations (except SCNR), and they must run **after**
gradient/background extraction and **before** the stretch. Our test stretched the raw linear stack
with *none* of this applied — that's exactly why it stayed green.

---

## 1. Why the green cast (root cause)

✅ Three compounding causes leave the **green channel relatively elevated** on a raw OSC/DSLR stack:

1. **Bayer CFA 2× green.** A color sensor's RGGB matrix has *twice* as many green photosites as red
   or blue. After demosaic the green plane carries more signal (and a different noise character) than
   R/B for the same scene.
2. **No in-camera white balance.** The per-channel WB multipliers a camera bakes into a JPEG are
   **not** present in the raw/linear data lumos stacks. The channels are at their native, unbalanced
   levels.
3. **Sky background.** Light pollution and atmospheric extinction bias the channels unequally,
   adding a colored pedestal on top.

✅ **Why it's safe to remove the green** (the SCNR rationale, PixInsight verbatim): *"with the
exception of some planetary nebulae, there is no green object in the deep-sky. There are no green
stars. Emission nebulae are deeply red. Reflection nebulae are blue. Oxygen III emission corresponds
to a mix of blue and green. … if we find green pixels on a color balanced, deep-sky astrophoto, they
are noise."* Siril and StarTools state the same. So once the image is calibrated and gradient-free,
leftover green is artifact.

⚠️ **The exception that bites:** OIII (500.7 nm) gives bright **planetary nebulae** and comets a
genuinely greenish/teal color. Full-strength green removal desaturates that real signal — so expose
an **amount/blend** control rather than clipping unconditionally (§3).

---

## 2. Background neutralization (the linear core)

✅ Goal: equalize the per-channel **sky background** so the background is neutral gray (R=G=B). This
is what removes the *cast in the background* and is the single most important green-fixing step.

Algorithm (PixInsight `BackgroundNeutralization` default; Siril *"calculates the median of each
channel and equalizes them"*):

1. For each channel `x ∈ {R,G,B}`, estimate the background level `BI_x` = the **median of the pixels
   inside a sigma-clipped sampling range** over a background region (robust — rejects stars/nebula).
2. Choose the common target background `b = min(BI_R, BI_G, BI_B)` — the darkest channel's background.
3. **Additive shift** each channel to that target:

   ```
   IN_x = I_x − BI_x + b          for x ∈ {R, G, B}
   ```

After this, every channel's background sits at `b`, so the sky is neutral. (✅ verified verbatim from
the PixInsight forum + authorized mirror, corroborated by Siril.)

⚠️ **Two honest caveats the research turned up:**
- The additive-to-`min` form is the **default**, *not the only* model — PixInsight's tool also has
  `Rescale` / `Rescale-as-needed` / `Truncate` / explicit `Target Background` modes. Additive-to-min
  is the right default; don't assume it's the sole valid model.
- Do **not** conflate this with Siril's background-*extraction* `Subtraction` vs `Division` modes —
  those are **gradient removal** (light-pollution vs vignetting), a *different* step that runs before
  this one.

**Pitfall:** if a channel is **clipped/over-saturated** the median is biased; sigma-clip and prefer a
genuinely dark background region. Subtracting an offset can push faint pixels slightly negative —
fine in linear (the stretch's black point handles it), but don't clamp to 0 here.

### Rust (against `lumos` types)

```rust
use crate::math::statistics::sigma_clipped_median_mad;

/// Neutralize the per-channel sky background: shift each channel so its (robust) background level
/// matches the darkest channel's, making the sky neutral. Linear-domain; run before the stretch.
pub fn neutralize_background(image: &mut AstroImage, kappa: f32, iterations: usize) {
    if !image.is_rgb() {
        return; // nothing to neutralize on a single channel
    }
    // Per-channel robust background = sigma-clipped median (rejects stars/nebula).
    let mut scratch = Vec::new();
    let mut bg = [0.0f32; 3];
    for c in 0..3 {
        let mut samples = image.channel(c).to_vec();
        bg[c] = sigma_clipped_median_mad(&mut samples, &mut scratch, kappa, iterations).median;
    }
    let target = bg[0].min(bg[1]).min(bg[2]); // b = min(BI_R, BI_G, BI_B)
    for c in 0..3 {
        let offset = target - bg[c]; // IN_x = I_x - BI_x + b
        image
            .channel_mut(c)
            .pixels_mut()
            .par_iter_mut()
            .for_each(|v| *v += offset);
    }
}
```

(`sigma_clipped_median_mad` already lives in `math::statistics`; for a large frame, sampling a
sub-region or a strided subset is a fine speed/robustness refinement — PixInsight uses a user-chosen
background reference region.)

---

## 3. SCNR — the dedicated green killer

✅ **SCNR** = *Subtractive Chromatic Noise Reduction*, the tool whose whole job is removing the green
cast. Four methods; the **default is Average Neutral**. Verified against the PixInsight legacy doc,
the Siril docs, **and** the Siril C source `src/filters/scnr.c`:

```
Average Neutral (default):   m = 0.5·(R + B);   G' = min(G, m)
Maximum Neutral:             m = max(R, B);     G' = min(G, m)
```

✅ The **Amount** parameter is **not used** by the two *neutral* methods — they are a pure clamp.
`scnr.c` literally: `case SCNR_AVERAGE_NEUTRAL: m = 0.5*(red+blue); green = min(green, m);`.

For **partial strength** (to protect genuine teal), the two **mask** methods are amount-blended:

```
Maximum Mask:    m = max(R, B);        G' = G·(1−a)·(1−m) + m·G
Additive Mask:   m = min(1, R + B);    G' = G·(1−a)·(1−m) + m·G
```

✅ `a` (amount ∈ [0,1]) is available **only** for the mask methods. ⚠️ Note this is the *documented*
blend — **not** the naive `G' = (1−a)·G + a·min(G,m)` that several secondary tutorials describe
(that paraphrase is wrong; implement the forms above).

**The pragmatic core formula:** **`G' = min(G, (R+B)/2)`**. Wherever green exceeds the average of red
and blue, pull it down to that average — that excess *is* the green cast. Where green is already
≤ (R+B)/2 (most real signal), it's untouched.

✅ **Placement:** SCNR is *designed for non-linear (stretched) images* (Siril: *"designed to be used
on non-linear images … Make sure the histogram has been stretched before using this tool"*) and
**assumes color calibration + gradient removal are already done**. It is the **last** color step,
after the stretch — not a substitute for calibration.

⚠️ **Pitfall (genuine green):** Average Neutral at full strength desaturates OIII-teal planetary
nebulae. Offer the mask form with a tunable `amount`, and/or mask bright OIII regions.

### Rust (using `par_map_pixels` + `Rgb`)

```rust
use crate::image_ops::rgb::Rgb;

/// SCNR Average-Neutral green removal: G' = min(G, (R+B)/2). Run on the stretched image.
pub fn scnr_average_neutral(image: &mut AstroImage) {
    image.par_map_pixels(
        |l| l, // grayscale: no chroma to neutralize
        |px| Rgb {
            r: px.r,
            g: px.g.min(0.5 * (px.r + px.b)),
            b: px.b,
        },
    );
}

/// SCNR Additive-Mask with an amount knob (0 = off, 1 = full), to spare genuine teal:
///   m = min(1, R+B);  G' = G·(1−a)·(1−m) + m·G
pub fn scnr_additive_mask(image: &mut AstroImage, amount: f32) {
    image.par_map_pixels(
        |l| l,
        |px| {
            let m = (px.r + px.b).min(1.0);
            Rgb {
                r: px.r,
                g: px.g * (1.0 - amount) * (1.0 - m) + m * px.g,
                b: px.b,
            }
        },
    );
}
```

`par_map_pixels` dispatches `L`/`Rgb` once and parallelizes — the SCNR fits its shape exactly.

---

## 4. Color calibration (channel white-balance)

Background neutralization fixes the *background*; **color calibration** sets the overall channel
balance so star and galaxy colors are correct. Two tiers.

### 4.1 Cheap: white-reference / "stars are white" 📘⚠️

The non-catalog approach: pick a neutral/white reference — a chosen region, or the **average color of
many stars** (assumed white on average) — and scale each channel so the reference is neutral:

```
K_x = ref_target / mean_x(reference)          apply  I_x *= K_x   (then normalize, e.g. K_G = 1)
```

⚠️ The exact non-catalog "average-of-stars" math wasn't pinned to a primary source in this research —
treat the formula above as the standard *form* (per-channel multiplicative balance from a reference),
not a verified algorithm. It's a good-enough first pass; PCC/SPCC is the rigorous version.

### 4.2 Accurate: PCC / SPCC (catalog-based) — the extension ✅

The accurate-color method (PixInsight **SPCC**, Siril **PCC/SPCC**). **Heavy** — needs star detection
+ PSF photometry + an **astrometric plate solve** + a **Gaia DR3 spectra extract**. The SPCC pipeline:

1. **Plate-solve** the frame (astrometry) to match image stars to a catalog.
2. **PSF-fit** star fluxes; per matched star compute the image **R/G** and **B/G** flux ratios.
3. Per catalog star, compute the **catalog R/G, B/G** by integrating its **Gaia DR3 `xp_sampled`
   spectrum × each channel's filter transmission** (+ optional QE). (Simpson 1/3, 0.1 nm step,
   336–1020 nm.)
4. **Robust linear fit** (repeated-median / Siegel): catalog ratio (x) vs image ratio (y), per ratio.
5. Evaluate at the **white-reference** ratios (e.g. *Average Spiral Galaxy*) → R/G and B/G factors;
   **normalize to a unit vector**.
6. **Apply by pixel-wise multiplication** to R, G, B.
7. SPCC then runs its **background neutralization after** the white-balance multiply (sigma = 1.4826·MAD).

Siril **PCC** (simpler) compares measured vs catalog star colors (NOMAD default; APASS / Gaia DR3) →
**three per-channel multiplicative coefficients** (green ≈ unity reference), reporting the average
absolute deviation as a quality metric.

✅ **Hard constraint:** PCC/SPCC **must** run on **linear** (un-stretched) data — photometry on
stretched pixels is wrong. It cannot be retrofitted onto a stretched image.

⚠️ **Open:** a full SPCC also applies a 3×3 working-color-space matrix on effective primaries after
the per-channel scale. Whether a pure 3-scalar multiply is "good enough" for lumos vs needing the
matrix for accurate color is unresolved — start with the 3 scalars, revisit if color accuracy matters.

---

## 5. Workflow ordering

✅ The canonical order — everything except SCNR on **linear** data:

```
LINEAR DOMAIN:
  1. calibrate / stack                         → linear stacked master      (lumos: stacking)
  2. gradient / background extraction          (DBE / ABE / GraXpert)  ── MUST precede color steps
  3. background neutralization                 (per-channel median → additive to b = min)   §2
  4. color calibration                         (white-reference  OR  PCC/SPCC)              §4

  ─────────────────────────────  ★ STRETCH ★  ─────────────────────────────  (lumos: stretching)

NON-LINEAR DOMAIN:
  5. SCNR Average-Neutral green removal        (amount knob to spare OIII)                  §3
```

✅ SCNR is **last** because it assumes calibrated, gradient-free, *stretched* data. (StarTools is the
architectural outlier — it color-calibrates *post-stretch* via its signal-evolution Tracking engine,
and exposes "Cap Green" as a last-resort green clamp. Every other tool calibrates in linear.)

**Why background/gradient extraction must come first (📘):** a color gradient (light pollution, moon)
makes the "background" different across the frame, so a single per-channel offset can't neutralize it.
Remove the gradient first, *then* neutralize the flat residual background.

---

## 6. Implementation plan for `lumos`

A new `color_calibration` feature (sibling of `stacking/` and `stretching/`), wiring into the pipeline
between **stacking** and **stretching** (the linear color stage). SCNR is post-stretch — it can live
here too (applied after the stretch) or alongside `stretching`.

**Module shape:**
```
src/color_calibration/
├── mod.rs            // neutralize_background(), scnr_*(), configs
└── tests.rs
```

**Build order (pragmatic → accurate):**
1. **`neutralize_background`** (§2) — per-channel sigma-clipped median (reuse `sigma_clipped_median_mad`)
   → additive shift to `b = min`. Removes the background cast. *Build first.*
2. **`scnr_average_neutral` / `scnr_additive_mask`** (§3) — via `par_map_pixels`. Kills residual green
   post-stretch. *Build with #1 — together they fix the tint.*
3. **Cheap white-reference color calibration** (§4.1) — per-channel multiplicative balance.
4. **PCC/SPCC** (§4.2) — the accurate extension. Large: pulls in plate-solving + a Gaia spectra
   extract; lumos already has star detection + PSF photometry (`stacking::star_detection`) to build on,
   but the astrometric solve and catalog are new. *Defer.*

**Reuse already in the crate:**
- `sigma_clipped_median_mad` → `ClippedStats { median, sigma, mean }` (`math::statistics`) — the
  per-channel robust background.
- `image_ops::par_map_pixels(mono, rgb)` + the domain-local `image_ops::rgb::Rgb { r, g, b }` — the SCNR per-pixel map.
- `channel(c)` / `channel_mut(c)`, planar `Buffer2<f32>` — per-channel offsets.
- `stacking::star_detection` (PSF flux, centroids) — the photometry PCC/SPCC need.

**Value-range note:** linear stacks are **not** in `[0,1]` (our frame's max was 1.50). Background
neutralization is additive and works fine on linear data of any range. SCNR runs **post-stretch**, on
the `[0,1]` output of the `stretching` module, so its `min`/mask math stays in range.

**Quick fix for the test image:** the cleanest path is `neutralize_background` on the *linear* stack
*then* stretch. As a shortcut, `scnr_average_neutral` applied to the already-stretched
`stacked_light_*.jpg` would knock the green back, but neutralizing in linear first is the correct order.

---

## 7. Pitfalls (collected)

- **Order matters:** gradient extraction → background neutralization → color calibration → stretch →
  SCNR. Skipping straight to SCNR on an un-calibrated, gradient-ridden image just smears the problem.
- **Genuine green:** OIII planetary nebulae/comets are really teal — use SCNR `amount` < 1 or mask them.
- **Clipping:** a saturated/clipped channel biases the background median and the photometric fit;
  sigma-clip and exclude saturated stars.
- **Don't clamp negatives after neutralization** — additive offsets can dip faint pixels below 0;
  that's normal in linear and the stretch's black point absorbs it.
- **Nebula color shift:** aggressive neutralization/SCNR can desaturate faint nebulosity; keep it
  background-driven (robust median) and modest.

---

## 8. Sources

Primary (✅ verified against these):

- PixInsight — **SCNR** reference (legacy), all four formulas verbatim:
  `pixinsight.com/doc/legacy/LE/21_noise_reduction/scnr/scnr.html`
- PixInsight — **SPCC** reference (full algorithm): `pixinsight.com/doc/docs/SPCC/SPCC.html`
- PixInsight forum (Conejero/Peris) — **BackgroundNeutralization** math (`IN_x = I_x − BI_x + b`,
  `b = min(...)`); mirror `pixinsight.com.ar/docs/186`
- Siril — **colors / Remove Green Noise (SCNR)** docs (stable + latest):
  `siril.readthedocs.io/en/stable/processing/colors.html`
- Siril — **PCC** and **SPCC** docs: `siril.readthedocs.io/en/latest/processing/color-calibration/{pcc,spcc}.html`
- **Siril C source** `src/filters/scnr.c` (`gitlab.com/free-astro/siril`) — the decisive code-level
  confirmation of all four SCNR formulas and the neutral-method default.
- StarTools — **Color module / Cap Green**: `startools.org/modules/color`

Supporting (📘):

- jonrista.com — BackgroundNeutralization & SCNR practitioner guides; Sky & Telescope / astropix.com
  color-balance background.

⚠️ **Not covered:** Astro Pixel Processor and DeepSkyStacker were in scope but have no public
algorithm docs — no surviving claims describe their specific math. Coverage is concentrated on
PixInsight, Siril, and StarTools.

---

*Research method: 5 search angles → 17 sources fetched → 80 claims extracted → 25 adversarially
verified (3-vote, ≥2/3 to confirm), 23 confirmed / 2 killed. The killed claims ("BN is purely
additive"; "Siril Subtraction/Division = additive/multiplicative neutralization") are reflected as
caveats above. SCNR/SPCC formulas rest on locally-captured primary docs + the Siril C source, not just
snippets. Items tagged 📘/⚠️ were gathered but fell outside the verified set or are genuinely open.*
