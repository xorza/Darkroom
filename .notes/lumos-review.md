# lumos review

> **When you address an item, delete it from this file.** Do not mark it done. The file lists open items only.

Scope: all production code in `lumos/src/`. Paths below are relative to `lumos/src/`. Line numbers are from commit `da6cbc3b0`.

Each item has a tag:
- `[C]` means the reviewer traced the code path or reproduced the arithmetic.
- `[P]` means the mechanism is real, but the size of the effect depends on the data.

References give the established practice that the item compares against.

Groups are sorted by severity × benefit. Correctness comes first, then precision, then performance, then design.

---

## 1. Rejection is not robust: outliers survive the default combine

Sigma clip is the default and the light preset. Winsorized is the bias and dark preset.

- [ ] **The sigma-clip shortcut skips rejection on low-noise data** — `combine/rejection/sigma_clip_config.rs:179`
  - `variance < f32::EPSILON` compares σ² with 1.19e-7. Any pixel with a trimmed σ below 3.45e-4 of full scale (≈22 ADU at 16 bit) skips rejection.
  - The comment says this "matches" the full path. It does not: the full path tests σ, not σ².
  - Example: 20 bias frames, noise 1e-4, one cosmic ray at 0.5. The full path keeps 18 samples. The shortcut keeps all 20, so the mean moves by 240× the per-frame noise.
  - The shortcut runs again inside the loop (`:103`). `[C]`
- [ ] **In the shortcut, two outliers hide each other** — `combine/rejection/sigma_clip_config.rs:146-189`
  - The shortcut trims one min and one max, then uses a non-robust stdev.
  - Example: {−0.1, −0.05, 0, 0, 0, 0, 0.05, 0.1, 10, 10} at k = 2.5.
    - The shortcut computes mean 1.26 and σ 3.53. The largest deviation, 8.74, is below 8.83, so it rejects nothing.
    - The MAD path computes σ = 0.074 and rejects both 10s.
  - A shortcut that changes the result is a bug. Its bound must follow from the MAD path, or the shortcut must go. `[C]`
- [ ] **Winsorized clipping starts from a non-robust σ** — `combine/rejection/winsorized_clip_config.rs:78`
  - σ₀ = 1.134·stdev of all samples. With 3 outliers at +10σ in 10 frames, the Huber iteration keeps all three inside the band.
  - Simulation (500 trials, k = 3), mean rejected per pixel:

    | Frames / outliers | Current start | MAD start |
    |---|---|---|
    | 10 / 3 | 0.006 | 2.70 |
    | 12 / 4 | 0.0 | 3.17 |
    | 20 / 6 | 1.45 | 5.86 |

  - PixInsight PCL `WinsorizedSigmaClippingRejection` starts from 1.1926·Sn, a robust estimator. `[C]`
- [ ] **Winsorized has no outer loop, no survivor floor, and no Huber location step** — `combine/rejection/winsorized_clip_config.rs:115-136`
  - It rejects once and returns. Siril loops `while changed && N > 3`. PixInsight loops until stable and keeps ≥ 3.
  - The centre is always the plain median. PCL re-takes the mean of the winsorized values.
  - The doc at `:12` ("matching PixInsight/Siril") is false. `[C]`
- [ ] **Linear-fit clipping rejects clean data** — `combine/rejection/linear_fit_clip_config.rs:67-140`
  - Pass 0 clips at k·1.4826·MAD. Later passes clip at k × the mean absolute residual of a line through the sorted order statistics, which is a much smaller unit.
  - Pure Gaussian data at k = 3, fraction rejected:

    | Frames | Linear fit | Sigma clip |
    |---|---|---|
    | 20 | 4.4% | 1.9% |
    | 50 | 6.3% | 0.8% |
    | 200 | 7.8% | 0.4% |

  - The method has no survivor floor (Siril: N − r ≤ 4).
  - With `max_iterations = 1` it never fits a line, so it is plain sigma clip.
  - "Matches PixInsight/Siril" and "relationship to a reference value" (`:12`, `:51`) are false: the x-axis is the sorted rank. `[C]`
- [ ] **The GESD automatic `max_outliers` cap is 2 below 25 frames and 10 above** — `combine/rejection/gesd_config.rs:50-51`
  - GESD resists masking only when r ≥ the true outlier count.
  - Example: n = 20, three outliers at 10σ, r = 2. It rejects 2 and keeps 1, so the mean moves by 0.55σ.
  - Siril and PixInsight use 0.3·N. `[P]`
- [ ] **Percentile clipping does nothing on the small stacks it is documented for** — `combine/rejection/percentile_clip_config.rs:12,71`
  - ⌊0.1·n⌋ = 0 for n < 10, and the preset turns off the small-N fallback.
  - The name clashes with Siril's percentile clipping (deviation from the median as a fraction of the median). This method is a trimmed mean.
  - `(p/100)·n` in f32 gives 62 for 42% of 150. `(p·n)/100` is exact. `[C]`
- [ ] **Survivors can drop to zero, and the pixel is written as 0 but reported as covered** — `combine/rejection/mod.rs:263-272`
  - Validation accepts any k > 0. There is no minimum-survivor rule (Siril 4, PixInsight 3). `[C]`
- [ ] **The doc says winsorized "replaces outliers", but it only clips** — `combine/rejection/mod.rs:155`, `combine/stack/quantization.rs:7`, `combine/stack/mod.rs:366-369`
  - From this false premise, winsorized gets the `conservative` quantization σ (the worst single frame) instead of survivor tracking. That overstates the figure by ≈√N. `[C]`

## 2. Absolute `EPSILON` floors on quantities that scale with the data

`math/statistics/mod.rs:320-327` already explains why such a floor is wrong and uses `σ <= |median|·EPS`. No other module uses that form.

**Failure scenario:** 16-bit data in a 32-bit integer FITS normalizes by 2³²−1, so σ ≈ 2e-9. Every floor below then trips.

- [ ] **Rejection degeneracy checks** — `combine/rejection/sigma_clip_config.rs:110`, `winsorized_clip_config.rs:80,97,122`, `linear_fit_clip_config.rs:75,121`
  - `sigma < f32::EPSILON`: no rejection happens on such data. `[C]`
- [ ] **Weighting and normalization**
  - `combine/stack/mod.rs:253`: σ below EPS gives weight 0, and if all frames have weight 0 the stack silently falls back to equal weights.
  - `combine/normalization/photometric_gain.rs` (`deming_gain`, `Seed::of`, `ResidualWindow::at`): the gain silently becomes 1, or the fit is skipped. `[P]`
- [ ] **Star SNR variance floor** — `star_detection/centroid/mod.rs:570`
  - The floor `f32::EPSILON` makes σ_total ≥ 3.45e-4. Every star's SNR is 10³–10⁴× too low, so every star fails `min_snr`.
  - On 16-bit frames, sky σ below ≈2.5 ADU is also understated. `[C]`
- [ ] **Fit weight floor 1e-12** — `star_detection/centroid/stamp.rs:84`
  - On such data every weight is equal, so the "weighted" fit runs unweighted. `[C]`
- [ ] **LM singular pivot 1e-15** — `star_detection/centroid/lm_optimizer.rs:13`, `:163-167`
  - The Hessian scales as A². A sound fit at A ≈ 1e-6 reads as singular and falls back to the biased seed (group 10).
  - Solving the Marquardt-scaled system (unit diagonal) makes the threshold scale-free. `[P]`
- [ ] **Amplitude seed floored at 0.01 normalized** — `star_detection/centroid/stamp.rs:228-237`
  - 0.01 is 655 ADU at 16 bit. The fit costs about one extra LM iteration. `[C]`
- [ ] **Detection channel weights** — `star_detection/detector/stages/prepare/mod.rs:75`
  - `sigma > f32::EPSILON` sets the weight of every channel to 0, then falls back to equal weights. `[C]`
- [ ] **Two sigma-clip implementations with different floors** — `math/statistics::sigma_clip_iteration` and `SigmaClipConfig::reject`
  - One source of truth (one relative-floor helper) prevents this whole group. `[P]`

## 3. The default registration prior rejects real sessions

- [ ] **Default `max_rotation` = 10° drops every frame after a meridian flip** — `registration/ransac/config.rs:35`, `registration/ransac/mod.rs:113-127`, `pipeline/config.rs:24`
  - A German-equatorial flip is a 180° rotation. Triangle matching survives it, but every RANSAC hypothesis fails `is_plausible`. The pipeline then drops the frame as "registration failed".
  - Alt-az mounts (smart telescopes) pass 10° of field rotation in one session.
  - Siril, PixInsight and DSS register across a flip by default. `[C]`

