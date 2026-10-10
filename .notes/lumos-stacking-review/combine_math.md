# Combine engine review: per-pixel statistics, weighting, normalization

Scope: `lumos/src/combine/{rejection,normalization,stack,config,pixel_coverage.rs}`,
`combine/cache/{frame_weights,sample_noise,sample}.rs`, the gather loop in `combine/cache/mod.rs`,
`stack_product/`, and `math/{statistics,noise,sum}` as the combine uses them.
`cargo test -p lumos --tests --features ml combine::` gives 117 passed, 2.3 s.

The Monte Carlo figures below come from pure-Python reimplementations of the lumos algorithms:
the same consistency table, floors and termination. The scripts are in
`scratchpad/sim/{floor.py,wins.py,mad.py}`.

---

## Findings

### CMB-3: The variance plane ignores flat-field noise amplification
- **Where**: `math/noise/ccd_noise.rs:6-27`, `combine/cache/sample_noise.rs:229-266`, `combine/cache/sample.rs:148-185`. No flat term exists anywhere in the noise model; `rg` finds no flat or vignetting factor reaching `CcdNoise` or the confidence planes.
- **Category**: precision (science product)
- **Impact**: medium-high for the "measurable master" goal. After division by a normalized flat `f(p)`, a calibrated pixel has a read and dark variance of `σ_r²/f²` and a source and sky term of `x/(e·f)`. The model uses one global MRS σ and one global electrons per unit. In corners at `f = 0.5`, the variance plane under-reports by a factor of 2 (sky-limited) to 4 (read-limited); near the centre it over-reports slightly. The `dispersion` cross-check will flag this as model error, but the variance plane is the published error bar.
- **Confidence**: likely. The absence is confirmed; the magnitude is analytical.
- **Evidence**: ccdproc propagates uncertainty through flat division (`.tmp/ccdproc/ccdproc/core.py` `flat_correct`, which calls `CCDData.divide` with NDData uncertainty arithmetic).
- **Direction**: carry the master flat's per-pixel gain into the per-sample noise model at calibration time. One option is a per-pixel variance factor that rides with the frame as confidence does, warped with it. The background term then becomes `σ_bg²/f²`, measured on the uncalibrated scale, and the photon term `1/(e·f)`. The floor (CMB-2) and the CcdModel clip become field-correct too.

