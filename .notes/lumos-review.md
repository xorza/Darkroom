# lumos review

> **When you address an item, delete it from this file.** Do not mark it done. The file lists open items only.

Scope: all production code in `lumos/src/`. Paths below are relative to `lumos/src/`. Line numbers are from commit `da6cbc3b0`. The original `pipeline/` and `frame_store/` references came from a tree before the module restructure (`b17fe5b00`). They are corrected below against `da6cbc3b0`, which has the same `lumos/src/` as `9c8837ed7`.

Each item has a tag:
- `[C]` means the reviewer traced the code path or reproduced the arithmetic.
- `[P]` means the mechanism is real, but the size of the effect depends on the data.

References give the established practice that the item compares against.

LibRaw source references (`utils_dcraw.cpp:…`, `tiff.cpp:…` and others) are line numbers in LibRaw 0.20.1. The crate now builds 0.22.2 from `libraw-sys/LibRaw`, so look them up again there.

Groups are sorted by severity × benefit. Correctness comes first, then precision, then performance, then design.

---

## 1. Rejection is not robust: outliers survive the default combine

Sigma clip is the default and the light preset. Winsorized is the bias and dark preset.

- [ ] `1.1` **The sigma-clip shortcut skips rejection on low-noise data** — `combine/rejection/sigma_clip_config.rs:179`
  - `variance < f32::EPSILON` compares σ² with 1.19e-7. Any pixel with a trimmed σ below 3.45e-4 of full scale (≈22 ADU at 16 bit) skips rejection.
  - The comment says this "matches" the full path. It does not: the full path tests σ, not σ².
  - Example: 20 bias frames, noise 1e-4, one cosmic ray at 0.5. The full path keeps 18 samples. The shortcut keeps all 20, so the mean moves by 240× the per-frame noise.
  - The shortcut runs again inside the loop (`:103`). `[C]`
- [ ] `1.2` **In the shortcut, two outliers hide each other** — `combine/rejection/sigma_clip_config.rs:146-189`
  - The shortcut trims one min and one max, then uses a non-robust stdev.
  - Example: {−0.1, −0.05, 0, 0, 0, 0, 0.05, 0.1, 10, 10} at k = 2.5.
    - The shortcut computes mean 1.26 and σ 3.53. The largest deviation, 8.74, is below 8.83, so it rejects nothing.
    - The MAD path computes σ = 0.074 and rejects both 10s.
  - A shortcut that changes the result is a bug. Its bound must follow from the MAD path, or the shortcut must go. `[C]`
- [ ] `1.3` **Winsorized clipping starts from a non-robust σ** — `combine/rejection/winsorized_clip_config.rs:78`
  - σ₀ = 1.134·stdev of all samples. With 3 outliers at +10σ in 10 frames, the Huber iteration keeps all three inside the band.
  - Simulation (500 trials, k = 3), mean rejected per pixel:

    | Frames / outliers | Current start | MAD start |
    |---|---|---|
    | 10 / 3 | 0.006 | 2.70 |
    | 12 / 4 | 0.0 | 3.17 |
    | 20 / 6 | 1.45 | 5.86 |

  - PixInsight PCL `WinsorizedSigmaClippingRejection` starts from 1.1926·Sn, a robust estimator. `[C]`
  - Siril (`rejection_float.c`) also starts from a plain `siril_stats_float_sd`. So the robust start departs from Siril on purpose: it follows PCL, because the plain start fails the table above.
- [ ] `1.4` **Winsorized has no outer loop, no survivor floor, and no Huber location step** — `combine/rejection/winsorized_clip_config.rs:115-136`
  - It rejects once and returns. Siril loops `while changed && N > 3`. PixInsight loops until stable and keeps ≥ 3.
  - The centre is always the plain median. PCL re-takes the mean of the winsorized values.
  - The doc at `:12` ("matching PixInsight/Siril") is false. `[C]`
- [ ] `1.5` **Linear-fit clipping rejects clean data** — `combine/rejection/linear_fit_clip_config.rs:67-140`
  - Pass 0 clips at k·1.4826·MAD. Later passes clip at k × the mean absolute residual of a line through the sorted order statistics, which is a much smaller unit.
  - Pure Gaussian data at k = 3, fraction rejected:

    | Frames | Linear fit | Sigma clip |
    |---|---|---|
    | 20 | 4.4% | 1.9% |
    | 50 | 6.3% | 0.8% |
    | 200 | 7.8% | 0.4% |

  - The method has no survivor floor (Siril: N − r ≤ 4).
  - With `max_iterations = 1` it never fits a line, so it is plain sigma clip.
  - Pass 0 (median ± k·1.4826·MAD) is a lumos addition. Siril fits the line from the first pass.
  - "Relationship to a reference value" (`:12`) is false: the x-axis is the sorted rank.
  - Correction: the sorted-rank x-axis and the mean-absolute-residual unit are Siril's own method (`rejection_float.c`, `LINEARFIT`). The growth of the rejection rate with N on clean data is a property of that reference method: sorted Gaussian samples follow the normal quantile curve, not a line. The lumos-only defects are the pass-0 unit change and the missing survivor floor. `[C]`
- [ ] `1.6` **The GESD automatic `max_outliers` cap is 2 below 25 frames and 10 above** — `combine/rejection/gesd_config.rs:50-51`
  - GESD resists masking only when r ≥ the true outlier count.
  - Example: n = 20, three outliers at 10σ, r = 2. It rejects 2 and keeps 1, so the mean moves by 0.55σ.
  - Siril and PixInsight use 0.3·N. `[P]`
- [ ] `1.7` **Percentile clipping does nothing on the small stacks it is documented for** — `combine/rejection/percentile_clip_config.rs:12,71`
  - ⌊0.1·n⌋ = 0 for n < 10, and the preset turns off the small-N fallback.
  - The name clashes with Siril's percentile clipping (deviation from the median as a fraction of the median). This method is a trimmed mean.
  - `(p/100)·n` in f32 gives 62 for 42% of 150. `(p·n)/100` is exact. `[C]`
- [ ] `1.8` **Survivors can drop to zero, and the pixel is written as 0 but reported as covered** — `combine/rejection/mod.rs:263-272`
  - Validation accepts any k > 0. There is no minimum-survivor rule (Siril 4, PixInsight 3). `[C]`
- [ ] `1.9` **The doc says winsorized "replaces outliers", but it only clips** — `combine/rejection/mod.rs:155`, `combine/stack/quantization.rs:7`, `combine/stack/mod.rs:366-369`
  - From this false premise, winsorized gets the `conservative` quantization σ (the worst single frame) instead of survivor tracking. That overstates the figure by ≈√N. `[C]`

## 2. Absolute `EPSILON` floors on quantities that scale with the data

`math/statistics/mod.rs:320-327` already explains why such a floor is wrong and uses `σ <= |median|·EPS`. No other module uses that form.

**Failure scenario:** 16-bit data in a 32-bit integer FITS normalizes by 2³²−1, so σ ≈ 2e-9. Every floor below then trips.

- [ ] `2.1` **Rejection degeneracy checks** — `combine/rejection/sigma_clip_config.rs:110`, `winsorized_clip_config.rs:80,97,122`, `linear_fit_clip_config.rs:75,121`
  - `sigma < f32::EPSILON`: no rejection happens on such data. `[C]`
- [ ] `2.2` **Weighting and normalization**
  - `combine/stack/mod.rs:253`: σ below EPS gives weight 0, and if all frames have weight 0 the stack silently falls back to equal weights.
  - `combine/normalization/photometric_gain.rs` (`deming_gain`, `Seed::of`, `ResidualWindow::at`): the gain silently becomes 1, or the fit is skipped. `[P]`
- [ ] `2.3` **Star SNR variance floor** — `star_detection/centroid/mod.rs:570`
  - The floor `f32::EPSILON` makes σ_total ≥ 3.45e-4. Every star's SNR is 10³–10⁴× too low, so every star fails `min_snr`.
  - On 16-bit frames, sky σ below ≈2.5 ADU is also understated. `[C]`
- [ ] `2.4` **Fit weight floor 1e-12** — `star_detection/centroid/stamp.rs:84`
  - On such data every weight is equal, so the "weighted" fit runs unweighted. `[C]`
- [ ] `2.5` **LM singular pivot 1e-15** — `star_detection/centroid/lm_optimizer.rs:13`, `:163-167`
  - The Hessian scales as A². A sound fit at A ≈ 1e-6 reads as singular and falls back to the biased seed (group 10).
  - Solving the Marquardt-scaled system (unit diagonal) makes the threshold scale-free. `[P]`
- [ ] `2.6` **Amplitude seed floored at 0.01 normalized** — `star_detection/centroid/stamp.rs:228-237`
  - 0.01 is 655 ADU at 16 bit. The fit costs about one extra LM iteration. `[C]`
- [ ] `2.7` **Detection channel weights** — `star_detection/detector/stages/prepare/mod.rs:75`
  - `sigma > f32::EPSILON` sets the weight of every channel to 0, then falls back to equal weights. `[C]`
- [ ] `2.8` **Two sigma-clip implementations with different floors** — `math/statistics::sigma_clip_iteration` and `SigmaClipConfig::reject`
  - One source of truth (one relative-floor helper) prevents this whole group. `[P]`

## 3. The default registration prior rejects real sessions

- [ ] `3.1` **Default `max_rotation` = 10° drops every frame after a meridian flip** — `registration/ransac/config.rs:35`, `registration/ransac/mod.rs:113-127`, `pipeline/config.rs:24`
  - A German-equatorial flip is a 180° rotation. Triangle matching survives it, but every RANSAC hypothesis fails `is_plausible`. The pipeline then drops the frame as "registration failed".
  - Alt-az mounts (smart telescopes) pass 10° of field rotation in one session.
  - Siril, PixInsight and DSS register across a flip by default. `[C]`

## 4. Calibration does not reconcile offsets and exposures

- [ ] `4.1` **Flats are scaled by a median ratio before the bias or flat-dark is subtracted** — `calibration_masters/mod.rs:68-92` with `combine/config/mod.rs:285-297` (`Normalization::Multiplicative`), subtraction at `calibration_masters/mod.rs:174-185`
  - Each raw flat is scaled while it still contains its offset. The subtraction runs once, on the finished master. The residual is b·(mean gain − 1).
  - Example: offset 0.02, flats at 0.5·f and 0.25·f. The corner/centre ratio is 0.509 instead of 0.500, a ≈2% vignetting residual. It also skews rejection between frames.
  - RAW frames lose the black level at decode, so on RAW only dark current leaks. FITS cameras keep `OFFSET` in the data.
  - PixInsight WBPP and Siril calibrate each flat before they integrate it multiplicatively. `[C]`
- [ ] `4.2` **A flat or a light with no additive subtractor is accepted silently** — `calibration_masters/mod.rs:174-185` (`_ => None`), `:263-279`
  - A light with a flat but no dark or bias gives (S + b)/flat, which adds an inverse-vignetting pattern. `[C]`
- [ ] `4.3` **Darks are never matched to the light** — `calibration_masters/mod.rs:263-274`, `validate_against_light` at `:295-350`
  - Only the pattern, size and sample domain are checked. `exposure_time`, `ccd_temp` and `iso` are never read.
  - A 120 s dark on 300 s lights removes 40% of the thermal signal and amp glow, and calibration still reports success.
  - There is no dark scaling or optimization (PixInsight "Optimize", Siril `-opt`). Scaling also needs a bias-free dark, but the masters store the dark with its bias.
  - The RAW CFA loader does not record exposure (`io/raw/mod.rs:1131-1148`), so a check has nothing to compare. `[C]`
- [ ] `4.5` **The flat floor `MIN_NORMALIZED_FLAT = 0.1` is applied without a report** — `calibration_masters/prepared_flat/mod.rs:16,69,114`
  - It is applied with no count and no warning. Deep smooth vignetting is under-corrected. `[C]`

## 5. Sample scale and quantization σ do not survive I/O

- [ ] `5.6` **Quantization tracking uses channel 0's gain for every channel** — `combine/stack/quantization.rs:52,66` `[C]`

## 6. Noise is estimated from whole-frame spread, which includes signal

- [ ] `6.1` **Combine noise weights use whole-frame MAD** — `combine/stack/mod.rs:236-258`, `combine/frame_stats.rs:34`
  - On a signal-dominated field, gain·MAD is the same for every frame, so Noise weighting degenerates to equal weights.
  - Gradients make the MAD larger, so frames with gradients are underweighted.
  - PixInsight weights by MRS noise (Starck & Murtagh, first wavelet layer). The same MAD also feeds reference selection and the Deming noise ratio. `[C]` mechanism.
- [ ] `6.2` **One weight per frame, averaged over RGB** — `combine/stack/mod.rs:241-252`
  - A frame with a bad blue channel is over-trusted in blue. PixInsight weights each channel separately. `[C]`
- [ ] `6.3` **Reference normalization compares raw `average_mad` across sample domains** — `combine/normalization/mod.rs:190-205` `[C]`
- [ ] `6.4` **Detection RGB weights use whole-frame MAD** — `star_detection/detector/stages/prepare/mod.rs:66-91`
  - A red nebula down-weights R. Each frame copies 3 planes and runs 2 quickselects per plane.
  - Use MAD of first differences ÷ √2 on a strided sample. `[C]` mechanism.