## 4. Calibration does not reconcile offsets and exposures

- [ ] **Flats are scaled by a median ratio before the bias or flat-dark is subtracted** — `calibration_masters/mod.rs:68-92` with `combine/config/mod.rs:285-297` (`Normalization::Multiplicative`), subtraction at `calibration_masters/mod.rs:174-185`
  - Each raw flat is scaled while it still contains its offset. The subtraction runs once, on the finished master. The residual is b·(mean gain − 1).
  - Example: offset 0.02, flats at 0.5·f and 0.25·f. The corner/centre ratio is 0.509 instead of 0.500, a ≈2% vignetting residual. It also skews rejection between frames.
  - RAW frames lose the black level at decode, so on RAW only dark current leaks. FITS cameras keep `OFFSET` in the data.
  - PixInsight WBPP and Siril calibrate each flat before they integrate it multiplicatively. `[C]`
- [ ] **A flat or a light with no additive subtractor is accepted silently** — `calibration_masters/mod.rs:174-185` (`_ => None`), `:263-279`
  - A light with a flat but no dark or bias gives (S + b)/flat, which adds an inverse-vignetting pattern. `[C]`
- [ ] **Darks are never matched to the light** — `calibration_masters/mod.rs:263-274`, `validate_against_light` at `:295-350`
  - Only the pattern, size and sample domain are checked. `exposure_time`, `ccd_temp` and `iso` are never read.
  - A 120 s dark on 300 s lights removes 40% of the thermal signal and amp glow, and calibration still reports success.
  - There is no dark scaling or optimization (PixInsight "Optimize", Siril `-opt`). Scaling also needs a bias-free dark, but the masters store the dark with its bias.
  - The RAW CFA loader does not record exposure (`io/raw/mod.rs:1131-1148`), so a check has nothing to compare. `[C]`
- [ ] **`SampleDomain` models scale only, not the zero point** — `io/image/sample_domain.rs:52-61`, `io/image/image_provenance.rs:176-184`
  - RAW is (v − black)/span. Integer FITS is v/65535 with the pedestal kept. Both have unit None, so `conversion_to` calls them "exactly convertible".
  - A master dark from Siril-converted FITS (black kept), applied to RAW lights, subtracts the black a second time. Every light goes negative by ≈black/span (≈0.13), with no error. `[C]` mechanism, `[P]` frequency.
- [ ] **The flat floor `MIN_NORMALIZED_FLAT = 0.1` is applied without a report** — `calibration_masters/prepared_flat/mod.rs:16,69,114`
  - It is applied with no count and no warning. Deep smooth vignetting is under-corrected. `[C]`

## 5. Sample scale and quantization σ do not survive I/O

- [ ] **A reloaded `QNTZSIG` is divided by `LUMSCALE` a second time** — `io/image/fits/decode/mod.rs:94-107`, writer `io/image/fits/metadata/mod.rs:99-106,161-166`
  - The writer stores the normalized σ. On reload, `physical_scale = LUMSCALE` (`decode/pixels.rs:111`), and σ is divided by it.
  - A RAW-derived frame (span 15360) comes back with σ 15360× too small. This affects every reloaded master and every saved CFA frame.
  - Downstream effects:
    - the cosmic-ray `full_scale` (`calibration_masters/cosmic_ray/noise_model.rs:32`)
    - the defect floor (`calibration_masters/defect_map/mod.rs:308`)
    - combine σ (`combine/stack/quantization.rs`)
  - The round-trip tests write no provenance, so they miss it.
  - Derive both σ figures from `sample_scale.divisor`. For integer HDUs the step is `QSPS/(2^bits−1)`, so no f32 BSCALE detour is needed. `[C]`
- [ ] **An assumed float scale does not round-trip, so a saved master is refused later** — `io/image/fits/metadata/mod.rs:99-106`, `io/image/fits/decode/plan.rs:155-164`
  - A float ADU frame with DATAMAX 65535 decodes as divisor 65535 (Assumed). It is saved with DATAMAX 1.0 and no `LUMSCALE`, and it reloads as divisor 1.
  - `master_scale` then fails with `SampleDomainMismatch`. The writer comment ("the reader can make the guess again") is false. `[C]`
- [ ] **A float FITS in ADU with no DATAMAX loads as-is** — `io/image/fits/decode/plan.rs:155-164`, consumer `star_detection/detector/mod.rs:114`
  - The saturation level becomes 0.95 ADU, so almost every star is flagged as saturated.
  - The normalize pass already reads every sample. Fail when max ≫ 1 and no scale is declared.
  - The doc calls the DATAMAX>10 rule "Siril's", but Siril applies it to a scanned maximum. `[C]` path, `[P]` frequency.
- [ ] **The ADC step comes from the container, not the ADC** — `io/image/fits/decode/mod.rs:102-107`
  - 12/14-bit data left-justified in BITPIX 16 gets σ 16× or 4× too small. The trailing-zero count of the OR of all samples gives the real step. `[P]`
- [ ] **RAW quantization assumes 1-ADU steps** — `io/raw/mod.rs:1154`
  - Curve-compressed RAWs (Sony cRAW, Nikon lossy NEF, Canon C-RAW) have steps of 2–8+ ADU at highlights. Derive the step from `color.curve`, or report `None`. `[P]`
- [ ] **Quantization tracking uses channel 0's gain for every channel** — `combine/stack/quantization.rs:52,66` `[C]`

## 6. Noise is estimated from whole-frame spread, which includes signal

- [ ] **Combine noise weights use whole-frame MAD** — `combine/stack/mod.rs:236-258`, `combine/frame_stats.rs:34`
  - On a signal-dominated field, gain·MAD is the same for every frame, so Noise weighting degenerates to equal weights.
  - Gradients make the MAD larger, so frames with gradients are underweighted.
  - PixInsight weights by MRS noise (Starck & Murtagh, first wavelet layer). The same MAD also feeds reference selection and the Deming noise ratio. `[C]` mechanism.
- [ ] **One weight per frame, averaged over RGB** — `combine/stack/mod.rs:241-252`
  - A frame with a bad blue channel is over-trusted in blue. PixInsight weights each channel separately. `[C]`
- [ ] **Reference normalization compares raw `average_mad` across sample domains** — `combine/normalization/mod.rs:190-205` `[C]`
- [ ] **Detection RGB weights use whole-frame MAD** — `star_detection/detector/stages/prepare/mod.rs:66-91`
  - A red nebula down-weights R. Each frame copies 3 planes and runs 2 quickselects per plane.
  - Use MAD of first differences ÷ √2 on a strided sample. `[C]` mechanism.
- [ ] **Cosmic-ray empirical noise is one whole-frame (or whole-colour) median/MAD** — `calibration_masters/cosmic_ray/mono.rs:232-238`, `calibration_masters/cosmic_ray/xtrans.rs:244-271`
  - Gradients make N larger, so faint hits are missed. astroscrappy uses a per-pixel `sqrt(m5 + rn² + bkg)`. `[P]`
- [ ] **The parametric cosmic-ray noise model drops the subtracted dark/sky level** — `calibration_masters/cosmic_ray/mono.rs:281-295`
  - N is understated on warm long exposures, so too many pixels are flagged. astroscrappy adds `bkg`/`pssl`. `[P]`
- [ ] **Denoise σ at coarse scales comes from the scale's own coefficients** — `image_ops/denoise/mod.rs:330,351-362`
  - Nebula structure raises σ_j, so faint filaments are removed.
  - Use the multiresolution support (iterate on non-significant coefficients), or σ_I·σ_j^e with the B3 constants 0.889, 0.200, 0.086, 0.041, 0.020. `[P]`
- [ ] **Denoise uses one global σ per scale and ignores the stack's coverage/variance planes** — `image_ops/denoise/mod.rs:330-343`
  - Low-coverage edges stay noisy. `[P]`

## 7. The variance quality plane is not a variance

AGENTS.md promises photometry-grade error bars. These planes cannot give them.

- [ ] **`linear_variance` = Σw²/(Σw)² holds no per-frame σ or gain** — `combine/cache/sample.rs:46-72`, documented at `stack_product/mod.rs:51-55`
  - The variance of a weighted mean is Σwᵢ²gᵢ²σᵢ²/(Σw)². The combine already has σᵢ, gᵢ and q.
  - Example: equal weights, σ 1 and 2. The plane gives 0.5, but the truth is 1.25. `[C]`
- [ ] **Noise and Manual weights are normalized to sum to 1** — `combine/stack/mod.rs:38-42`
  - So the weight plane cannot serve as inverse variance either. `[C]`
