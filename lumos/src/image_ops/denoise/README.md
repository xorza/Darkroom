# Denoising

How astrophotography denoising works, where it sits relative to the stretch, and the exact math to
implement a best-in-class denoiser for `lumos`. The research/reference for the (not-yet-built)
`denoise` module — the third in the color/tone family alongside `docs/image-stretching.md` and
`color_calibration/README.md`.

A stacked deep-sky master is *linear* (flux-proportional) and still noisy: stacking N frames only
cuts noise by ~√N, so a real residual remains — background grain, color/chroma noise, low-frequency
mottle. Denoising removes that residual without erasing faint signal.

## Confidence legend

- ✅ **Verified** — confirmed in a multi-source adversarial pass (≥2/3 votes) against **primary**
  sources: the Starck *Starlet Transform* chapter, the Mäkitalo–Foi exact-unbiased-inverse paper
  (IEEE TIP 20(9), 2011), the Siril denoising docs, and the RC-Astro NoiseXTerminator manual.
- 📘 **Established** — standard signal-processing knowledge (textbook NLM/BM3D/TV formulas, the
  starlet noise-propagation table) — high confidence, gathered but outside the 25-claim verified set.
- ⚠️ **Open / approximate** — flagged in the research as medium-confidence or unresolved.

---

## 0. TL;DR — what to build

**Denoise the linear master, before the stretch**, with the classic implementable pipeline:

```
1. Variance-stabilize   — generalized Anscombe transform (shot+read → ~constant-variance Gaussian)
2. Denoise              — starlet (à trous) wavelet thresholding, per-scale K·σ from MAD
3. Inverse-stabilize    — Mäkitalo–Foi exact unbiased inverse (NOT the naive algebraic inverse)
```

| Tier | Method | Domain | Cost |
|---|---|---|---|
| **Pragmatic core** | **Anscombe VST + à trous starlet wavelet thresholding** | linear, pre-stretch | trivial–moderate |
| Alternative | Non-Local Means (Siril uses NL-Bayes), TV/ROF, BM3D | linear | moderate–heavy |
| Chroma | denoise the **color** channels hard, protect luminance detail | either | trivial |
| Accurate (heavy) | **AI** (NoiseXTerminator, GraXpert AI, Cosmic Clarity) — CNN/U-Net | linear or non-linear | very heavy (trained model) |

**Pipeline slot for `lumos`:** a new `denoise` stage in the **linear domain**, after
stacking + `color_calibration`, **before** `stretching`. (A light *chroma* denoise can also run
post-stretch.)

---

## 1. The nature of astrophoto noise

✅📘 A stacked master carries two physical noise sources plus structure left by imperfect stacking:

- **Shot (Poisson) noise** — photon arrival is Poisson, so the noise is **signal-dependent**:
  variance ∝ signal, `σ_shot = √(signal in electrons)`. This is the dominant source on real targets.
