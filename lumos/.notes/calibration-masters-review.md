# `stacking/calibration_masters` — correctness review against reference implementations

Scope: `lumos/src/stacking/calibration_masters/**` production code — master assembly (`mod.rs`),
flat preparation (`prepared_flat/`), defect detection and repair (`defect_map/`, `same_color.rs`),
cosmic-ray rejection (`cosmic_ray/`), and the FITS bundle (`fits.rs`). Test bodies were read only to
establish coverage.

References used: PixInsight `ImageCalibration` / `CosmeticCorrection` / master-frame tutorial, Siril
1.4 calibration docs, `ccdproc` reduction toolbox, van Dokkum (2001) L.A.Cosmic and `astroscrappy`.

Items are anchored to symbol names, not line numbers. **Delete an item once addressed** — this file
lists open findings only.

---

## Verdict

The per-pixel mathematics is sound and in several places better than the reference tools. The gaps
are at the *edges* of the module: what it refuses to accept, what it records, and one step-ordering
choice inside master-flat construction that every reference does the other way round.

Nothing here is a crash or a memory-safety problem. Findings 1 and 2 both produce a silently wrong
calibrated light — no error, no warning, plausible-looking output.

---

## 1. The flat's pedestal is subtracted after combination, not before each frame

`from_files` stacks the raw flats under `StackConfig::flat()` — `Normalization::Multiplicative`, then
σ-clipped mean — and only afterwards does `from_images` call `prepared_flat::subtract` to take off the
single master flat-dark/bias. Every reference does the opposite: calibrate each flat, *then* scale,
*then* integrate. PixInsight's master-frame tutorial states the reason directly — after bias/dark
subtraction the flats "are strictly composed of illumination data", which is what makes the
illumination levels matchable.

`Normalization::Multiplicative` (in `combine/normalization`) computes `gain = median(ref)/median(frame)`
with `offset = 0`. With frame `i` written as `Fᵢ(x) = kᵢ·S(x) + B(x)` — illumination level `kᵢ`, flat
shape `S`, pedestal `B` — and `gᵢ = uᵣ/uᵢ` where `uᵢ = kᵢ·m + b`:

    master  M(x) = S(x)·mean(gᵢkᵢ)  +  B(x)·mean(gᵢ)
    after subtract:  M(x) − B(x) = A·S(x) + (G−1)·B(x),   G = mean(gᵢ)

Two consequences:

- [ ] **A structured residual `(G−1)·B(x)` is left in the master flat.** `G ≥ 1` by Jensen, with
      `G − 1 ≈ CV²` for `CV` the coefficient of variation of the flat levels. It carries `B`'s
      *spatial* pattern — amp glow from a flat-dark, column FPN from a bias — so it is a structured
      multiplicative error in every calibrated light, of the kind stacking cannot average away. At
      `CV = 20 %` (sky flats) and a pedestal 5 % of the flat level it is ≈0.2 %; at constant flat
      level it is exactly zero, which is why a well-controlled panel-flat set shows nothing.
- [ ] **The σ-clip sees frames that are not actually level-matched.** `gᵢ` is derived from
      `illumination + pedestal`, so the scaled signal is `kᵢ·uᵣ/uᵢ ≠ kᵣ`. The residual spread is
      `≈ CV · b/(km+b)`, ~1 % under the numbers above, against flats whose own noise is ~0.1 % —
      so the rejection is cutting on frame-level offsets rather than on outliers.

Note the same argument does *not* apply to the dark and bias roles: they use `Normalization::None`.

## 2. Nothing checks that the masters and the light describe the same exposure

`CalibrationMasters::calibrate` → `validate_against_light` checks CFA pattern and pixel extent, and
nothing else. `ImageMetadata` already carries `exposure_time`, `ccd_temp`, `gain`, `egain` and `iso`,
and `io/image/fits/metadata.rs` reads and writes `EXPTIME` / `CCD-TEMP` — but a grep across `lumos/src`
production code finds no consumer of any of them outside metadata plumbing.