- [ ] **Drizzle gives gated pixels `fill_value` with non-zero variance, weight and coverage** — `drizzle/accumulator/mod.rs:299-319` `[P]`
- [ ] **Drizzle Lanczos marks coverage for exactly-zero-weight deposits** — `drizzle/accumulator/output_band.rs:445-449` → `:513-519`
  - Coverage reaches 3 px past the footprint, against `accumulator/mod.rs:168-170`. `[C]`

## 8. Missing-data masks are dropped after decode

- [ ] **Drizzle never reads `LinearImage::nulls`** — `drizzle/accumulator/frame_source.rs:384-390` (`deposit_weight`), `drizzle/accumulator/mod.rs:175-181`
  - The decoder fills null pixels with the frame median. Drizzle deposits those values at full weight and counts them as coverage.
  - NaN borders and `BLANK` pixels pull the output toward the sky level.
  - `deposit_weight` should return `None` for a null pixel. `[C]`
- [ ] **Star detection never reads `nulls`** — `star_detection/` (no reference)
  - A null region becomes a flat patch with zero noise. A wholly-null tile gets σ = 0, so the threshold falls to `σ·noise_floor`, and the pixels next to the gap merge into one huge component.
  - Pass `nulls` as the mesh mask, clear them from the threshold mask, and handle them in stamps. `[C]` path, `[P]` magnitude.
- [ ] **RAW `zero_is_bad` is ignored** — `io/raw/mod.rs:1110-1112`, `:1155-1157`
  - LibRaw sets the flag for Panasonic and for some table cameras. lumos normalizes such a 0 to −black/span and declares `may_carry_nulls: false`.
  - Map zeros into a `NullMask`. This is needed before RW2 is accepted (group 17). `[C]`

## 9. Star detection measures and splits on the wrong plane

- [ ] **On demosaiced frames, flux, peak, FWHM and centroid are measured on the 3×3-median plane** — `star_detection/detector/stages/prepare/mod.rs:38-44`, `star_detection/detector/mod.rs:161,209-216`
  - The comment in `prepare` itself says filtering blurs the PSF that flux and FWHM are read from.
  - Effect on a Gaussian:
    - FWHM 2: 56% of flux and 42% of peak are kept, and the FWHM reads 2.23.
    - FWHM 3: 79% of flux is kept.
  - The centroid shows pixel locking of up to ±0.008 px.
  - Use the median plane only for the threshold mask, and measure on the unfiltered plane. `[C]`
- [ ] **The matched-filter threshold assumes white noise, but the median plane is strongly correlated** — `star_detection/convolution/mod.rs:39-81`
  - The true filtered σ is 2.18–2.63× the σ in the map (filter FWHM 2.5–6). A "4σ" threshold is then ≈1.6σ, so ≈5% of sky pixels pass it on OSC frames.
  - Measure the noise of the filtered plane itself. That is exact under any correlation (demosaic, resampling). `[C]`
- [ ] **Saturation is flagged on the channel-combined median plane** — `star_detection/detector/mod.rs:182-184`, `star_detection/detector/stages/prepare/mod.rs:94-104`
  - A star clipped in G only, (0.6, 1.0, 0.6), combines to 0.8, which is below 0.95.
  - The median also pulls a small clipped core below the level.
  - Flag saturation on the input channels before `prepare`. `[C]`
- [ ] **Both deblenders split on the unfiltered residual, in units that do not match detection** — `star_detection/detector/stages/detect/mod.rs:59-103,182-205`, `star_detection/deblend/component.rs:394-410`, `star_detection/deblend/local_maxima/mod.rs:68-75`
  - The footprint is cut at σ in filtered-SNR units. The multi-threshold floor is in residual units (≈3× lower at FWHM 4).
  - Level 0 (`multi_threshold/mod.rs:512`) breaks the footprint into noise islands, and those islands become children.
  - LocalMaxima at prominence 0.3 splits about half of the faint stars.
  - SEP thresholds the filtered value (`cdvalue`), and photutils deblends the convolved data. `[C]` mechanism, `[P]` frequency.
- [ ] **Sub-threshold branches have no minimum area** — `star_detection/deblend/multi_threshold/mod.rs:528-534,601-641`
  - A 1-pixel region is a valid node. SEP calls `lutz(..., minarea)` inside the deblend loop. `[C]`
- [ ] **The multi-threshold significance walk runs top-down and drops deeper splits** — `star_detection/deblend/multi_threshold/mod.rs:665-685`
  - Example: root(100) → [A(80) → [A1(35), A2(30)], B(5)], contrast 0.2. lumos returns 1 object. SEP returns {A1, A2}.
  - SEP `deblend.c` walks from the bottom up and propagates `ok[]`. `[C]`
- [ ] **Branch flux includes the pedestal below the split level** — `star_detection/deblend/multi_threshold/mod.rs:504-508,614`
  - SExtractor/SEP test `fdflux − thresh·fdnpix > mincont·root`. Bumps on bright wings carry level × npix of host light, which over-splits the wings.
  - The docs say "flux above that threshold". `[C]`
- [ ] **The refinement mask uses the detection σ (4) on the unfiltered plane, with square dilation** — `star_detection/background/background_estimate.rs:251-262`, `star_detection/mask_dilation/mod.rs`
  - Faint wings stay unmasked and bias the sky upward. photutils uses ≈2σ on convolved data and a circular footprint. `[P]`

## 10. Shape metrics measure position, not shape

- [ ] **SROUND measures sub-pixel phase** — `star_detection/roundness/mod.rs:42-46`, fed by `star_detection/centroid/mod.rs:454-545`
  - It is the marginal asymmetry about the stamp's centre pixel (`pos.round()`), so a star off the pixel centre is "lopsided".
  - Round stars rejected as `NotRound` (uniform phase):

    | FWHM | Default threshold 0.5 | Threshold 0.3 |
    |---|---|---|
    | 2 | 57% | 85% |
    | 2.5 | 21% | 73% |
    | 3 | 3% | 57% |

  - Auto-FWHM goes through `Rejection::of`, so it is biased upward.
  - Use the DAOFIND/photutils `roundness1` pinwheel quadrant sum: translation cancels to first order, and it catches 45° elongation. Nothing catches diagonal elongation today. `[C]`
- [ ] **GROUND is not DAOFIND ROUND** — `star_detection/roundness/mod.rs`
  - It uses the max-sample marginals and no factor 2, so the scale is half of DAOFIND's and it depends on phase. DAOFIND fits 1-D Gaussians to the marginals.
  - The docs claim "the DAOFIND roundness metrics". `[C]`
- [ ] **Sharpness divides the whole stamp's peak by the 3×3 core flux** — `star_detection/centroid/mod.rs:471,530-534`
  - The fainter star of a pair 5 px apart gets ratio 1.0 > 0.7, so it is rejected as `CosmicRay`.
  - The `star.rs:275-285` docs give two different thresholds (0.8 and 0.7). Neither matches DAOFIND's sharpness. `[C]`
- [ ] **`max_fwhm_deviation` is documented as "MAD-scaled" but multiplies the raw MAD** — `star_detection/config/filter_config.rs:16`, `star_detection/detector/stages/filter/mod.rs:119-120`
  - 3 raw MAD ≈ 2.0σ. `[C]`

## 11. Centroids stop before they converge

- [ ] **The default `WeightedMoments` stops after 10 plain fixed-point steps** — `star_detection/centroid/mod.rs:84,299-316,397`
  - The error contracts by c = σ_s²/(σ_s²+σ_w²) per step (0.61 at the matched window). Reaching 1e-4 needs ≈15 steps.
  - Bias at a 0.4 px start offset:

    | Star FWHM (window 3) | Bias after 10 steps | Converged |
    |---|---|---|
    | 3 | −0.0035 px | −0.0002 px |
    | 4.5 | −0.034 px | — |
    | 6 | −0.092 px | — |

  - Wide stars in field corners are hit hardest.
  - SExtractor XWIN uses `x += 2·Σ…` with σ_w = σ_s, which is exact in one step for a Gaussian. The adaptive-moments Newton step scales the shift by σ_w²/(σ_w² − C_obs).
  - A step-size stop also understates the remaining error by c/(1−c). `[C]`
- [ ] **A rejected or non-converged fit falls back to the 2-step moments seed** — `star_detection/centroid/mod.rs:179-185,236-253`
  - Bias −0.155 px at a 0.4 px offset, up to −0.30 px for wide stars.
  - This happens for undersampled stars, saturated stars, close pairs, and LM giving up. `[C]`
- [ ] **`LocalAnnulus`: the moments run before the local sky is measured and never subtract it** — `star_detection/centroid/mod.rs:185` vs `:209`
  - A pedestal of 0.2× peak raises the bias from −0.0035 to −0.0146 px. `[C]`