- [ ] `6.5` **Cosmic-ray empirical noise is one whole-frame (or whole-colour) median/MAD** — `calibration_masters/cosmic_ray/mono.rs:232-238`, `calibration_masters/cosmic_ray/xtrans.rs:244-271`
  - Gradients make N larger, so faint hits are missed. astroscrappy uses a per-pixel `sqrt(m5 + rn² + bkg)`. `[P]`
- [ ] `6.6` **The parametric cosmic-ray noise model drops the subtracted dark/sky level** — `calibration_masters/cosmic_ray/mono.rs:281-295`
  - N is understated on warm long exposures, so too many pixels are flagged. astroscrappy adds `bkg`/`pssl`. `[P]`
- [ ] `6.7` **Denoise σ at coarse scales comes from the scale's own coefficients** — `image_ops/denoise/mod.rs:330,351-362`
  - Nebula structure raises σ_j, so faint filaments are removed.
  - Use the multiresolution support (iterate on non-significant coefficients), or σ_I·σ_j^e with the B3 constants 0.889, 0.200, 0.086, 0.041, 0.020. `[P]`
- [ ] `6.8` **Denoise uses one global σ per scale and ignores the stack's coverage/variance planes** — `image_ops/denoise/mod.rs:330-343`
  - Low-coverage edges stay noisy. `[P]`

## 7. The variance quality plane is not a variance

AGENTS.md promises photometry-grade error bars. These planes cannot give them.

- [ ] `7.1` **`linear_variance` = Σw²/(Σw)² holds no per-frame σ or gain** — `combine/cache/sample.rs:46-72`, documented at `stack_product/mod.rs:51-55`
  - The variance of a weighted mean is Σwᵢ²gᵢ²σᵢ²/(Σw)². The combine already has σᵢ, gᵢ and q.
  - Example: equal weights, σ 1 and 2. The plane gives 0.5, but the truth is 1.25. `[C]`
- [ ] `7.2` **Noise and Manual weights are normalized to sum to 1** — `combine/stack/mod.rs:38-42`
  - So the weight plane cannot serve as inverse variance either. `[C]`
- [ ] `7.3` **Drizzle gives gated pixels `fill_value` with non-zero variance, weight and coverage** — `drizzle/accumulator/mod.rs:299-319` `[P]`
- [ ] `7.4` **Drizzle Lanczos marks coverage for exactly-zero-weight deposits** — `drizzle/accumulator/output_band.rs:445-449` → `:513-519`
  - Coverage reaches 3 px past the footprint, against `accumulator/mod.rs:168-170`. `[C]`

## 8. Missing-data masks are dropped after decode

- [ ] `8.2` **Star detection never reads `nulls`** — `star_detection/` (no reference)
  - A null region becomes a flat patch with zero noise. A wholly-null tile gets σ = 0, so the threshold falls to `σ·noise_floor`, and the pixels next to the gap merge into one huge component.
  - Pass `nulls` as the mesh mask, clear them from the threshold mask, and handle them in stamps. `[C]` path, `[P]` magnitude.

## 9. Star detection measures and splits on the wrong plane

- [ ] `9.1` **On demosaiced frames, flux, peak, FWHM and centroid are measured on the 3×3-median plane** — `star_detection/detector/stages/prepare/mod.rs:38-44`, `star_detection/detector/mod.rs:161,209-216`
  - The comment in `prepare` itself says filtering blurs the PSF that flux and FWHM are read from.
  - Effect on a Gaussian:
    - FWHM 2: 56% of flux and 42% of peak are kept, and the FWHM reads 2.23.
    - FWHM 3: 79% of flux is kept.
  - The centroid shows pixel locking of up to ±0.008 px.
  - Use the median plane only for the threshold mask, and measure on the unfiltered plane. `[C]`
- [ ] `9.2` **The matched-filter threshold assumes white noise, but the median plane is strongly correlated** — `star_detection/convolution/mod.rs:39-81`
  - The true filtered σ is 2.18–2.63× the σ in the map (filter FWHM 2.5–6). A "4σ" threshold is then ≈1.6σ, so ≈5% of sky pixels pass it on OSC frames.
  - Measure the noise of the filtered plane itself. That is exact under any correlation (demosaic, resampling). `[C]`
- [ ] `9.4` **Both deblenders split on the unfiltered residual, in units that do not match detection** — `star_detection/detector/stages/detect/mod.rs:59-103,182-205`, `star_detection/deblend/component.rs:394-410`, `star_detection/deblend/local_maxima/mod.rs:68-75`
  - The footprint is cut at σ in filtered-SNR units. The multi-threshold floor is in residual units (≈3× lower at FWHM 4).
  - Level 0 (`multi_threshold/mod.rs:512`) breaks the footprint into noise islands, and those islands become children.
  - LocalMaxima at prominence 0.3 splits about half of the faint stars.
  - SEP thresholds the filtered value (`cdvalue`), and photutils deblends the convolved data. `[C]` mechanism, `[P]` frequency.
- [ ] `9.5` **Sub-threshold branches have no minimum area** — `star_detection/deblend/multi_threshold/mod.rs:528-534,601-641`
  - A 1-pixel region is a valid node. SEP calls `lutz(..., minarea)` inside the deblend loop. `[C]`
- [ ] `9.6` **The multi-threshold significance walk runs top-down and drops deeper splits** — `star_detection/deblend/multi_threshold/mod.rs:665-685`
  - Example: root(100) → [A(80) → [A1(35), A2(30)], B(5)], contrast 0.2. lumos returns 1 object. SEP returns {A1, A2}.
  - SEP `deblend.c` walks from the bottom up and propagates `ok[]`. `[C]`
- [ ] `9.7` **Branch flux includes the pedestal below the split level** — `star_detection/deblend/multi_threshold/mod.rs:504-508,614`
  - SExtractor/SEP test `fdflux − thresh·fdnpix > mincont·root`. Bumps on bright wings carry level × npix of host light, which over-splits the wings.
  - The docs say "flux above that threshold". `[C]`
- [ ] `9.8` **The refinement mask uses the detection σ (4) on the unfiltered plane, with square dilation** — `star_detection/background/background_estimate.rs:251-262`, `star_detection/mask_dilation/mod.rs`
  - Faint wings stay unmasked and bias the sky upward. photutils uses ≈2σ on convolved data and a circular footprint. `[P]`

## 10. Shape metrics measure position, not shape

- [ ] `10.1` **SROUND measures sub-pixel phase** — `star_detection/roundness/mod.rs:42-46`, fed by `star_detection/centroid/mod.rs:454-545`
  - It is the marginal asymmetry about the stamp's centre pixel (`pos.round()`), so a star off the pixel centre is "lopsided".
  - Round stars rejected as `NotRound` (uniform phase):

    | FWHM | Default threshold 0.5 | Threshold 0.3 |
    |---|---|---|
    | 2 | 57% | 85% |
    | 2.5 | 21% | 73% |
    | 3 | 3% | 57% |

  - Auto-FWHM goes through `Rejection::of`, so it is biased upward.
  - Use the DAOFIND/photutils `roundness1` pinwheel quadrant sum: translation cancels to first order, and it catches 45° elongation. Nothing catches diagonal elongation today. `[C]`
- [ ] `10.2` **GROUND is not DAOFIND ROUND** — `star_detection/roundness/mod.rs`
  - It uses the max-sample marginals and no factor 2, so the scale is half of DAOFIND's and it depends on phase. DAOFIND fits 1-D Gaussians to the marginals.
  - The docs claim "the DAOFIND roundness metrics". `[C]`
- [ ] `10.3` **Sharpness divides the whole stamp's peak by the 3×3 core flux** — `star_detection/centroid/mod.rs:471,530-534`
  - The fainter star of a pair 5 px apart gets ratio 1.0 > 0.7, so it is rejected as `CosmicRay`.
  - The `star.rs:275-285` docs give two different thresholds (0.8 and 0.7). Neither matches DAOFIND's sharpness. `[C]`
- [ ] `10.4` **`max_fwhm_deviation` is documented as "MAD-scaled" but multiplies the raw MAD** — `star_detection/config/filter_config.rs:16`, `star_detection/detector/stages/filter/mod.rs:119-120`
  - 3 raw MAD ≈ 2.0σ. `[C]`

## 11. Centroids stop before they converge

- [ ] `11.1` **The default `WeightedMoments` stops after 10 plain fixed-point steps** — `star_detection/centroid/mod.rs:84,299-316,397`
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
- [ ] `11.2` **A rejected or non-converged fit falls back to the 2-step moments seed** — `star_detection/centroid/mod.rs:179-185,236-253`
  - Bias −0.155 px at a 0.4 px offset, up to −0.30 px for wide stars.
  - This happens for undersampled stars, saturated stars, close pairs, and LM giving up. `[C]`
- [ ] `11.3` **`LocalAnnulus`: the moments run before the local sky is measured and never subtract it** — `star_detection/centroid/mod.rs:185` vs `:209`
  - A pedestal of 0.2× peak raises the bias from −0.0035 to −0.0146 px. `[C]`
- [ ] `11.4` **`max(0)` clipping adds a rectified-noise pedestal to the moments** — `star_detection/centroid/mod.rs:381`
  - Contraction slows. SExtractor uses signed values. `[P]`
- [ ] `11.5` **The PSF is evaluated at pixel centres, not integrated over the pixel** — `star_detection/centroid/gaussian_fit/mod.rs:271-277`, `star_detection/centroid/moffat_fit/mod.rs:156-161`, `star_detection/centroid/covariance.rs`
  - FWHM bias: +16% at FWHM 1.2, +5.9% at 2, +2.6% at 3. `MIN_SIGMA` allows fits down to FWHM 1.18, where this dominates.
  - For a Gaussian, an erf-integrated model is exact. `[P]`
- [ ] `11.6` **A fit may move up to the full `stamp_radius` and is never re-stamped** — `star_detection/centroid/mod.rs:139-141` vs `:400` `[P]`
- [ ] `11.7` **The annulus starts at the stamp radius, inside the Moffat wings** — `star_detection/centroid/mod.rs:199-207,292-294`
  - It removes ≈4% of the flux at β 2.5 and FWHM 3. `[P]`
- [ ] `11.8` **LM convergence uses only absolute step and Δχ² tests** — `star_detection/centroid/lm_optimizer.rs:186-202`
  - A heavily damped step at large λ meets both tests. A rejected step can set `converged`.
  - Madsen–Nielsen–Tingleff use ‖g‖∞ ≤ ε₁ and ‖δ‖ ≤ ε₂(‖x‖+ε₂). `[P]`
- [ ] `11.9` **The IRLS second pass runs after a failed first fit** — `star_detection/centroid/stamp.rs:217-224` `[C]`
- [ ] `11.10` **A NaN in the residual reaches `Star.flux` and `snr`** — `star_detection/centroid/mod.rs:466-471`
  - A NaN SNR passes `snr < min_snr`, and `validate_catalog` checks only `pos` and `fwhm`. `[P]`

## 12. SNR follows neither the CCD equation nor the measured noise

- [ ] `12.1` **Read noise is counted twice** — `star_detection/config/measurement_config.rs:87-102`, used at `star_detection/centroid/mod.rs:562` and `star_detection/centroid/stamp.rs:79-85`
  - The empirical background σ already holds RN². Merline & Howell add RN² to the sky shot term only. `[C]`
- [ ] `12.2` **The sky-estimate error term n_pix(1 + n_pix/n_B) is missing** — `star_detection/centroid/mod.rs:557-571`
  - Sky-limited SNR reads up to 1.48× high in `LocalAnnulus` mode. `[C]`
- [ ] `12.3` **Measurement ignores `SkyNoise::floor`, which the threshold applies** — `star_detection/centroid/mod.rs:191,492,522` `[C]`
- [ ] `12.4` **There is no per-star positional uncertainty** — `star_detection/centroid/lm_optimizer.rs:211-216`, `star_detection/star.rs`
  - (JᵀWJ)⁻¹·χ²/(n−p) is one solve on a Hessian that already exists. Registration needs it for weights (group 13). `[C]` absent.

## 13. The final registration fit discards precision

- [ ] `13.1` **Match recovery searches only the brightest `max_stars` (200)** — `registration/mod.rs:144-155,355-365`
  - The final fit never sees the rest of the catalog. With 2000 stars this costs ≈3× in precision. Triangle matching needs the cap, but the final fit does not. `[C]`
- [ ] `13.2` **The recovery and inlier gates are ≈1.5 FWHM wide, and the fit inside them is plain least squares** — `registration/tuning/mod.rs:33,44`, `registration/recovery.rs:80,93,132`, `registration/ransac/mod.rs:320-327`
  - The gate is 30–100× the centroid noise. Nothing tightens it or clips residuals, so one blend 4 px off pulls the fit by r/N.
  - Use MAGSAC++ σ-consensus IRLS, or shrinking LO thresholds. `[C]`
