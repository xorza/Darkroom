# Lumos pipeline — code roadmap

Remaining work toward the most precise and most performant stacking pipeline, ranked by
impact on the final combined image. Anchored to source `file:line`.

Status: ☐ todo · ◐ in progress · ⊘ deferred (deliberate)

---

## Tier 1 — silent corruption of the default output

- ◐ **T1.1 — wire `warp()` coverage into a production warp→stack path** · Critical · M
  Coverage is emitted (`WarpResult { image, coverage }`), border flux is renormalized for the
  non-negative kernels (nearest/bilinear), and the negative-lobe kernels (bicubic/**Lanczos**,
  the default) now renormalize the value by the in-bounds weight in-sampler under deringing
  (`interpolation/mod.rs` + `warp/mod.rs` slow path). Remaining: wire coverage into a production
  warp→stack path as `pixel_weight_maps` — `warp`/`drizzle_stack` are bench/test-only today (no
  production consumer), so this is blocked on the end-to-end registered-stack entry point.

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

- ☐ **PR4 — FITS f32 output writer; drop lossy formats from the result path** · High. The only
  output is `AstroImage::save` → TIFF f32 (lossless) or PNG/JPEG (lossy 8-bit); lumos reads
  FITS but cannot write it. Add a FITS f32 writer and restrict the result path to lossless
  formats (TIFF f32 + FITS). PNG/JPEG belong in a viewer, not the pipeline output.

## Performance queue (ARM is the profiled target)

- ☐ **PF1 — NEON Lanczos/bilinear warp** · High (ARM). The default Lanczos3 warp is scalar on
  aarch64; the SSE/AVX FMA kernel has no NEON twin (`interpolation/warp/`). #1 ARM win.
- ☐ **PF7 — SIMD weighted LM fit.** PR1's inverse-variance weighted fit runs scalar (the
  unweighted default path keeps its AVX2/NEON kernels). Add weighted AVX2/NEON
  `batch_build_normal_equations`/`batch_compute_chi2` so `NoiseModel`-driven fits vectorize.
  Low priority (weighted fit is opt-in).
- ☐ **PF4** (x86) AVX2 `raw/normalize` (~2×); **PF5** parallelize the serial per-color flat-mean
  pass (`cfa.rs:272`) + defect-map sample collection (60 MB throwaway); **PF6** (x86)
  threshold_mask AVX2 (bandwidth-bound, modest).