- [ ] **`max(0)` clipping adds a rectified-noise pedestal to the moments** — `star_detection/centroid/mod.rs:381`
  - Contraction slows. SExtractor uses signed values. `[P]`
- [ ] **The PSF is evaluated at pixel centres, not integrated over the pixel** — `star_detection/centroid/gaussian_fit/mod.rs:271-277`, `star_detection/centroid/moffat_fit/mod.rs:156-161`, `star_detection/centroid/covariance.rs`
  - FWHM bias: +16% at FWHM 1.2, +5.9% at 2, +2.6% at 3. `MIN_SIGMA` allows fits down to FWHM 1.18, where this dominates.
  - For a Gaussian, an erf-integrated model is exact. `[P]`
- [ ] **A fit may move up to the full `stamp_radius` and is never re-stamped** — `star_detection/centroid/mod.rs:139-141` vs `:400` `[P]`
- [ ] **The annulus starts at the stamp radius, inside the Moffat wings** — `star_detection/centroid/mod.rs:199-207,292-294`
  - It removes ≈4% of the flux at β 2.5 and FWHM 3. `[P]`
- [ ] **LM convergence uses only absolute step and Δχ² tests** — `star_detection/centroid/lm_optimizer.rs:186-202`
  - A heavily damped step at large λ meets both tests. A rejected step can set `converged`.
  - Madsen–Nielsen–Tingleff use ‖g‖∞ ≤ ε₁ and ‖δ‖ ≤ ε₂(‖x‖+ε₂). `[P]`
- [ ] **The IRLS second pass runs after a failed first fit** — `star_detection/centroid/stamp.rs:217-224` `[C]`
- [ ] **A NaN in the residual reaches `Star.flux` and `snr`** — `star_detection/centroid/mod.rs:466-471`
  - A NaN SNR passes `snr < min_snr`, and `validate_catalog` checks only `pos` and `fwhm`. `[P]`

## 12. SNR follows neither the CCD equation nor the measured noise

- [ ] **Read noise is counted twice** — `star_detection/config/measurement_config.rs:87-102`, used at `star_detection/centroid/mod.rs:562` and `star_detection/centroid/stamp.rs:79-85`
  - The empirical background σ already holds RN². Merline & Howell add RN² to the sky shot term only. `[C]`
- [ ] **The sky-estimate error term n_pix(1 + n_pix/n_B) is missing** — `star_detection/centroid/mod.rs:557-571`
  - Sky-limited SNR reads up to 1.48× high in `LocalAnnulus` mode. `[C]`
- [ ] **Measurement ignores `SkyNoise::floor`, which the threshold applies** — `star_detection/centroid/mod.rs:191,492,522` `[C]`
- [ ] **There is no per-star positional uncertainty** — `star_detection/centroid/lm_optimizer.rs:211-216`, `star_detection/star.rs`
  - (JᵀWJ)⁻¹·χ²/(n−p) is one solve on a Hessian that already exists. Registration needs it for weights (group 13). `[C]` absent.

## 13. The final registration fit discards precision

- [ ] **Match recovery searches only the brightest `max_stars` (200)** — `registration/mod.rs:144-155,355-365`
  - The final fit never sees the rest of the catalog. With 2000 stars this costs ≈3× in precision. Triangle matching needs the cap, but the final fit does not. `[C]`
- [ ] **The recovery and inlier gates are ≈1.5 FWHM wide, and the fit inside them is plain least squares** — `registration/tuning/mod.rs:33,44`, `registration/recovery.rs:80,93,132`, `registration/ransac/mod.rs:320-327`
  - The gate is 30–100× the centroid noise. Nothing tightens it or clips residuals, so one blend 4 px off pulls the fit by r/N.
  - Use MAGSAC++ σ-consensus IRLS, or shrinking LO thresholds. `[C]`
- [ ] **One mismatch can push the Auto ladder to an overfit model** — `registration/mod.rs:283,410-417`, `registration/result/mod.rs:215-226`
  - The RMS is non-robust: one 4 px mismatch among 50 true matches gives 0.57 px > 0.5, so the ladder escalates to Affine or Homography.
  - Points that the SIP fit clipped also stay in the RMS. `[C]`
- [ ] **There is no weighting by centroid σ, and saturated stars are used** — `registration/mod.rs:144-153`, `registration/ransac/transforms.rs`
  - Brightest-first selection prefers saturated stars, which have flat tops. The weight should be 1/σ² with σ ≈ FWHM/(2.355·SNR).
  - PixInsight excludes stars above its upper limit. `[P]`
- [ ] **The homography is refined only by algebraic DLT** — `registration/ransac/transforms.rs:257-310`
  - OpenCV and Hartley–Zisserman refine with LM on reprojection error. `[C]`
- [ ] **SIP is fitted after the linear transform is frozen** — `registration/distortion/sip/mod.rs:153-263`, `registration/mod.rs:377-402`
  - The monomials are not orthogonal to the affine terms, so this is one Gauss–Seidel step, not the joint optimum.
  - The result also depends on the per-frame default reference point (`sip/mod.rs:180-183`).
  - astrometry.net `fit_sip_wcs` fits both together. `[C]`
- [ ] **LO-RANSAC can commit a refit whose score was cut short** — `registration/ransac/mod.rs:185-192,200`
  - It accepts a refit on inlier count when the score dropped. The preemptive scorer then exits early, so the inlier list is a prefix and the score is overstated.
  - Accept on score only (Chum 2003). `[C]`
- [ ] **RANSAC degeneracy tests depend on scale and miss some cases** — `registration/ransac/mod.rs:75,448-483`
  - The collinearity test uses an absolute cross product of 1 px². It tests only triplets that include p₀.
  - OpenCV `checkSubset` tests every triplet and rejects orientation flips. `[P]`
- [ ] **Triangle flatness uses absolute thresholds, and three tests overlap** — `registration/triangle/geometry.rs:14,70-72,85-89,108-110`
  - One relative test (area/longest² ≥ c) replaces all three. `[C]`
- [ ] **The vertex order of near-isosceles triangles is unstable** — `registration/triangle/geometry.rs:55-60` `[P]`

## 14. Resampling: ringing, aliasing, unstable normalization

- [ ] **Lanczos has no ringing clamp** — `registration/resample/row/simd/mod.rs:66-89`
  - Undershoot is up to ≈13% of peak around bright or undersampled stars.
  - PixInsight StarAlignment clamps at 0.3 by default. Siril clamps by default. `[C]`
- [ ] **No prefilter when the warp downsamples** — `registration/resample/kernel/mod.rs`, `registration/resample/row/simd/mod.rs`
  - The `mosaic` preset allows scale 0.5–2. Stretch the kernel by the scale factor, or switch the kernel. `[C]`
- [ ] **Masked frames divide by a signed kernel sum** — `registration/resample/masked_warp.rs:94-125`
  - Two adjacent nulls at half-pixel phase leave a sum of 0.25, which amplifies noise. Normalized convolution needs a non-negative applicability.
  - The border path already falls back to bilinear. Do the same here. `[P]`
- [ ] **A bilinear band within `a` px of the source edge gives a visible sharpness step** — `registration/resample/row/simd/mod.rs:72-74` `[C]`

## 15. Spill and memory planning disagree with the machine

- [ ] **The default spill directory is on tmpfs** — `combine/cache_config.rs:27` (`env::temp_dir()`)
  - On this host `/tmp` is a 13.6 GB tmpfs. Debian 13 and Arch use tmpfs by default.
  - Spill files then use RAM, fill the tmpfs (ENOSPC), and evict the memory the plan counted on.
  - Default to a disk-backed directory (`$XDG_CACHE_HOME`, `/var/tmp`), or refuse tmpfs after a `statfs` check. `[C]`
- [ ] **The memory reading ignores cgroup limits** — `memory/mod.rs:20-43`
  - sysinfo `available_memory()` is the host `MemAvailable`. The GitHub runner containers (14g/8g, no swap) get OOM-killed instead of spilling.
  - Take min(available, `cgroup_limits().free_memory`). `[C]`
- [ ] **`align_and_stack` counts its input frames twice** — `pipeline/align.rs:157-178`
  - `RunMemory::read` runs after the inputs are allocated, and the plan charges them again. 30 RGB 24 MP frames spill ≈14 GB that would fit. `[C]`
- [ ] **On the spill tier, warp buffers stay allocated through the whole combine** — `pipeline/align.rs:311,400`
  - ≈3.8 GB for 8 workers on RGB 24 MP. The combine chunk sizing does not know about it. Drop them before the combine. `[C]`
