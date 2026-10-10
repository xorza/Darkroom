# Calibration review — lumos

Scope: `lumos/src/calibration_masters/**`, `lumos/src/pipeline/calibrate.rs`, the per-light path in
`pipeline/light_source.rs`, and the calibration-specific use of `combine` (master presets, master
metadata). References: Siril (`src/core/preprocess.c`, `src/core/siril.c`,
`src/filters/cosmetic_correction.c`, `src/stacking/normalization.c`), ccdproc (`ccdproc/core.py`),
DSS (`DeepSkyStackerKernel/DarkFrame.cpp`, `FlatFrame.cpp`).

---

## CAL-5 — Hot-pixel threshold is tied to the master dark's own spread, not to what the dark leaves in a light

- **Where:** `lumos/src/calibration_masters/defect_map/mod.rs:301-355`, `mod.rs:48`
  (`DEFAULT_SIGMA_THRESHOLD = 5.0`)
- **Category:** precision
- **Impact:** medium — on a well-stacked master dark the robust σ (MAD / lower 1% tail) is the master's
  read-noise/√N plus DSNU. 5σ of that flags many warm pixels whose dark current the dark subtracts
  exactly and whose added shot noise is negligible against the sky. Each one is replaced by an
  interpolated neighbour median and flagged `DEFECT|REPAIRED`. With dithering the combine drops them
  (coverage loss). Without dithering, interpolated values enter the stack.
- **Confidence:** likely (statistical argument; the flagged fraction was not measured on real data)
- **Evidence:** Siril's `find_deviant_pixels` (`src/filters/cosmetic_correction.c:201-233`) uses
  `median + k·stddev`. DSS `FindHotPixels` (`DarkFrame.cpp:1738-1740`) uses `median + 16·stddev`.
  In both, the plain std-dev is inflated by the hot tail itself, which makes them far less
  aggressive than a robust 5σ. The module doc says the defects "pass 1% of a colour" on uncooled
  sensors.
- **Recommended direction:** Decide what a hot pixel *promises*: residual error after scaled dark
  subtraction, or shot noise that dominates the light's noise. Derive the threshold from that, for
  example dark-current shot noise in the light against the light's noise (gain known), plus
  non-linearity or saturation. At minimum, report the flagged fraction per colour and test it on a
  real cooled-CMOS master.

## CAL-6 — Cosmic-ray pass ignores saturation and existing quality flags

- **Where:** `lumos/src/calibration_masters/cosmic_ray/mod.rs:59-101`
- **Category:** correctness
- **Impact:** medium — saturated star cores (flat tops with sharp edges) are the classic L.A.Cosmic
  false positive. They get in-painted and flagged `COSMIC_RAY|REPAIRED`. In-paints also draw from
  saturated or `NO_DATA`-filled neighbours.
- **Confidence:** confirmed (by reading; no use of `image.flags` before detection)
- **Evidence:** astroscrappy, which ccdproc wraps (`ccdproc/core.py:2569-2583`, `satlevel=65535`),
  finds saturated stars and removes them from the CR mask. lumos reads neither `SATURATED` nor
  `NO_DATA`, and the in-paint (`mono.rs:346-380`, `xtrans.rs:313-351`) masks only CR pixels.
