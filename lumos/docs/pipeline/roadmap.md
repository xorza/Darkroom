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

- ◐ **T1.1 — `warp()` coverage mask + border flux renormalization** · Critical · M — *Scope A done*
  - `warp()` now returns `WarpResult { image, coverage }` (`registration/mod.rs`).
    Coverage is computed by warping an all-ones source through the *same* sampler
    with a zero border (`warp_coverage`), so `coverage = Σ_in(w)/Σ_all(w)` ∈ [0,1]
    for every method (incl. SIMD) with no sampler changes and no drift. Border-flux
    darkening is fixed by `value /= coverage` for the non-negative kernels
    (nearest/bilinear) when `border_value == 0` — interior pixels stay bit-exact.
    New test `warp_emits_coverage_and_renormalizes_bilinear_border`; all 9 callers
    (tests/benches/examples) updated to `.image`; `WarpResult` re-exported.
  - **Deferred (Scope B):** value-renormalization for the negative-lobe kernels
    (bicubic/**Lanczos**, the default) — needs in-sampler in-bounds-weight tracking,
    not the ones-warp post-pass; their coverage *is* emitted, so downstream
    down-weighting already mitigates the darkening. And wiring coverage into a
    production warp→stack path as `pixel_weight_maps` (no production consumer exists
    yet — `warp`/`drizzle_stack` are bench/test-only today).

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

- ☐ **T2.2 — saturation: carry true white level + mask in fits** · High · M (Stage 1→3→4)
  - `load_raw` never sets `metadata.data_max` (`raw/mod.rs:762`); saturation tested at
    magic `0.95` (`detector/stages/filter.rs:46`); saturated cores enter LM fit
    unmasked (`gaussian_fit/mod.rs:166`) → biased positions leak to registration.
  - Fix: carry per-channel saturation from raw `maximum`/`cblack`; mask sat pixels in
    centroid/FWHM.

- ☐ **T2.3 — drizzle output variance/weight map** · High (quantitative) · L
  - `DrizzleResult` has only image + coverage (`drizzle/mod.rs:699`). At default
    scale=2/pixfrac=0.8, R≈2 → output RMS understates true noise ~2× with no weight map.
  - Fix: accumulate + return per-channel weight (and optional variance) map.

- ☐ **T2.4 — deblend contrast vs root flux, not parent** · High (crowded) · M
  - `deblend/multi_threshold/mod.rs:810-816` uses `min_contrast*node.flux` (parent);
    SExtractor uses `mincont*root_flux`. Over-splits bright stars → poisons registration.
  - Fix: thread root flux down; test `child.flux ≥ min_contrast*root_flux`.

---

## Tier 3 — medium (opt-in / cheap / deep-stack)

- ☐ **T3.1 — flat division floor** · Medium · S (one-liner)
  - `cfa.rs:233/244/308` skips division when `norm_flat ≤ EPSILON`, leaving pixels
    un-flat-corrected. Fix: `*l /= norm_flat.max(min_value)`.
- ☐ **T3.2 — GESD over-rejection** · Medium (opt-in) · S+M
  - `rejection.rs:737-742`: off-by-`i` sample size (`n−2i` vs `n−i`, fix S);
    inverse-normal not Student-t (`:759`); median+MAD not mean+sd. No default uses GESD.
- ☐ **T3.3 — warp/drizzle pixel-center mismatch (~0.5px)** · Medium · S
  - drizzle uses `+0.5` center (`drizzle/mod.rs:352`), warp integer (`interpolation/mod.rs:313`).
- ☐ **T3.4 — `Auto` ladder skips Euclidean/Affine** · Medium · S — `mod.rs:163-185`.
- ☐ **T3.5 — drizzle uncompensated f32 accumulation** · Medium (deep stacks) · M — `drizzle/mod.rs:619`.
- ☐ **T3.6 — NaN-safe combine on in-memory path** · Medium · M — `statistics/mod.rs:66`, couples T1.1/T3.1.

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