- [ ] `13.3` **One mismatch can push the Auto ladder to an overfit model** — `registration/mod.rs:283,410-417`, `registration/result/mod.rs:215-226`
  - The RMS is non-robust: one 4 px mismatch among 50 true matches gives 0.57 px > 0.5, so the ladder escalates to Affine or Homography.
  - Points that the SIP fit clipped also stay in the RMS. `[C]`
- [ ] `13.4` **There is no weighting by centroid σ, and saturated stars are used** — `registration/mod.rs:144-153`, `registration/ransac/transforms.rs`
  - Brightest-first selection prefers saturated stars, which have flat tops. The weight should be 1/σ² with σ ≈ FWHM/(2.355·SNR).
  - PixInsight excludes stars above its upper limit. `[P]`
- [ ] `13.5` **The homography is refined only by algebraic DLT** — `registration/ransac/transforms.rs:257-310`
  - OpenCV and Hartley–Zisserman refine with LM on reprojection error. `[C]`
- [ ] `13.6` **SIP is fitted after the linear transform is frozen** — `registration/distortion/sip/mod.rs:153-263`, `registration/mod.rs:377-402`
  - The monomials are not orthogonal to the affine terms, so this is one Gauss–Seidel step, not the joint optimum.
  - The result also depends on the per-frame default reference point (`sip/mod.rs:180-183`).
  - astrometry.net `fit_sip_wcs` fits both together. `[C]`
- [ ] `13.7` **LO-RANSAC can commit a refit whose score was cut short** — `registration/ransac/mod.rs:185-192,200`
  - It accepts a refit on inlier count when the score dropped. The preemptive scorer then exits early, so the inlier list is a prefix and the score is overstated.
  - Accept on score only (Chum 2003). `[C]`
- [ ] `13.8` **RANSAC degeneracy tests depend on scale and miss some cases** — `registration/ransac/mod.rs:75,448-483`
  - The collinearity test uses an absolute cross product of 1 px². It tests only triplets that include p₀.
  - OpenCV `checkSubset` tests every triplet and rejects orientation flips. `[P]`
- [ ] `13.9` **Triangle flatness uses absolute thresholds, and three tests overlap** — `registration/triangle/geometry.rs:14,70-72,85-89,108-110`
  - One relative test (area/longest² ≥ c) replaces all three. `[C]`
- [ ] `13.10` **The vertex order of near-isosceles triangles is unstable** — `registration/triangle/geometry.rs:55-60` `[P]`

## 14. Resampling: ringing, aliasing, unstable normalization

- [ ] `14.1` **Lanczos has no ringing clamp** — `registration/resample/row/simd/mod.rs:66-89`
  - Undershoot is up to ≈13% of peak around bright or undersampled stars.
  - PixInsight StarAlignment clamps at 0.3 by default. Siril clamps by default. `[C]`
- [ ] `14.2` **No prefilter when the warp downsamples** — `registration/resample/kernel/mod.rs`, `registration/resample/row/simd/mod.rs`
  - The `mosaic` preset allows scale 0.5–2. Stretch the kernel by the scale factor, or switch the kernel. `[C]`
- [ ] `14.3` **Masked frames divide by a signed kernel sum** — `registration/resample/masked_warp.rs:94-125`
  - Two adjacent nulls at half-pixel phase leave a sum of 0.25, which amplifies noise. Normalized convolution needs a non-negative applicability.
  - The border path already falls back to bilinear. Do the same here. `[P]`
- [ ] `14.4` **A bilinear band within `a` px of the source edge gives a visible sharpness step** — `registration/resample/row/simd/mod.rs:72-74` `[C]`

## 15. Spill and memory planning disagree with the machine

- [ ] `15.1` **The default spill directory is on tmpfs** — `combine/cache_config.rs:27` (`env::temp_dir()`)
  - On this host `/tmp` is a 13.6 GB tmpfs. Debian 13 and Arch use tmpfs by default.
  - Spill files then use RAM, fill the tmpfs (ENOSPC), and evict the memory the plan counted on.
  - Default to a disk-backed directory (`$XDG_CACHE_HOME`, `/var/tmp`), or refuse tmpfs after a `statfs` check. `[C]`
- [ ] `15.2` **The memory reading ignores cgroup limits** — `memory/mod.rs:20-43`
  - sysinfo `available_memory()` is the host `MemAvailable`. The GitHub runner containers (14g/8g, no swap) get OOM-killed instead of spilling.
  - Take min(available, `cgroup_limits().free_memory`). `[C]`
- [ ] `15.3` **`align_and_stack` counts its input frames twice** — `pipeline/align.rs:75-96`, `memory/mod.rs:229-234`
  - `RunMemory::read` runs after the inputs are allocated, and the plan charges them again. 30 RGB 24 MP frames spill ≈14 GB that would fit. `[C]`
- [ ] `15.4` **On the spill tier, warp buffers stay allocated through the whole combine** — `pipeline/align.rs:228,317`
  - ≈3.8 GB for 8 workers on RGB 24 MP. The combine chunk sizing does not know about it. Drop them before the combine. `[C]`
- [ ] `15.5` **Spilled calibrated frames are never deleted after read-back** — `pipeline/tier.rs:81-92`, `pipeline/frame.rs:27-32`
  - The peak disk use is ≈2×.
  - `StoredImage::load` copies the whole map into a new `Vec` (`frame_store/stored_image.rs:50-63`), where the warp could read the map directly. `[C]`
- [ ] `15.6` **With `keep_cache`, per-run spill files go into the shared cache, leak, and can collide** — `pipeline/tier.rs:64-74`, `frame_store/spill_directory.rs:37-48`
  - `calib_{i}` and `warped_{i}` are never committed or reused. Two concurrent runs can map each other's `warped_3_c0.bin` and stack the wrong frame.
  - Per-run spills must always use a per-run directory. `[C]` leak, `[P]` collision.
- [ ] `15.7` **Per-run spills could be unlinked temp files** — `frame_store/spill_directory.rs:114-140`
  - `O_TMPFILE` or unlink-after-map lets the OS clean up after a crash. That removes the marker file and the pid scan.
  - It also blocks replacement of a file under a live map (the hazard in the SAFETY comment at `frame_store/frame_spill.rs:223`).
  - The pid check deletes another host's live run on a NAS share. `[P]`
- [ ] `15.8` **Deblend grids are sized to the component bbox, kept per job, and not in the memory planner** — `star_detection/deblend/multi_threshold/mod.rs:90-111,194-204`, `star_detection/deblend/local_maxima/mod.rs:80`
  - A satellite trail across a 24 MP frame allocates ≈480 MB per job. Deblending runs before the `max_area` filter (`star_detection/detector/stages/detect/mod.rs:124,142-148`). `[C]`
- [ ] `15.9` **FITS checksum verification buffers the whole data unit, outside the budget, and reads it twice** — `io/image/fits/decode/selection.rs:134-150`, `io/image/fits/selected_fits.rs:78-89`
  - ≈124 MB extra for a 62 MP frame, on every Lumos-written CFA file. Accumulate the checksum per chunk during the decode. `[C]`

## 16. RAW: the preview is a second decoder, and LibRaw facts are lost

- [ ] `16.1` **The preview RCD reads masked optical-black margins** — `io/raw/mod.rs:541-556`, `io/raw/demosaic/bayer/rcd/mod.rs:123-167,646-733`
  - Canon frames preview with a dark, colour-fringed left and top band. Preview and science disagree in that band.
  - LibRaw and RawTherapee demosaic only the visible area. `[C]`
- [ ] `16.2` **Replace the preview path with `load_raw_cfa` → `CfaImage::demosaic` → clamp** — `io/raw/mod.rs:534-620`
  - This fixes the item above.
  - It removes `BlackRepeat::at_raw`, `raw_filter_color`, `apply_bayer_black_corrections`, the `CLAMP=true` normalize instance, `CfaPattern::at_raw_origin` (always the identity), `raw_xtrans_pattern`, `XTransNormalization`, `PixelSource::{U16, U16WithRepeat}`, `XTransImage::with_margins` and `process_xtrans`.
  - All margin arithmetic in RCD and Markesteijn collapses to active coordinates. The per-read `match` in Markesteijn's inner loop becomes a slice index. `[C]`
- [ ] `16.3` **RCD always copies into three new output planes** — `io/raw/demosaic/bayer/rcd/mod.rs:424-439`
  - This costs 288 MB of peak memory and 3 copies per 24 MP frame. `[C]`
- [ ] `16.4` **The LibRaw fallback uses `adjust_maximum`, so its scale is wrong** — `io/raw/mod.rs:629-646`
  - The default threshold 0.75 divides by the frame's own maximum when that maximum is within 75–100% of white, but `physical_scale = span` is recorded. Set `adjust_maximum_thr = 0`. `[C]`
- [ ] `16.5` **The LibRaw fallback rotates by EXIF orientation and stretches by pixel aspect** — `io/raw/mod.rs:629-646,1073-1074`
  - Portrait frames decode as H×W in this path only, and the `row_order: TopDown` provenance is false. Set `user_flip = 0` and `use_fuji_rotate = 0`.
  - The 8-bit branch at `:730-745` is dead (`output_bps = 16`). `[C]`
- [ ] `16.6` **SuperCCD `fuji_width` is ignored** — `io/raw/mod.rs:777-873`
  - RCD demosaics a 45°-rotated layout. Route it to the fallback, or refuse it. `[P]`
- [ ] `16.7` **`raw_pitch` is assumed to be `2·raw_width`** — `io/raw/mod.rs:471-483`
  - Any other pitch shears the image silently. Check the pitch and return an error. `[P]`
- [ ] `16.8` **The black level is applied in two roundings** — `io/raw/mod.rs:414-449,493-530`, `io/raw/normalize/mod.rs:5-8`
  - This breaks the module's "correctly rounded" promise. Every black term is an integer ADU, exact in f32.
  - Do one pass `(v − black_row[x]) / span` with a black row per row phase. Keep integer ADU in `BlackLevel` as the one source, and derive `per_channel`, `common`, `channel_delta_norm` and `delta_norm` from it.
  - The `delta.abs() > f32::EPSILON` test (`:337`, `:502-504`) becomes `cblack != 0`. `[C]`
- [ ] `16.9` **Untrusted black metadata can overflow or pass validation** — `io/raw/mod.rs:204,222,240,249,252-260`
  - `u32 +=` on file data. `BlackExceedsMaximum` checks only `common`. `[C]`
- [ ] `16.10` **The black level is truncated to whole ADU before lumos sees it** — LibRaw `utils_dcraw.cpp:247-251`, `tiff.cpp:1058-1092`
  - The OB mean in f64 and `dng_levels.dng_fblack` give the exact value. `[P]`, low.
- [ ] `16.11` **Both demosaics run on CFA data that is not white-balanced** — `io/raw/demosaic/bayer/rcd/mod.rs`, `io/raw/demosaic/xtrans/markesteijn_steps/mod.rs:792-817`
  - The direction decisions assume balanced channels. dcraw, RawTherapee, darktable and ART white-balance before they demosaic.
  - Multiply by the camera WB (green = 1) before the kernel and divide after. `[P]`
- [ ] `16.12` **X-Trans uses Markesteijn 1-pass** — `io/raw/demosaic/xtrans/markesteijn/mod.rs:1-14,44`
  - The LibRaw default and RawTherapee "best" are 3-pass. It costs ≈2–3× the time. `[P]`
- [ ] `16.13` **RCD has no golden cross-check against librtprocess** — `io/raw/demosaic/bayer/rcd/tests.rs`
  - Markesteijn has one. `[C]` absent.

## 17. Format coverage and FITS header facts

- [ ] `17.4` **`RAW_EXTENSIONS` refuses formats that LibRaw decodes** — `io/raw/mod.rs:47`
  - ORF, RW2, PEF, NRW, SRW, IIQ, 3FR, ERF, MRW and RWL are refused. Siril accepts LibRaw's full list. RW2 needs `zero_is_bad` (group 8) first. `[C]`
- [ ] `17.5` **Sony YCC pseudo-RAW (LibRaw 0.22) is already white-balanced, and lumos does not read `color.as_shot_wb_applied`** — `io/raw/mod.rs:297-321`
  - The camera WB recorded in the metadata then describes a balance the samples already carry. `[P]`
- [ ] `17.6` **Common header aliases are not read** — `io/image/fits/metadata/mod.rs:26-45`
  - `EXPOSURE`, `CCD_TEMP`, `TEMPERAT`, `BINX`/`BINY`, `PIXSIZE1`, `XPIXELSZ`, `FRAMETYP`, `FILT-1`, `BLKLEVEL` (Siril `fits_keywords.c`). `[C]`
- [ ] `17.7` **`read_cfa_hdu` is a second FITS entry point with its own validation** — `io/image/fits/decode/mod.rs:163-198`
  - It skips `validate_cfa_image_header`. `read_master` checks `LUMOSFMT` again by hand.
  - It uses `LoadContext::default()`, so it ignores the caller's cancel token and FITS options. `[C]`

## 18. Defect and cosmic-ray correction depart from the references