- [ ] **Spilled calibrated frames are never deleted after read-back** — `pipeline/tier.rs:268-277`, `pipeline/frame.rs:27-32`
  - The peak disk use is ≈2×.
  - `StoredImage::load` copies the whole map into a new `Vec` (`frame_store/stored_image.rs:108-121`), where the warp could read the map directly. `[C]`
- [ ] **With `keep_cache`, per-run spill files go into the shared cache, leak, and can collide** — `pipeline/tier.rs:255`, `frame_store/spill_directory.rs:56-63`
  - `calib_{i}` and `warped_{i}` are never committed or reused. Two concurrent runs can map each other's `warped_3_c0.bin` and stack the wrong frame.
  - Per-run spills must always use a per-run directory. `[C]` leak, `[P]` collision.
- [ ] **Per-run spills could be unlinked temp files** — `frame_store/spill_directory.rs:131-156`
  - `O_TMPFILE` or unlink-after-map lets the OS clean up after a crash. That removes the marker file and the pid scan.
  - It also blocks replacement of a file under a live map (the hazard in the SAFETY comment at `frame_store/frame_spill.rs:400`).
  - The pid check deletes another host's live run on a NAS share. `[P]`
- [ ] **Deblend grids are sized to the component bbox, kept per job, and not in the memory planner** — `star_detection/deblend/multi_threshold/mod.rs:90-111,194-204`, `star_detection/deblend/local_maxima/mod.rs:80`
  - A satellite trail across a 24 MP frame allocates ≈480 MB per job. Deblending runs before the `max_area` filter (`star_detection/detector/stages/detect/mod.rs:124,142-148`). `[C]`
- [ ] **FITS checksum verification buffers the whole data unit, outside the budget, and reads it twice** — `io/image/fits/decode/selection.rs:134-150`, `io/image/fits/selected_fits.rs:78-89`
  - ≈124 MB extra for a 62 MP frame, on every Lumos-written CFA file. Accumulate the checksum per chunk during the decode. `[C]`

## 16. RAW: the preview is a second decoder, and LibRaw facts are lost

- [ ] **The preview RCD reads masked optical-black margins** — `io/raw/mod.rs:541-556`, `io/raw/demosaic/bayer/rcd/mod.rs:123-167,646-733`
  - Canon frames preview with a dark, colour-fringed left and top band. Preview and science disagree in that band.
  - LibRaw and RawTherapee demosaic only the visible area. `[C]`
- [ ] **Replace the preview path with `load_raw_cfa` → `CfaImage::demosaic` → clamp** — `io/raw/mod.rs:534-620`
  - This fixes the item above.
  - It removes `BlackRepeat::at_raw`, `raw_filter_color`, `apply_bayer_black_corrections`, the `CLAMP=true` normalize instance, `CfaPattern::at_raw_origin` (always the identity), `raw_xtrans_pattern`, `XTransNormalization`, `PixelSource::{U16, U16WithRepeat}`, `XTransImage::with_margins` and `process_xtrans`.
  - All margin arithmetic in RCD and Markesteijn collapses to active coordinates. The per-read `match` in Markesteijn's inner loop becomes a slice index. `[C]`
- [ ] **RCD always copies into three new output planes** — `io/raw/demosaic/bayer/rcd/mod.rs:424-439`
  - This costs 288 MB of peak memory and 3 copies per 24 MP frame. `[C]`
- [ ] **The LibRaw fallback uses `adjust_maximum`, so its scale is wrong** — `io/raw/mod.rs:629-646`
  - The default threshold 0.75 divides by the frame's own maximum when that maximum is within 75–100% of white, but `physical_scale = span` is recorded. Set `adjust_maximum_thr = 0`. `[C]`
- [ ] **The LibRaw fallback rotates by EXIF orientation and stretches by pixel aspect** — `io/raw/mod.rs:629-646,1073-1074`
  - Portrait frames decode as H×W in this path only, and the `row_order: TopDown` provenance is false. Set `user_flip = 0` and `use_fuji_rotate = 0`.
  - The 8-bit branch at `:730-745` is dead (`output_bps = 16`). `[C]`
- [ ] **SuperCCD `fuji_width` is ignored** — `io/raw/mod.rs:777-873`
  - RCD demosaics a 45°-rotated layout. Route it to the fallback, or refuse it. `[P]`
- [ ] **`raw_pitch` is assumed to be `2·raw_width`** — `io/raw/mod.rs:471-483`
  - Any other pitch shears the image silently. Check the pitch and return an error. `[P]`
- [ ] **The black level is applied in two roundings** — `io/raw/mod.rs:414-449,493-530`, `io/raw/normalize/mod.rs:5-8`
  - This breaks the module's "correctly rounded" promise. Every black term is an integer ADU, exact in f32.
  - Do one pass `(v − black_row[x]) / span` with a black row per row phase. Keep integer ADU in `BlackLevel` as the one source, and derive `per_channel`, `common`, `channel_delta_norm` and `delta_norm` from it.
  - The `delta.abs() > f32::EPSILON` test (`:337`, `:502-504`) becomes `cblack != 0`. `[C]`
- [ ] **Untrusted black metadata can overflow or pass validation** — `io/raw/mod.rs:204,222,240,249,252-260`
  - `u32 +=` on file data. `BlackExceedsMaximum` checks only `common`. `[C]`
- [ ] **The black level is truncated to whole ADU before lumos sees it** — LibRaw `utils_dcraw.cpp:247-251`, `tiff.cpp:1058-1092`
  - The OB mean in f64 and `dng_levels.dng_fblack` give the exact value. `[P]`, low.
- [ ] **Both demosaics run on CFA data that is not white-balanced** — `io/raw/demosaic/bayer/rcd/mod.rs`, `io/raw/demosaic/xtrans/markesteijn_steps/mod.rs:792-817`
  - The direction decisions assume balanced channels. dcraw, RawTherapee, darktable and ART white-balance before they demosaic.
  - Multiply by the camera WB (green = 1) before the kernel and divide after. `[P]`
- [ ] **X-Trans uses Markesteijn 1-pass** — `io/raw/demosaic/xtrans/markesteijn/mod.rs:1-14,44`
  - The LibRaw default and RawTherapee "best" are 3-pass. It costs ≈2–3× the time. `[P]`
- [ ] **RCD has no golden cross-check against librtprocess** — `io/raw/demosaic/bayer/rcd/tests.rs`
  - Markesteijn has one. `[C]` absent.

## 17. Format coverage and FITS header facts

- [ ] **The BOTTOM-UP Bayer parity uses the BINTABLE `NAXIS2` on tile-compressed HDUs** — `io/image/fits/metadata/mod.rs:309-318`
  - For a `ZIMAGE` HDU, `NAXIS2` is the tile-row count. An fpacked 4175-row frame with 100×100 tiles flips the pattern, and the whole frame is mis-debayered. Use `plan.dimensions.height()`. `[C]`
- [ ] **Lumos-written mono sensor files are refused as "mosaic"** — `io/image/fits/decode/mod.rs:59-72`, `io/image/fits/decode/pixels.rs:117-123`
  - The writer emits `CFATYPE='MONO'`. `[C]`
- [ ] **A 3-plane cube with a stale `BAYERPAT` is refused, and an unparseable `BAYERPAT` fails non-CFA loads** — `io/image/fits/decode/pixels.rs:69-70` `[C]`
- [ ] **`RAW_EXTENSIONS` refuses formats that LibRaw decodes** — `io/raw/mod.rs:47`
  - ORF, RW2, PEF, NRW, SRW, IIQ, 3FR, ERF, MRW and RWL are refused. Siril accepts LibRaw's full list. RW2 needs `zero_is_bad` (group 8) first. `[C]`
- [ ] **LibRaw 0.20.1 (2020) has no Sony lossless-compressed ARW decoder** — workspace `Cargo.toml` (`libraw-rs-sys 0.0.4`)
  - A1, A7 IV, A7R V and A7S III files fail to open. A dependency update needs your go-ahead. `[C]`
- [ ] **Common header aliases are not read** — `io/image/fits/metadata/mod.rs:26-45`
  - `EXPOSURE`, `CCD_TEMP`, `TEMPERAT`, `BINX`/`BINY`, `PIXSIZE1`, `XPIXELSZ`, `FRAMETYP`, `FILT-1`, `BLKLEVEL` (Siril `fits_keywords.c`). `[C]`
- [ ] **`read_cfa_hdu` is a second FITS entry point with its own validation** — `io/image/fits/decode/mod.rs:163-198`
  - It skips `validate_cfa_image_header`. `read_master` checks `LUMOSFMT` again by hand.
  - It uses `LoadContext::default()`, so it ignores the caller's cancel token and FITS options. `[C]`