- [ ] A 300 s master dark applied to a 60 s light over-subtracts by 240 s of dark current, silently.
      A dark taken 15 °C warmer over-subtracts by roughly a factor of three. Both produce a
      plausible-looking, clipped-black frame with no diagnostic.
- [ ] `ccdproc.subtract_dark` refuses to guess: it requires `data_exposure`/`dark_exposure` (or the
      keyword) and an explicit `scale=`. Siril requires matched exposure *or* dark optimization.
      PixInsight WBPP groups frames by exposure, temperature, gain and binning before it will pair
      them. Lumos is the only one of the four that pairs unconditionally.
- [ ] The same applies *within* a role: `stack_cfa_master` will happily average a 60 s and a 300 s
      dark into one master.

Finding 3 is the reason this one is ranked where it is — with no scaling path, exposure agreement is
not an optimization, it is a precondition.

## 3. No dark scaling, and the bias is unused whenever a dark exists

`calibrate` applies `dark` *or*, failing that, `bias` — never `light − bias − k·(dark − bias)`. That is
correct arithmetic for a matched-exposure dark that still contains its own bias, which is what the
masters are (`StackConfig::dark()` stacks raw darks, nothing subtracts a bias from them).

- [ ] There is no `k`. Siril's dark optimization (golden-section search minimizing output noise) and
      PixInsight's "optimize master dark" both exist precisely for the mismatched case, and both
      require the bias to be subtracted from dark and light first. Lumos cannot express that
      decomposition, so a user with one dark library and several exposure lengths has no path.
- [ ] With no scaling *and* no check (finding 2), the mismatch is neither corrected nor reported.

## 4. Per-CFA-channel flat normalization is hard-coded, and the mono/CFA split keys off metadata

`prepared_flat::normalize` dispatches on `cfa_type.num_colors() == 3`: CFA flats are normalized to a
mean of one *per colour* (`normalize_cfa`), mono flats to a single global mean (`normalize_mono`).

The per-channel behaviour is right, and matches what PixInsight's "separate CFA flat scaling factors"
and Siril's `-equalize_cfa` do — it keeps a non-neutral flat illuminant from imposing a colour cast on
the calibrated lights. But:

- [ ] Both references make it an **option**; PixInsight ships it off by default and its own guide calls
      it something to experiment with. Lumos offers no way to get the single-factor behaviour, so a
      set calibrated here is not reproducible against either tool, and a user who *wants* the flat's
      illuminant divided out cannot ask for it.
- [ ] The mono/CFA choice is made from *metadata presence*, not from the sensor. A CFA master that
      lost its `cfa_type` gets whole-frame normalization — a different numerical result from the same
      pixels, with no warning. (`pattern_or_mono`'s "absent means mono" convention is deliberate and
      fine elsewhere; here it silently changes the calibration model.)
- [ ] `prepared_flat/tests.rs` covers mono, Bayer and X-Trans preparation bit-exactly, but nothing
      pins the *colour-balance* property that per-channel normalization exists for — that a
      non-neutral flat leaves the light's channel ratios unchanged.

## 5. The defect count is computed and never looked at

`DefectSummary` / `DefectMap::percentage` are public and have no consumer anywhere in `lumos/src` or
`lens`. Nothing in `pipeline/calibrate.rs` reads them, and nothing warns.

- [ ] A bad master dark, or `sigma_threshold` mis-set low (it is clamped only at
      `MIN_SIGMA_THRESHOLD = 1.0`, which on a clean dark flags ~16 % of the sensor), silently replaces
      that fraction of every light with same-colour neighbour medians. That is a resolution loss with
      no error, no log line, and an output that still looks like an image.
- [ ] PixInsight's CosmeticCorrection puts the count in front of the user with a real-time preview
      precisely because this is the parameter people get wrong. A single `tracing::warn!` above,
      say, 1 % would cost nothing — the number is already computed.

## 6. σ is the max of two estimators, on top of an already-conservative default

