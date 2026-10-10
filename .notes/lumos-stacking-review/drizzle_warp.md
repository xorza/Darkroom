# Review: drizzle and warp/resample (lumos)

Scope: `lumos/src/drizzle/**`, `lumos/src/registration/resample/**`, `lumos/src/math/lanczos`, and how warped
frames reach the combine (`pipeline/align.rs`, `pipeline/frame_tier.rs`, `combine/cache/mod.rs` gather).
References: Siril's STScI cdrizzle port (`.tmp/siril/src/drizzle/cdrizzlebox.c`), Siril's OpenCV warp
(`.tmp/siril/src/opencv/opencv.cpp`), PixInsight PCL `LanczosInterpolation` / DrizzleIntegration (documented
behaviour), DrizzlePac (documented behaviour).
`cargo test -p lumos --tests --features ml drizzle`: 49 passed.

Paths below are relative to `/home/xxorza/Projects/darkroom/lumos/src/`.

---

## Findings

### DRZ-1 — Drizzle has no outlier rejection and no frame normalization
- **Where:** `drizzle/accumulator/frame_source.rs:214` (`EXCLUDED`), `drizzle/accumulator/frame_source.rs:418` (`deposit_weight`), `drizzle/stack.rs:72` / `:100` (entry points), `drizzle/accumulator/mod.rs:176` (`add_frame`)
- **Category:** correctness
- **Impact:** high. A satellite trail, plane, unflagged cosmic ray or hot pixel in one frame goes into the master at full weight. Frames with different sky levels or transparency are averaged raw, so seams appear wherever the frames' footprints differ.
- **Confidence:** confirmed
- **Evidence:** A drop is withheld only for a zero weight or for one of the flags `NO_DATA | COSMIC_RAY | REPAIRED`. Nothing compares a sample against the other frames, and no `FrameNorm` gain or offset is applied. Frame weights come from the caller (`DrizzleFrame::weight`), and no pipeline path carries registration, normalization, noise weighting or rejection into a drizzle: there is no caller outside `drizzle/` (expected by scope, but the path does not exist either). The reference tools all reject before or during drizzle:
  - Siril drizzles at registration (`registration/applyreg.c`, `driz->scale = regargs->output_scale`) and then stacks the drizzled frames through its normal normalization and rejection.
  - PixInsight DrizzleIntegration reads ImageIntegration's normalization and rejection data (`.xdrz`).
  - DrizzlePac runs median, then blot, then `driz_cr`, before the final drizzle.
- **Direction:** Feed drizzle the statistical combine's results. Run the regular warp+combine first, which already normalizes and rejects. Map each frame's rejected samples back to input pixels (blot the per-frame rejection mask, or use a `driz_cr`-style test of input pixel against the blotted median) and pass them as zero pixel weight or `COSMIC_RAY`. Apply the frame's normalization gain and offset at deposit, and take frame weights from the same `FrameWeights` resolution.

### DRZ-2 — Band overscan grows with field rotation and with thread count; square/SIP pay the full transform before rejection
- **Where:** `drizzle/accumulator/output_band.rs:116-145` (`scan`, `for ix in 0..width`), `drizzle/accumulator/frame_source.rs:282-310` (`quad`: 4 `map.position` per visit), `drizzle/accumulator/frame_source.rs:326-347` (`input_rows`, row extent only), `drizzle/accumulator/mod.rs:434` (bands = 4 × threads)
- **Category:** performance
- **Impact:** high. At large rotations (alt-az field rotation, a re-framed camera across nights, 90° camera turns) every band scans almost the whole input frame. Overscan work is roughly `4·T·N` pixel visits for `T` threads, so it does not shrink as threads are added. For square + SIP each visit is 4 Newton inversions of about 5 SIP evaluations each.
- **Confidence:** confirmed (code path). The cost estimate is reasoned from the code and not measured.
- **Evidence:** A band is a full-width strip of output rows. `input_rows` bounds only the input *rows* it maps back to, and `scan` then walks every column of each row. The bench header says a rotated band takes in `h·cosθ + W·sinθ` rows, "nearly all of it rejected", but the bench measures only 1° (`drizzle/bench.rs:47`). At 90° the scan covers the whole frame for every band.
  - `scan` calls `drop(pixel)` *before* the band test (`output_band.rs:131`). For the square kernel that is 4 corner maps plus an area computation. For SIP it is 4 Newton solves (`registration/transform/inverse_warp.rs:67-90`).
  - At pixfrac = 1 each corner is shared by 4 pixels, so it is computed 4 times.
  - The "~0.9 ns per rejected visit" figure in the `input_rows` doc holds only for a cheap affine droplet.
  - STScI/Siril compute the pixmap once per frame and get the square kernel's corners from it (`cdrizzlebox.c:449`, `:1022` `interpolate_point`).