- [ ] `18.1` **Cosmic-ray mask growth differs from L.A.Cosmic** — `calibration_masters/cosmic_ray/masks.rs:65-92`
  - astroscrappy grows twice (at `sigclip`, then at `sigcliplow`) with no objlim test. lumos grows one ring and applies objlim, so the wings of bright hits stay. `[C]` deviation, `[P]` impact.
- [ ] `18.2` **The hot-pixel σ estimator breaks down above ≈1% defect density** — `calibration_masters/defect_map/mod.rs:396-405`
  - p99(|r|) falls inside the warm population on uncooled DSLR darks. `[P]`
- [ ] `18.3` **Bayer green defects are repaired from stride-2 neighbours only** — `io/image/cfa/same_color/mod.rs:33-42`
  - The four diagonal greens at √2 px are the nearest. `[P]`
- [ ] `18.4` **The X-Trans neighbour cut-off has a directional bias** — `io/image/cfa/same_color/mod.rs:152-178`
  - Manhattan ties are broken by scan order. Use Euclidean distance with a symmetric tie-break. `[P]`
- [ ] `18.5` **A fully or heavily masked background tile still produces a sky value** — `background_mesh/tile_stats/mod.rs:179-192,305-354`
  - photutils `exclude_percentile` and SExtractor bad meshes interpolate those tiles from good neighbours instead. `[C]` behaviour.
- [ ] `18.6` **The last `MeshAxis` tile can be a 1–5 px sliver** — `background_mesh/mesh_axis.rs:14-18`, `calibration_masters/defect_map/dark_background.rs:55-56,93-110`
  - A sliver can lack a CFA colour, which gives false hot pixels at the edge. `[C]`, low.

## 19. Display-domain operations: colour and tone errors

- [ ] `19.1` **The colour-preserving stretch computes ratios on data that still holds the sky pedestal** — `image_ops/stretching/mod.rs:604-606`, `image_ops/rgb/mod.rs:237-254`, `image_ops/stretching/simd/mod.rs:140-157`
  - Faint Hα (0.055, 0.05, 0.05) comes out nearly grey.
  - Lupton 2004 and PixInsight ArcsinhStretch subtract the black point before they form the ratio. Auto-asinh needs a black point. `[C]`
- [ ] `19.2` **HDR compresses by subtraction, which gives black halos** — `image_ops/hdr/mod.rs:103-107`
  - An M31 halo pixel goes to 0. Durand & Dorsey compress the base in the log domain: r' = mean·(r/mean)^(1−amount). `[C]`
- [ ] `19.3` **HDR turns near-black RGB pixels into saturated colour speckle** — `image_ops/hdr/mod.rs:103-107`, `image_ops/rgb/mod.rs:239-242`
  - (I+δ)/I has no bound. Grey and RGB also disagree at I ≤ 0. `[C]`
- [ ] `19.4` **The STF preset uses 1.5σ / 0.2, not −2.8·MADN / 0.25** — `image_ops/stretching/mod.rs:99-103`
  - The module calls it "standard". `[C]`
- [ ] `19.6` **NaN becomes 0 in the SIMD paths but propagates in the scalar paths** — `image_ops/stretching/simd/mod.rs:135,144` vs `image_ops/stretching/mod.rs:345,376,482,495` `[C]`
- [ ] `19.7` **ML tile stride validation allows overlaps too small for the feather** — `image_ops/ml/backend/mod.rs:242-249`
  - Bound the stride at `WINDOW − 2·FEATHER_RAMP`. `[P]`
- [ ] `19.8` **SCNR has no `amount` for Average Neutral, and no Maximum Neutral or Maximum Mask** — `image_ops/color_calibration/mod.rs:92-98` `[C]` gap.

## 20. Drizzle defaults and geometry

- [ ] `20.3` **No CFA drizzle** — `drizzle/accumulator/mod.rs:340-341`
  - OSC data pays the demosaic interpolation before it drizzles. Siril offers CFA drizzle. Gap.

## 21. Reference choice and stage order

- [ ] `21.1` **`Reference::Auto` picks by star count only** — `pipeline/align.rs:350-353`, `pipeline/config.rs:15`
  - The median FWHM is already measured. Siril uses lowest FWHM or wFWHM. Ties go to the last index. `[C]`
- [ ] `21.2` **`Reference::Index` still spills every frame twice** — `pipeline/calibrate.rs:112-150`
  - When the anchor is known, decode → detect → register → warp → store needs one write. `[C]`
- [ ] `21.3` **`align_and_stack` runs a serial non-finite check over all inputs before detection** — `pipeline/align.rs:63-72`, `combine/cache/frame_check.rs:31-40`
  - The calibrated entry does it inside the parallel closure. `[C]`
- [ ] `21.4` **The result discards per-frame registration (transform, RMS, inliers)** — `pipeline/result.rs:25-55` `[C]`

## 22. Run-to-run determinism

- [ ] `22.1` **The RANSAC default seed is random** — `registration/ransac/config.rs:14,32`, `registration/ransac/sampling.rs:37`
  - Each run gives different stacked pixels. This defeats the SIMD bit-exactness work in `simd/mod.rs:8-14`. `[C]`
- [ ] `22.2` **Parallel float reductions are order-nondeterministic** — `calibration_masters/prepared_flat/mod.rs:61,79-97`
  - rayon `sum`/`reduce`. Use fixed chunking. `[P]`
- [ ] `22.3` **The triangle vote matrix switches to a SipHash `HashMap` from 500×500** — `registration/triangle/voting.rs:28,52-57`
  - A sorted flat `Vec` of pairs is deterministic and needs one representation. `[C]`
- [ ] `22.4` **`try_par_map_bounded` returns the error of whichever slot fails first in slot order** — `concurrency/mod.rs:153-158` `[C]`, low.

## 24. Hot-path performance

- [ ] `24.1` **Markesteijn step 6 is mostly serial** — `io/raw/demosaic/xtrans/markesteijn_steps/mod.rs:932-951,1031-1087`
  - It takes 335 of 956 ms. It builds 4 serial SATs, and `demosaic_border` walks all W×H pixels. `[C]`
- [ ] `24.2` **Markesteijn step 3 does three integer divisions per element** — `io/raw/demosaic/xtrans/markesteijn_steps/mod.rs:509-595`
  - It takes 273 ms. Use row-parallel loops. `[P]`
- [ ] `24.3` **The demosaics use full-frame arenas, not tiles** — `io/raw/demosaic/xtrans/markesteijn/mod.rs:82-96`, `io/raw/demosaic/bayer/rcd/mod.rs:146-176,329-332`
  - Markesteijn uses 1668 MB for 24 MP. dcraw tiles at 512 and RawTherapee RCD at 194. RCD is fully scalar. `[P]`
- [ ] `24.4` **Warp tap weights are computed again per channel and per map** — `registration/resample/mod.rs:163-174`, `registration/resample/row/simd/mod.rs:78-79`
  - Loop over the channels inside the pixel loop. `[C]`
- [ ] `24.5` **SIP is evaluated generically per pixel** — `registration/resample/row_positions.rs:33-39`, `registration/distortion/sip/mod.rs:347-408`
  - Per row it collapses to two polynomials in u (Horner, exact). `[C]`
- [ ] `24.6` **`InverseWarp::apply` builds the monomials twice per Newton step, plus a final Jacobian that drizzle discards** — `registration/transform/inverse_warp.rs:66-85` `[C]`
- [ ] `24.7` **The elliptical matched filter is a full k² 2-D convolution** — `star_detection/convolution/mod.rs:125-163`
  - It is 289 taps at FWHM 6. It is separable at 0/π/2. Geusebroek 2003 handles general angles. `[C]`
- [ ] `24.8` **`Component::scan` walks the whole bbox 3–4 times** — `star_detection/deblend/component.rs:394-410`
  - Store the labeler's runs grouped by label, so the cost is O(area). `[C]`
- [ ] `24.9` **Each multi-threshold level filters every pixel again** — `star_detection/deblend/multi_threshold/mod.rs:523`
  - Sort once, and each level becomes a prefix. `[C]`
- [ ] `24.10` **Labeling uses a lock-free shared union-find for strip-local work** — `star_detection/labeling/union_find.rs`, `star_detection/labeling/labeler.rs:122-143`
  - A `SeqCst fetch_add` per run on one cache line. Use disjoint label blocks per strip. `[C]`
- [ ] `24.11` **FITS decode allocates twice per chunk, converts serially, and rounds three times** — `io/image/fits/decode/pixels.rs:45-49,265-314`
  - Use one fused rayon pass with one f64 `(bzero + bscale·raw)/divisor` narrowed once. `[C]`
- [ ] `24.12` **Moffat with a non-half-integer β runs scalar `powf` per lane** — `star_detection/centroid/moffat_fit/simd.rs:88-90` `[C]`
- [ ] `24.13` **LM uses a fixed λ ×10 / ×0.1 schedule** — `star_detection/centroid/lm_optimizer.rs:180-208`
  - Nielsen's ρ-based update needs fewer iterations. `[P]`
- [ ] `24.14` **Cosmic-ray noise and background are computed again in every iteration, serially** — `calibration_masters/cosmic_ray/mono.rs:114-129`, `calibration_masters/cosmic_ray/xtrans.rs:248-271`, `calibration_masters/cosmic_ray/masks.rs:74-92` `[C]`
- [ ] `24.15` **GHS uses scalar `ln_1p`/`exp_m1` per pixel** — `image_ops/stretching/mod.rs:478-496`
  - It is the only curve without a vector or LUT path. `[C]`
- [ ] `24.16` **Star detection allocates per frame** — `noise_floor_from`, `from_stars`, `filter_fwhm_outliers`, the dedup `HashMap`, the median buffer, and the kernel `Vec`
  - `labels.fill(0)` writes 96 MB per 24 MP frame. Clear only the previous runs. `[C]`
- [ ] `24.17` **Survivor weights are gathered twice per pixel** — `combine/rejection/mod.rs:263-271` `[C]`
- [ ] `24.18` **`GlobalMap` noise is averaged per pixel in the stamp loop** — `star_detection/centroid/mod.rs:490-494` `[P]`

## 25. Docs that state false facts

- [ ] `25.1` `registration/mod.rs:24`: the doctest does not compile (`&config.warp`, E0308). `registration/mod.rs:98-113` and `registration/config/mod.rs:145-157` use private paths and `TransformType`.
- [ ] `25.2` `registration/distortion/mod.rs:16-17`, `registration/distortion/sip/mod.rs:1-10`: they claim FITS WCS SIP and Astrometry.net/Siril/ASTAP compatibility.
  - In fact the coefficients are normalized about a centroid. There is no AP/BP, no export, and the model is relative ref→target.
- [ ] `25.3` `registration/ransac/mod.rs:3-11`: they call the scorer MAGSAC++ ("marginalizing over noise scales"). It is a truncated Welsch loss.
- [ ] `25.4` `registration/mod.rs:378-381`: "`unzip` fills both". The code uses `gather_matched`.
- [ ] `25.5` `drizzle/config.rs:37`: Square says "Sutherland-Hodgman", but it is `sgarea`/`boxer`.
- [ ] `25.6` `drizzle/config.rs:51-52`: Gaussian says "configurable FWHM", but the FWHM is fixed at pixfrac·scale.
- [ ] `25.7` `drizzle/config.rs:155`: `with_min_weight_fraction` says "coverage threshold".
- [ ] `25.8` `image_ops/denoise/mod.rs:217` vs `:229`: the doc says the default is Hard, but the code defaults to Soft.
- [ ] `25.9` `star_detection/mod.rs:10-14`: it says prepare applies defect correction, and that the background is bilinear (it is a cubic spline).
- [ ] `25.10` `star_detection/convolution/mod.rs:44-45`: it claims SEP's matched filter, but the formula differs for varying σ.
- [ ] `25.11` `star_detection/centroid/mod.rs:3-7,45-49,155-157`, `star_detection/centroid/gaussian_fit/mod.rs:11-12`, `star_detection/centroid/moffat_fit/mod.rs:10-11`:
  - The "~0.05 px" and "~0.01 px" accuracies are stated with no basis.
  - "99% of flux" is really 99.98% for a Gaussian and ≈91% for a β 2.5 Moffat.
- [ ] `25.12` `star_detection/centroid/mod.rs:408`: a stray `Cov2` doc line sits on `compute_star`.
- [ ] `25.13` `io/raw/demosaic/xtrans/markesteijn/mod.rs:14`: "<500 ms". Measured 956 ms.
- [ ] `25.14` `io/raw/mod.rs:532-533,563`: "fast SIMD demosaic". RCD has no SIMD.
- [ ] `25.15` `memory/run_memory.rs:15-16`: it promises a "share" for parallel stacks, but no code computes one.
- [ ] `25.16` `frame_store/mod.rs:1`, `lib.rs:14`: they say frame_store does memory planning. That code is in `memory/`.
- [ ] `25.17` `math/mod.rs:3-9`: it lists 5 of 9 submodules.
- [ ] `25.18` `background_mesh/mod.rs:87-89`: the doc and `#[inline]` of `find_lower_tile_y` sit on `sigma_range`.

## 26. One fact in two places, wide signatures, and style deviations

