# Astrophotography processing functions — catalogue & roadmap

The astrophotography-**specific** image-processing operations a deep-sky pipeline needs to turn a
stacked master into a finished picture, surveyed from the major software (PixInsight, Siril,
GraXpert, RC-Astro's *XTerminator* suite, StarTools, APP). Scoped deliberately:

- **In scope:** operations whose *algorithm or purpose is specific to astronomical images* — light-
  pollution gradients, star/PSF-aware work, narrowband, multiscale structure, catalog-based colour.
- **Out of scope (general photo editing, explicitly excluded):** curves/levels, generic unsharp
  mask, generic saturation/HSL, dodge & burn, crop/rotate, clone-stamp, generic blur/sharpen. These
  are Photoshop/Lightroom territory and are *not* what this library is for. A short exclusion list is
  at the end.

This is the post-stack "processing" half. The pipeline already owns the capture→master half
(calibrate → detect → register → combine → drizzle).

**Status legend:** ✅ implemented · ◐ partial / foundation exists · 🔲 missing.
**Priority** (impact on a nice final image): ★★★ transformational · ★★ strong · ★ nice-to-have.

---

## A. Linear-domain corrections (before the stretch)

These run on the linear master and matter most — errors here are baked in before stretching.

| Fn | Pri | Status | What it does |
|---|---|---|---|
| **Gradient / background extraction** | ★★★ | ◐ | `background_extraction::extract_background` (+ `background_extraction/README.md`) — **core implemented**: robust tiled sky estimate (shared `background_mesh::TileGrid` SExtractor estimator, promoted out of `star_detection`) → **low-order 2D polynomial** surface (least squares + iterative residual sigma-clip) → **subtract or divide**, **per channel**. = PixInsight *DBE/ABE*, Siril *Background Extraction*, GraXpert AI, StarTools *Wipe*. **Distinct from** the background *neutralization* lumos has (that only equalizes per-channel offsets; this removes spatial gradients). *Open:* **TPS/RBF** surface, explicit **object mask** (`TileGrid` already accepts one), and **pipeline wiring**. |
| **Photometric colour calibration (PCC / SPCC)** | ★★★ | 🔲 | Plate-solve the frame, match detected stars to a catalog (**Gaia DR3**), and derive per-channel white balance from real stellar photometry. **SPCC** refines it with sensor QE + filter-transmission curves. The rigorous, objective colour calibration — replaces eyeballed white balance. = PixInsight *PCC/SPCC*, Siril *PCC/SPCC*. |
| **Deconvolution (PSF / star-aware)** | ★★★ | 🔲 | Sharpen by reversing the optical+atmospheric blur, using the **PSF measured from stars** in the frame. Classical **Richardson–Lucy** or regularized; **star-aware** (suppress ringing around stars via a mask); **spatially-variant** for field-varying PSF (StarTools *SVDecon*). The AI incarnation is *BlurXTerminator*. Astro-specific because the PSF comes from the stars. |
| **Continuum subtraction** (narrowband) | ★★ | 🔲 | Isolate emission-line signal by subtracting a scaled, PSF-matched broadband/continuum: `NB − k·(BB − median(BB))`. Removes the stellar continuum bleeding through narrowband filters so faint nebulosity stands out. |
| **Synthetic flat / residual flat-fielding** | ★ | ◐ | Derive a flat from the background model to correct residual vignetting/dust the calibration flat missed (StarTools *Wipe* does this as a by-product of gradient removal). Overlaps with gradient extraction. |
| Denoise (starlet wavelet) | — | ✅ | `denoise/` — à trous wavelet thresholding. |
| **ML denoise (CNN, display-domain)** | ★★ | ◐ | `ml::ml_denoise` (feature `ml`) — the same **ort (ONNX Runtime)** tiled backend as star removal; runs a caller-supplied *DeepSNR*-style `.onnx` on the **stretched** image. Verified end-to-end on DeepSNR v2 over the full frame. lumos ships **no model** (caller supplies weights). = *NoiseXTerminator*, GraXpert AI denoise. Note: runs **after** the stretch, not on the linear master. |
| Background neutralization | — | ✅ | `color_calibration::neutralize_background`. |

## B. Star processing

Modern deep-sky processing is "stars vs everything else" — separate, process independently, recombine.

| Fn | Pri | Status | What it does |
|---|---|---|---|
| **Star removal / starless separation** | ★★★ | ◐ | `ml::remove_stars` (feature `ml`, + `ml/README.md`) — **CNN backend working**: an **ort (ONNX Runtime)** runner (CPU), tiled 512² + feather-blended, returns starless + stars (unscreen). lumos ships **no model** (StarNet2/XTerminator licenses forbid it); the caller supplies their own `.onnx`. Verified end-to-end on StarNet2 over the full frame (~60 s for 24 MP on the 4 P-cores of an M-series CPU — memory-bound, so it stays sequential). *Missing:* the license-free **classical morphological/inpainting** fallback. |
| **Star reduction / de-emphasis** | ★★ | 🔲 | Shrink and/or dim stars (morphological erosion, or scaling the separated star layer) so they stop dominating a nebula-rich frame. Usually done on the stars-only layer post-separation. |
| **Star mask generation** | ★★ | ◐ | Build a mask from detected stars (size-graded, dilated) to **protect or select** stars during sharpening/stretch/denoise. lumos already detects stars — this is mostly a rasterization step on top. |
| **Halo / fringe removal ("unpurple")** | ★ | 🔲 | Remove the blue/violet halos and purple fringing around bright stars from refractor chromatic aberration. = Siril *unpurple*, PI *halo* scripts. Localized chroma correction keyed to bright-star positions. |

## C. Stretching & tone (display domain)

| Fn | Pri | Status | What it does |
|---|---|---|---|
| STF / MTF auto-stretch | — | ✅ | `stretching::AutoStf`. |
| Colour-preserving arcsinh | — | ✅ | `stretching::AutoAsinh` / `Asinh`. |
| **Generalized Hyperbolic Stretch (GHS)** | ★★ | ✅ | `stretching::StretchMethod::Ghs` (+ `stretching/ghs.md`) — designer stretch with independent control of **where** contrast lands: strength `D`, local intensity `b`, symmetry/focus point `sp`, shadow/highlight protection `lp`/`hp`. = PI/Siril *GHS*. (Auto-`sp`-from-background variant still open.) |
| **HDR multiscale transform** | ★★ | ✅ | `hdr::compress_dynamic_range` (+ `hdr/README.md`) — à trous starlet, attenuate the coarse residual toward its mean to compress the bright core's large-scale glow while preserving fine detail. Post-stretch. = PI *HDRMultiscaleTransform*, StarTools *HDR*. |
| **Local contrast enhancement (LHE / CLAHE)** | ★★ | ✅ | `local_contrast::enhance_local_contrast` (+ `local_contrast/README.md`) — tiled Contrast-Limited Adaptive Histogram Equalization (clip-limited, bilinear-blended) for medium-scale structure: dust lanes, rifts, filaments. = PI *LocalHistogramEqualization*, StarTools *Contrast*. |
| Masked stretch | ★ | 🔲 | Stretch while protecting already-bright regions via a luminance mask (keeps star cores/bright nebula from blowing out). |

## D. Colour & channel combination

| Fn | Pri | Status | What it does |
|---|---|---|---|
| SCNR green removal | — | ✅ | `color_calibration::scnr`. |
| **Narrowband palette combination (SHO/HOO/…)** | ★★ | 🔲 | Map narrowband channels (Hα, OIII, SII) to RGB with selectable palettes — Hubble **SHO**, **HOO**, bicolor, or a **linear-fit / dynamic** combination that balances channels before mapping. The defining step of narrowband imaging. |
| **LRGB combination** | ★★ | 🔲 | Combine a high-SNR **luminance** (detail) with lower-SNR **RGB** chrominance (colour) — process L and colour separately (sharpen L, denoise colour hard), then merge in a luminance/chroma space. Standard broadband workflow. = PI *LRGBCombination*. |
| **Chrominance-weighted denoise** | ★ | ◐ | Denoise the colour channels harder than luminance (colour noise carries little real detail). The chroma-split extension already sketched in `denoise/README.md`. |

## E. Detail enhancement (multiscale)

| Fn | Pri | Status | What it does |
|---|---|---|---|
| **Multiscale (wavelet) sharpening** | ★★ | ◐ | Scale-selective detail **boost** — amplify chosen layers, recombine. **The starlet is now a shared `wavelet::StarletTransform`** (forward/reconstruct), used by `denoise` (threshold) and `hdr` (compress); sharpening is the same transform with layers *amplified*. = PI *MultiscaleLinearTransform / MMT*, Siril *wavelets*. Nearly free now. |

## F. Composition & multi-frame (astro-specific)

| Fn | Pri | Status | What it does |
|---|---|---|---|
| **Mosaic assembly** | ★ | 🔲 | Stitch overlapping panels into one field with **per-panel background/gradient matching** and seam blending (each panel has its own sky gradient and zero-point). Astro-specific registration + photometric matching. |
| **HDR exposure composition** | ★ | 🔲 | Merge short + long exposure stacks to recover **saturated star cores** / bright cores the long stack clipped (≠ HDRMT, which is tonal). Mask-blend the unsaturated short-exposure cores into the deep stack. |
| **Comet / moving-object stacking** | ★ | 🔲 | Register on a **moving** target (comet/asteroid) so it stays sharp while stars trail (or are rejected). A second registration mode alongside the existing star alignment. |

---

## Suggested priority order

If the goal is "linear master → nice picture," the highest-leverage additions, in order:

1. **Gradient / background extraction** (★★★) — nothing else looks good over a light-pollution gradient. **Core ◐ done** (`background_extraction`, polynomial surface + subtract/divide, per channel); remaining: tiled-mesh/TPS variants, object mask, pipeline wiring.
2. **Photometric colour calibration (PCC/SPCC)** (★★★) — needs the plate-solve + catalog match; objective colour.
3. **Star removal / starless separation** (★★★) — unlocks the modern stretch-the-nebula-hard workflow. (**AI backend ✅ done** via ort; the license-free **classical** fallback is the remaining gap.)
4. **Deconvolution** (★★★) — reuses the star/PSF machinery the detector already has.
5. **Multiscale sharpening** (★★) — small, high-impact, reuses the `denoise/` starlet directly (amplify scales instead of attenuating). (**GHS stretch** ✅ done.)
6. **HDR multiscale** + **local contrast** (★★) — reveal cores and structure. ✅ both done.
7. **LRGB / narrowband combination** (★★) — when targeting those acquisition styles.

The cluster **3–5** is where lumos's existing assets (star detection, the à trous transform, the
registration plate-solve) pay off most — much of the hard infrastructure is already in the crate.