### CMB-4: CFA mosaics are normalized with one affine for all colours, while noise and weights are per colour
- **Where**: `combine/normalization/mod.rs:222-277` (`source_medians` / `multiplicative_norms` on the mosaic's single channel), `:312-397` (`global_norms`), `combine/cache/slots.rs:36-38` (`Slots::channel` maps every colour to channel 0 for norms), `combine/cache/mod.rs:486-489`
- **Category**: correctness
- **Impact**: medium. The flat master preset (`StackConfig::flat()`, multiplicative) divides each flat by its whole-mosaic median. When the illumination colour drifts between flats, as twilight sky flats do through dusk, each colour's samples land at different levels per frame. Rejection sees that drift as scatter and clips real frames. The mean is a mixture with per-colour gains no one measured. `stack_cfa_master` takes any `StackConfig`, so `Global` on a mosaic fits one gain across colours.
  - The stratified sample also aliases with the CFA on some sizes. On a 4096×4096 mosaic, `k·n/m = 256k` selects only even x, so only R and G2 for RGGB (`normalization/mod.rs:458-460`).
- **Confidence**: confirmed in code; the field effect is analytical. No normalization test covers a CFA set.
- **Evidence**: `SampleNoise` and `FrameWeights` are already per slot (colour), while `FrameNorm.channels` is per channel. For mosaics the two disagree.
- **Direction**: index `FrameNorm` by slot (colour) as noise and weights already are. Measure each colour's median, and for Global each colour's gain, over that colour's photosites, and stratify the sampled indices per colour.

### CMB-6: The gather loop branches per sample and walks up to 4N plane streams per pixel
- **Where**: `combine/cache/mod.rs:469-511`
- **Category**: performance
- **Impact**: medium. This loop sets the worst-case row time. Per sample it tests six loop-invariant `Option`s (`coverage`, `confidence`, `frame_norms`, `weights`, `flags`, `noise`). It also calls `noise.model(frame, slot)` and computes `background/q` and `1/(electrons·q)`, a division, for every sample. The pixel-outer, frame-inner order touches one cache line in each of N image planes, plus N coverage, N confidence and N flag planes, per 16 pixels. Beyond about 32 streams the L2 prefetcher stops tracking them, so with spilled (mmap) frames each new line is a demand miss. Nothing in the hot loop is vectorized except the f64 weighted sum.
- **Confidence**: likely. Read from the code; `bench_stack_300` now measures large N and a ragged count.
- **Evidence**: Siril has the same per-pixel strided gather (`stacking/stacking.c` / `median_and_mean.c`: per-frame row blocks, then `stack[frame] = pix[frame][x]`), so lumos is no worse than the reference. Still, this loop dominates for large N.
- **Direction**: for each row tile of 64–256 px, loop frames on the outside. Apply gain and offset with the `simd::Isa` kernels into a `[tile][frame]` transposed buffer, together with coverage mask bits, effective weights and per-sample noise terms. Then reduce each pixel from contiguous memory. Hoist the invariant `Option`s by monomorphizing, and precompute per frame and slot `1/electrons` so only one multiply by `1/q` remains.

### CMB-7: The master quantization σ costs an O(survivors) pass per pixel to produce one worst-pixel scalar
- **Where**: `combine/stack/mod.rs:341-392`, `combine/stack/quantization.rs:42-56,101-135`
- **Category**: performance / design
- **Impact**: low-medium. Whenever every frame declares a quantization σ (all RAW input, and calibration masters even with `IMAGE_ONLY`), every pixel with any rejection or partial coverage pays a second loop over its survivors: a `norms[i].channels[c].gain` lookup, a `powi`, and a guarded atomic. The figure kept is the maximum over pixels. On any registered stack that is the edge pixel with 1 to `min_survivors` frames, which is close to a single frame's σ whatever the rest of the field did.
- **Confidence**: confirmed in code.
- **Direction**: decide whether a worst-pixel scalar is the product wanted. If it is, gate the per-pixel work on `survivor_count ≤` a running minimum (an atomic min). For equal-weight, equal-σ sets the figure is `σ/√m_min` exactly, so the loop can be dropped there.

### CMB-8: The coverage plane is a second full pass that re-reads every frame's coverage plane
- **Where**: `combine/cache/mod.rs:254-300` (`finish_product`) against `:470-511`, where the gather already counts `covered` with the same `PixelCoverage` rule
- **Category**: performance / simplification
- **Impact**: medium for spilled tiers. N more plane reads through mmap after the combine, plus a filter and count over N per pixel, for a number the gather already computed.
- **Confidence**: confirmed in code.
- **Direction**: write `covered / frame_count` from the gather on the first channel's pass (coverage is shared across channels), and drop the second walk and `coverage_layout`.

### CMB-9: Pixels with no weight report variance 0, and zero-weight survivors give a value of 0
- **Where**: `combine/cache/sample.rs:174-178,188-196`, `math/sum/mod.rs:93-97`, `combine/config/mod.rs:330-341`
- **Category**: correctness (science product)
- **Impact**: medium. Variance 0 claims an exact pixel, so an inverse-variance consumer gives it infinite weight. That is what an uncovered pixel gets, and also a pixel whose survivors all carry zero weight. Zero weights are reachable: validation allows individual `Manual` weights of 0. At a registered edge covered only by zero-weight frames, or where rejection removed every positive-weight sample, `weighted_mean_f32` returns 0.0 while coverage reports the pixel as covered. That is silently wrong.
- **Confidence**: confirmed in code.
- **Direction**: write `+∞` or NaN to the variance plane where `Σw = 0`, or publish inverse variance, where 0 is the natural "no information". For the value, either reject `Manual` weights of 0 at validation (dropping the frame is the honest form) or fall back to the unweighted mean of the survivors and flag the pixel.

### CMB-10: The median path computes a dispersion on value/weight pairs it has permuted apart
- **Where**: `combine/stack/mod.rs:311-325`
- **Category**: design (latent bug) / performance
- **Impact**: low. `median_mut(samples.values)` reorders `values` in place, then `from_survivors(value, samples.values, samples.weights, 0..count, None)` pairs `values[i]` with an unrelated `weights[i]`. The dispersion is computed and then discarded, because `QualityPlanes::resolve` drops it for a median. The extra O(N) pass is wasted, and any later use of the pairs is wrong.
- **Confidence**: confirmed in code.
- **Direction**: for a median, write only `Σw` (order-independent) and do not call `from_survivors`.

### CMB-11: GESD records statistics on the hot path that only a test reads
- **Where**: `combine/rejection/gesd_config.rs:104`, `combine/rejection/scratch_buffers.rs:17`. The only reader is `rejection/tests.rs:737`.
- **Category**: dead-code
- **Impact**: low. A per-removal push into a scratch vector for every pixel, kept for one test.
- **Confidence**: confirmed in code.
- **Direction**: have the NIST test recompute the statistics through a gated helper beside `GesdConfig`, and remove `statistics` from production scratch.

### CMB-12: Rejection config API and validation are inconsistent
- **Where**: `combine/rejection/sigma_clip_config.rs:30-41`, `linear_fit_clip_config.rs:43-59`, `winsorized_clip_config.rs:106-116`, `gesd_config.rs:131-144`
- **Category**: design
- **Impact**: low, but a caller can silently get a no-op method.
  - `SigmaClipConfig::new(sigma, iters)` and `WinsorizedClipConfig::new(sigma)` are symmetric, while `LinearFitClipConfig::new(low, high, iters)` is asymmetric.
  - `max_iterations` counts all passes for sigma clip but only the fitted passes after the first for linear fit (`passes() = 1 + max_iterations`).
  - GESD accepts `alpha = 0`, whose critical value is the Samuelson bound `(L−1)/√L`, so it never rejects. It also accepts `max_outliers: Some(0)`. Both disable the method silently.
  - The GESD error field reads "GESD alpha", while the other configs use the field name.
- **Confidence**: confirmed in code.
- **Direction**: one constructor shape, `new(SigmaBounds, max_passes)`, with one meaning of passes. Validate `alpha ∈ (0, 1)` and `max_outliers ≥ 1`.

### CMB-17: Winsorized does two passes per clamp step
- **Where**: `combine/rejection/winsorized_clip_config.rs` (`estimate`)
- **Category**: performance
- **Impact**: low. Each clamp step walks the copy twice: once for the mean, once for the squared deviations, up to 50 steps × N per pixel. Winsorized now clips once, so the earlier N-pass worst case is gone.
- **Confidence**: confirmed in code.
- **Direction**: fuse each step into one f64 pass (shifted sum and sum of squares about the previous centre).
---

## Checked and found OK
- **GESD** (`gesd_config.rs`):
  - Rosner's `λ = (L−1)t/√((L−2+t²)L)` with `t` at `1−α/(2L)` and `L−2` degrees of freedom, rearranged correctly as `(L−1)/√(L(1+(L−2)/t²))`.
  - The statistic uses the live sample SD with `L−1` in the denominator.
  - "Last passing removal" selects the outliers correctly; the reverse Welford update is correct.
  - The cap `min(r, n−min_survivors, n−2)` keeps `L ≥ 3`.
  - The default r = 30% matches Siril (`rejection_float.c:319-357`).
  - The NIST handbook example and the α false-positive rate are tested.
- **Winsorized constant**: the clamped-Gaussian variance at c = 1.5 is `0.866386 − 0.388553 + 0.300632 = 0.778465`, and `1/√v = 1.133393`, which matches the constant. Convergence at 0.0005 matches Siril. The robust (MAD) start is a sound improvement over Siril's plain-SD start.
- **MAD small-sample factors `b_n`**: rechecked by an independent simulation (3·10⁵ trials) for n = 3, 5, 6, 7, 9, giving 1.4896, 1.2172, 1.1903, 1.1378, 1.1008 against the table's 1.4869, 1.2170, 1.1895, 1.1378, 1.1011, all within simulation error. `MAD_TO_SIGMA = 1/Φ⁻¹(3/4)` is correct. The even-n midpoint MAD merge is tested against a sort.
- **Linear fit**:
  - Blom scores `Φ⁻¹((i−3/8)/(n+1/4))`.
  - The censored OLS on full-count scores is a sound censored-sample estimator (Gupta's simplified linear estimator).
  - Removing a true outlier and treating it as a censored tail biases σ up, toward keeping samples, never into a cascade.
- **Deming slope**: `β = (Δ + √(Δ²+4λs_xy²))/(2s_xy)` with `Δ = s_yy − λs_xx`, and the rationalized branch `2λs_xy/(√… − Δ)` for Δ < 0 is algebraically identical. The offset from the medians is exact under affine maps.
- **Weighted mean**:
  - Accumulates in f64 with exact f32×f32 products; it rounds once.
  - The weights are normalized by `Σw`.
  - Rejection is unweighted, which matches PixInsight, Siril and DSS and keeps GESD's critical values valid.
- **Variance propagation**:
  - `Σw²v/(Σw)²` is correct, with `v` taken at the combined value.
  - The noise model maps through normalization correctly: variance × g², sky·g + o, electrons ÷ g.
  - The warp confidence `q` divides the variance and multiplies the weight, so with noise weighting `wᵢvᵢ = 1` and the variance is `1/Σw`, consistent with the weight plane.
- **Dispersion**: `Σwᵢ(xᵢ−x̄)²/((n−1)Σw)` is unbiased when `wᵢ ∝ 1/σᵢ²`. Derivation: `E = Σwᵢσᵢ² − Σw·Var(x̄) = (n−1)c`.
- **Noise weighting**: `1/(g·σ)²` per slot, matching PixInsight's noise-evaluation weight of the scaled MRS noise. The MRS `LAYER_SIGMA` (0.889, 0.200, 0.086, 0.041) matches Starck & Murtagh's B3 table.
- **Quantization σ formulas**: the mean `√Σ(wgσ)²/Σw` is correct. The median factor is the variance of the middle uniform order statistic(s) relative to `Δ²/12`: `3/(n+2)` for odd n and `3n/((n+1)(n+2))` for even n.
- **Sort**: one sort per pixel on u64 keys that order as the floats, with gather positions in the low bits. Later passes only narrow a window. This beats Siril, which runs `quickmedian` per iteration and re-sorts per linear-fit iteration. The standard library's `sort_unstable` already uses small-sort networks for small N.
- **Survivor rule**:
  - `nearest` keeps the `min_survivors` samples nearest the pass centre.
  - Every method's pass loop terminates: each non-settling pass strictly narrows the window.
  - `values.len() ≤ min_survivors` skips rejection.
- **Trim counts**: exact `⌊p·n/100⌋` in f64.
- **Normalization reference choice**: the lowest noise in a shared domain. **Common domain**: uses the same `PixelCoverage` rule as the gather.
- **`MaxSigma`**: an atomic max on f32 bits is correct for non-negative values.

## Suggested batches
1. **Science planes**: CMB-3 (flat factor in the noise model; touches calibration → confidence plumbing), CMB-9 (variance and value at zero weight), CMB-10.
2. **CFA-aware normalization**: CMB-4 (per-slot `FrameNorm`, per-colour medians, gains and stratification).
3. **Hot-loop performance**: CMB-6 (tile-transposed SIMD gather), CMB-8 (coverage from the gather), CMB-7 (quantization gate), CMB-17. Measure with `bench_stack_300`.
4. **Cleanups**: CMB-11, CMB-12.