- **Direction:**
  - Restrict each scanned input row to the column interval whose drops can reach the band. For affine and homography maps that interval has a closed form; for SIP use the sampled boundary plus a margin.
  - Test the landing row before computing magnification or the other corners.
  - For the square kernel, compute the corner lattice once per input row (shared between neighbours: `W+1` points at pixfrac = 1, `2W` otherwise). For affine maps, use `centre ± J·h` instead of 4 full maps.
  - Add a 45° and a 90° leg to `bench_drizzle_kernels`, and a SIP leg.

### DRZ-3 — Interpolation confidence used as a per-pixel combine weight down-weights the reference and makes weights depend on sub-pixel phase
- **Where:** `combine/cache/mod.rs:481-496` (`eff_weights = frame weight × q`), `registration/resample/tap_window.rs:59` / `interior_window.rs:81` (`confidence` = Kish ESS), `pipeline/frame_tier.rs:172-183` (reference stored with `for_unwarped` → no confidence → `q = 1`)
- **Category:** precision / design
- **Impact:** medium. Every warped sample is weighted by the white-noise ESS of its interpolation weights, which ranges 1 to 1.62 for Lanczos3 (mean 1.27) and 1 to 4 for bilinear (mean 2.47). Consequences:
  - The unwarped reference, which `Reference::Auto` picks as the *sharpest* frame, enters at ~0.79× (Lanczos3) or ~0.41× (bilinear) the weight of an average warped frame.
  - A frame whose shift is near-integer weighs less than one shifted by half a pixel.
  - With rotation the phase beats across the field, so per-pixel weights, and with them the stack's effective PSF mix and noise, form a moiré pattern.
- **Confidence:** confirmed (code path). The ESS values are computed from the Lanczos/bilinear definitions: Lanczos3 2D ESS is 1.000 at phase 0, 1.252 at 0.25 and 1.620 at 0.5.
- **Evidence:** `q` multiplies the weight and also divides the noise model (`noise_background / q`). The noise division is right: the sample's white-noise variance really is σ²/q. Using `q` as a weight, though, prefers samples that were *smoothed more*, and the reduced variance was paid for in resolution. Siril, PixInsight and DSS weight per frame, not per interpolation phase. DrizzlePac's per-pixel overlap weights are a different mechanism (geometric share of a measurement), not a reward for smoothing.
- **Direction:** Keep `q` in the per-sample noise model (rejection, variance plane), but weight with frame-level weights only. At most, use a frame-constant mean `q` so per-pixel weights do not beat with phase. If per-pixel `q` weighting stays, give the reference its own, otherwise it is systematically discounted. Either way, state the trade-off on `WarpResult::confidence`.

### DRZ-4 — Warp flags are dilated over the whole kernel window, whatever each tap's weight
- **Where:** `registration/resample/mod.rs:178-186` (`dilate_window(reach, ..)`), `registration/resample/mod.rs:201-211` (one byte per output pixel from the dilated map), `registration/resample/kernel/warp_kernel/mod.rs:191` (`window_reach`), `combine/cache/sample.rs:10-14` (`SOFT_EXCLUDED`)
- **Category:** correctness / precision
- **Impact:** medium.
  - With Lanczos3, each flagged source pixel (`COSMIC_RAY`, `REPAIRED`, `DEFECT`, `SATURATED`, `FLAT_FLOOR`) marks 36 output pixels, and 64 once the stretch exceeds 1 by any amount (see DRZ-10). The combine then soft-excludes all of them.
  - This happens even at integer phase, where only the centre tap has non-zero weight.
  - A frame with a 0.1% defect/CR fraction loses ~3.6% of its samples.
  - Around stars saturated only in good-seeing frames, the sharp frames are excluded over a 6×6-grown halo, so core and wings come from different frame subsets.
  - The unwarped reference keeps its flags undilated, so warped frames and the reference are treated inconsistently.