## 18. Defect and cosmic-ray correction depart from the references

- [ ] **Cosmic-ray mask growth differs from L.A.Cosmic** — `calibration_masters/cosmic_ray/masks.rs:65-92`
  - astroscrappy grows twice (at `sigclip`, then at `sigcliplow`) with no objlim test. lumos grows one ring and applies objlim, so the wings of bright hits stay. `[C]` deviation, `[P]` impact.
- [ ] **The hot-pixel σ estimator breaks down above ≈1% defect density** — `calibration_masters/defect_map/mod.rs:396-405`
  - p99(|r|) falls inside the warm population on uncooled DSLR darks. `[P]`
- [ ] **Bayer green defects are repaired from stride-2 neighbours only** — `io/image/cfa/same_color/mod.rs:33-42`
  - The four diagonal greens at √2 px are the nearest. `[P]`
- [ ] **The X-Trans neighbour cut-off has a directional bias** — `io/image/cfa/same_color/mod.rs:152-178`
  - Manhattan ties are broken by scan order. Use Euclidean distance with a symmetric tie-break. `[P]`
- [ ] **A fully or heavily masked background tile still produces a sky value** — `background_mesh/tile_stats/mod.rs:179-192,305-354`
  - photutils `exclude_percentile` and SExtractor bad meshes interpolate those tiles from good neighbours instead. `[C]` behaviour.
- [ ] **The last `MeshAxis` tile can be a 1–5 px sliver** — `background_mesh/mesh_axis.rs:14-18`, `calibration_masters/defect_map/dark_background.rs:55-56,93-110`
  - A sliver can lack a CFA colour, which gives false hot pixels at the edge. `[C]`, low.

## 19. Display-domain operations: colour and tone errors

- [ ] **The colour-preserving stretch computes ratios on data that still holds the sky pedestal** — `image_ops/stretching/mod.rs:604-606`, `image_ops/rgb/mod.rs:237-254`, `image_ops/stretching/simd/mod.rs:140-157`
  - Faint Hα (0.055, 0.05, 0.05) comes out nearly grey.
  - Lupton 2004 and PixInsight ArcsinhStretch subtract the black point before they form the ratio. Auto-asinh needs a black point. `[C]`
- [ ] **HDR compresses by subtraction, which gives black halos** — `image_ops/hdr/mod.rs:103-107`
  - An M31 halo pixel goes to 0. Durand & Dorsey compress the base in the log domain: r' = mean·(r/mean)^(1−amount). `[C]`
- [ ] **HDR turns near-black RGB pixels into saturated colour speckle** — `image_ops/hdr/mod.rs:103-107`, `image_ops/rgb/mod.rs:239-242`
  - (I+δ)/I has no bound. Grey and RGB also disagree at I ≤ 0. `[C]`
- [ ] **The STF preset uses 1.5σ / 0.2, not −2.8·MADN / 0.25** — `image_ops/stretching/mod.rs:99-103`
  - The module calls it "standard". `[C]`
- [ ] **`BackgroundMode::Divide` silently does nothing when the model mean is ≤ 0** — `image_ops/background_extraction/mod.rs:147-149`
  - Return an `OpError`. `[C]`
- [ ] **NaN becomes 0 in the SIMD paths but propagates in the scalar paths** — `image_ops/stretching/simd/mod.rs:135,144` vs `image_ops/stretching/mod.rs:345,376,482,495` `[C]`
- [ ] **ML tile stride validation allows overlaps too small for the feather** — `image_ops/ml/backend/mod.rs:242-249`
  - Bound the stride at `WINDOW − 2·FEATHER_RAMP`. `[P]`
- [ ] **SCNR has no `amount` for Average Neutral, and no Maximum Neutral or Maximum Mask** — `image_ops/color_calibration/mod.rs:92-98` `[C]` gap.

## 20. Drizzle defaults and geometry

- [ ] **The default kernel is Turbo** — `drizzle/config.rs:46-47,110`
  - DrizzlePac, PixInsight and Siril default to Square, which is already implemented exactly. Turbo is wrong for any rotation other than 0/90/180°. `[C]`
- [ ] **The input-row bound ignores how far drops reach horizontally** — `drizzle/accumulator/frame_source.rs:302-305,326-368`
  - Lanczos at 90° rotation loses taps in the first 2–3 columns. The result depends on the band count, against `drizzle/accumulator/output_band.rs:68-74`.
  - Widen x by `reach.output_rows`. `[C]`
- [ ] **No CFA drizzle** — `drizzle/accumulator/mod.rs:340-341`
  - OSC data pays the demosaic interpolation before it drizzles. Siril offers CFA drizzle. Gap.
- [ ] **An invalid config is caught only after the first frame is decoded** — `drizzle/stack.rs:421-435` `[C]`

## 21. Reference choice and stage order

- [ ] **`Reference::Auto` picks by star count only** — `pipeline/align.rs:433-436`, `pipeline/config.rs:32`
  - The median FWHM is already measured. Siril uses lowest FWHM or wFWHM. Ties go to the last index. `[C]`
- [ ] **`Reference::Index` still spills every frame twice** — `pipeline/calibrate.rs:112-150`
  - When the anchor is known, decode → detect → register → warp → store needs one write. `[C]`
- [ ] **`align_and_stack` runs a serial non-finite check over all inputs before detection** — `pipeline/align.rs:147-155`, `pipeline/frame_check.rs:170-190`
  - The calibrated entry does it inside the parallel closure. `[C]`
- [ ] **The result discards per-frame registration (transform, RMS, inliers)** — `pipeline/result.rs:209-216` `[C]`

## 22. Run-to-run determinism

- [ ] **The RANSAC default seed is random** — `registration/ransac/config.rs:14,32`, `registration/ransac/sampling.rs:37`
  - Each run gives different stacked pixels. This defeats the SIMD bit-exactness work in `simd/mod.rs:8-14`. `[C]`
- [ ] **Parallel float reductions are order-nondeterministic** — `calibration_masters/prepared_flat/mod.rs:61,79-97`
  - rayon `sum`/`reduce`. Use fixed chunking. `[P]`
- [ ] **The triangle vote matrix switches to a SipHash `HashMap` from 500×500** — `registration/triangle/voting.rs:28,52-57`
  - A sorted flat `Vec` of pairs is deterministic and needs one representation. `[C]`
- [ ] **`try_par_map_bounded` returns the error of whichever slot fails first in slot order** — `concurrency/mod.rs:153-158` `[C]`, low.

## 23. Soundness and logic-error handling

- [ ] **`UnsafeSendPtr::new` is safe, but it makes any `T: Copy` Send + Sync** — `concurrency/mod.rs:17-30`
  - `&Cell<_>` then races from safe code. Restrict it to raw pointers, or make `new` unsafe. `[C]`
- [ ] **Union-find guards hide logic errors** — `star_detection/labeling/union_find.rs:202-204,225-227`
  - These guards cannot trigger in correct code, so they must be asserts. `[C]`
- [ ] **`find` has no path compression and union has no rank** — `star_detection/labeling/union_find.rs:198-213,257-276`
  - Comb masks give quadratic `build_label_map`. `parent[i] ≤ i` allows one forward flatten pass. `[P]`
- [ ] **`sort_by_flux` uses `partial_cmp(..).unwrap_or(Equal)`** — `star_detection/detector/stages/filter/mod.rs:101`
  - The sort can panic on Rust ≥ 1.81. Use `total_cmp`. `[C]`

## 24. Hot-path performance

- [ ] **Markesteijn step 6 is mostly serial** — `io/raw/demosaic/xtrans/markesteijn_steps/mod.rs:932-951,1031-1087`
  - It takes 335 of 956 ms. It builds 4 serial SATs, and `demosaic_border` walks all W×H pixels. `[C]`
- [ ] **Markesteijn step 3 does three integer divisions per element** — `io/raw/demosaic/xtrans/markesteijn_steps/mod.rs:509-595`
  - It takes 273 ms. Use row-parallel loops. `[P]`
- [ ] **The demosaics use full-frame arenas, not tiles** — `io/raw/demosaic/xtrans/markesteijn/mod.rs:82-96`, `io/raw/demosaic/bayer/rcd/mod.rs:146-176,329-332`
  - Markesteijn uses 1668 MB for 24 MP. dcraw tiles at 512 and RawTherapee RCD at 194. RCD is fully scalar. `[P]`
- [ ] **Warp tap weights are computed again per channel and per map** — `registration/resample/mod.rs:163-174`, `registration/resample/row/simd/mod.rs:78-79`
  - Loop over the channels inside the pixel loop. `[C]`
