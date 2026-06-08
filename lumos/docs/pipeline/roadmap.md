# Lumos pipeline — code roadmap

Remaining work toward the most precise and most performant stacking pipeline, ranked by
impact on the final combined image. Anchored to source `file:line`.

Status: ☐ todo · ◐ in progress · ☑ done · ⊘ deferred (deliberate)

---

## Tier 1 — silent corruption of the default output

- ☑ **T1.1 — wire `warp()` coverage into the warp→stack path** · Critical · M
  Done. `align_and_stack` carries each warped frame's coverage (the unwarped reference gets a
  full-`1.0` map) into `stack_images(pixel_weight_maps)`, held on the `ImageCache`; the combine
  (`cache.rs` `process_chunked`) includes a frame at a pixel only where coverage >
  `COVERAGE_EPSILON`, weighted by `coverage × per-frame weight`, and fills `0` where no frame
  covers. Excluding sub-ε coverage keeps warp border-fill out of the rejection set, so the dark
  warped-edge ring is gone. Per-kernel value renormalization (nearest/bilinear/bicubic/Lanczos)
  was done earlier.

## Tier 2 — high (corrupts output; narrower or quantitative)

- ☑ **T2.3 — drizzle output variance/weight map** · High (quantitative) · L
  Done. `DrizzleAccumulator` now also accumulates `Σwᵢ²`, and `DrizzleResult` returns a `weight`
  map (absolute `Σwᵢ`, the WHT) and a `variance` map (`Σwᵢ²/(Σwᵢ)²` = output variance per unit
  input variance — the true per-pixel noise the correlation-suppressed image RMS understates;
  multiply by input noise variance for absolute). The per-channel weight buffers collapsed to one
  (the weight is geometric, channel-independent). Coverage (normalized `[0,1]`) is unchanged.

## Tier 3 — medium (opt-in / cheap / deep-stack)

- ☐ **T3.2 — GESD critical values** · Low (opt-in). The off-by-`i` sample-size bug is fixed;
  remaining is second-order: inverse-**normal** vs Student-t critical values (`rejection.rs:759`)
  and median+MAD vs mean+sd Grubbs statistic. No default preset uses GESD.
- ☑ **T3.3 — warp/drizzle pixel-center mismatch (~0.5px)** · Already resolved (stale entry). Both
  use integer-center mapping: warp applies the transform to the raw output index
  (`warp/mod.rs` `wt.apply(DVec2::new(x_idx, output_y))`), drizzle to the raw input index
  (`drizzle/mod.rs` `transform.apply(DVec2::new(ix, iy))`); drizzle's `+0.5` is only the output
  *cell* extent `[o-0.5, o+0.5)` for overlap area, not a coordinate offset. All four drizzle kernels
  + warp + star centroids agree — no offset to fix.
- ☑ **T3.4 — `Auto` transform ladder skips Euclidean/Affine** · Done. `register`'s `Auto` path now
  uses `auto_ladder` (Euclidean → Similarity → Affine → Homography), accepting the first model
  within 0.5px RMS so same-scale rigid sets aren't fit with a needless scale DOF and mild linear
  distortion doesn't overshoot to the full projective model; falls through to Homography otherwise.
- ☐ **T3.5 — drizzle uncompensated f32 accumulation** · Medium (deep stacks) · M — `drizzle/mod.rs:619`.

## Tier 4 — missing features (deliberate; schedule when needed)

- ☐ **Streaming warp→disk for low-memory registered stacking** · Medium · L. The combine cache
  is coverage-disk-*ready* — `WeightedFrame.coverage: Option<Plane>` and `Plane::Mapped` mean a
  warped frame's channels **and** coverage can be memory-mapped together. What's missing is a
  *producer*: today `warp` returns a full `AstroImage` in RAM and `align_and_stack` holds
  `Vec<AstroImage>`, so a large registered stack OOMs at the warp stage regardless. Add a
  streaming path — warp frame *i* → spill its channels+coverage to the disk cache → drop RAM →
  repeat — so registered/coverage-weighted stacking scales past RAM. Needs: a spilling
  `LightCache` constructor (write a `WeightedFrame` to mmap files) + `align_and_stack`
  feeding frames one at a time.
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