`compute_per_color_residual_stats` takes `sigma = max(mad_to_sigma(mad), tail_sigma, sigma_floor)`,
where `tail_sigma` is the 99th percentile of `|residual|` converted back to σ. Both terms estimate
the *same* quantity on clean data, so their max is upward-biased by roughly the sampling spread of
whichever is noisier — the effective cut sits above the nominal `sigma_threshold·σ`.

- [ ] Stacked with `DEFAULT_SIGMA_THRESHOLD = 5.0` against PixInsight's 3.0, that is two
      conservatisms in series. The module documents choosing 5.0 for fewer false positives; it does
      not document that σ itself is inflated. The population this misses is exactly the "warm"
      pixels the references say survive dark subtraction and are the reason cosmetic correction runs
      at all.
- [ ] The tail term's justification (keeping broad model error and column structure out of the
      defect tail) is real. Worth measuring which of the two actually binds on real masters — if it
      is always `tail_sigma`, the MAD is dead weight and the effective threshold should be restated.

## 7. `DarkBackground` extrapolates past its outermost tile centres

`interpolation_spans` clamps the *span index* to `[1, centers.len()-1]` but not the `fraction`, so for
a position beyond the last tile centre the fraction exceeds 1 and the bilinear model linearly
extrapolates. On a 6000-px axis with 64-px tiles the last pixel sits at `fraction ≈ 1.49` — half a
tile of extrapolation.

- [ ] That is the frame border, which is where amp glow is steepest and least linear. Overshoot
      raises the modelled background and hides real hot pixels there; undershoot flags cold ones.
      Clamping the fraction to `[0, 1]` (edge replication, the usual convention for this kind of
      coarse background model) removes the failure mode outright.

## 8. The saved bundle records no provenance

`fits::save` writes `LUMOSFMT` / `LUMOSVER` / `LUMROLE` / `LUMPREP` plus whatever `ImageMetadata` the
master inherited from its reference frame — so a master's `EXPTIME` and `CCD-TEMP` are one input
frame's, presented as the master's.

- [ ] No `NCOMBINE` (the FITS convention for how many frames went into a master), no combine method,
      no per-role frame count, no exposure/temperature *range* over the inputs. For the stated goal
      of a measurable science product this is the provenance that makes a master auditable — and it
      is also the information finding 2 needs in order to check anything.
- [ ] `calibrate` sets `metadata.calibrated = true` and records nothing about *which* masters were
      applied, so a calibrated light cannot be traced back to its bundle.

## 9. Structural: the cosmic-ray detector is not a calibration master

`cosmic_ray/` lives under `calibration_masters` but neither builds a master nor consumes one — it is
a per-light step, called from `pipeline/calibrate.rs` after `masters.calibrate()`. Its only tie to
this module is borrowing `same_color::XTransOffsets`.

- [ ] 900 production lines under a module whose name does not describe them. `stacking/cosmic_ray/`
      beside `calibration_masters/`, with `same_color` staying where it is, matches what it does.

---

## Verified correct

Checked against the references and found right — recorded so a later reviewer need not redo it.

- **Flat normalized by its mean**, not median. Matches PixInsight (`s0` is the mean of the master
  flat) and `ccdproc.flat_correct` (mean unless `norm_value` is given).
- **`MIN_NORMALIZED_FLAT = 0.1`** is `ccdproc`'s `min_value` by another name, applied after
  normalization so it reads as "10 % of this channel's mean". Bounds amplification at ×10.
- **Cold-pixel detection runs on the subtracted, un-normalized flat**, before the floor clamps
  near-zero photosites away. Ordering is correct and the comment says why.
- **Cold pixels are found against a same-colour *local* median, hot pixels against a smooth tiled
  background.** Both are better than the global cut the amateur tools use: the local reference tracks
  vignetting (a global `median − kσ` on a real flat goes negative and flags nothing), and the tile
  model keeps gradients and amp glow from becoming thousands of false point defects. The asymmetry is
  justified in the module docs.