- [ ] `26.1` **The bundle keeps masters it never reads again** — `calibration_masters/mod.rs:208-215`
  - `flat_dark` (and `bias` when a dark is present) stay resident (≈96–240 MB each) and are saved. Keep only what `calibrate` reads, and record the inputs as provenance. `[C]`
- [ ] `26.2` **The prepared flat is not its own type** — `calibration_masters/calibration_set.rs`, `calibration_masters/master_role.rs:110-112`, `calibration_masters/fits.rs:208-217`, `calibration_masters/prepared_flat/mod.rs`
  - A `PreparedFlat` struct makes the invariant a type. It also removes the forwarding `subtract` and turns 3 free fns into methods. `[C]`
- [ ] `26.3` **`NoiseModel` behaviour is in `mono.rs`, and the dispatch is written twice** — `calibration_masters/cosmic_ray/mono.rs:224-246`, `calibration_masters/cosmic_ray/xtrans.rs:234-279` `[C]`
- [ ] `26.4` **`StackConfig::bias()` and `dark()` are identical** — `combine/config/mod.rs:265-282` `[C]`
- [ ] `26.5` **`DarkBackground` implements a tile mesh again** — `calibration_masters/defect_map/dark_background.rs:44-170`
  - A per-colour mode on `background_mesh` removes the parallel copy. `[P]`
- [ ] `26.6` **`measure_star` takes `expected_fwhm` and a grid built from it, then asserts that they agree** — `star_detection/centroid/mod.rs:168-173`
  - The grid can carry the window σ and the annulus radius, which removes arguments from `moments_centroid`, `refine_centroid`, `compute_star` and `windowed_covariance`. `[C]`
- [ ] `26.7` **`LMConfig` never varies** — `star_detection/centroid/lm_optimizer.rs:16-40`
  - Make it constants. That removes `GaussianFitConfig` and `MoffatFitConfig.lm`. `[C]`
- [ ] `26.8` **`converged` plus a caller-side filter duplicates `None`** — `star_detection/centroid/gaussian_fit/mod.rs:233`, `star_detection/centroid/moffat_fit/mod.rs:54`, `star_detection/centroid/mod.rs:236,248` `[C]`
- [ ] `26.9` **`MAX_ANNULUS_OUTER_RADIUS` copies the formula of `annulus_outer_radius`** — `star_detection/centroid/mod.rs:71` vs `:292` `[C]`
- [ ] `26.10` **`amplitude_seed`/`min_amplitude` take `background` again** — `star_detection/centroid/stamp.rs:228,235`
  - `StampFit.sky` already holds it. `[C]`
- [ ] `26.11` **The matched-filter PSF is split across two configs** — FWHM in `FwhmConfig`, axis ratio and angle in `DetectionConfig`
  - `wide_field` sets `Connectivity::Eight`, which is the default. `[C]`
- [ ] `26.12` **`residual, sky, saturation` travel together through 5–6-argument fns** — `fwhm::estimate`, `DetectResult::from_image`, `measure`
  - `extract_and_filter_candidates`/`extract_candidates` is one function split thinly. `[C]`
- [ ] `26.13` **`KernelPlan` is rebuilt per frame, although the doc says "once per run"** — `drizzle/accumulator/mod.rs:182`, `drizzle/accumulator/output_band.rs:38-42`
  - It has two `impl` blocks with `OutputBand` between them (`:92`, `:122`). `[C]`
- [ ] `26.14` **`drizzle_stack` takes a `LoadContext` whose `cancel` it ignores** — `drizzle/stack.rs:54-82`
  - It honours `context.fits`, while `stack` always uses default FITS options (`memory/run_memory.rs:47-49`). Its memory ceiling comes from the caller, not from `CacheConfig`. `[C]`
- [ ] `26.15` **Entry points disagree on `progress`/`cancel`** — `drizzle_stack`/`drizzle_images` take references, all others take values (`drizzle/stack.rs:69-75` vs `combine/stack/mod.rs:112-117`). `[C]`
- [ ] `26.16` **`lib.rs:95-131` has 12 renamed re-exports** (`Config as StarDetectionConfig`, `Error as StackError`, …)
  - Rename the types, so rustc and docs show the public names. `[C]`
- [ ] `26.17` **`SipFitResult` is public but not exported, and its diagnostics are computed and discarded** — `registration/distortion/sip/mod.rs:122`, `registration/mod.rs:398` `[C]`
- [ ] `26.18` **`SipPolynomial.terms` is a pure function of the order** — `registration/distortion/sip/mod.rs:115` `[C]`
- [ ] `26.19` **TPS defects** — `registration/distortion/tps/mod.rs`
  - The default regularization 0 interpolates centroid noise exactly.
  - `DistortionMap::interpolate` returns 0 past the grid.
  - `compute_residuals` duplicates `transform`. `[C]`
- [ ] `26.20` **Dead code**
  - `DMat3` `IndexMut` (test-only, `math/dmat3/mod.rs:141-146`)
  - the bounds check in `resolve_matches` (`registration/triangle/voting.rs:222-223`)
  - `Rejection::None => values.len()` (`combine/rejection/mod.rs:220`)
  - `mask.fill(false)` (`star_detection/detector/stages/detect/mod.rs:83`)
  - `.filter(|d| d.area > 0)` (`:178`)
  - the 8-bit LibRaw branch (group 16)
  - `let planar = image;` (`image_ops/ml/backend/mod.rs:282`)
- [ ] `26.21` **`NoFrames` is checked three times** — `combine_cached`, `from_stack_frames`, `load_tiered`
  - `combine_cached` is the documented gate. `[C]`
- [ ] `26.22` **`check_cancel` is a free fn in `combine/error.rs:175`** (rule: error.rs is for errors only). `[C]`
- [ ] `26.23` **The "subsample a plane into a Vec" code occurs three times** — `image_ops/stretching/mod.rs:240-244`, `image_ops/color_calibration/mod.rs:64-69`, `image_ops/denoise/mod.rs:351-357` `[C]`
- [ ] `26.24` **`compact_by_mask` reimplements `Vec::retain`, and the dedup has two paths (`_simple`, `_hashed`)** — `star_detection/detector/stages/filter/mod.rs:229-244`
  - One sorted-cell pass replaces both paths. `[C]`
- [ ] `26.25` **Exposed free fns that belong as methods**
  - `memory/mod.rs` `frame_bytes`/`quality_plane_bytes` → `ImageDimensions`
  - `frame_store/frame_spill.rs` `write_file`/`map_file`
  - `compute_annulus_background`, `windowed_covariance`, `stamp_centre`, `compute_stamp_radius`, `fit_is_plausible` (centroid)
  - `dilate_mask` → `BitBuffer2`
  - `deblend_local_maxima` and `deblend_multi_threshold` → `Component`
  - `denoise_plane` (6 args) → `Denoise`
  - FITS error constructors (`fits/error.rs`, `standard.rs:13`). `pixels.rs:295` builds `ImageError::Cancelled` inline. `[C]`
- [ ] `26.26` **Several files hold more than one major struct, or are not named after their struct**
  - `memory/mod.rs` (4 structs)
  - `pipeline/tier.rs` (`FrameTier`, `StagePlan`, `StoredWarp`)
  - `pipeline/frame.rs` (`PipelineFrame`, `DetectedFrame`)
  - `progress/mod.rs`
  - `concurrency/mod.rs` `[C]`
- [ ] `26.27` **`Region` has `pub` fields inside a `pub(crate)` type** — `star_detection/deblend/region.rs:12-20` `[C]`
- [ ] `26.28` **Missing `const fn`**
  - `Cov2::trace`/`det`/`inverse`, `Gaussian2D::curvature_range`
  - `safe_ratio`, `Star::is_cosmic_ray`, `is_round`
- [ ] `26.29` **`reserve` where the count is exact** — `registration/resample/row_positions.rs:31` → `reserve_exact`.
- [ ] `26.30` **Comments that narrate or restate names** — `star_detection/centroid/lm_optimizer.rs:18-27`, `star_detection/centroid/local_background.rs:279`, `star_detection/centroid/moffat_fit/mod.rs:207`.

## 27. Dependencies

- [ ] `27.1` **`parking_lot` (3 files) → `std::sync::Mutex`.**
- [ ] `27.2` **`blake3` has one production use, a filename stem** (`frame_store/frame_spill.rs:63`).
  - The crate's FNV-1a (`frame_store/cache_key.rs:62`) does the same job. Keep `blake3` as a dev-dependency for the pin tests.
- [ ] `27.3` **`smallvec` has two uses.** One is a `HashMap<_, SmallVec>` grid (`star_detection/detector/stages/filter/mod.rs:169`), which breaks the flat-collections rule.

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

---

# Assessment of this review

The review is accurate in substance. Spot checks against the code at `9c8837ed7` confirmed these items: 1.1, 1.2 (arithmetic again), 1.3, 1.4, 1.6, 1.7 (`0.42·150` gives 62 in f32), 1.9, 2.1, 3.1, 4.1, 5.1, 7.1, 9.1, 13.1, 15.1, 15.2, 15.3, 15.4, 15.5, 15.6, 17.1, 21.1 (ties go to the last index), 22.1 and 23.4.

Each item carries a stable id `g.n`: group g, item n. Ids are not reused when an item is deleted.

Corrections, now applied in place above:

- The `pipeline/` and `frame_store/` paths and lines came from an older tree. `pipeline/frame_check.rs` never existed. The mechanisms are real, so only the references changed.
- Item 1.5 (linear fit): the sorted-rank x-axis is Siril's method, not a lumos invention. The lumos-only defects are the pass-0 unit change and the missing survivor floor.
- Item 1.3 (winsorized): Siril also starts from a plain standard deviation. A robust start is a deliberate step past Siril, toward PCL.

Two gaps in the review itself:

- The groups are sorted by severity, but several fixes depend on others. For example, relative floors (group 2) need the quantization step (group 5), and registration weights (group 13) need per-star σ (group 12). The plan below orders the work by dependency.
- Some items are missing features, not defects: CFA drizzle (20.3), SCNR Maximum Neutral (19.8) and Markesteijn 3-pass (16.12). They need a scope decision. They are not bugs.

# Root causes

The 27 groups come from ten structural causes. Each cause is a missing or incomplete type, a stage that two concerns share, or the same work written more than once.

1. **The sample contract is incomplete, and operations do not update it.** `SampleDomain` holds scale, origin and unit, and it is computed again from the decode provenance at each read. The pedestal, the quantization step and the saturation level live in other places (`CfaImage::quantization_sigma`, `ImageMetadata::data_max`) or nowhere (the RAW black level). Dark subtraction and flat division change these facts, but nothing records the change. This causes 4.4, group 5 and part of group 2.
2. **Pixel-quality facts have no common carrier.** Missing data is a `NullMask`. Saturation is a scalar test on calibrated data. Defects are index lists that are repaired and then forgotten. Cosmic rays are repaired and counted, and the flat floor is silent. Each consumer sees a different subset, and most consumers see none. This causes group 8, 9.3, 4.5, and part of groups 7 and 18.
3. **No shared robust-statistics core.** Five rejection methods, `sigma_clip_iteration`, the detection weights, the frame statistics, the cosmic-ray noise and the denoise σ each estimate spread and test degeneracy in their own way, mostly with an absolute `f32::EPSILON`. This causes groups 1 and 2.
4. **The combine loses frame identity.** The gather packs the covered frames to the front and keeps no frame index. This causes 1.9, 5.6 and part of group 7.
5. **Noise is measured as spread, and no noise model exists.** `FrameStats` holds median and MAD only. The CCD equation is written once in star detection and once in the cosmic-ray model, and the combine has no form of it. This causes group 6, group 7 and group 12.
6. **Calibration is a bag of optional masters.** `CalibrationSet<Option<CfaImage>>` cannot state which masters a light needs, which exposure a dark must match, or that a flat must be calibrated before integration. This causes group 4 and items 26.1 and 26.2.
7. **Three ingest paths do the same job.** `combine/cache/loader`, `align_and_stack` and `calibrate_align_stack` each decode, check, measure statistics, choose a memory tier and build their own `RunShape`. The per-frame work (calibrate, cosmic rays, demosaic, detect) exists only in one of them. This causes 4.1, 15.3, 21.2, 21.3, 26.14, 26.15 and 26.21.
8. **Star detection uses one plane for two jobs.** The same plane gives the threshold (needs smoothing) and the measurement (needs the raw PSF). The measurement then has no convergence contract and no error output. This causes groups 9, 10 and 11.
9. **Registration fits once, on the hypothesis set.** The final transform comes from the RANSAC inliers of the brightest 200 stars, unweighted, with a wide gate and a non-robust model choice. Three modules solve least squares with three copies of the rank rule. This causes groups 3 and 13.
10. **Run scratch and the persistent cache share one directory and one rule.** This causes group 15.

Groups 14, 16, 17, 19, 20 and 22 to 27 are mostly local. The plan handles them in the phase of the code they touch.

# Shared components

These components are built once. The stage designs that follow use them, and no stage keeps a private copy. Each component names its consumers, so a later change reaches all of them.

## S1. `PixelFlags`: one data-quality plane

Replaces `NullMask`, the detector's saturation `BitBuffer2`, and the coverage planes that `FrameQuality::for_unwarped` builds from nulls. Closes 8.1, 8.3 and 9.3, and it carries 4.5 and the repairs of group 18.