- **Recommended direction:** Exclude dilated `SATURATED` regions from the primary mask (as
  astroscrappy's satstar mask does). Keep `UNMEASURED` pixels out of the in-paint gathers. Add a
  test with a clipped star core.

## CAL-7 — X-Trans CR detector reuses astroscrappy's thresholds on a different statistic

- **Where:** `lumos/src/calibration_masters/cosmic_ray/xtrans.rs:187-288`
- **Category:** precision
- **Impact:** medium — `sigclip=4.5` and `objlim=5` were calibrated (van Dokkum 2001, astroscrappy)
  for `S' = L⁺/(2N) − med₅` of a ×2-subsampled Laplacian. The X-Trans path thresholds
  `(v − median₈)/N` with no `S'` step and an irregular-stencil `F`. Its false-positive rate and
  star protection at the defaults are therefore uncalibrated. The noise of `v − median₈` alone is
  about 1.1 N, not N.
- **Confidence:** likely
- **Evidence:** The only X-Trans test (`cosmic_ray/tests.rs:272-319`) is an 18×18 flat field with one
  spike and loose assertions (`count >= 1`, `< 0.05`, `< 0.02`). There is no noise-only
  false-positive test and no star-preservation test for X-Trans, though both exist for mono.
- **Recommended direction:** Calibrate the X-Trans statistic so the default `sigclip` means the same
  tail probability: normalize by the measured σ of `v − median₈` on pure noise. Add a noise-only
  false-positive test (exact count 0 at the default seed) and a star-preservation test, as for mono.

## CAL-8 — Per-light calibration makes 4–6 full-frame sweeps plus serial flag recounts

- **Where:** `lumos/src/calibration_masters/mod.rs:423-437`, `io/image/cfa/mod.rs:444-506`,
  `prepared_flat/mod.rs:59-83`, `defect_map/mod.rs:264-270`, `io/image/pixel_flags.rs:125-145,340`
- **Category:** performance
- **Impact:** medium — this is the per-frame hot path. Each of these passes the whole frame:
  `record_saturation` (scan), bias subtract (r/w L + read B), dark subtract (r/w L + read D), flat
  divide (r/w L + read F), and `FLAT_FLOOR` `add_where`. Defect `add_where` scans the whole frame
  to set a sparse index list. Every `add_where` on an existing plane ends in `counts_of`, a
  single-threaded scan of the full byte plane. On a 60 MP frame that is about 2.6 GB of f32
  traffic, where a fused pass needs about 1.2 GB, plus up to four serial 60 MB scans.
- **Confidence:** likely (from reading; not benchmarked, per instructions)
- **Evidence:** Siril applies offset, dark and flat in one `preprocess` (`preprocess.c:121-152`),
  though also as separate `imoper` sweeps.
- **Recommended direction:** One fused row-parallel kernel `(L − B·g_b − o_b − s·(D·g_d + o_d)) /
  F`. It is also one rounding instead of three, so precision gains too. Saturation and master-flag
  propagation go in the same pass. Precompute `FLAT_FLOOR` into the divisor's own flags once, so
  `take_master_flags` ORs it. Set defect flags by index and update counts incrementally rather than
  rescanning.

## CAL-9 — Cosmic-ray iterations 2..n recompute the whole frame although only CR neighbourhoods changed

- **Where:** `lumos/src/calibration_masters/cosmic_ray/mono.rs:100-144`, `xtrans.rs:203-228`
- **Category:** performance
- **Impact:** medium — each iteration runs a Laplacian, four window medians (3×3, 7×7, 5×5, 5×5)
  and the noise map over the full plane, up to `niter = 4` times. After iteration 1 only pixels
  within about 6 px of an in-painted pixel can change S′, F or N: the Laplacian reaches ±1, `med₅`
  for N ±2, `med₅(S)` ±2 more, `med₃∘med₇` ±4, and growth ±2. Every other pixel recomputes to the
  identical value.
- **Confidence:** likely
- **Recommended direction:** After the first pass, recompute only tiles or rows touched by the
  dilated new-CR mask. The result is bit-identical, so precision is unaffected. Verify against the
  full recompute in a test.

## CAL-10 — Master FITS bundle load reads the whole file into memory and reports errors as strings

- **Where:** `lumos/src/calibration_masters/fits.rs:107-164,422-424`
- **Category:** performance / style
- **Impact:** low — `fs::read` holds the whole bundle (three f32 masters on a 60 MP sensor is about
  720 MB) beside the decoded masters, roughly doubling peak memory at load. Every failure is
  `io::Error::new(InvalidData, String)`, which breaks the rule that errors are enums of cases.
- **Confidence:** confirmed (by reading)
- **Recommended direction:** Read HDUs through a memory map or a streaming reader. Give bundle
  loading a typed error enum (`BundleError::{NotABundle, Version{found}, Checksum{hdu},
  DuplicateExtension, …}`).

## CAL-11 — `quantization_sigma` survives calibration unchanged

- **Where:** `lumos/src/calibration_masters/mod.rs:423-438`
- **Category:** precision
- **Impact:** low — after the dark (which adds its own quantization variance) and the flat (which
  scales the step by `1/f`, up to 10× at the floor), the light's recorded `quantization_sigma` no
  longer describes its samples. It is used as a noise floor downstream
  (`frame_store/frame_stats.rs:148,164`, `MosaicNoise::measure` via `io/image/cfa/mod.rs:340-347`).
- **Confidence:** speculative (impact depends on how often the floor binds)
- **Recommended direction:** Either propagate it (√(q_L² + s²·q_D² + q_B²)/f_min, documented as a
  bound) or drop it after flat division, as demosaic already drops it.

## CAL-12 — `Gain` noise model is a constant after flat division

- **Where:** `lumos/src/calibration_masters/cosmic_ray/noise_model.rs:43-51`
- **Category:** precision
- **Impact:** low — CR detection runs after the flat (`pipeline/light_source.rs:393-401`), so
  Poisson variance per unit is `1/(g·f)`, not `1/g`. With `NoiseEstimation::Gain` the source term is
  under-estimated by `1/f` in vignetted corners, which over-flags there. `Measured` mode adapts
  through the local mesh, but only for the sky term.
- **Confidence:** likely
- **Recommended direction:** Pass the prepared flat divisor (or `1/f` per pixel) into the noise model
  for the source term. Alternatively, document that the stated gain is per flat-normalized unit.

## CAL-13 — Calibration errors lose which light failed

- **Where:** `lumos/src/pipeline/light_source.rs:393`, `pipeline/error.rs:39-40`
- **Category:** design
- **Impact:** low — a `DarkTemperatureMismatch` or `SampleDomainMismatch` on one light of hundreds is
  reported without its path, while `Load` and `CosmicRay` both carry one.
- **Confidence:** confirmed
- **Recommended direction:** `AlignStackError::Calibration { path, source }`.

## CAL-14 — Master-building policy is duplicated between lumos internals and lens

- **Where:** `lumos/src/calibration_masters/mod.rs:572-621` (`internals::masters_from_files`),
  `lens/src/astro/nodes/calibration.rs:127-240`, public `stack_cfa_master` (`mod.rs:98`)
- **Category:** design
- **Impact:** low-medium — the rule "stack each role under its preset, flats take flat-dark else bias
  per frame, then `from_images`" is written twice.
- **Confidence:** confirmed
- **Recommended direction:** A lumos-owned
  `CalibrationMasters::build(CalibrationSet<&[P]>, …)` (with an optional per-role cache hook for
  lens). It owns the subtractor choice and validation. `stack_cfa_master` becomes role-aware
  (`MasterRole::stack(paths, subtract, …)`) or private.

## CAL-15 — `DefectMap` is published with no external caller

- **Where:** `lumos/src/lib.rs:65`, `defect_map/mod.rs:94-271` (`pub fn new / detect_hot /
  detect_cold / correct / count / hot_indices / cold_indices`)
- **Category:** style
- **Impact:** low — visibility rule: escalate only for a real caller. lens and darkroom use only
  `defect_summary()`. `count()` has only test callers.
- **Confidence:** confirmed
- **Recommended direction:** Make `DefectMap` and its methods `pub(crate)`. Fix the one rustdoc
  link `crate::DefectMap` in `io/image/cfa/mod.rs:294`. Gate `count()` for tests or drop it.

## CAL-16 — Bayer green repairs mix the Gr and Gb phases

- **Where:** `lumos/src/io/image/cfa/cfa_lattice.rs:122-147` (`median`: "for a Bayer green … its
  four diagonal greens at √2")
- **Category:** precision
- **Impact:** low — the four nearest greens of a Gr site are Gb sites. On sensors with Gr/Gb
  imbalance, defect, null and CR in-paints on green carry the other phase's level, a small fixed
  pattern at repaired pixels.
- **Confidence:** likely
- **Recommended direction:** Either use same-phase greens only (distance 2), or keep the 8 and
  correct by the Gr/Gb ratio measured once. Decide by measuring Gr/Gb on the real-data set.

## CAL-17 — X-Trans detector takes a `&CfaType` and panics on others

- **Where:** `lumos/src/calibration_masters/cosmic_ray/xtrans.rs:159-170`
- **Category:** simplification
- **Impact:** low — the dispatch in `cosmic_ray/mod.rs:76-91` already matched X-Trans. Taking the
  `XTransPattern` makes the panic unrepresentable and drops the duplicate `cfa` field next to
  `lattice`.
- **Confidence:** confirmed
- **Recommended direction:** `XtransDetector::new(config, noise, pattern: XTransPattern)`. Do the
  same for `BayerDetector` (it needs only the phase-to-colour map).

## CAL-18 — Comment and doc nits in files under review

- **Where:** `fits.rs:40-41` (version history in a comment: "Version 2 stores…; version 3 keeps…"),
  `mono.rs:33` (over-long doc line mixing measurement narrative), `mod.rs:42-43` and
  `calibration_set.rs:7-8` (doc comment glued to the import block)
- **Category:** style
- **Impact:** low — narrates history instead of carrying the why, per the comment rules.
- **Confidence:** confirmed
- **Recommended direction:** Trim when those files are next edited.

## CAL-19 — No dark optimization or temperature scaling for unregulated sensors

- **Where:** `lumos/src/calibration_masters/mod.rs:444-475`
- **Category:** design (feature gap)
- **Impact:** low — a DSLR declares no temperature, so its dark is subtracted at scale 1 and the run is
  marked `unverified_temperature`. Siril offers noise-minimizing dark optimization
  (`preprocess.c:85-117,233-247`, golden-section on k ∈ [0, 2]). DSS offers entropy- and
  hot-pixel-based scaling (`DarkFrame.cpp:1316-1440`). The 1 °C tolerance also accepts up to about
  12% thermal error silently.
- **Confidence:** speculative (whether this is in scope is a product call)
- **Recommended direction:** If in scope, fit k per light by least squares on hot-pixel residuals.
  That is exact for a linear model and less sky-biased than noise minimization. Otherwise surface
  the 1 °C acceptance as a recorded scale or uncertainty.

## CAL-20 — CFA flats are normalized frame-to-frame by one median across colours

- **Where:** `lumos/src/combine/normalization/mod.rs:164-178` (`Multiplicative` on a 1-channel CFA
  frame), preset `combine/config/mod.rs:287-299`
- **Category:** precision
- **Impact:** low — twilight sky flats change colour during a session. One gain per frame leaves a
  per-colour, per-frame offset that σ-clip then reads as outliers at every pixel of that colour.
  Rejection of whole frames per colour raises noise but does not bias the shape. Siril behaves the
  same (no CFA-aware normalization in `stacking/normalization.c`).
- **Confidence:** speculative
- **Recommended direction:** For mosaic stacks, measure the multiplicative gain per CFA colour
  (three channels of one plane). The combine owner should weigh it.

---

## Checked and found OK

- **Operation order:** bias, then dark, then flat, then defect repair, then null repair, then CR
  (`mod.rs:423-437`, `light_source.rs:393-401`). Defects are repaired after the flat, so neighbour
  medians are in one response. CR runs on calibrated, flat-fielded data, as astroscrappy expects.
- **Dark scaling** only when the bias is separated (`DarkBias::Removed`). A bias-included dark of
  another exposure is refused (`DarkExposureMismatch`), the rule ccdproc documents for
  `subtract_dark(scale=True)`. Exposure tolerance is relative, at 1%.
- **Dark-flat handling:** flats are bias- or flat-dark-subtracted per frame before multiplicative
  normalization (the PixInsight/Siril practice). `from_images` recomputes each subtractor's map
  after each subtraction. A flat with an offset and no subtractor is refused, and so is a light with
  an offset, a flat and no subtractor.
- **Negative values** are preserved after subtraction, with no clamping (`cfa/mod.rs:434-437`).
- **Flat normalization** is per CFA colour (equivalent to Siril's `equalize_cfa`,
  `siril.c:453-513`, and DSS's per-colour means, `FlatFrame.cpp:119-241`). The mean is taken over
  measured photosites only, in f64, in deterministic row order. Normalizing over the whole frame
  instead of Siril's central third changes only the global scale.
- **Floor** at 0.1 of the colour mean, flagged `FLAT_FLOOR` and counted in the run report.
- **Cold-pixel detection** runs on the subtracted, unfloored flat against a local same-colour median
  at 0.5. That survives vignetting and dust, which a global cut cannot.
- **Hot-pixel candidate confirmation** uses a robust local plane through a ring
  (`ring_reference.rs`), with clipping before the LSQ fit and Cramer's rule in f64. Sample
  stratification (`sampling.rs`) is deterministic and CFA-phase aware.
- **Saturation** is recorded before samples move (`record_saturation`). A master's `SATURATED` or
  `NO_DATA` becomes the light's `NO_DATA` (`take_master_flags`). The flat mean excludes
  `UNMEASURED` pixels.
- **L.A.Cosmic (mono and Bayer phases)** matches astroscrappy: ×2 block subsample, clipped
  Laplacian, rebin, `S = L⁺/(2N)`, `S' = S − med₅(S)`, `F = med₃ − med₇(med₃)` clipped at
  0.01·noise, `S' > sigclip ∧ S'/F > objlim`, and two-stage growth (sigclip, then sigclip·sigfrac).
  Border replication is consistent across radii. Bayer deinterleaving keeps neighbours same-colour.
  The noise model `σ_local² + (m₅ − sky)/g` matches astroscrappy's `√(m₅/g + rn²)` with measured
  sky.
- **Master presets:** Winsorized 3σ with no normalization for bias, dark and flat-dark; σ-clip 3
  with multiplicative normalization and a median fallback below 8 for flats; quality planes off for
  masters. Subtracting the bias after stacking an un-normalized Winsorized dark equals subtracting
  it per frame.
- **Storage:** BITPIX −32 masters, per-HDU checksums verified before decode, dark bias state stored,
  flat stored prepared, defect map as an i64 table with bounds checks, dimensions re-validated on
  load.
- **Tests** mostly hand-compute exact dyadic values (`calibrate_divides_by_the_flat_less_its_own_subtractor`,
  `prepared_flat_matches_hand_computed_mono_calibration`, the bit-exact bundle round-trip, the
  thread-count independence of flat preparation).

## Suggested batches

1. **Fused per-light kernel (CAL-8, with CAL-11):** one pass for bias, dark, flat and flags.
   Incremental flag counts. Decide what `quantization_sigma` means after calibration.
2. **Defect policy (CAL-5, CAL-16):** redefine the hot-pixel threshold from what a defect promises;
   measure the flagged fraction and Gr/Gb on real data.
3. **Cosmic rays (CAL-6, CAL-7, CAL-9, CAL-12, CAL-17):** saturated-star protection and flag-aware
   in-paint, X-Trans threshold calibration with noise-only and star tests, local re-iteration, a
   flat-aware gain term, and typed detectors.
4. **API and layout (CAL-14, CAL-15, CAL-13, CAL-10, CAL-18):** a lumos-owned master builder,
   narrower visibility, path-carrying calibration errors, a typed bundle error with mmap load, and
   the comment trims.
5. **Optional or product calls (CAL-19, CAL-20).**