- **Detection order in `calibrate`** — dark, then flat, then defects — matches PixInsight's
  ImageCalibration → CosmeticCorrection ordering.
- **Defect repair masks *all* defects before repairing any**, so a hot column or an adjacent
  same-colour pair cannot pull a bad neighbour into its own median, and `hot ⧺ cold` order does not
  change the result. The reference tools are weaker here.
- **Same-colour-only repair** (8-connected mono, stride-2 Bayer, per-phase X-Trans table) preserves
  the mosaic for the demosaic that follows.
- **`flat_dark` preferred over `bias` as the flat's subtractor** — the CMOS-era recommendation, and
  the CCD case still works.
- **Rejection presets**: Winsorized σ=3 for dark/bias at any N, σ-clipped mean σ=3 for flats with a
  median fallback below 8 frames. Consistent with `ccdproc.combine`'s 3σ default and with the usual
  advice not to run σ-clip statistics on a handful of smooth flats.
- **`MAD_TO_SIGMA = 1.4826022`** and the p99-to-σ constant `0.38822448 ≈ 1/2.5758` are both right.
- **Negative pixels are not clipped** after dark subtraction (`CfaImage::subtract` is a bare `-=`).
  Correct — clipping at zero biases the background estimate. Siril offers an output pedestal for
  users who need non-negative data; lumos does not, which is a choice, not an error.
- **Cosmic-ray rejection is a faithful L.A.Cosmic.** `L⁺` from the ×2-subsampled clipped Laplacian,
  `S = L⁺/(2N)`, `S' = S − median₅(S)`, `F = median₃ − median₇(median₃)`, the contrast test in
  astroscrappy's noise-normalized form with its `0.01` floor, `sigfrac` growth, `medmask`-equivalent
  in-painting, iterated. Defaults (4.5 / 5.0 / 0.3 / 4) match astroscrappy. The Bayer
  deinterleave and the X-Trans same-colour stencil are documented deviations with their weakened
  fine-structure test called out honestly, and `NoiseEstimation::Empirical` is flagged in its own doc
  as non-canonical.

## Coverage gaps in the tests

Not findings in themselves — the behaviours above that nothing currently pins.

- The colour-balance property of per-channel flat normalization (finding 4).
- Flat-integration order: no test constructs flats at differing illumination levels and checks the
  master against a per-frame-calibrated reference (finding 1).
- Master/light exposure disagreement — nothing to test, since nothing checks (finding 2).
- `DarkBackground` behaviour in the extrapolated border region (finding 7).

## Sources

- [PixInsight — Master Calibration Frames: Acquisition and Processing](https://www.pixinsight.com/tutorials/master-frames/)
- [Guide to PixInsight's ImageCalibration (Bernd Landmann)](https://sh-cosmiccanvas.s3.us-west-2.amazonaws.com/Resources/20200902_GuideToPIsImageCalibration.pdf)
- [When to check "Enable CFA" / "Separate CFA flat scaling factors" — PixInsight Forum](https://pixinsight.com/forum/index.php?threads/when-to-check-enable-cfa-separate-cfa-flat-and-dslr-cmos-calibration-needs.16397/)
- [PixInsight Cosmetic Correction to Remove Hot Pixels](https://chaoticnebula.com/cosmetic-correction/)
- [Siril 1.5 — Calibration](https://siril.readthedocs.io/en/latest/preprocessing/calibration.html)
- [Siril — Enough with dark flats](https://siril.org/2021/12/enough-with-dark-flats/)
- [ccdproc — Reduction toolbox](https://ccdproc.readthedocs.io/en/latest/reduction_toolbox.html)
- [ccdproc — `ccd_process`](https://ccdproc.readthedocs.io/en/latest/api/ccdproc.ccd_process.html)
- [Light Vortex Astronomy — Pre-processing in PixInsight](https://www.lightvortexastronomy.com/tutorial-pre-processing-calibrating-and-stacking-images-in-pixinsight.html)