- [ ] **SIP is evaluated generically per pixel** — `registration/resample/row_positions.rs:33-39`, `registration/distortion/sip/mod.rs:347-408`
  - Per row it collapses to two polynomials in u (Horner, exact). `[C]`
- [ ] **`InverseWarp::apply` builds the monomials twice per Newton step, plus a final Jacobian that drizzle discards** — `registration/transform/inverse_warp.rs:66-85` `[C]`
- [ ] **The elliptical matched filter is a full k² 2-D convolution** — `star_detection/convolution/mod.rs:125-163`
  - It is 289 taps at FWHM 6. It is separable at 0/π/2. Geusebroek 2003 handles general angles. `[C]`
- [ ] **`Component::scan` walks the whole bbox 3–4 times** — `star_detection/deblend/component.rs:394-410`
  - Store the labeler's runs grouped by label, so the cost is O(area). `[C]`
- [ ] **Each multi-threshold level filters every pixel again** — `star_detection/deblend/multi_threshold/mod.rs:523`
  - Sort once, and each level becomes a prefix. `[C]`
- [ ] **Labeling uses a lock-free shared union-find for strip-local work** — `star_detection/labeling/union_find.rs`, `star_detection/labeling/labeler.rs:122-143`
  - A `SeqCst fetch_add` per run on one cache line. Use disjoint label blocks per strip. `[C]`
- [ ] **FITS decode allocates twice per chunk, converts serially, and rounds three times** — `io/image/fits/decode/pixels.rs:45-49,265-314`
  - Use one fused rayon pass with one f64 `(bzero + bscale·raw)/divisor` narrowed once. `[C]`
- [ ] **Moffat with a non-half-integer β runs scalar `powf` per lane** — `star_detection/centroid/moffat_fit/simd.rs:88-90` `[C]`
- [ ] **LM uses a fixed λ ×10 / ×0.1 schedule** — `star_detection/centroid/lm_optimizer.rs:180-208`
  - Nielsen's ρ-based update needs fewer iterations. `[P]`
- [ ] **Cosmic-ray noise and background are computed again in every iteration, serially** — `calibration_masters/cosmic_ray/mono.rs:114-129`, `calibration_masters/cosmic_ray/xtrans.rs:248-271`, `calibration_masters/cosmic_ray/masks.rs:74-92` `[C]`
- [ ] **GHS uses scalar `ln_1p`/`exp_m1` per pixel** — `image_ops/stretching/mod.rs:478-496`
  - It is the only curve without a vector or LUT path. `[C]`
- [ ] **Star detection allocates per frame** — `noise_floor_from`, `from_stars`, `filter_fwhm_outliers`, the dedup `HashMap`, the median buffer, and the kernel `Vec`
  - `labels.fill(0)` writes 96 MB per 24 MP frame. Clear only the previous runs. `[C]`
- [ ] **Survivor weights are gathered twice per pixel** — `combine/rejection/mod.rs:263-271` `[C]`
- [ ] **`GlobalMap` noise is averaged per pixel in the stamp loop** — `star_detection/centroid/mod.rs:490-494` `[P]`
- [ ] **On x86 without AVX2+FMA, every kernel calls libm `fmaf` per lane** — `simd/portable.rs:133-135`
  - This is deliberate for bit-exactness. The cost on Ivy Bridge-class CPUs is large. `[P]`

## 25. Docs that state false facts

- [ ] `registration/mod.rs:24`: the doctest does not compile (`&config.warp`, E0308). `registration/mod.rs:98-113` and `registration/config/mod.rs:145-157` use private paths and `TransformType`.
- [ ] `registration/distortion/mod.rs:16-17`, `registration/distortion/sip/mod.rs:1-10`: they claim FITS WCS SIP and Astrometry.net/Siril/ASTAP compatibility.
  - In fact the coefficients are normalized about a centroid. There is no AP/BP, no export, and the model is relative ref→target.
- [ ] `registration/ransac/mod.rs:3-11`: they call the scorer MAGSAC++ ("marginalizing over noise scales"). It is a truncated Welsch loss.
- [ ] `registration/mod.rs:378-381`: "`unzip` fills both". The code uses `gather_matched`.
- [ ] `drizzle/config.rs:37`: Square says "Sutherland-Hodgman", but it is `sgarea`/`boxer`.
- [ ] `drizzle/config.rs:51-52`: Gaussian says "configurable FWHM", but the FWHM is fixed at pixfrac·scale.
- [ ] `drizzle/config.rs:155`: `with_min_weight_fraction` says "coverage threshold".
- [ ] `image_ops/denoise/mod.rs:217` vs `:229`: the doc says the default is Hard, but the code defaults to Soft.
- [ ] `star_detection/mod.rs:10-14`: it says prepare applies defect correction, and that the background is bilinear (it is a cubic spline).
- [ ] `star_detection/convolution/mod.rs:44-45`: it claims SEP's matched filter, but the formula differs for varying σ.
- [ ] `star_detection/centroid/mod.rs:3-7,45-49,155-157`, `star_detection/centroid/gaussian_fit/mod.rs:11-12`, `star_detection/centroid/moffat_fit/mod.rs:10-11`:
  - The "~0.05 px" and "~0.01 px" accuracies are stated with no basis.
  - "99% of flux" is really 99.98% for a Gaussian and ≈91% for a β 2.5 Moffat.
- [ ] `star_detection/centroid/mod.rs:408`: a stray `Cov2` doc line sits on `compute_star`.
- [ ] `io/raw/demosaic/xtrans/markesteijn/mod.rs:14`: "<500 ms". Measured 956 ms.
- [ ] `io/raw/mod.rs:532-533,563`: "fast SIMD demosaic". RCD has no SIMD.
- [ ] `memory/run_memory.rs:15-16`: it promises a "share" for parallel stacks, but no code computes one.
- [ ] `frame_store/mod.rs:1`, `lib.rs:14`: they say frame_store does memory planning. That code is in `memory/`.
- [ ] `math/mod.rs:3-9`: it lists 5 of 9 submodules.
- [ ] `background_mesh/mod.rs:87-89`: the doc and `#[inline]` of `find_lower_tile_y` sit on `sigma_range`.

## 26. One fact in two places, wide signatures, and style deviations

- [ ] **The bundle keeps masters it never reads again** — `calibration_masters/mod.rs:208-215`
  - `flat_dark` (and `bias` when a dark is present) stay resident (≈96–240 MB each) and are saved. Keep only what `calibrate` reads, and record the inputs as provenance. `[C]`
- [ ] **The prepared flat is not its own type** — `calibration_masters/calibration_set.rs`, `calibration_masters/master_role.rs:110-112`, `calibration_masters/fits.rs:208-217`, `calibration_masters/prepared_flat/mod.rs`
  - A `PreparedFlat` struct makes the invariant a type. It also removes the forwarding `subtract` and turns 3 free fns into methods. `[C]`
- [ ] **`NoiseModel` behaviour is in `mono.rs`, and the dispatch is written twice** — `calibration_masters/cosmic_ray/mono.rs:224-246`, `calibration_masters/cosmic_ray/xtrans.rs:234-279` `[C]`
- [ ] **`StackConfig::bias()` and `dark()` are identical** — `combine/config/mod.rs:265-282` `[C]`
- [ ] **`DarkBackground` implements a tile mesh again** — `calibration_masters/defect_map/dark_background.rs:44-170`
  - A per-colour mode on `background_mesh` removes the parallel copy. `[P]`
- [ ] **`measure_star` takes `expected_fwhm` and a grid built from it, then asserts that they agree** — `star_detection/centroid/mod.rs:168-173`
  - The grid can carry the window σ and the annulus radius, which removes arguments from `moments_centroid`, `refine_centroid`, `compute_star` and `windowed_covariance`. `[C]`
- [ ] **`LMConfig` never varies** — `star_detection/centroid/lm_optimizer.rs:16-40`
  - Make it constants. That removes `GaussianFitConfig` and `MoffatFitConfig.lm`. `[C]`
- [ ] **`converged` plus a caller-side filter duplicates `None`** — `star_detection/centroid/gaussian_fit/mod.rs:233`, `star_detection/centroid/moffat_fit/mod.rs:54`, `star_detection/centroid/mod.rs:236,248` `[C]`
- [ ] **`MAX_ANNULUS_OUTER_RADIUS` copies the formula of `annulus_outer_radius`** — `star_detection/centroid/mod.rs:71` vs `:292` `[C]`
- [ ] **`amplitude_seed`/`min_amplitude` take `background` again** — `star_detection/centroid/stamp.rs:228,235`
  - `StampFit.sky` already holds it. `[C]`