- **Read noise** — additive **Gaussian**, signal-independent, `σ_read` constant per pixel.
- So the per-pixel noise is **Poisson(shot) + Gaussian(read)** — *not* pure Poisson (a research
  claim that it's "Poisson-dominant" was **refuted**; both matter on amateur masters).

✅ **Stacking** N frames reduces random noise by ~**√N** (averaging uncorrelated noise). What's left
after a good stack: **background grain**, **chroma/color noise** (often the most objectionable),
and **low-frequency mottle** (large-scale blotches that √N reduces slowly).

**Why this drives method choice:** because noise is signal-dependent (heteroscedastic), a plain
Gaussian-noise denoiser is mis-tuned (it assumes constant variance). The fix is a **variance-
stabilizing transform** (§3) that makes the noise ~constant-variance Gaussian, *then* a standard
denoiser works correctly.

---

## 2. Linear vs non-linear — where denoise belongs

✅ **Denoise the linear master, before the stretch.** Siril's docs state noise reduction is best on
linear data, and place it pre-stretch. Two reasons:

1. In the **linear** domain the noise is well-characterized (shot+read, §1) and **white** (spatially
   uncorrelated) — exactly what denoisers and the VST assume.
2. The **stretch is non-linear and non-uniform**: it amplifies shadow/background noise hugely (the
   curve is steepest near black) and compresses highlights, so it makes the noise **non-white** and
   signal-dependent in a way no denoiser models. Denoising *after* the stretch fights distorted noise.

⚠️ **Nuance (a claim was refuted here):** "denoise *must* precede the stretch" is a **strong
preference, not a hard requirement** (refuted 0-3). In practice:
- Heavy **structural** denoise (wavelet/NLM/BM3D) → **linear, pre-stretch**.
- Light **chroma** cleanup → fine **post-stretch** (color noise is visible there and luminance detail
  is protected).
- **AI denoisers** (NXT) internally stretch→denoise→reverse, so they're robust on either domain and
  are typically applied after channel combination.

**Ordering vs color calibration (⚠️ open):** denoise after background/gradient extraction and color
calibration is the common practice (so you don't bake a color cast into the denoised result), but the
exact order vs `color_calibration` wasn't pinned to a primary source. Recommend:
`stack → background/color-calibration → denoise → stretch`.

---

## 3. Variance-stabilizing transforms (Anscombe)

✅ To denoise signal-dependent (Poisson) noise with a Gaussian-noise denoiser, first **stabilize the
variance** so it's ~constant (≈1) everywhere, denoise, then invert.

**Anscombe (pure Poisson):**
```
forward:   A(x) = 2·√(x + 3/8)          // output std ≈ 1 (for mean ≳ ~4–20 counts; degrades below)
```
✅ The forward map makes Poisson data ≈ Gaussian with unit variance. It **breaks below ~20 counts/px**
(low-flux background), which is exactly where astro backgrounds sit — hence the generalized form +
the careful inverse below matter.

**Generalized Anscombe (shot + read), for `x = α·z + read`:** with sensor gain `α` (e-/ADU), read
noise std `σ`, read offset mean `μ`:
```
forward:   y = (2/α)·√(α·x + 3α²/8 + σ² − α·μ)     // ≈ unit-variance Gaussian
```
(`α`, `σ`, `μ` come from the sensor / calibration — lumos has gain/`egain` in `ImageMetadata`.)

**Inverse — use the exact unbiased inverse (✅ verified, load-bearing):**
> The *choice of inverse* is the critical step — the naive **algebraic** inverse
> `x̂ = (D/2)² − 3/8` is **biased**; use the **Mäkitalo–Foi exact unbiased (= maximum-likelihood)
> inverse** (closed-form, IEEE TIP 20(9) 2011) to recover the correct mean.

So the pipeline is **`A(·)` → AWGN denoise → `A⁻¹_exact(·)`**. For a first implementation you can use
the algebraic inverse and accept a small bias; for correctness, port the closed-form unbiased inverse.

---

## 4. The à trous starlet wavelet — the implementable core

✅ The classic astro denoiser (the engine inside PixInsight MLT/MMT and Siril's wavelets) is the
**starlet** = *isotropic undecimated (à trous, "with holes")* wavelet transform: a redundant,
shift-invariant multiscale decomposition that's cheap and exact-reconstructing.

**Decomposition** (J scales) using the **B3-spline** low-pass filter:
```
h (1-D) = [1, 4, 6, 4, 1] / 16            // separable: apply along rows, then columns
c₀ = image
for j = 0 .. J-1:
    c_{j+1} = h_j ∗ c_j                    // convolve with h, but with 2^j zeros ("holes") between taps
    w_{j+1} = c_j − c_{j+1}                // the wavelet plane = detail lost at this scale  ✅
```
- The **à trous** trick: at scale `j`, the filter taps are spaced `2^j` apart (holes between them), so
  each scale captures structure ~`2^j` px wide — no downsampling (shift-invariant, no aliasing).
- ✅ **Exact reconstruction:** `c₀ = c_J + Σ_{j=1..J} w_j` (residual smooth + all detail planes). So
  denoising = **threshold the `w_j`, then sum back**.

**Per-scale thresholding (the denoise):**
```
σ_j  = 1.4826 · MAD(w_j)                   // robust per-scale noise std (✅ MAD; ⚠️ exact factor medium-conf)
t_j  = K · σ_j                             // K ∈ [3,5]; K=3 ⇒ 0.27% two-sided tail = "significant"  ✅
hard threshold:   w_j ← w_j · [ |w_j| ≥ t_j ]      // zero coefficients below the noise floor
soft threshold:   w_j ← sign(w_j)·max(|w_j|−t_j, 0)   // softer, less ringing
```
- ✅ A coefficient is **significant** iff `|w_j| ≥ K·σ_j` — i.e. it stands above the noise. `K=3`
  keeps only coefficients with <0.27% chance of being pure noise.
- Estimating `σ_j` **per scale via MAD of that scale's coefficients** is robust and self-calibrating
  (the median absolute deviation ignores the sparse bright real structure). 📘 The theoretical
  alternative is the tabulated B3-spline noise-propagation factors (`σ_j/σ ≈` 0.890, 0.201, 0.0857,
  0.0413, 0.0206 for scales 1–5), but per-scale MAD avoids needing them.
- **Protect the finest + coarsest scales thoughtfully:** the finest scale (`w_1`) is mostly noise
  (threshold hard); deep scales hold real large-scale signal (threshold gently or not at all to keep
  faint nebulosity). Leaving the residual `c_J` untouched preserves the overall light distribution.

This is the **pragmatic core**: Anscombe → à trous starlet hard-threshold (K=3, per-scale MAD) →
inverse Anscombe. Cheap (separable convolutions + MAD), shift-invariant (no blocking), and the
standard astro method.

---

## 5. Alternatives (when wavelets aren't enough)

📘 All beat naive Gaussian blur; pick by need. (An A&A 2020 benchmark found a cluster — TV-L2, BM3D,
starlet, undecimated wavelets, Perona–Malik, NLM, TV-Chambolle — all clearly beating Gaussian
filtering; a specific "TV-L2 wins by 0.2 mag" claim was **refuted** as overstated.)

- **Non-Local Means (NLM)** — average pixels weighted by *patch* similarity:
  `û(p) = Σ_q w(p,q)·u(q)`, `w(p,q) ∝ exp(−‖patch(p)−patch(q)‖² / h²)`. Excellent edge/detail
  preservation; ✅ **Siril's classical denoiser is NL-Bayes** (a Bayesian NLM variant, faster/better
  than BM3D), optionally refined by one DA3D/SOS pass.
- **BM3D** — block-matching + collaborative filtering in a transform domain; the long-time quality
  benchmark; the canonical partner for the generalized-Anscombe pipeline (Mäkitalo–Foi). Heavier.
- **Total Variation / ROF** (and **TGV** = PixInsight *TGVDenoise*) — minimize
  `½‖u−f‖² + λ·TV(u)`; smooths flats while keeping edges. TGV reduces TV's "staircasing." Good for
  background mottle; risks a flat/plastic look if pushed.
- **Bilateral / anisotropic (Perona–Malik) diffusion / Wiener** — simpler edge-aware smoothers;
  serviceable but generally inferior to wavelet/NLM for astro.

---

## 6. Luminance vs chrominance

✅ **Denoise color (chroma) much harder than luminance.** Split into a luminance + 2 chroma channels
(Lab or YCbCr), denoise the chroma channels aggressively (color noise carries little real detail and
is the most objectionable), and denoise luminance lightly (it holds the structure you want to keep).
NoiseXTerminator exposes exactly this split — its defaults push **color** harder than **intensity**
(high-freq color ~90–100% vs intensity ~80–90%; low-freq color 100% vs intensity ~50–70%).

📘 Protect real signal with a **luminance/structure mask**: scale the denoise strength down where the
luminance (or a wavelet structure map) is high (bright nebula/stars) and up in the dark background, so
you smooth the noisy background without erasing detail or haloing stars.

---

## 7. Modern AI denoisers (the heavy extension)

✅ The current state of the art is **CNN/U-Net denoisers trained on astro data**: **RC-Astro
NoiseXTerminator**, **GraXpert** (AI denoise model), **Russell Croman Cosmic Clarity**, Topaz DeNoise.
They learn the astro noise+signal prior and outperform classical methods, especially on color noise
and faint-signal preservation. NXT internally **stretches → denoises → reverses**, separates
intensity vs color noise, and is best applied **after channel combination**. These are a **heavy ML
extension** (trained model weights + an inference runtime) — out of scope for a from-scratch Rust
core, but the obvious later add (e.g. via an ONNX runtime) once the classical path exists.

---

## 8. Noise estimation

✅ Robust noise std from the **median absolute deviation**: `σ ≈ 1.4826 · MAD`. For wavelet
thresholding, estimate it **per scale** from that scale's coefficients: `σ_j = 1.4826 · MAD(w_j)` —
MAD shrugs off the sparse bright real structure, so it measures the noise floor even where signal is
present. The global single-image estimate uses the **finest scale** (`w_1`), which is noise-dominated:
`σ ≈ 1.4826 · MAD(w_1)`. lumos already has `mad_f32_*` / `mad_to_sigma` (`MAD_TO_SIGMA = 1.4826022`)
in `math::statistics`.

---

## 9. Pitfalls

- **Over-smoothing / "plastic" look** — too-high `K` or TV `λ` flattens texture; keep `K≈3` and
  protect deep scales / use a luminance mask.
- **Erasing faint signal** — don't threshold the coarse wavelet scales hard; faint nebulosity lives
  there. Leave `c_J` untouched.
- **Star halos / rings** — hard thresholding can ring around bright stars; soft thresholding or a star
  mask helps.
- **Residual low-frequency mottle** — wavelets at large scales target it, but it's the hardest; more
  integration time is the real fix.
- **Domain & order** — heavy denoise belongs in **linear, pre-stretch**; do it **after** background
  extraction + color calibration so you don't denoise a gradient/cast into permanence.

---

## 10. Implementation plan for `lumos`

A new **`denoise`** feature (sibling of `color_calibration` / `stretching`), slotting into the
**linear** stage: `stacking → color_calibration → denoise → stretching`.

**Module shape:**
```
src/denoise/
├── mod.rs        // anscombe(), à trous starlet transform + threshold, denoise() entry
└── tests.rs
```

**Build order (pragmatic → accurate):**
1. **à trous starlet transform** — `forward(plane) -> (Vec<Buffer2<f32>> wavelet planes, residual)`,
   `reconstruct(planes, residual) -> Buffer2`. Separable B3-spline conv with `2^j` holes; reuse the
   separable-convolution machinery pattern from `stacking::star_detection::convolution`. *Build first.*
2. **Per-scale hard/soft threshold** — `σ_j = mad_to_sigma(mad(w_j))`, zero/shrink below `K·σ_j`.
   Together with #1 this is a working wavelet denoiser. *Build with #1.*
3. **Anscombe VST wrapper** — forward generalized Anscombe (gain/read from metadata) → denoise →
   inverse (start algebraic, upgrade to Mäkitalo–Foi exact unbiased). Makes the threshold correct on
   signal-dependent noise.
4. **Chroma split** — for RGB, denoise luminance lightly + chroma hard (Lab/YCbCr). Reuse
   `Rgb`/`intensity_plane`. *Later.*
5. **AI denoise** — ONNX-runtime CNN. *Far later; the heavy extension.*

**Reuse already in the crate:**
- `math::statistics::{mad_f32_with_scratch, mad_to_sigma}` — per-scale noise σ.
- planar `Buffer2<f32>` channels + `channel`/`channel_mut`, `par_map_pixels` — per-channel work.
- `stacking::star_detection::convolution` (separable Gaussian/elliptical conv, SIMD) — the template for
  the à trous separable B3-spline convolution.
- `ImageMetadata` `gain`/`egain` — the generalized-Anscombe `α` / read-noise parameters.

**Value-range note:** operates on **linear** data (any range ≥ 0, not `[0,1]`). The starlet detail
planes are signed; the residual + reconstruction stay in the original linear range.

### Rust sketch — à trous starlet core

```rust
/// B3-spline à trous: J wavelet planes (signed detail) + the final smooth residual.
/// Exact reconstruction: image == residual + Σ planes.
fn starlet_forward(image: &Buffer2<f32>, scales: usize) -> StarletTransform {
    let mut c = image.clone();
    let mut planes = Vec::with_capacity(scales);
    for j in 0..scales {
        let smoothed = b3_spline_atrous(&c, 1 << j); // separable [1,4,6,4,1]/16 with 2^j holes
        planes.push(subtract(&c, &smoothed));        // w_{j+1} = c_j − c_{j+1}
        c = smoothed;
    }
    StarletTransform { planes, residual: c }
}

fn denoise_plane(image: &Buffer2<f32>, scales: usize, k: f32) -> Buffer2<f32> {
    let mut t = starlet_forward(image, scales);
    let mut scratch = Vec::new();
    for w in &mut t.planes {
        let sigma = mad_to_sigma(mad_f32_with_scratch(w.pixels(), 0.0, &mut scratch)); // median(w)≈0
        let thresh = k * sigma;                       // K ≈ 3
        w.pixels_mut().par_iter_mut().for_each(|v| if v.abs() < thresh { *v = 0.0 });
    }
    reconstruct(&t) // residual + Σ thresholded planes
}
```
(Wrap each plane in `anscombe_forward` before / `anscombe_inverse` after to handle signal-dependent
noise. For color, run per channel, or split luminance/chroma and denoise chroma with a larger `K`.)

---

## 11. Sources

Primary (✅ verified against these):

- **Starck, Murtagh, Fadili** — *The Starlet Transform* (à trous / isotropic undecimated wavelet),
  `jstarck.free.fr/Chapter_Starlet2011.pdf` — B3-spline filter, `w_{j+1}=c_j−c_{j+1}`,
  `c₀=c_J+Σw_j`, K·σ thresholding.
- **Mäkitalo & Foi** — *Optimal inversion of the generalized Anscombe transformation for
  Poisson–Gaussian noise*, IEEE TIP 20(9) 2011; `webpages.tuni.fi/foi/invansc/` — exact unbiased
  inverse, generalized Anscombe (shot+read).
- **Anscombe transform** — `en.wikipedia.org/wiki/Anscombe_transform` (forward/inverse forms).
- **Siril** — denoising docs (`siril.readthedocs.io/en/latest/processing/denoising.html`) + workflow
  — NL-Bayes, Anscombe VST, linear-domain placement.
- **RC-Astro** — NoiseXTerminator manual (`rc-astro.com/.../noisexterminator-...`) — AI denoiser,
  intensity-vs-color split, internal stretch/reverse.

Supporting (📘):

- A&A 2020 denoising benchmark (`aanda.org/.../aa36278-19`); Buades NLM (IPOL); PixInsight TGVDenoise
  references; GraXpert AI-denoise docs.

⚠️ **Refuted / not relied on:** "denoise *must* precede the stretch" (it's a strong preference);
"TV-L2 wins by 0.2 mag"; "Poisson is the sole dominant noise model" (it's shot+read). **Open:**
the exact per-scale starlet noise factors (use per-scale MAD instead), wavelet-vs-NL-Bayes on
low-noise masters, denoise-vs-color-calibration ordering, and the internals of GraXpert/Topaz/Cosmic
Clarity.

---

*Research method: 5 search angles → 23 sources fetched → 107 claims extracted → 25 adversarially
verified (3-vote, ≥2/3 to confirm), 22 confirmed / 3 killed. The killed claims are reflected as
caveats. Core formulas (starlet, Anscombe, exact-unbiased inverse) rest on primary papers; items
tagged 📘/⚠️ were gathered but fell outside the verified set or are genuinely open.*
