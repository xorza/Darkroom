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
- **Decided (plan Batch 14):** Siril's design. Each frame is drizzled on its own and enters the normal combine with its drop-weight plane; the drop weight multiplies the frame weight in the mean. The direction below is superseded.
- **Direction (superseded):** Feed drizzle the statistical combine's results. Run the regular warp+combine first, which already normalizes and rejects. Map each frame's rejected samples back to input pixels (blot the per-frame rejection mask, or use a `driz_cr`-style test of input pixel against the blotted median) and pass them as zero pixel weight or `COSMIC_RAY`. Apply the frame's normalization gain and offset at deposit, and take frame weights from the same `FrameWeights` resolution.

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

### DRZ-5 — No CFA (Bayer/X-Trans) drizzle
- **Where:** `drizzle/accumulator/mod.rs:40` (`DrizzleFrame<T>` is only ever `LinearImage`), `drizzle/accumulator/mod.rs:368` (`cfa_type: None`, "Drizzle takes demosaiced frames")
- **Category:** precision (missing capability)
- **Impact:** medium-high for one-shot-colour data, the main use of drizzle in amateur astrophotography. Demosaic interpolation leaves correlated noise and colour artifacts that drizzle cannot undo, and the inverse variance plane then treats demosaic-interpolated samples as independent measurements, so it overstates the inverse variance there.
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
- **Decided (plan Batch 14):** drizzle follows the warp's flag split. `RESAMPLE_EXCLUDED` pixels deposit nothing, `RESAMPLE_CARRIED` pixels deposit and carry their flag. The combine then gives the product its flags and report. The direction below is superseded.
- **Direction (superseded):** Define the exclusion policy once, in `pixel_flags`, and use it from both producers. Drizzle cannot apply a survivor floor, so it should exclude the full fill/bias set. Emit `NO_DATA` flags on gated pixels, and count excluded deposits into the `RunReport`.

### DRZ-11 — Smaller per-drop costs in the droplet kernels
- **Where:** `drizzle/accumulator/frame_source.rs:141-159` (`landing` computes the homography Jacobian determinant, or the SIP inverse Jacobian through `InverseWarp::apply`, before any band test), `drizzle/accumulator/output_band.rs:102` (drizzle Lanczos: two `sin` and two divisions per tap, 14 taps per drop)
- **Category:** performance
- **Impact:** low. This compounds DRZ-2 for homography and SIP frames. The resample side already has an exact-enough Lanczos3 table.
- **Confidence:** confirmed
- **Direction:** Return the landing position first, reject on rows and columns, then compute magnification for drops that land. For drizzle Lanczos, reuse `LanczosOrder::Three.lut()`, which interpolates between its entries.

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
- **Anti-alias stretch:** the largest singular value of the output→source Jacobian (DeForest 2004 / reproject adaptive) is correctly directed and applied only when the frame is minified.
- **Ringing clamp:** matches PCL's rule: lobe ratio, soft `1 − ((r−t)/(1−t))²` factor, positive-lobe mean past `r = 1`. It is invariant on a flat field, and the default threshold 0.3 is PixInsight's. Siril's alternative (fall back to a guide image below 0.98× it) is cruder.
- **Edges and nulls:** normalized convolution over surviving taps, with Kish conditioning (`(ΣL)² ≥ ΣL²`) and a bilinear fallback. Coverage is the share of `Σ|L|`, so coverage and confidence vanish together (tested). Oracle and cross-ISA bit-identity tests are thorough.
- **No Jacobian in the warp:** the surface-brightness convention matches flat-fielded data, where the flat already removes pixel-area variation. It is the same convention as the drizzle output. A global plate-scale difference is absorbed by the normalization gain.
- **Test quality:** hand-derived bounds with stated arithmetic, a flux conservation test (Σout = s²·Σin within 33ε), a linear-ramp Jacobian test, polynomial reproduction, and a Nyquist-grating aliasing test. The gaps are DRZ-6's period-average weight and a rotated (≥ 45°) or SIP performance leg (DRZ-2).

---

## Suggested batches

1. **Drizzle as a science producer** (DRZ-1, DRZ-7, DRZ-8, DRZ-13): rejection and normalization fed in from the combine, finite-sample validation, a shared flag policy plus output flags, and the unit stated on the API.
2. **Drizzle scatter performance** (DRZ-2, DRZ-11): per-row column intervals, reject before transforming fully, a shared corner lattice, and bench legs at 45°/90° and with SIP.
3. **CFA drizzle** (DRZ-5): a feature of its own, with a per-channel weight plane.
4. **Small precision fixes** (DRZ-6): a consistent kernel weighting principle.
5. **Layout cleanup** (DRZ-12).
