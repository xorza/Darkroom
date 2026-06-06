# Lumos pipeline — code roadmap

Remaining work toward the most precise and most performant stacking pipeline, ranked by
impact on the final combined image. Anchored to source `file:line`.

Status: ☐ todo · ◐ in progress · ⊘ deferred (deliberate)

---

## Tier 1 — silent corruption of the default output

- ◐ **T1.1 — `warp()` coverage mask + border-flux renormalization** · Critical · M
  Coverage is emitted (`WarpResult { image, coverage }`) and border flux renormalized for the
  non-negative kernels (nearest/bilinear). Remaining: (1) value-renormalization for the
  negative-lobe kernels (bicubic/**Lanczos**, the default) — coverage is emitted but exact
  value renorm under deringing needs true in-sampler weight tracking; (2) wire coverage into a
  production warp→stack path as `pixel_weight_maps` (`warp`/`drizzle_stack` are bench/test-only
  today — no production consumer yet).

## Tier 2 — high (corrupts output; narrower or quantitative)

- ☐ **T2.3 — drizzle output variance/weight map** · High (quantitative) · L
  `DrizzleResult` has only image + coverage (`drizzle/mod.rs:699`). At default
  scale=2/pixfrac=0.8, R≈2 → output RMS understates true noise ~2× with no weight map.
  Fix: accumulate + return a per-channel weight (and optional variance) map.

## Tier 3 — medium (opt-in / cheap / deep-stack)

- ☐ **T3.2 — GESD critical values** · Low (opt-in). The off-by-`i` sample-size bug is fixed;
  remaining is second-order: inverse-**normal** vs Student-t critical values (`rejection.rs:759`)
  and median+MAD vs mean+sd Grubbs statistic. No default preset uses GESD.
- ☐ **T3.3 — warp/drizzle pixel-center mismatch (~0.5px)** · Medium · S. drizzle uses `+0.5`
  center (`drizzle/mod.rs:352`), warp integer (`interpolation/mod.rs:313`).
- ☐ **T3.4 — `Auto` transform ladder skips Euclidean/Affine** · Medium · S — `mod.rs:163-185`.
- ☐ **T3.5 — drizzle uncompensated f32 accumulation** · Medium (deep stacks) · M — `drizzle/mod.rs:619`.

## Tier 4 — missing features (deliberate; schedule when needed)

- ⊘ dark **scaling** for mismatched exposures (+ bias-free-dark path); single-frame
  **cosmic-ray** rejection (L.A.Cosmic); calibration **uncertainty plane**; drizzle
  **blot/drizzle-CR**; **CFA/Bayer-drizzle** wiring; **TPS** wired into `register()`;
  **SIP auto-order**.

---

## Precision queue

- ☐ **PR1 — weighted (inverse-variance) PSF fit** · High. LM minimizes plain `Σr²`
  (`centroid/lm_optimizer.rs:88`, gaussian/moffat + AVX2/NEON); the ML estimator for
  Poisson/CCD noise is `w = 1/σ²` (over-weights the bright core → biases the sub-px
  centroid/FWHM/flux that feed registration). `NoiseModel` + per-pixel noise map already exist.
  Shifts results → validate on real data.
- ☐ **PR4 — FITS f32 output writer; drop lossy formats from the result path** · High. The only
  output is `AstroImage::save` → TIFF f32 (lossless) or PNG/JPEG (lossy 8-bit); lumos reads
  FITS but cannot write it. Add a FITS f32 writer and restrict the result path to lossless
  formats (TIFF f32 + FITS). PNG/JPEG belong in a viewer, not the pipeline output.

## Performance queue (ARM is the profiled target)

- ☐ **PF1 — NEON Lanczos/bilinear warp** · High (ARM). The default Lanczos3 warp is scalar on
  aarch64; the SSE/AVX FMA kernel has no NEON twin (`interpolation/warp/`). #1 ARM win.
- ☐ **PF4** (x86) AVX2 `raw/normalize` (~2×); **PF5** parallelize the serial per-color flat-mean
  pass (`cfa.rs:272`) + defect-map sample collection (60 MB throwaway); **PF6** (x86)
  threshold_mask AVX2 (bandwidth-bound, modest).