- [ ] **The matched-filter PSF is split across two configs** — FWHM in `FwhmConfig`, axis ratio and angle in `DetectionConfig`
  - `wide_field` sets `Connectivity::Eight`, which is the default. `[C]`
- [ ] **`residual, sky, saturation` travel together through 5–6-argument fns** — `fwhm::estimate`, `DetectResult::from_image`, `measure`
  - `extract_and_filter_candidates`/`extract_candidates` is one function split thinly. `[C]`
- [ ] **`KernelPlan` is rebuilt per frame, although the doc says "once per run"** — `drizzle/accumulator/mod.rs:182`, `drizzle/accumulator/output_band.rs:38-42`
  - It has two `impl` blocks with `OutputBand` between them (`:92`, `:122`). `[C]`
- [ ] **`drizzle_stack` takes a `LoadContext` whose `cancel` it ignores** — `drizzle/stack.rs:54-82`
  - It honours `context.fits`, while `stack` always uses default FITS options (`frame_store/cache_key.rs:391-392`). Its memory ceiling comes from the caller, not from `CacheConfig`. `[C]`
- [ ] **Entry points disagree on `progress`/`cancel`** — `drizzle_stack`/`drizzle_images` take references, all others take values (`drizzle/stack.rs:69-75` vs `combine/stack/mod.rs:112-117`). `[C]`
- [ ] **`lib.rs:95-131` has 12 renamed re-exports** (`Config as StarDetectionConfig`, `Error as StackError`, …)
  - Rename the types, so rustc and docs show the public names. `[C]`
- [ ] **`SipFitResult` is public but not exported, and its diagnostics are computed and discarded** — `registration/distortion/sip/mod.rs:122`, `registration/mod.rs:398` `[C]`
- [ ] **`SipPolynomial.terms` is a pure function of the order** — `registration/distortion/sip/mod.rs:115` `[C]`
- [ ] **TPS defects** — `registration/distortion/tps/mod.rs`
  - The default regularization 0 interpolates centroid noise exactly.
  - `DistortionMap::interpolate` returns 0 past the grid.
  - `compute_residuals` duplicates `transform`. `[C]`
- [ ] **Dead code**
  - `DMat3` `IndexMut` (test-only, `math/dmat3/mod.rs:141-146`)
  - the bounds check in `resolve_matches` (`registration/triangle/voting.rs:222-223`)
  - `Rejection::None => values.len()` (`combine/rejection/mod.rs:220`)
  - `mask.fill(false)` (`star_detection/detector/stages/detect/mod.rs:83`)
  - `.filter(|d| d.area > 0)` (`:178`)
  - the 8-bit LibRaw branch (group 16)
  - `let planar = image;` (`image_ops/ml/backend/mod.rs:282`)
- [ ] **`NoFrames` is checked three times** — `combine_cached`, `from_stack_frames`, `load_tiered`
  - `combine_cached` is the documented gate. `[C]`
- [ ] **`check_cancel` is a free fn in `combine/error.rs:175`** (rule: error.rs is for errors only). `[C]`
- [ ] **The "subsample a plane into a Vec" code occurs three times** — `image_ops/stretching/mod.rs:240-244`, `image_ops/color_calibration/mod.rs:64-69`, `image_ops/denoise/mod.rs:351-357` `[C]`
- [ ] **`compact_by_mask` reimplements `Vec::retain`, and the dedup has two paths (`_simple`, `_hashed`)** — `star_detection/detector/stages/filter/mod.rs:229-244`
  - One sorted-cell pass replaces both paths. `[C]`
- [ ] **Exposed free fns that belong as methods**
  - `memory/mod.rs` `frame_bytes`/`quality_plane_bytes` → `ImageDimensions`
  - `frame_store/frame_spill.rs` `write_file`/`map_file`
  - `compute_annulus_background`, `windowed_covariance`, `stamp_centre`, `compute_stamp_radius`, `fit_is_plausible` (centroid)
  - `dilate_mask` → `BitBuffer2`
  - `deblend_local_maxima` and `deblend_multi_threshold` → `Component`
  - `denoise_plane` (6 args) → `Denoise`
  - FITS error constructors (`fits/error.rs`, `standard.rs:13`). `pixels.rs:295` builds `ImageError::Cancelled` inline. `[C]`
- [ ] **Several files hold more than one major struct, or are not named after their struct**
  - `memory/mod.rs` (4 structs)
  - `pipeline/tier.rs` (`FrameTier`, `StagePlan`, `StoredWarp`)
  - `pipeline/frame.rs` (`PipelineFrame`, `DetectedFrame`)
  - `progress/mod.rs`
  - `concurrency/mod.rs` `[C]`
- [ ] **`Region` has `pub` fields inside a `pub(crate)` type** — `star_detection/deblend/region.rs:12-20` `[C]`
- [ ] **Missing `const fn`**
  - `Cov2::trace`/`det`/`inverse`, `Gaussian2D::curvature_range`
  - `safe_ratio`, `Star::is_cosmic_ray`, `is_round`
- [ ] **`reserve` where the count is exact** — `registration/resample/row_positions.rs:31` → `reserve_exact`.
- [ ] **Comments that narrate or restate names** — `star_detection/centroid/lm_optimizer.rs:18-27`, `star_detection/centroid/local_background.rs:279`, `star_detection/centroid/moffat_fit/mod.rs:207`.

## 27. Dependencies

- [ ] **`parking_lot` (3 files) → `std::sync::Mutex`.**
- [ ] **`blake3` has one production use, a filename stem** (`frame_store/frame_spill.rs:240`).
  - The crate's FNV-1a (`frame_store/cache_key.rs:370`) does the same job. Keep `blake3` as a dev-dependency for the pin tests.
- [ ] **`smallvec` has two uses.** One is a `HashMap<_, SmallVec>` grid (`star_detection/detector/stages/filter/mod.rs:169`), which breaks the flat-collections rule.

---

## Checked and fine

The reviewers verified these parts against the references:

- **SIMD:** dispatch is sound, and the AGENTS.md asm check prints `0`. min/max/NaN are bit-identical across AVX2, NEON and portable, and the `reduce_sum` order is the documented one.
- **RAW:** `consolidate_black_levels` matches LibRaw `adjust_bl`. RCD steps 1–4.3 match librtprocess. Markesteijn tables, weights, homogeneity and blend match dcraw/librtprocess. The u16→f32 kernel is correctly rounded.
- **FITS:** integer normalization `|BSCALE|·(2^bits−1)` is exact for u16. The BOTTOM-UP rule matches Siril `adjust_Bayer_pattern`. Untrusted-input paths return errors, not panics.
- **Calibration:** the light order is correct, with no early negative clamp. Flat normalization is per CFA colour in f64. MAD→σ = 1.4826. The L.A.Cosmic core (Laplacian, S, S′, F, objlim) is correct.
- **Background mesh:** SExtractor mode with median fallback, reflected 3×3 median, natural spline on non-uniform knots.
- **Star detection:** separable matched-filter normalization, labeling stitch and connectivity, deterministic labels, SEP threshold ladder, root-relative contrast.
- **Centroid:**
  - Gaussian and Moffat Jacobians derived by hand.
  - FWHM formulas (Moffat 2α√(2^{1/β}−1), Gaussian 2√(2ln2)σ).
  - Window deconvolution unbiased to SNR 10.
  - f64 accumulation, no per-star heap allocation.
- **Registration:**
  - Umeyama similarity.
  - Hartley-normalized affine and DLT.
  - Adaptive iteration count, A-Res sampling.
  - Lanczos normalized by the actual separable sum.
  - f64 split before f32 narrowing.
  - k-d tree pruning.
  - Kish effective sample size.
- **Combine:**
  - MAD and 1.134 constants.
  - GESD λᵢ, p and df match Rosner.
  - Welford down-date in f64.
  - Deming slope.
  - Normalization before rejection.
  - Median quantization factors.
  - No per-pixel allocation.
- **Drizzle:**
  - `sgarea`/`boxer` match STScI `cdrizzlebox.c` line by line.
  - Square weight w/|area|.
  - Output grid mapping.
  - Lanczos restricted to scale 1 and pixfrac 1.
- **Image ops:**
  - MTF and the midtones solve.
  - GHS T and T′ match Sharpless & Cranfield.
  - B3 à trous with mirror edges.
  - Background polynomial with SVD rank check.
  - SCNR Average Neutral and Additive Mask.
  - CLAHE.
- **Infrastructure:**
  - Overflow-safe planner arithmetic.
  - Page-aligned mapped slices.
  - `try_par_map_bounded` ordering.
  - Config validated before decode.
