# Lumos pipeline — code roadmap

Remaining work toward the most precise and most performant stacking pipeline, grouped by impact
tier (highest first). Anchored to source `file:line`. Completed items are dropped — this lists only
what's left.

Status: ☐ todo · ⊘ deferred (deliberate)

---

## Tier 3 — medium (opt-in / cheap / deep-stack)

- ☐ **T3.2 — GESD critical values** · Low (opt-in). The off-by-`i` sample-size bug is fixed;
  remaining is second-order: inverse-**normal** vs Student-t critical values (`rejection.rs:759`)
  and median+MAD vs mean+sd Grubbs statistic. No default preset uses GESD.
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
- ☑ **Cosmic-ray rejection (L.A.Cosmic)** · Done. `calibration_masters::cosmic_ray` +
  `AlignStackConfig.cosmic_ray` (off by default). Mono = subsampled L.A.Cosmic; **Bayer** =
  deinterleave-by-phase + per-plane mono reuse; **X-Trans** = `color_at` same-color detector. Noise:
  empirical (default) or parametric. Ground-truth tested (mono/Bayer/X-Trans synthetic). Remaining
  follow-ups: a CR **mask → stack coverage** path (exclude vs in-paint), and X-Trans **perf** (the
  per-pixel same-color gather is unoptimized). See `docs/pipeline/cosmic-ray-rejection-plan.md`.
- ⊘ dark **scaling** for mismatched exposures (+ bias-free-dark path); calibration
  **uncertainty plane**; drizzle **blot/drizzle-CR**; **CFA/Bayer-drizzle** wiring; **TPS** wired
  into `register()`; **SIP auto-order**.

---

## Precision queue

- ☐ **PR4 — FITS f32 output writer; drop lossy formats from the result path** · High. The only
  output is `AstroImage::save` → TIFF f32 (lossless) or PNG/JPEG (lossy 8-bit); lumos reads
  FITS but cannot write it. Add a FITS f32 writer and restrict the result path to lossless
  formats (TIFF f32 + FITS). PNG/JPEG belong in a viewer, not the pipeline output. The drizzle
  *and* stacking (`StackResult` / `AlignStackResult`) `weight`/`variance`/`coverage` planes are the
  natural FITS extension HDUs (WHT/VAR) for the science product.
- ☐ **PR5 — post-rejection per-channel stack variance/weight planes** · Medium (precision) · M.
  `stack`/`stack_images`/`align_and_stack` now emit geometric `coverage`/`weight`/`variance` planes
  (`StackResult`, `LightCache::geometry_planes` in `stacking/cache.rs`) — channel-independent and
  computed **pre-rejection**, matching drizzle's `Σwᵢ`/`Σwᵢ²` contract. Refinement: rejection drops
  frames per channel, so under aggressive clipping the true effective `N` (hence the variance) is
  per-channel and slightly below the geometric estimate. Make the planes reflect the *surviving*
  set — have `combine_mean` also return `Σw`/`Σw²` over its post-rejection survivors (the indices it
  already tracks in `scratch`), so `weight`/`variance` become exact per-channel `PixelData`. Cost:
  the combine return type grows from `f32` to a small struct and the aux planes need a parallel
  write inside the engine (`UnsafeSendPtr`), so it touches the hot path — deferred from the initial
  geometric version for that reason.

## Performance queue (ARM is the profiled target)

- ☑ **PF5 — parallelize per-color flat-mean + defect sampling** · Done (arch-independent).
  `flat_per_color_inv_means` (`cfa.rs`) reduces the per-color flat sums across rows with rayon:
  **36.6 → 4.5 ms (−88%)** per light frame. `collect_color_samples` (`defect_map.rs`) now
  stride-samples each CFA color in one pass instead of materializing every matching pixel then
  subsampling: **41 → 9.5 ms (−77%)**, and the ~60 MB throwaway alloc is gone.
- ☐ **PF7 — SIMD weighted LM fit** · Low priority (opt-in). PR1's inverse-variance weighted
  centroid fit runs scalar; the unweighted default keeps its AVX2/NEON
  `batch_build_normal_equations`/`batch_compute_chi2`. Vectorizing the weighted path is ~4 new
  weighted kernels (gaussian + moffat × build + chi2; ~400–500 lines NEON now, AVX2 later) plus a
  weighted-fit bench (none exists). Only helps `NoiseModel`-driven centroiding.
- ☐ **PF4** (x86) AVX2 `raw/normalize` (~2×); **PF6** (x86) `threshold_mask` AVX2 (bandwidth-bound,
  modest). Both are x86 AVX2 — deferred until on x86 hardware (can't bench-verify on the arm64 dev
  machine).