- **Confidence:** confirmed (code path); the impact sizes are estimated, not measured.
- **Evidence:** The flag plane is the source flags grown by the kernel reach, then read at the sample's cell. No tap weight enters. The `NO_DATA` path already does this properly: normalized convolution over the surviving taps, with coverage taken as the share of `Σ|L|` they keep (`masked_sources.rs`, `frame_sampler/mod.rs:235-283`).
- **Direction:**
  - Treat the fill-type flags (`COSMIC_RAY`, `REPAIRED`, `DEFECT`) as `NO_DATA` inside the interpolation, through the existing masked path, so a fill never enters the sample and the loss is graded.
  - Propagate the bound-type flags (`SATURATED`, `FLAT_FLOOR`) only where the flagged taps carry a non-negligible share of `|L|`. The share can come from a second validity plane through `masked_weights`.

### DRZ-5 — No CFA (Bayer/X-Trans) drizzle
- **Where:** `drizzle/accumulator/mod.rs:40` (`DrizzleFrame<T>` is only ever `LinearImage`), `drizzle/accumulator/mod.rs:368` (`cfa_type: None`, "Drizzle takes demosaiced frames")
- **Category:** precision (missing capability)
- **Impact:** medium-high for one-shot-colour data, the main use of drizzle in amateur astrophotography. Demosaic interpolation leaves correlated noise and colour artifacts that drizzle cannot undo, and the variance plane then treats demosaic-interpolated samples as independent measurements, so it understates the variance there.
- **Confidence:** confirmed
- **Evidence:** Siril's cdrizzle deposits each photosite only into its own channel (`cdrizzlebox.c:448` `chan = FC_array(j, i, cfa, cfadim)`, in every kernel). PixInsight's DrizzleIntegration has a CFA mode, and DSS has Bayer drizzle. Lumos has a `CfaImage` type, but drizzle cannot take it.
- **Direction:**
  - Accept calibrated, un-demosaiced CFA frames.
  - Deposit each photosite only into the planes of its filter colour, with per-channel weight (and coverage) planes. `QualityMap` already has a per-plane form. The scan and kernels are unchanged except for the channel selection in `accumulate`.

### DRZ-6 — Droplet kernels divide by local magnification, unlike STScI; kernels then disagree on relative frame weights
- **Where:** `drizzle/accumulator/frame_source.rs:263-278` (`weight · grid_area / magnification`), `drizzle/tests/jacobian.rs:24`
- **Category:** design / correctness
- **Impact:** low-medium. It matters only when frames differ in plate scale (mixed optics, focal-length drift) or under strong SIP. The square drop grows with magnification and deposits `w` in total, so a frame's mean per-output-pixel weight goes as `s²/mag`. A Turbo/Point/Gaussian/Lanczos drop keeps a fixed footprint but also deposits `w·s²/mag`, so its mean per-pixel weight goes as `(s²/mag)²`. A frame at +5% linear scale gets 0.91 relative weight under Square but 0.83 under the other kernels.
- **Confidence:** confirmed (derivation from code). The test checks only a pixel where a drop lands, not the average over a period.
- **Evidence:** STScI divides by the Jacobian only in the square kernel. Siril's port, `cdrizzlebox.c`:
  - point, `:462-468`: "we DON'T scale by the Jacobian".
  - turbo, `:906-912`: `dover *= scale2 * ac`, no Jacobian.
  - square, `:1043-1047`: `w / jaco`, "to ensure conservation of weight".
- **Direction:** Choose one principle for weight per unit output area. Either drop `/magnification` for the fixed-footprint kernels (STScI), or scale their footprint by the local Jacobian. Then extend `a_magnified_frame_weighs_less_per_output_pixel` to the mean weight over one lattice period.