```rust
/// One byte per pixel. Absent when no pixel carries a flag.
pub(crate) struct PixelFlags { bits: Buffer2<u8> }

NO_DATA      // the source holds no measurement (NaN, BLANK, LibRaw zero_is_bad)
SATURATED    // the raw value reached the sensor's linear limit
DEFECT       // hot or cold in the defect map
COSMIC_RAY   // found by L.A.Cosmic
REPAIRED     // the value is an interpolation from neighbours
FLAT_FLOOR   // the flat divisor was clamped at MIN_NORMALIZED_FLAT
```

- Producers set flags where the fact is exact:
  - The decoder sets `SATURATED` on the raw ADU, before any subtraction. The limit is LibRaw `color.linear_max` per channel when present, else `maximum`. FITS uses `DATAMAX`, else the integer range. After dark subtraction and flat division, the ceiling differs per pixel, so a scalar test on calibrated data cannot be exact.
  - The decoder sets `NO_DATA` (today's `NullMask`), including the RAW `zero_is_bad` zeros.
  - Calibration sets `DEFECT | REPAIRED` and `FLAT_FLOOR`. The cosmic-ray pass sets `COSMIC_RAY | REPAIRED`.
- Operations carry the flags:
  - The demosaic dilates every bit except `NO_DATA` by its own support radius on the same lattice.
  - The warp sends `NO_DATA` through the masked path that exists today (it becomes coverage). It ORs the other bits over the non-zero taps of each output pixel.
- Each consumer has an exclusion mask of bits:
  - The combine excludes `NO_DATA` always. It excludes `SATURATED`, `COSMIC_RAY` and `REPAIRED` samples while enough unflagged samples remain for `min_survivors`. A saturated value is a lower bound, not a measurement. Thus a set of mixed exposures gives a correct high-dynamic-range core, and a core saturated in every frame is flagged `SATURATED` in the product.
  - Detection excludes `NO_DATA` from the mesh, the threshold and the stamps, and it reads `SATURATED` for the star flag.
  - Drizzle gives a flagged pixel no deposit (8.1).
- The combine reads the byte plane directly, so an unwarped frame with nulls no longer needs two f32 planes. The byte plane costs a quarter of one f32 plane, and the memory planner charges it.
- The stack product gets a `flags` plane (`NO_DATA`, `SATURATED`). FITS stores it as an image extension `LUMFLAGS` with `BITPIX = 8`, in the same way as the HST and JWST `DQ` arrays.

Practice: HST and JWST pipelines carry a per-pixel `DQ` bit plane from the first step to the last, and each step decides by bits, not by repaired values. IRAF `imcombine` takes bad-pixel masks for the same reason.

## S2. `SampleDomain` and `FrameNoise`: what a value means, and how noisy it is

Closes 4.4 and 5.1 to 5.5. It is the base of the variance plane, the SNR, the rejection floor and the cosmic-ray model.

```rust
pub struct SampleDomain {
    pub scale: f64,         // source units per normalized unit
    pub origin: ScaleOrigin,
    pub pedestal: Pedestal, // Removed | Kept(f64 source units) | Unknown
    pub unit: Option<String>,
}

pub(crate) struct FrameNoise {   // per channel, in image units
    background: f32,             // measured (S10), includes read and quantization noise
    quantization: f32,           // step / √12, a lower bound
    electrons_per_unit: Option<f32>, // egain × scale, when the gain is known
}
```

- Built in phase 2: `SampleDomain` (f64 scale, pedestal, affine `conversion_to`, `after_subtracting`) and `ImageMetadata::quantization_sigma` are stored metadata, and the FITS codec is `io/image/fits/metadata/domain_keywords.rs`. `FrameNoise` and `CcdNoise` come with the noise estimator in phase 5, because their background term is measured there.
- Both become stored fields of `ImageMetadata`, not values computed again from the provenance. Operations update them through methods:
  - `subtract(master)` sets the pedestal to `Removed` and adds the master's own variance to `background`.
  - `scale_by(c)` multiplies the noise terms by |c| and divides `electrons_per_unit` by c.
  - The combine gives the product the reference frame's domain.
- `conversion_to` returns an affine map (`gain`, `offset`). A `Kept` pedestal against a `Removed` one converts exactly when the level is known. `Unknown` against a different pedestal is refused (4.4).
- The decoders fill the fields:
  - RAW: pedestal `Removed`, step 1 ADU for a linear curve, and the largest curve step, or `None`, for a compressed one (5.5).
  - Integer FITS: step `|BSCALE|·2^z`, where `z` is the trailing-zero count of the OR of all raw samples, accumulated in the normalize pass (5.4).
  - Float FITS with no declared scale: the normalize pass keeps the maximum, and the decode fails when the maximum exceeds 1 (5.3).
- One FITS codec, `io/image/fits/sample_domain_keywords.rs`, writes and reads `LUMSCALE`, `LUMSORIG`, `LUMPEDST`, `LUMQSIG`, `LUMEPU` and `BUNIT`. An assumed scale reloads as assumed (5.2). Every value is in image units, so a reload applies no second division (5.1).
- `CcdNoise` is the one form of the CCD equation (Merline & Howell 1995):

  `variance(x) = background² + max(x − sky, 0) / electrons_per_unit`

  The source term is present only when the gain is known. Three consumers use it, and none of them keeps a copy:
  - the combine variance plane (C1),
  - the star SNR (C4),
  - the parametric cosmic-ray noise (6.6).

  The measured `background` already holds read noise and quantization noise. So no consumer adds them again. That is the mistake of 12.1.

## S3. `Spread`: robust scale and its floor

Closes 2.1 and 2.8. Consumers: the rejection driver (S4), `sigma_clip_iteration` in the background mesh, the frame statistics and the detection noise.

- `Spread { centre, sigma }` comes from a sorted window: the median, and 1.4826·MAD with the small-sample consistency factors of Croux & Rousseeuw (1992) for n ≤ 9.
- The spread has a floor:

  `sigma_eff = max(sigma, floor)`, where `floor = max(window_background, |centre|·ε)`

  - `window_background` is the RMS of `gain_i · background_i` over the frames in the window, from S2. It solves two problems that a per-pixel estimate cannot solve:
    - On integer data with few distinct values, more than half the samples can tie, so MAD is 0 or one ADU. Today that stops rejection, and a cosmic ray in a bias stack survives.
    - An outlier cannot raise the floor, because the frame noise is measured on the whole frame.
  - `|centre|·ε` is one ULP at the centre. The samples near the centre are spaced by that ULP, so a smaller σ is not representable. The floor uses the centre, not the largest sample: one hot pixel at 1e6 would otherwise give a floor of 0.12 and turn off the rejection.
  - When `sigma_eff` is 0, the window is constant, and nothing can be rejected. That happens only for synthetic data with no noise model.
- The rule is scale-free: data × s + o gives the same decisions. The S9 harness proves this for every consumer.

## S4. `SortedWindow` and one rejection driver

Closes 1.1 to 1.9 and 24.17, and the dead arm `Rejection::None => values.len()` of 26.20. It replaces the five private loops.

- Every pixel's samples are sorted together with their frame ids. For n ≤ 32, one sorting network sorts 8 pixels at once in the `F32x8` lanes, branch-free, with +∞ in the empty slots. This generalizes the `median9` network that `star_detection/median_filter/simd` already has. Above 32, a scalar sort runs.
- The contract of every method is one function: from a sorted window `[lo, hi)`, give a narrower window. Sigma clip, winsorized, GESD and trim reject only from the ends by their nature. Linear fit is made to peel only from the ends, because in sorted order a middle value describes the shape of the distribution, not an outlier.
- The driver owns the outer loop to a fixed point (with the `max_iterations` cap), the floor from S3, and the survivor rule:
  - `StackConfig::min_survivors` (default 3, as in PixInsight) is validated `>= 1`.
  - When a step proposes fewer than m survivors, the driver keeps the m samples nearest the current centre, with ties to the lower position. The result is deterministic, and a pixel never has zero survivors (1.8).
- The median combine is the centre of the same sorted window. Median and rejection share one sort.
- The window unit is `RejectionScale::Robust` (S3), the default. `RejectionScale::CcdModel` uses the per-pixel σ from `CcdNoise` (S2), as IRAF `imcombine reject=ccdclip` does. It needs the gain, and it removes the scale-estimation noise that small stacks suffer from.
- The methods:
  - **Sigma clip:** a band of ±kσ about the median. `no_outliers_possible` goes away (1.1, 1.2). The sorting network pays for its sort.
  - **Winsorized:** follows PCL. The start is the robust σ. Each Huber step clamps at c = 1.5 and takes the location again from the mean of the clamped values. The outer loop runs until no sample changes (1.3, 1.4).
  - **Linear fit:** the sorted values are regressed on expected normal order statistics (Blom scores, `Φ⁻¹((i − 3/8)/(n + 1/4))`). The slope is a σ in Gaussian units, and k means the same thing in every pass. Pass 0 goes away. The clean-data rejection rate stays at the Gaussian tail rate for every N (1.5). The scores are computed once per run, as one flat `Vec<f32>` with `starts`.
  - **GESD:** automatic `max_outliers = ⌊0.3·n⌋`, capped by `n − min_survivors` (1.6).
  - **Trim** (today's percentile): counts `⌊p·n/100⌋` in integers (1.7).
- Every method only rejects. Winsorized gets survivor tracking like the others (1.9).

## S5. `CfaLattice`: one description of the colour lattice

Closes 26.3 and 26.5. Consumers: noise estimation (S10), cosmic rays, defect detection and repair, the background mesh, flat normalization, and the flag dilation of S1.

- `CfaLattice` gives the colour of a pixel, the colour classes, and the same-colour neighbour stencils as one flat table per pattern phase. It has a fast path that deinterleaves a Bayer frame into four dense planes.
- `SameColorMedian`, the cosmic-ray Bayer and X-Trans detectors, `DarkBackground` and the per-colour flat normalization use it in place of their own pattern dispatch.
- The X-Trans tie-break is Euclidean with a symmetric order (18.4). Bayer green repair includes the four diagonal greens (18.3).

## S6. A numerics kit

Closes 2.5, 13.5 and the solver parts of 11.8. Each item replaces copies that exist today.

- `LmController<N>`: Nielsen's ρ update of λ, the Madsen–Nielsen–Tingleff stop tests, and a Cholesky solve of the Marquardt-scaled system with the relative pivot `pivot <= n·ε·max(diag)`. A model supplies its `NormalEquations<N>` and its χ², as the centroid models already do through `LMModel`. Consumers: the Gaussian and Moffat fits, and the homography refinement with N = 8.
- `Lstsq`: an SVD solve with the rank tolerance `rows·ε·σ_max`. Today the SIP fit, the background extraction and the homography DLT each hold a copy of this rule. The final registration fit (C5) uses it too.
- `Irls`: Cauchy or Tukey weights from a MAD scale of the residuals, with a stop on the change of the weighted χ². Consumers: the final registration fit, the second pass of the stamp fit, and the Deming photometric normalization.

## S7. One ingest stage

Closes 4.1 (with C2), 15.3, 21.2, 21.3, 26.14, 26.15 and 26.21.

```rust
enum FrameSource<'a> { Paths(&'a [PathBuf]), Frames(Vec<LinearImage>) }
enum FrameOp<'a> { Subtract(&'a MasterBias), Calibrate(&'a CalibrationPlan),
                   CosmicRay(CosmicRayConfig), Demosaic, Detect(&'a DetectorPool) }
struct FrameRecord { image: PipelineFrame, flags: Option<PixelFlags>,
                     stats: FrameStats, stars: Option<Vec<Star>> }
```

- One function runs `source → ops → checks → statistics → tier` for every entry point. It does the dimension check, the non-finite check (in the parallel closure, 21.3), the frame-facts admission and the cancel checks once.
- Each `FrameOp` states its memory: its peak and its output for a given frame shape. The `RunShape` is built from the op list, and a `Frames` source charges no resident input again (15.3). The four hand-built shapes go away.
- The entry points become short:
  - `stack(paths)`: ingest with no ops, then combine.
  - `stack_cfa_master` for flats: ingest with `Subtract`, then combine. Each flat sub is calibrated before the multiplicative normalization (4.1).
  - `align_and_stack(frames)`: ingest with `Detect`, then register, warp and combine.
  - `calibrate_align_stack`: ingest with `Calibrate`, `CosmicRay`, `Demosaic` and `Detect`. With `Reference::Index`, the op list also registers and warps, so each frame is written once (21.2).
- Every entry takes `LoadContext` the same way and passes its FITS options and cancel token (26.14). Progress and cancel are passed the same way everywhere (26.15). `NoFrames` is checked once (26.21).

## S8. `RunReport`: no silent decisions

Every fallback and every check that was not possible goes into one report that the result returns: dropped frames and their reasons, calibration matches that could not be verified, floored flat pixels, interpolated background tiles, flag counts per bit, and a variance plane without a source term. Today these paths are a `tracing` line at most (4.2, 4.5, 2.2).

## S9. A test kit for invariants and references

- `internals/invariance.rs` runs a stage on transformed inputs and checks that the outputs transform exactly:
  - affine data, x·s + o: survivor sets, detections and weights (up to the scale),
  - a 180° rotation, on frames whose size is a multiple of the mesh tile: star positions map exactly,
  - a permutation of the frames: the combine output is bit-identical.
  Each phase adds its stage to the harness. This harness catches every regression of the class of group 2.
- `internals/reference/` holds small golden fixtures from Siril, SEP, photutils, astroscrappy and librtprocess, with the scripts that made them. This extends the existing Markesteijn cross-check to RCD (16.13), detection and cosmic rays.

## S10. Noise estimation on any lattice

Closes 6.4 to 6.8, 2.7, 18.5 and 18.6. It uses S1, S3 and S5.

- `FrameStats` gets the per-channel `background` noise of S2, measured before interpolation.
- The estimator depends on the lattice:
  - Full planes (mono, RGB): MRS noise (Starck & Murtagh). It takes the first B3 à trous layer, applies an iterated k-σ clip, and divides by the layer constant 0.889. The clip at k removes tail variance, so the estimate is also divided by the truncated-Gaussian factor `sqrt(1 − 2kφ(k)/(2Φ(k) − 1))`, which is 0.9866 at k = 3.
  - CFA frames: MAD of the differences between nearest same-colour neighbours, divided by √2, per colour class (S5). The B3 kernel would mix colours on a mosaic.
  - Both estimators skip pixels with `NO_DATA`.
- One estimate serves every consumer: combine weights, reference normalization, the Deming noise ratio (6.1, 6.3), and the detection channel weights. The ingest gives the detector the statistics it already measured, so the detector no longer copies three planes for its weights (6.4, 2.7).
- `background_mesh` becomes the one local-statistics engine:
  - It takes a lattice and flags.
  - A tile with too few valid pixels is interpolated from good neighbours, as photutils `exclude_percentile` and SExtractor bad meshes do (18.5).
  - The last `MeshAxis` tile merges into its neighbour when it is narrower than half a tile (18.6).
  - A per-colour mode replaces `DarkBackground` (26.5).
  - The cosmic-ray pass takes its local background and σ from the mesh: `sqrt(m5 + rn² + bkg)` as in astroscrappy (6.5, 6.6).
- Denoise uses the B3 constants 0.889, 0.200, 0.086, 0.041 and 0.020 from σ_I (6.7). It reads the stack's `variance` plane when it is present, so the threshold follows the local σ (6.8).

# Stage designs

## C1. Combine: weights, variance, flags

Closes group 7, 2.2 and 6.1 to 6.3. It uses S1 to S4.

- `CombineScratch` gets `frame_ids: Vec<u32>` beside `values` and `eff_weights`, filled in the same gather. Quantization tracking, per-channel gain and variance work under any coverage. `frame_indices_are_stable` goes away (5.6).
- The weights are per channel, `w_ic = 1 / (g_ic² · background_ic²)` (6.2), and they are not normalized (7.2). Manual weights stay relative, and the docs say so. A background of 0 can only come from synthetic data with no noise. With `Weighting::Noise`, that is an error that names the frame (2.2).
- `CombinedSample::variance = Σ wᵢ² vᵢ(x) / (Σ wᵢ)²`, with `vᵢ(x) = g_i² · CcdNoise_i(x) / confidence_i` (7.1). The plane is in image units², so `linear_variance` is renamed `variance`. Without the gain, the plane holds the background term only, and `RunReport` says so. The median has no variance plane, because no exact form exists.
- An optional `dispersion` plane gives the weighted scatter of the survivors. It costs one Welford pass over data the rejection already holds, and it checks the model variance without a model.
- Drizzle uses the same formula with drop weights. Flagged and gated pixels get zero weight, zero variance and zero coverage (7.3). A zero-weight deposit marks no coverage (7.4). The docs state that drizzle noise is correlated between output pixels (Fruchter & Hook 2002), and that the plane is the per-pixel variance only.

## C2. Calibration as a typed plan

Closes 4.2, 4.3, 4.5, 26.1 and 26.2. 4.1 closes through S7. It uses S1, S2, S7 and S8.

- Typed masters replace `CalibrationSet<Option<CfaImage>>`: `MasterBias`, `MasterDark { thermal, exposure, temperature, bias: Included | Removed }`, and `PreparedFlat` with its divisor and its floored-pixel count (26.2). Each master carries its `FrameNoise`, so the subtraction adds its variance (S2).
- `CalibrationPlan::new(masters, light_facts)` checks the plan before any light is touched:
  - A flat without an additive subtractor is an error (4.2).
  - A dark must match the light's exposure. It must also match the temperature when both frames declare one. A bias-removed dark with a bias may be scaled by `t_light / t_dark`. Any other mismatch is an error (4.3).
  - A fact that one side does not declare cannot be compared. It is the same rule as `SampleDomain::units_agree`. The plan then records "unverified" for that fact in `RunReport`. It does not refuse, because a DSLR declares no temperature, and it does not pass the fact silently.
- `CalibrationMasters` keeps only what `calibrate` reads: one subtractor, one `PreparedFlat` and one `DefectMap` (26.1).
- Defect repair sets `DEFECT | REPAIRED`, and the flat floor sets `FLAT_FLOOR`. Both counts go into `RunReport` (4.5).

## C3. Detection: one plane to threshold, one plane to measure

Closes group 9 except 9.3 (S1), 8.2, 15.8, 26.11 and 26.12. It uses S1 and S10.

- `prepare` returns a `PreparedFrame` that holds what travels together today in 5- and 6-argument functions (26.12):
  - `measure`: the noise-weighted channel combination, never filtered,
  - `detect`: `measure` minus sky, median-filtered when the frame is demosaiced, then matched-filtered,
  - `sky` and its σ map,
  - the flags.
- The noise of `detect` is measured on `detect` with the same mesh. That is exact under any correlation (9.2). The threshold and both deblenders work on `detect` in the units of its own σ, as SEP does (9.4).
- The multi-threshold walk runs bottom-up and propagates `ok[]`, as in SEP `deblend.c` (9.6). It measures flux above the split level (9.7) and applies `minarea` in the loop (9.5).
- The refinement mask thresholds `detect` at 2σ with a circular footprint (9.8).
- Every measurement reads `measure` (9.1).
- The `max_area` filter runs before deblending, and the deblend grids come from the detector's pool (15.8).
- The matched-filter PSF lives in one config (26.11).

## C4. Measurement with a convergence contract and an error output

Closes groups 10, 11 and 12, 2.3, 2.4, 2.6, 26.6 to 26.10, 24.12 and 24.13. It uses S2, S3 and S6.

- A `MeasureGrid` holds what follows from the expected FWHM: the window σ, the stamp radius and the annulus radii (26.6, 26.9). The inner annulus radius encloses a stated flux fraction of a Moffat with β = 2.5 (11.7).
- The windowed centroid subtracts the local sky first (11.3). It uses signed values (11.4) and the adaptive-moments Newton step `σ_w² / (σ_w² − C_obs)` (11.1). It stops on the bound `c/(1 − c)·‖Δ‖` of the remaining error.
- The PSF models are integrated over the pixel. The Gaussian uses erf differences, which is exact (11.5). The Moffat uses Gauss–Legendre quadrature, with an order that keeps its error below 1% of the centroid noise at the minimum FWHM.
- The fits run on `LmController` (S6), with fixed constants (26.7). A failure returns `None` (26.8). The IRLS pass runs only after a successful fit (11.9). The fit weight floor and the amplitude seed floor are fractions of the stamp's sky σ (2.4, 2.6).
- A failed fit falls back to the converged windowed centroid (11.2). A fit that moves more than half the stamp radius is stamped again, once (11.6).
- Every star gets `position_sigma` (12.4):
  - after a fit, from `(JᵀWJ)⁻¹·χ²/(n − p)`,
  - after the centroid fallback, from the windowed-moment error, as SExtractor `ERRX2WIN` computes it.
  Registration needs a σ for every star, so no star leaves without one.
- The SNR comes from `CcdNoise` (S2) with the sky-estimate term `n_pix(1 + n_pix/n_B)` (12.2) and the threshold's floor (12.3). Read noise is counted once (12.1). The variance floor is the S3 rule (2.3).
- Shape metrics follow DAOFIND and photutils: `roundness1` from the pinwheel quadrant sum (10.1), `roundness2` from 1-D Gaussian fits to the marginals (10.2), and sharpness from the star's own peak (10.3). `max_fwhm_deviation` multiplies 1.4826·MAD (10.4).
- A non-finite flux or SNR makes the star invalid, and `validate_catalog` checks every float field (11.10).

## C5. Registration: hypothesis, then final fit

Closes groups 3 and 13, 21.1, 21.4, 22.1, 26.17 and 26.18. It uses S6 and C4.

1. **Hypothesis:** triangle matching and RANSAC on the brightest `max_stars`, as today.
   - `max_rotation` defaults to `None` (3.1).
   - `RansacConfig::seed` is a plain `u64` with a fixed default (22.1).
   - LO-RANSAC accepts a refit on score only (13.7). The degeneracy tests are relative and test every triplet (13.8).
   - Triangle flatness is one relative test, area / longest² ≥ c (13.9). A near-isosceles triangle is matched in both vertex orders (13.10).
2. **Final fit** (`registration/final_fit/`), from the hypothesis transform:
   - It matches the full catalogs with the k-d tree, without `SATURATED` stars (13.1, 13.4).
   - It weights each pair by `1/(σ_ref² + σ_target²)` from `position_sigma`.
   - It runs `Irls` (S6) with a Cauchy loss and a gate that shrinks with the scale (13.2).
   - It solves the linear and SIP terms together with `Lstsq` (S6), with the SIP origin at the image centre (13.6, 26.17, 26.18). Homography gets `LmController` on the reprojection error (13.5).
3. **Model choice:** GRIC (Torr 1998) on the same final matched set, with the residuals in units of `position_sigma` (13.3).
4. `AlignStackResult` keeps each frame's registration (21.4).
5. `Reference::Auto` picks the lowest median FWHM among frames with enough stars, with ties to the lowest index (21.1).

## C6. Run resources

Closes 15.1, 15.2 and 15.4 to 15.7. 15.9 closes in C7.

- `RunMemory::read` takes `min(MemAvailable, cgroup limit − usage)` from `sysinfo::System::cgroup_limits` (15.2).
- `DecodeCache` (persistent, content-keyed, only with `keep_cache`) and `RunScratch` (always private) replace one directory with two rules.
- Every `RunScratch` file is deleted while it is open, on every platform:
  - Unix unlinks the file after it is mapped.
  - Windows opens it with `FILE_FLAG_DELETE_ON_CLOSE` and `FILE_SHARE_DELETE` through `std::os::windows::fs::OpenOptionsExt`.
  A crash leaves nothing, and the marker file and the pid scan go away (15.6, 15.7). The plan must test this on the macOS laptop and on Windows before the marker code is removed.
- The default root is disk-backed: `$XDG_CACHE_HOME/lumos`, then `~/.cache/lumos`, then `/var/tmp/lumos`. On Linux, a `tmpfs` root from `/proc/self/mountinfo` is refused with an error that names the path (15.1).
- `StoredImage` gives its planes as slices of the map, and the warp reads them directly. A dropped frame frees its disk space at once (15.5).
- The warp buffers drop before the combine starts (15.4).

## C7. One RAW path and one FITS path

Closes group 16 except 16.12, and 17.4 to 17.7 (17.1 to 17.3 close in phase 0). It uses S1 and S2.

- The preview is `load_raw_cfa → CfaImage::demosaic → clamp` (16.1, 16.2).
- `BlackLevel` keeps integer ADU as its only source. LibRaw's unrounded values are used where they exist (16.10). One pass computes `(v − black_row[x]) / span` (16.8). File values use checked arithmetic (16.9).
- Both demosaics run on white-balanced CFA data and divide the balance out after (16.11). RCD writes into the caller's planes (16.3).
- The LibRaw fallback sets `adjust_maximum_thr = 0`, `user_flip = 0` and `use_fuji_rotate = 0` (16.4, 16.5). SuperCCD and an unexpected `raw_pitch` are refused (16.6, 16.7).
- `RAW_EXTENSIONS` takes LibRaw's list after `zero_is_bad` sets `NO_DATA` (17.4). `as_shot_wb_applied` is read (17.5).
- `read_cfa_hdu` merges into the main FITS path with the caller's `LoadContext` (17.7). One alias table maps every keyword to its field (17.6). The checksum accumulates per chunk in the decode (15.9). The `LUMFLAGS` extension is read and written.

# Implementation plan

Each phase builds and passes the verification chain on its own. A phase closes its items, and those items are then deleted from this file, together with the phase. Phase order follows the dependencies. Each phase adds its stage to the S9 harness.

## Phase 3. Pixel flags and run report (S1, S8)

0. Add `RunReport` to `StackProduct` and `AlignStackResult`. Its first entries are the flag counts of this phase.
1. Done: `PixelFlags` replaced `NullMask` (`NO_DATA`), drizzle skips flagged pixels, and RAW `zero_is_bad` zeros are flagged through a `libraw-sys` shim.
2. Done: the RAW decoder flags `SATURATED` per channel from `linear_max` or `maximum`, a FITS `DATAMAX` flags it too, `ImageMetadata::saturation_flagged` records which, the demosaic dilates flags by its measured reach of 10, and detection reads the flags or tests each input channel.
3. Carry the flags through calibration, the warp, the kept decode cache and FITS. A warped frame keeps `saturation_flagged` from its source today but no flags; the warp must carry them.
4. Read them in the combine (exclusion and survivor floor) and in drizzle. Remove the f32 planes of `for_unwarped`.
- **Tests:**
  - A star clipped at the raw limit in G only is flagged. The flag survives dark subtraction and flat division.
  - One flagged pixel in a Lanczos-3 warp with a shift of (0.5, 0.5) flags exactly 6 × 6 = 36 output pixels. At half-pixel phase none of the 6 taps per axis is zero.
  - 10 frames, 3 of them saturated at a pixel: the output is the mean of the other 7. All 10 saturated: the output pixel is flagged `SATURATED`.
  - A frame with a NaN border drizzles to the same output as the frame cropped. A Panasonic zero is `NO_DATA`, not `−black/span`.
- **Closes:** nothing left beyond the steps above (8.1, 8.3 and 9.3 are closed).

## Phase 4. Spread, sorted window and rejection driver (S3, S4, C1 gather)

1. Add `Spread` with its floor. Port `sigma_clip_iteration`.
2. Add the sorting network over `F32x8` lanes and `SortedWindow`. Bench it against today's shortcut and sort before the methods move.
3. Write the driver. Port sigma clip, winsorized, GESD, trim and linear fit, in that order. Add `RejectionScale::CcdModel`.
4. Add `frame_ids` to the gather. Remove `frame_indices_are_stable`.
- **Tests:**
  - Review example 1.2, `{−0.1, −0.05, 0, 0, 0, 0, 0.05, 0.1, 10, 10}` at k = 2.5 with no frame noise. Pass 1: median 0, MAD 0.05, σ 0.0741, band ±0.185, so both 10s go. Pass 2: MAD (0 + 0.05)/2 = 0.025, σ 0.0371, band ±0.0927, so ±0.1 go. Pass 3: MAD 0, constant window, stop. Exactly 6 survivors.
  - 20 integer bias frames with a measured background of 0.7 ADU and one hit at +50 ADU: the floor is 0.7 ADU, so the band is ±1.75 ADU. The survivors are exactly the samples within 1.75 ADU of the median, which a hand count on the fixed data gives. Today MAD is 0 on this data and the hit survives.
  - A hot pixel at 1e6 among samples of σ 1e-3 is rejected (it cannot raise the floor).
  - Review example 1.1 scaled by 2e-5 gives the same survivors (S9).
  - Winsorized, 10 frames, 3 outliers at +10σ: all 3 rejected.
  - Linear fit on fixed-seed Gaussian data at k = 3 for N = 20, 50 and 200: the rejected fraction stays in the 99.9% binomial interval around 0.27%. That interval is the tolerance, and it states why it exists.
  - GESD, n = 20, 3 outliers at 10σ: all 3 rejected (cap 6). Trim of 42% from 150: 63 low samples go, not 62.
  - No method leaves fewer than `min_survivors`.
  - The network sorts every permutation of 8 values correctly (40320 cases), and it matches the scalar sort on random lanes with empty slots.
- **Bench:** the combine bench before and after. Record both numbers here.
- **Closes:** group 1, 2.1, 2.8, 5.6, 24.17, part of 26.20.

## Phase 5. Lattice, noise estimation, mesh, weights and variance (S5, S10, C1)

0. Add `FrameNoise` and `CcdNoise` (S2): the background term is the noise this phase measures.
1. Add `CfaLattice`, and move `SameColorMedian`, the cosmic-ray detectors and the flat normalization onto it.
2. Add the two noise estimators and `FrameStats` background noise.
3. Give `background_mesh` the lattice, the flags, bad-tile interpolation and the sliver merge. Remove `DarkBackground`. Move the cosmic-ray background onto the mesh.
4. Make the weights per channel and not normalized. Add the variance and dispersion planes. Bring drizzle onto the same formula.
5. Move denoise to the B3 constants and the variance plane.
- **Tests:**
  - MRS on white noise of σ 0.01 plus a ramp from 0 to 1: within 3 standard errors of 0.01. The MAD of the ramp alone is 0.25 (median 0.5, deviations uniform on [0, 0.5]), so MAD gives 1.4826·0.25 = 0.37, 37× too large.
  - A Bayer master with σ 0.01, 0.02, 0.02 and 0.03 per colour: each estimate within 3 standard errors.
  - Variance with equal weights, σ 1 and 2: (1 + 4)/4 = 1.25. Inverse-variance weights for σ 1 and 2 are 1 and 0.25: (1 + 0.0625·4)/1.25² = 0.8 = 1/(1 + 0.25).
  - A frame with a bad blue channel loses weight in blue only.
  - A fully masked tile takes the interpolation of its neighbours.
- **Closes:** group 6, group 7 except the parts of S1, 2.2, 2.7, 18.3 to 18.6, 26.3, 26.5.

## Phase 6. Ingest and calibration plan (S7, C2)

1. Write the ingest stage with `FrameSource`, `FrameOp` and `FrameRecord`. Move the four entry points onto it, one at a time. Each move is a refactor, so its output must be bit-identical to the output before it, on the existing fixtures.
2. Add the typed masters and `CalibrationPlan`, and record the RAW exposure.
3. Build flat masters with `Subtract`. Reduce `CalibrationMasters`, and update the bundle format.
4. Add the `Reference::Index` single-write path.
- **Tests:**
  - Review example 4.1: offset 0.02, flats at 0.5·f and 0.25·f. The corner/centre ratio of the master is 0.500 exactly. Today it is 0.509.
  - A flat with no subtractor is refused. A 120 s dark on 300 s lights is refused without a bias, and it is scaled by 2.5 when it is bias-removed.
  - A DSLR light and dark with no temperature: the plan runs, and the report holds one "unverified temperature".
  - A plan for 30 resident RGB 24 MP frames on a machine with room for them stays in RAM.
- **Closes:** 4.1 to 4.3, 4.5, 15.3, 21.2, 21.3, 26.1, 26.2, 26.14, 26.15, 26.21.

## Phase 7. Detection planes (C3)

1. Return `PreparedFrame`. Measure on `measure`, and threshold and deblend on `detect`.
2. Rewrite the multi-threshold walk. Move the `max_area` filter before deblending, and pool the grids.
- **Tests:**
  - A Gaussian star of FWHM 2 on a demosaiced frame keeps 100% of its flux. Today 56% remains.
  - On pure noise filtered to FWHM 4, the fraction of pixels above 4σ is 3.2e-5 within its binomial interval. Today it is about 5%.
  - Review example 9.6, `root(100) → [A(80) → [A1(35), A2(30)], B(5)]` at contrast 0.2, returns {A1, A2}.
  - Detections are invariant under x·s + o, and under a 180° rotation on a tile-aligned frame (S9).
- **Closes:** group 9 except 9.3, 8.2, 15.8, 26.11, 26.12.

## Phase 8. Numerics kit and measurement (S6, C4)

1. Add `LmController`, `Lstsq` and `Irls`. Move the centroid fits, the SIP fit and the background extraction onto them.
2. Add `MeasureGrid`, the converged centroid and the integrated models. Add `position_sigma` with both of its sources.
3. Move the SNR onto `CcdNoise`. Add the DAOFIND shape metrics.
- **Tests:**
  - Review table 11.1 on noise-free stars: the bias at a 0.4 px start for FWHM 3, 4.5 and 6 is below 1e-4 px.
  - An integrated fit of a star with FWHM 1.2 recovers 1.2 within the fit's σ. Today the bias is +16%.
  - Round stars of FWHM 2 at uniform sub-pixel phase: the `NotRound` rate is at the noise rate, not 57%.
  - Both sources of `position_sigma` agree with the scatter of 1000 fixed-seed noise draws, within the standard error of a variance from 1000 samples.
  - `Lstsq` gives the same SIP and background solutions as today's code on the existing fixtures.
- **Closes:** groups 10, 11 and 12, 2.3 to 2.6, 26.6 to 26.10, 24.12, 24.13.

## Phase 9. Registration final fit (C5)

1. Remove the rotation prior and the random seed. Fix LO acceptance and the degeneracy tests.
2. Add `final_fit` and the homography LM. Add GRIC. Return each frame's registration.
- **Tests:**
  - A 180° rotated catalog registers.
  - 50 true matches and one 4 px blend: Auto stays at Euclidean. Today the RMS is 0.57 px, and Auto goes to Affine.
  - 2000 stars with a known transform: the scatter of the fitted parameters over fixed-seed draws matches the Cramér–Rao bound from the weighted Fisher information, within the Monte Carlo standard error.
  - Two runs with the default config give bit-identical stacks.
- **Closes:** groups 3 and 13, 21.1, 21.4, 22.1, 26.17, 26.18.

## Phase 10. Resampling (group 14)

- Add the Lanczos ringing clamp (PixInsight default 0.3). Stretch the kernel by `1/scale` when the warp's smallest singular value is below 1. For the masked path and the edge band, use the normalized valid-tap Lanczos, with a bilinear fallback below a stated tap sum.
- Compute the tap weights once per pixel for all channels (24.4). Collapse SIP per row with Horner (24.5). Build the monomials once per Newton step (24.6).
- **Tests:** a bright single pixel keeps its undershoot within the clamp. A 0.5× warp of a Nyquist grating gives no alias above the noise. SIMD and scalar stay bit-identical.
- **Closes:** group 14, 24.4 to 24.6.

## Phase 11. Run resources (C6)

1. Read the cgroup limit.
2. Split `DecodeCache` and `RunScratch`. Add delete-while-open, the disk-backed default and the tmpfs check.
3. Let the warp read stored planes in place. Drop the warp buffers before the combine.
- **Tests:** two concurrent `keep_cache` runs never open each other's scratch files. The scratch directory is empty while a run is in progress, on Linux, on the macOS laptop and on Windows. A `tmpfs` root from a fixture `mountinfo` is refused.
- **Closes:** 15.1, 15.2, 15.4 to 15.7.

## Phase 12. RAW and FITS paths (C7)

1. Replace the preview path, and remove the list in 16.2.
2. Add the integer `BlackLevel`, the one-pass normalize and checked file arithmetic.
3. Add the white-balanced demosaic and the LibRaw fallback settings. Refuse SuperCCD and odd pitch. Widen the extension list.
4. Merge the FITS entry points. Add the alias table and the streamed checksum.
5. Add the RCD golden cross-check (16.13).
- **Tests:** preview and science agree pixel for pixel in the former margin band. A Canon black level with `cblack` is exact against a hand sum in integers. A portrait frame through the fallback keeps W×H.
- **Closes:** group 16 except 16.12, 15.9, 17.4 to 17.7.

## Phase 13. Display operations

- Subtract the black point before the colour ratio (19.1). Use the log-domain base for HDR (19.2), with a bounded ratio (19.3). Use the standard STF constants (19.4). Use one NaN policy in SIMD and scalar (19.6). Bound the ML tile stride (19.7). Use one `subsample_plane` helper (26.23).
- **Tests:** faint Hα (0.055, 0.05, 0.05) above a sky of 0.05 keeps its hue after the stretch. The HDR halo pixel stays above 0.
- **Closes:** 19.1 to 19.4, 19.6, 19.7, 26.23.

## Phase 14. Remaining items

- Determinism: fixed-chunk parallel sums (22.2), a sorted flat vote `Vec` (22.3), the error of the lowest slot index (22.4).
- Cosmic rays and defects: 18.1 and 18.2.
- Performance: the rest of group 24, each with a bench before and after.
- Docs: the rest of group 25. A doc that a phase above rewrites is fixed in that phase.
- Style: the rest of group 26. Dependencies: group 27.
- Markesteijn 3-pass as an option (16.12). SCNR Maximum Neutral, Maximum Mask and an Average Neutral amount (19.8).
- **Closes:** 16.12, 18.1, 18.2, 19.8, 22.2 to 22.4, the rest of groups 24 to 27. 20.3 stays open until CFA drizzle enters the scope.

# Decisions

Confirmed on 2026-10-03.

1. **Linear fit (phase 4):** normal-score regression.
2. **`min_survivors` default (phase 3):** 3.
3. **Flagged samples in the combine (phase 3):** excluded while `min_survivors` unflagged samples remain.
4. **Flag storage (phase 3):** one byte plane.
5. **Dark scaling (phase 6):** exposure-ratio scaling only, for a bias-removed dark.
6. **Scope:** Markesteijn 3-pass as an option (16.12), and SCNR Maximum Neutral, Maximum Mask and an Average Neutral amount (19.8) are in scope. CFA drizzle (20.3) waits.

No phase needs a new dependency. `statrs` gives `Φ⁻¹` and `erf`. `sysinfo` gives the cgroup limits. `std` gives the Windows delete-on-close flags. `/proc/self/mountinfo` gives the file system type.