A natural split mirrors the denoise tiering: **classical, implementable cores** (gradient model, RL
deconvolution, morphological star work, GHS, wavelet sharpen, HDR/LHE, palette/LRGB combine) vs the
**heavy ML extensions** (CNN star removal / denoise / deconvolution — *StarXTerminator*,
*NoiseXTerminator*, *BlurXTerminator*, GraXpert AI). **The optional ONNX-runtime backend now exists**
(`ml` feature, **ort**, caller-supplied weights): CNN **star removal** (StarNet2) and **denoise**
(DeepSNR) work today; CNN deconvolution (*BlurXTerminator*-style) would slot into the same tiled
backend. The classical cores remain the priority — the ML path layers on top.

## Deliberately excluded (general image editing — not this library's job)

Curves / levels, generic unsharp mask & sharpening, generic saturation / vibrance / HSL, white/black
point sliders divorced from data, dodge & burn, clone-stamp / healing, crop / rotate / perspective,
generic Gaussian/bilateral blur, film-emulation and generic colour grading. These belong in
Photoshop/Affinity/GIMP; the value here is the astro-specific math above.

---

## Sources

- PixInsight — process reference & tutorials: *DBE/ABE*, *HDRMultiscaleTransform*, *LocalHistogramEqualization (CLAHE)*, *MultiscaleLinearTransform* — pixinsight.com/tutorials, pixinsight.com/examples/HDRWT
- RC Astro — *BlurXTerminator* (PSF deconvolution), *StarXTerminator* (star removal), *NoiseXTerminator*: rc-astro.com/software
- Siril — background extraction, PCC/SPCC, deconvolution, GHS, unpurple: siril.org (incl. siril.org/tutorials/ghs)
- StarTools — *Wipe* (gradient/synthetic flat), *Contrast*, *HDR*, *Decon*, *SVDecon* (spatially-variant), *Color*: startools.org/modules
- Narrowband continuum subtraction & combination: svalgaard.leif.org/mikael/contsubtut.html, nightphotons.com/guides/advanced-narrowband-combination
- GHS: siril.org/tutorials/ghs, remoteastrophotography.com (GeneralizedHyperbolicStretch)
