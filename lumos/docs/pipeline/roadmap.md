# Lumos pipeline — code roadmap

Derived from a stage-by-stage audit of `docs/pipeline/` (best-practices reference)
**against the actual code** (2026-06-06). Each item was confirmed against source with
`file:line`. Ranked by **impact on the final combined image**, weighting silent
corruption > missing feature, default-config path > opt-in, and downstream blast
radius. Verification notes for refuted/stale claims are at the bottom.

Status: ☐ todo · ◐ in progress · ☑ done · ⊘ deferred (deliberate)

## Verdict
Core algorithms are sound (calibration order, per-color flat, negative preservation,
Hartley-normalized DLT, reject-after-normalize, Winsorization 1.134, F&H boxer all
verified correct). Gaps cluster in three seams: the warp→stack handoff loses
coverage; rejection runs unguarded at low N on the default path; calibration/saturation
linearity edges.

---

## Tier 1 — fix first (silent corruption of the default output)

- ◐ **T1.1 — `warp()` coverage mask + border flux renormalization** · Critical · M — *Scope A+B done*
  - `warp()` returns `WarpResult { image, coverage }` (`registration/mod.rs`),
    `WarpResult` re-exported; all 9 callers (tests/benches/examples) use `.image`.
  - **Scope B (no scratch buffer):** coverage is a weight-only geometric pass
    (`interpolation::warp_coverage` / `coverage_at`) that mirrors each kernel's
    tap layout + weights to produce `Σ_in(w)/Σ_all(w)` ∈ [0,1] for every method —
    reads no pixel data, allocates no `ones` buffer, and runs in the same
    inverse-mapped row order as `warp_image` with the same incremental stepping.
    Replaced the earlier ones-warp (which needed a full-frame scratch + second
    sampling pass). The value SIMD kernels are untouched.
  - Border-flux darkening fixed by `value /= coverage` for the non-negative
    kernels (nearest/bilinear) when `border_value == 0`; interior bit-exact.
    Tests: `warp_emits_coverage_and_renormalizes_bilinear_border` (exact),
    `warp_emits_coverage_for_lanczos_without_renormalizing` (default kernel).
  - **Still open:** (1) value-renormalization for the negative-lobe kernels
    (bicubic/**Lanczos**, the default) — their coverage is emitted (so downstream
    down-weighting mitigates the darkening), but exact value renorm under deringing
    would need true in-sampler weight tracking; (2) wiring coverage into a
    production warp→stack path as `pixel_weight_maps` (no production consumer yet —
    `warp`/`drizzle_stack` are bench/test-only today).

- ☑ **T1.2 — small-N rejection guard in `stack()`** · High · S — *done*
  - `run_stacking` now calls `effective_combine_method`: σ-based rejection
    (`Rejection::needs_many_frames` = SigmaClip/LinearFit/GESD) downgrades to the
    median with a `tracing::warn!` below `MIN_FRAMES_FOR_REJECTION = 5`. Winsorized
    (small-stack design), Percentile (fixed-fraction), None, and explicit Median
    pass through. Floor is the correctness boundary (σ unsound at N≤4); calibration
    keeps its own larger quality threshold. `stack.rs` + `rejection.rs`, 4 new tests.

---

## Tier 2 — high (corrupts output; narrower or quantitative)

- ☑ **T2.1 — unclamped RAW calibration load** · High · M — *done*
  - `normalize.rs` + `apply_channel_corrections` now share a `const CLAMP: bool`
    kernel (branch-free SIMD, no duplication); added
    `normalize_u16_to_f32_parallel_unclamped` + `apply_channel_corrections_unclamped`.
    `extract_cfa_pixels(clamp)` branches once: `load_raw` mono-light → `true`
    (keeps the [0,1] display contract, unchanged), `load_raw_cfa` calibration →
    `false` (signed, un-clipped, so master dark/bias means aren't biased upward).
    X-Trans light path untouched. 1 new test; all 104 raw tests still green.

- ⊘ **T2.2 — saturation masking** · ~~High~~ → **Low (premise moot)** — *investigated, downgraded*
  - **The B5 "biased positions" premise does not hold.** `Star.peak = region.peak_value`
    is the **raw**-plane region max (`centroid/mod.rs:426`; regions extracted from raw
    `pixels` at `detect.rs:119`, not the convolved plane), and normalization maps the
    sensor white level to exactly `1.0`. So any genuinely clipped (flat-topped) star has
    `peak == 1.0 > 0.95` → already **rejected** by `filter.rs:46` (and excluded from FWHM
    at `fwhm.rs:131`) before its fit can bias anything. Kept stars (peak ≤ 0.95) aren't
    clipped → centroids already unbiased. No registration-accuracy gain available.
  - Residual (cosmetic, Low): `metadata.data_max` is set on the FITS path but never read
    by the detector (dead); `0.95` is hardcoded in 3 spots (could be a `Config` knob).
  - The WB-clip confound also resolves to *rejection*, not bias — same conclusion.

- ☐ **T2.3 — drizzle output variance/weight map** · High (quantitative) · L
  - `DrizzleResult` has only image + coverage (`drizzle/mod.rs:699`). At default
    scale=2/pixfrac=0.8, R≈2 → output RMS understates true noise ~2× with no weight map.
  - Fix: accumulate + return per-channel weight (and optional variance) map.

- ☑ **T2.4 — deblend contrast vs root flux, not parent** · High (crowded) · M — *done*
  - `collect_significant_leaves` (`deblend/multi_threshold/mod.rs`) now tests each
    branch against `min_contrast * root_flux` — the island's total isophotal flux
    (`process_root_level`'s root node), threaded down from `collect_roots_and_leaves`
    — instead of the shrinking-with-depth parent-node flux. Matches SExtractor's
    global per-island bar; stops over-splitting bright wings in crowded fields.
    New unit test `deblend_contrast_bar_is_root_flux_not_parent` (hand-built tree:
    root-relative → 2 objects, parent-relative would give 3). No detection/pipeline
    regressions (1970 tests green).
  - *Possible follow-up (Low):* SExtractor also subtracts the `thresh·npix` pedestal
    (`fdflux − thresh·fdnpix`) before the contrast test; lumos compares raw
    above-threshold flux. Minor refinement, not done.

---

## Tier 3 — medium (opt-in / cheap / deep-stack)

- ☑ **T3.1 — flat division floor** · Medium · S — *done*
  - All three flat-division paths (`cfa.rs` mono ±bias + per-CFA-color) now divide
    by `norm_flat.max(CfaImage::MIN_NORMALIZED_FLAT)` (0.1) instead of skipping when
    `norm_flat ≤ EPSILON`. Every pixel ends in one consistent calibration state,
    near-zero/negative divisors can't blow up or flip sign, and noise amplification
    in deep vignetting is bounded to 10× (ccdproc/PixInsight `min_value`). New test
    `test_divide_by_normalized_floors_dead_flat_pixel`; vignetting test (flats ≥ 0.1)
    unchanged.
- ◐ **T3.2 — GESD over-rejection** · Medium (opt-in) — *off-by-`i` fixed*
  - Fixed the sample-size bug (`rejection.rs`): the Rosner/GESD critical value now
    uses the live count `ni = n−i` directly instead of the double-subtracted `n−2i`,
    which had shrunk λ and over-rejected. Verified against Rosner's formula; the two
    stale hand-computed λ comments (`test_gesd_removes_single_bright_outlier`,
    `test_gesd_keeps_tight_cluster`) updated to the corrected values (step-1 λ
    1.63→≈1.74; outcomes unchanged, 13 GESD tests green).
  - *Left (Low, opt-in):* inverse-**normal** instead of Student-t critical values
    (`:759`) and median+MAD instead of mean+sd Grubbs statistic — both push the same
    over-reject direction but are second-order; no default preset uses GESD.
- ☐ **T3.3 — warp/drizzle pixel-center mismatch (~0.5px)** · Medium · S
  - drizzle uses `+0.5` center (`drizzle/mod.rs:352`), warp integer (`interpolation/mod.rs:313`).
- ☐ **T3.4 — `Auto` ladder skips Euclidean/Affine** · Medium · S — `mod.rs:163-185`.
- ☐ **T3.5 — drizzle uncompensated f32 accumulation** · Medium (deep stacks) · M — `drizzle/mod.rs:619`.
- ◐ **T3.6 — NaN-safe combine on in-memory path** · Medium — *guard added*
  - Added a `debug_assert!(values.iter().all(is_finite))` at the single combine
    chokepoint (`cache.rs` `process_chunked`, before the reducer closure) — catches a
    NaN/Inf leaking into the combine from any future upstream regression, debug-only.
    No live source today (FITS load sanitizes, T3.1 floored flat division, warp
    border-fills 0), so the full NaN-skipping rewrite of the reducers is unwarranted.

---

## Tier 4 — missing features & fidelity (deliberate; not bugs)

- ⊘ Missing capabilities (schedule deliberately): dark **scaling** for mismatched
  exposures (+ bias-free-dark path); single-frame **cosmic-ray** rejection (L.A.Cosmic);
  calibration **uncertainty plane**; drizzle **blot/drizzle-CR**; **CFA/Bayer-drizzle**
  wiring; **TPS** wired into `register()`; **SIP auto-order**.
- ⊘ Fidelity (correct enough, diverge from references, no pipeline corruption):
  - roundness1/2 swapped + ×2 dropped + marginal-max + unconvolved stamp
    (`centroid/mod.rs:671-694`) — **not** consumed downstream; README over-rates this.
  - background median/MAD, no Pearson mode estimator (`background/tile_grid.rs:452`).
  - MAGSAC++ loss is a bespoke bounded kernel (`ransac/magsac.rs:63`) — works; doc-comment wrong.

---

## Stale / refuted — docs audit a prior implementation (refresh pass needed)
- Integer-FITS `BLANK` "unhandled" — REFUTED; `fits-well` maps BLANK→NaN, lumos→0 (`fits.rs:166`).
- "only `primary_hdu()` / no multi-extension / tile-compressed FITS" — REFUTED;
  `fits.rs:36` selects first image HDU + decompresses RICE/GZIP/PLIO/HCOMPRESS.
  `01-load-decode.md` still describes a retired `rust-fitsio` loader throughout.
- "no tile-grid median filter" (Stage 3) — REFUTED; present at `tile_grid.rs:208`.
- Residual real sub-points: 0-substitution for BLANK/NaN biases means without a mask
  (`fits.rs:166`); float-FITS per-file divide-by-max breaks inter-frame scale (`fits.rs:173`).

## Verified correct (do not touch)
Calibration order + per-color flat + negative preservation (`cfa.rs`); reject-*after*-
normalize (`cache.rs:434`); Winsorization 1.134 (`rejection.rs:201`); Hartley-normalized
DLT + analytic Umeyama (`ransac/transforms.rs`); single-pass SIP-then-linear warp
(`transform.rs:346`); F&H boxer/sgarea (`drizzle/mod.rs:768`); matched-filter
normalization (`convolution/mod.rs`).

---

# Review pass 2 (2026-06-06): precision · performance · removal

Goal: most-precise calibration/detection/stacking + most-performant. Four parallel
analyst passes (perf ×2, removal, precision), de-noised + verified.

## Removal — done this session (~800 LOC, all verified zero-reference)
- ☑ `clear_buffer_pool` (trivial speculative), `write_grayscale_buffer` + its orphaned
  `rgb_to_luminance` helper (duplicate of `into_grayscale`).
- ☑ `background/simd/` sum/deviation family — `sum_and_sum_sq_*` / `sum_abs_deviations_*`
  across `mod.rs` (+ ~17 tests) and the whole `sse.rs` + `neon.rs` (they held only the
  sum backends). Duplicated `math/sum` and superseded by the median/MAD background. Kept
  the cubic-spline SIMD (the live path). Suite 1971→1950, green.

## Wired this session
- ☑ `detection_file.rs` sidecar — restored after an over-eager removal, then properly
  wired: `save_detection_result` / `load_detection_result` round-trip, plus a config- and
  mtime-aware cache (`load_if_fresh`) behind `StarDetector::detect_file_cached`. Sidecar
  stores a `{format_version, config_fingerprint, result}` envelope; a stale image (mtime),
  changed config (Debug-hash fingerprint), or version bump is a safe cache miss. Public via
  `lumos::{sidecar_path, save_detection_result, load_detection_result}` + demoed in
  `examples/star_detection.rs`. 6 tests (round-trip, mtime, config, corrupt/version, e2e).

## Kept (intentional — wire later, per decision)
- ⊘ `drizzle` (2,464 LOC) — no production caller yet; keep as the F&H combine feature.
- ⊘ `registration/distortion/tps` (1,037 LOC) — unwired SIP-alternative; keep to wire later.
- ⊘ `AstroImageMetadata` fields + `data_max` — forward-looking provenance; `exposure_time`
  is needed for the future dark-scaling item. Leave.
- Suggested (not done): gate the test-only accessors (`get_pixel_*`, `pattern_2x2`, defect
  counters, `RansacResult.{iterations,inlier_ratio}`, …) behind `test_support` per house
  style — they're test scaffolding in the prod surface, not dead.

## Precision queue (toward "most precise")
- ☐ **PR1 — weighted (inverse-variance) PSF fit** · High. LM minimizes plain `Σr²`
  (`centroid/lm_optimizer.rs:88`, gaussian/moffat + AVX2/NEON); ML for Poisson/CCD noise
  is `w=1/σ²` (over-weights the bright core → biases the sub-px centroid/FWHM/flux that
  feed registration). `NoiseModel` + per-pixel noise map already exist. Multi-backend;
  shifts results → validate with real-data tests.
- ☐ **PR2 — second moments on unclamped signed `(px−bg)`** · Med. `centroid/mod.rs:574`
  clamps wing negatives to 0, biasing FWHM/eccentricity low (feeds matched-filter sizing
  + registration `max_sigma`). Distinct from the known flux-clamp item.
- ☐ **PR3 — compensated weighted-mean after rejection** · Med (≈free). The default
  light path (`Noise`+`SigmaClip`) sums in naive f32 (`rejection.rs:954`); route through
  `math::sum::weighted_mean_f32` like the other combine branches.

## Performance queue (toward "most performant"; ARM is the profiled target)
- ☐ **PF1 — NEON Lanczos/bilinear warp** · High (ARM). Default Lanczos3 warp is scalar on
  aarch64; the SSE/AVX FMA kernel has no NEON twin (`interpolation/warp/`). #1 ARM win.
- ☐ **PF2 — RAW demosaic planar output** · High. RCD/Markesteijn build planar → interleave
  → `from_pixels` de-interleaves back (≈6·P traffic + 3·P alloc per RAW light frame). Give
  demosaic a planar contract → `from_planar_channels` (zero-copy).
- ☐ **PF3 — per-star ×3 f64 `Vec` alloc in LM fit** · High (when fitting). `gaussian_fit/mod.rs:242`
  + moffat, inside the parallel `measure` loop → per-thread scratch / f64 stamp fields.
- ☐ **PF4 (x86)** AVX2 `raw/normalize` (~2×); **PF5** parallelize the serial per-color flat-mean
  pass (`cfa.rs:272`) + defect-map sample collection (60 MB throwaway); **PF6 (x86)**
  threshold_mask AVX2 (bandwidth-bound, modest).
- Doc fix: CLAUDE.md's SIMD table is stale — `background`/`convolution`/`median_filter`
  already have AVX2; the real gaps are `raw/normalize` + `threshold_mask` (x86).