### DRZ-7 — Drizzle accepts non-finite samples
- **Where:** `drizzle/accumulator/mod.rs:379-418` (`validate` checks weights only), `drizzle/accumulator/frame_source.rs:249-259` (`fluxes`)
- **Category:** correctness
- **Impact:** low-medium. One NaN or ∞ in an unflagged pixel of an in-memory frame (`LinearImage::from_pixels` sets no flags) turns every output pixel its drop reaches into NaN, along with its variance. The statistical combine refuses the same input with `StackError::NonFiniteImageSample` (`combine/cache/frame_check.rs:173-192`).
- **Confidence:** confirmed
- **Direction:** Check sample finiteness in `DrizzleAccumulator::validate` the way `FrameCheck::sample_channels` does, with a new `DrizzleError` variant. The alternative is to treat a non-finite sample as `NO_DATA`. Keep the "accumulator unchanged on error" contract.

### DRZ-8 — Flag policy diverges between drizzle and combine; the drizzle product carries no flags
- **Where:** `drizzle/accumulator/frame_source.rs:214-216` (`EXCLUDED = NO_DATA|COSMIC_RAY|REPAIRED`), `combine/cache/sample.rs:10-14` (`SOFT_EXCLUDED = SATURATED|DEFECT|COSMIC_RAY|REPAIRED|FLAT_FLOOR`), `drizzle/accumulator/mod.rs:352-371` (image built with `flags: None`, `report: RunReport::default()`)
- **Category:** design / correctness
- **Impact:** low.
  - Drizzle deposits `FLAT_FLOOR` samples (under-corrected vignetting, biased low) at full weight, and also `DEFECT`-only samples (the FITS flag extension can carry `DEFECT` without `REPAIRED`).
  - Pixels at the fill value are not marked `NO_DATA`, unlike the combine's product. With `QualityPlanes::IMAGE_ONLY` they cannot be told apart from measured zeros.
  - The `RunReport` reports nothing excluded.
- **Confidence:** confirmed
- **Direction:** Define the exclusion policy once, in `pixel_flags`, and use it from both producers. Drizzle cannot apply a survivor floor, so it should exclude the full fill/bias set. Emit `NO_DATA` flags on gated pixels, and count excluded deposits into the `RunReport`.

### DRZ-9 — Nearest-entry Lanczos table read quantizes tap weights to ~1.7e-4
- **Where:** `registration/resample/kernel/mod.rs:16` (`LANCZOS_LUT_RESOLUTION = 4096`), `registration/resample/kernel/warp_kernel/mod.rs:233-237` (index `d·4096 + 0.5`, gather)
- **Category:** precision
- **Impact:** low. The worst per-tap weight error is `max|K'|/8192`: 1.37/8192 ≈ 1.7e-4 for Lanczos 2, 3 and 4. A point source's interpolated value is off by up to ~1.7e-4 of its peak per axis (simulated worst case 1.69e-4 in 1D). That is about a 1.2e-4 px position quantization, after `SourcePosition` keeps the fraction to 6e-8 specifically for precision. It is not systematic across frames, so it averages down in the stack.
- **Confidence:** confirmed (computed)
- **Direction:** Interpolate linearly between adjacent table entries: one more gather and an FMA, with error ≈ `max|K''|·h²/8` ≈ 1e-8. That makes the table no longer the precision limit. Update `a_lanczos_table_read_is_within_half_a_step_of_the_kernel` to the new bound.

### DRZ-10 — Kernel stretch has no tolerance band
- **Where:** `registration/resample/kernel/warp_kernel/mod.rs:160-177` (`frame_stretch`, `largest.max(1.0)`), `:191` (`window_reach` = `ceil(radius·stretch)`)
- **Category:** design / performance
- **Impact:** low. A frame whose largest singular value exceeds 1 by any amount, including fit noise or rounding (1 + 1e-12), leaves the `stretch == 1.0` branch, and its window reach jumps from `ceil(3) = 3` to `ceil(3.000…) = 4`. That changes the Lanczos3 flag dilation from 6×6 to 8×8 (feeds DRZ-4) and the null/clip windows likewise, while the anti-aliasing gained at stretch 1 + 1e-4 is nil. Roughly half the frames of a typical set sit on either side of 1.
- **Confidence:** confirmed (code); the frequency claim is likely.
- **Direction:** Treat a stretch within a small, justified tolerance of 1 as 1, for example where the cut-off moves by less than the table resolution. Alternatively, derive the reach from the kernel's support where it is non-negligible rather than `ceil`.

### DRZ-11 — Smaller per-drop costs in the droplet kernels
- **Where:** `drizzle/accumulator/frame_source.rs:141-159` (`landing` computes the homography Jacobian determinant, or the SIP inverse Jacobian through `InverseWarp::apply`, before any band test), `drizzle/accumulator/output_band.rs:102` (drizzle Lanczos: two `sin` and two divisions per tap, 14 taps per drop)
- **Category:** performance
- **Impact:** low. This compounds DRZ-2 for homography and SIP frames. The resample side already has an exact-enough Lanczos3 table.
- **Confidence:** confirmed
- **Direction:** Return the landing position first, reject on rows and columns, then compute magnification for drops that land. For drizzle Lanczos, reuse `LanczosOrder::Three.lut()` (interpolated, per DRZ-9).

### DRZ-12 — Layout / style
- **Where:**
  - `drizzle/accumulator/frame_source.rs:84-167`: `InputMap` / `SipMap` / `Landing` are a standalone input↔output map with their own impls.
  - `drizzle/accumulator/mod.rs:40`: public `DrizzleFrame` lives in `mod.rs`.
  - `drizzle/geometry.rs:26,93`: `pub(crate)` free fns `sgarea` / `boxer`.
  - `registration/resample/kernel/mod.rs:71`: free fn `nearest_index(size, SourcePosition)`.
  - `source_position.rs:62` and `warp_kernel/mod.rs:281`: the same truncating floor, written twice (f64 and f32).
  - `drizzle/config.rs:99`: `x2()` is `default()`.
  - `registration/registration_config/mod.rs:20-24`: Lanczos3 is documented "highest quality" and Lanczos4 "extreme quality", which contradict each other.
  - `drizzle/config.rs` `DrizzleKernel::Lanczos`: documented "Best quality", although it is restricted to s = 1, p = 1 and has negative weights.
- **Category:** style / simplification
- **Impact:** low (CLAUDE.md layout rules: one major struct per file, methods over exposed free fns)
- **Confidence:** confirmed
- **Direction:**
  - Move `InputMap` into `accumulator/input_map.rs` and `DrizzleFrame` into `accumulator/drizzle_frame.rs`.
  - Make `boxer` a method of the drop quad (`DropQuad::overlap(cell)`), with `sgarea` private.
  - Make `nearest_index` a `SourcePosition` method.
  - Keep one floor helper.
  - Drop `x2()`.
  - Fix the two doc claims.

### DRZ-13 — Output unit of drizzle is not stated on the API
- **Where:** `drizzle/config.rs:42-90`, `drizzle/drizzle_result.rs`. The convention appears only in the test doc `drizzle/tests/synthetic.rs:85-90`.
- **Category:** design (science product)
- **Impact:** low. Lumos keeps surface brightness in input-pixel units, so the image total is `s²` × the input flux. DrizzlePac's `d·scale2` convention conserves flux per output pixel instead. Photometry downstream needs to know which one it is reading.
- **Confidence:** confirmed
- **Direction:** State the unit on `DrizzleConfig::scale` / `DrizzleResult`, and how to convert to flux per output pixel (divide by `s²`).

---

## Checked and found OK

- **`sgarea` / `boxer`** (`drizzle/geometry.rs`): a faithful port of STScI `cdrizzlebox.c`, with clipping branches and `xtop` formulas matching. The 1e-14 vertical-edge threshold contributes at most 1e-14 of area. `|Σ|` is the overlap for a convex quad of either winding. Hand-value tests are present, including a turned square.
- **Square kernel weight:** `w/|area|` with the area from the diagonals' cross product, exactly STScI's `jaco`. Corners are mapped exactly through the full map, SIP included, which is more precise than STScI's pixmap interpolation.
- **Coordinate convention:** integer-centred on both sides. `o = s·p + (s−1)/2` maps footprint `[−½, w−½]` onto `[−½, s·w−½]`, consistent with the warp's footprint (`SourcePosition::within`) and with STScI. Turbo/point/radial row and column rounding at half-integers is correct, with zero-overlap boundaries excluded.
- **Pixfrac/scale semantics:** square half-drop in input pixels; Turbo drop `pixfrac·s` output pixels; Gaussian FWHM `pixfrac·s`, matching Siril's corrected `efac` (`cdrizzlebox.c:509-516`); Lanczos restricted to `s = p = 1` as in STScI. The radial neighbourhood (radius 3 around the rounded centre) covers every non-zero Lanczos3 tap.
- **Variance plane:** `Σw²v/(Σw)²` is exact per pixel for independent inputs, and contributions to one output pixel come from distinct input pixels. The closed-form test (`drizzle_quality_maps_have_their_closed_form`) checks exact values.
- **Parallel scatter:** bands own rows and process inputs in serial order, so the output is bit-identical for any band count (tested). `input_rows` is a true bound (homography horizon guard, SIP sampled outline plus 1 row), tested against a brute-force reach. The coverage bitset is cleared per frame on the workers.
- **f32 accumulators:** at ~850 deposits per pixel (500 frames, s = 2, p = 0.8, square), naive f32 summation gives ~2e-6 relative RMS error and 5e-5 worst case, below a stacked pixel's noise. STScI accumulates in float too. f64 would double ~8 output-grid planes (7.8 GB at 61 MP RGB, s = 2), which is not worth it.
- **Warp transform precision:** f64 throughout. The position is split into cell and fraction before narrowing. `RowPositions` is bit-identical to `WarpTransform::apply` (affine/homography), and SIP agrees to ~1e-15.
- **Anti-alias stretch:** the largest singular value of the output→source Jacobian (DeForest 2004 / reproject adaptive) is correctly directed and applied only when the frame is minified (apart from DRZ-10's tolerance).
- **Ringing clamp:** matches PCL's rule: lobe ratio, soft `1 − ((r−t)/(1−t))²` factor, positive-lobe mean past `r = 1`. It is invariant on a flat field, and the default threshold 0.3 is PixInsight's. Siril's alternative (fall back to a guide image below 0.98× it) is cruder.
- **Edges and nulls:** normalized convolution over surviving taps, with Kish conditioning (`(ΣL)² ≥ ΣL²`) and a bilinear fallback. Coverage is the share of `Σ|L|`, so coverage and confidence vanish together (tested). Oracle and cross-ISA bit-identity tests are thorough.
- **No Jacobian in the warp:** the surface-brightness convention matches flat-fielded data, where the flat already removes pixel-area variation. It is the same convention as the drizzle output. A global plate-scale difference is absorbed by the normalization gain.
- **Test quality:** hand-derived bounds with stated arithmetic, a flux conservation test (Σout = s²·Σin within 33ε), a linear-ramp Jacobian test, polynomial reproduction, and a Nyquist-grating aliasing test. The gaps are DRZ-6's period-average weight and a rotated (≥ 45°) or SIP performance leg (DRZ-2).

---

## Suggested batches

1. **Drizzle as a science producer** (DRZ-1, DRZ-7, DRZ-8, DRZ-13): rejection and normalization fed in from the combine, finite-sample validation, a shared flag policy plus output flags, and the unit stated on the API.
2. **Drizzle scatter performance** (DRZ-2, DRZ-11): per-row column intervals, reject before transforming fully, a shared corner lattice, and bench legs at 45°/90° and with SIP.
3. **Warp quality planes and flags** (DRZ-3, DRZ-4, DRZ-10): how confidence enters weights, weight-aware or masked flag propagation, and a tolerance on the stretch.
4. **CFA drizzle** (DRZ-5): a feature of its own, with a per-channel weight plane.
5. **Small precision fixes** (DRZ-6, DRZ-9): a consistent kernel weighting principle, and an interpolated Lanczos table.
6. **Layout cleanup** (DRZ-12).
