# Review: imaginarium, lens, lumos, simd

Reviewed 2026-10-26 at `cf59e2786`. Tests: 1679 pass, 10 skipped (gallery), slowest 0.84 s
(`pipeline::tests::calibrated_lights_on_disk_…`), 3.2 s wall under nextest — no speed finding.

## Proposal

The four crates are in good shape where the work is algorithmic: the demosaics match
librtprocess bit for bit, GRIC / LO-RANSAC / the ringing clamp follow their sources, the unsafe
code is sound, lumos's error domains and published surface follow the rules, and no simd tier
breaks a stated contract. The defects cluster around eight causes.

1. **No single rule for "no data".** `QualityFlags::NO_DATA` is one byte per pixel while drizzle
   coverage and the fill gate are per channel. The gather ORs it per channel, `fill_where` never
   sets it, the FITS mask writes it into every slice, and every image op but Denoise reads the
   fill as sky. The median fallback likewise reports an inverse-variance weight its UNCERT plane
   denies.
   **Target:** NO_DATA means "no channel measured this pixel", set in one place that both the
   gather and `fill_where` call. A per-channel absence lives only in the per-channel coverage
   and weight planes. `LinearImage` offers one "measured pixels" view (the NO_DATA mask) that
   every statistic reads. One `CombinedSample::of_median` decides what a median pixel reports.

2. **The I/O edge is not one layer.** Writers are methods of `LinearImage` / `CfaImage`
   (cycle `image ⇄ io`), the FITS decoder is bound to `File`, combine / drizzle / calibration
   each own a path-driven entry point, so the pipeline is generic over `P: AsRef<Path>`, and
   `frame_store` takes a whole `IngestRun` (cycle `frame_store ⇄ ingest`). Memory planning
   (`FrameInfo`'s footprints) lives in `io`, and plain types (`WeightUnits`, `DecoderBytes`) are
   imported from it.
   **Target:** `io` decodes and encodes over readers, writers and bytes, and holds header facts
   only. Saving mirrors `LoadCtx` on the io side. `pipeline` is the one layer that turns paths
   into frames; combine and drizzle work over stored frames. `frame_store` takes a storage
   config of its own. Footprints live with the planner (`memory` / `frame`). Below the public
   entry points nothing is generic over the path type.

3. **Loaded data is trusted.** The TIFF decoder's buffer meets an `assert_eq!`, the decoders
   run with no allocation limit, and a bundle's flat divisor skips the floor `PreparedFlat::new`
   enforces.
   **Target:** every boundary checks the decoded data against the type's invariant and returns
   an error case; panicking constructors stay for trusted callers.

4. **Contracts live in docs and debug asserts, not in types or code.** simd: `pow2i` rounds on
   AVX2 and truncates elsewhere, `ln_f32` overstates its domain, `div_scale` takes two values
   that must agree. lumos: the mirror knob cannot take effect, the DLT has no rank rule, the
   brightest-first input order is unasserted, `SmallN::falls_back` counts Median, the stretch
   kernels and the `simd_check` harness accept a tolerance where AGENTS.md demands identity.
   **Target:** each such contract becomes a type (a closed enum, a prepared owner) or a check in
   the code path, and the tests assert identity.

5. **Derived values are recomputed per use instead of owned once.** The reference catalogue is
   prepared again for every light; a frame's statistics are measured twice on a cache miss and
   thrown away on a hit; the noise model is rebuilt per gathered sample and the frame noise
   variance twice per stack; the ONNX session is rebuilt per node run; a mesh row is re-solved
   per star.
   **Target:** a `PreparedReference`, an admission that `FileSet::decode` returns, a per
   (frame, slot) `NoiseAt`, a session in `Invocation::state`, a row cache in `RowScratch`.

6. **Planar and interleaved grew apart in imaginarium.** Different names for one operation,
   panicking `From`s, interleaved → planar in three passes through an `f32` interleaved copy, no
   owned planar → interleaved, a `Preview` that reads only interleaved. lens and darkroom pay a
   full-image copy per crossing.
   **Target:** symmetric constructors and accessors on both descs and images; one row kernel from
   any interleaved format straight into `f32` planes, with alpha dropped by a borrowing named
   method; an owned `From<PlanarImage>`; `Preview` over both layouts.

7. **Mirrors and copies of one truth.** lens hand-mirrors lumos (`NormalizationChoice`, the σ
   default, lossy `From` projections); three clipped-regression loops with two plane solvers;
   `ColorMesh`'s own bilinear beside `MeshSurface`; `DMat3` beside `glam::DMat3`; three Gaussian
   generators; five hand-rolled `to_array`s; a test-only second SIP solver.
   **Target:** derive from the owner — lumos types used directly in lens, one clipped-regression
   helper in `math::statistics`, one mesh interpolant, glam, one seeded normal source.

8. **Tests are not hermetic and some sit in the wrong module.** 69 sites use
   `IngestConfig::default()`, which reads live free memory and `~/.cache/lumos`; ~76 FITS tests
   round-trip through temp files; FITS, cosmic-ray and frame-store tests sit in image,
   calibration and combine test files; several tests assert only a direction where an exact
   value is known.
   **Target:** one internals `IngestConfig` fixture (fixed memory, `TempDir` cache), in-memory
   FITS bytes, each test beside the code it pins, tables with exact values.

## Plan

Batches are sorted by impact. Each leaves `cargo drive check` and `cargo drive test` green.
Items that start with "Decided:" carry the owner's decision of 2026-10-26.

### 1. lumos — one NO_DATA rule and one median rule

Files: `lumos/src/combine/frame_cache/mod.rs`, `combine/stack/{mod.rs,mean_combine.rs}`,
`combine/reducer/combined_sample.rs`, `combine/stack_config/{small_n.rs,mod.rs}`,
`stack_product/{mod.rs,fits/mod.rs}`, `drizzle/{drizzle_run.rs,accumulator/mod.rs}`,
`image/linear_image.rs`, `image_ops/{hdr,stretch,extract_bg,color_calibration,local_contrast}`.

- [ ] `combine/frame_cache/mod.rs:545-557` — the per-channel pass ORs `NO_DATA` when that
  channel's `covered == 0`, so a CFA drizzle masks green and blue where only red is missing, and
  `NoDataMask` (`stack_product/fits/mod.rs:95-106`) writes it into every MASK slice. Set NO_DATA
  only where no channel is covered, by the rule `fill_where` states (`stack_product/mod.rs:150`),
  in one function both call. Add a drizzle test that asserts on the flags.
- [ ] `drizzle/drizzle_run.rs:280-293` → `StackProduct::fill_where` (`stack_product/mod.rs:140-186`)
  fills pixels under `min_weight_fraction` and leaves their flags clear, so the fill reads as
  data. `fill_where` sets NO_DATA by the same rule.
- [ ] `drizzle/accumulator/mod.rs:509-533` — the test oracle copies the fill-gate threshold, so it
  cannot catch a wrong gate. One gate type both read.
- [ ] `combine/stack/mod.rs:134-142` and `mean_combine.rs:377-388` — the median reduction is
  copied, and the small-N copy keeps `Σ1/σ²` in an `InverseVariance` plane while UNCERT reads 0
  (`reducer/combined_sample.rs:79-87`, `frame_cache/mod.rs:586`). One
  `CombinedSample::of_median(samples, quality)` that reports the weight as UNMEASURED.
- [ ] `combine/stack_config/small_n.rs:43-46` — `falls_back` treats `Median` as rejecting, so a
  Median config with `small_n` logs a fallback that never happens. True only for `Mean(r)` with
  `r != None`; `validate` (`stack_config/mod.rs:161-166`) refuses `small_n` on a method with
  nothing to fall back from.
- [ ] `image/linear_image.rs` — add the measured-pixels view (the NO_DATA mask). Use it in HDR
  (`image_ops/hdr/mod.rs:88,99`: a 10 % uncovered border scales the sky by 0.62), the auto
  stretch's MedianMad (`stretch/mod.rs:247,282`: MAD +13 % at 10 %, +58 % at 30 %), `ExtractBg`
  (`extract_bg/mod.rs:229` passes `mask: None`), `NeutralizeBg`
  (`color_calibration/neutralize_bg.rs:56`) and CLAHE (`local_contrast/mod.rs:199`), as Denoise
  does (`denoise/mod.rs:147-148`). One test per op with a NO_DATA border and an exact result.

### 2. lumos — decode once, at the right width

Files: `lumos/src/combine/frame_cache/loader/mod.rs`, `io/input_format/{mod.rs,tests.rs}`,
`io/raster.rs`, `io/frame_info/`, `frame/frame_peek.rs`, `frame_store/{file_set.rs,decode_cache/kept_frame.rs}`,
`pipeline/{light_files.rs,align_stack_run.rs}`, `drizzle/drizzle_run.rs`, `drizzle/drizzle_shape.rs`.

- [ ] `combine/frame_cache/loader/mod.rs:158` — `FrameStore` gets `ctx: &run.ctx`, not
  `intake.ctx()`, so each of `plan.width(DECODE)` concurrent decodes unpacks with every worker
  (`LoadCtx::for_decode_slots`, `ingest/frame_intake.rs:48`). Pass `intake.ctx()`, as
  `pipeline/align_stack_run.rs:185,367` do.
- [ ] `io/input_format/mod.rs:11` — `FITS_EXTENSIONS = ["fits", "fit"]` through
  `Path::extension` refuses `.fts` and `.fits.fz`, which fits-well decodes. Accept both; add
  them to `input_format/tests.rs`.
- [ ] `frame_store/file_set.rs:299`, `decode_cache/kept_frame.rs:221-291` — a cache miss measures
  `FrameStats` for the commit and every caller measures again (`pipeline/light_files.rs:153`,
  `drizzle/drizzle_run.rs:77,220`, `pipeline/align_stack_run.rs:73`); a hit drops the committed
  stats. `FileSet::decode` takes the intake's admission and returns the `Admitted`: a miss
  measures once, a hit reuses the commit after `check_facts`, as
  `combine/frame_cache/loader/mod.rs:244-283` already does.
- [ ] `io/frame_info/mod.rs:51-58`, `io/raster.rs:28-37,86-88` — a float TIFF is planned at 1×
  the frame (`DecoderBytes::default()`) but holds the interleaved buffer beside the planes
  (≈ 2×). The raster path states `DecoderBytes { beside: interleaved_len }`; once batch 9's
  one-pass kernel lands, recheck.
- [ ] `io/frame_info/mod.rs:8-12,60-100` and `frame/frame_peek.rs:14-53` — two types for one
  frame's plan; `From<FrameInfo> for FramePeek` drops `gain_bytes`, re-added by hand at
  `pipeline/light_files.rs:93-96`. One type: `io` returns header facts (`desc`, `cfa_type`,
  `DecoderBytes`, `gain_bytes`); the footprint methods (`Demosaic`, `DilationScratch`,
  `MosaicNoiseMeasure`) move to `frame`. Merge the two `DecoderBytes::peak` tests
  (`io/frame_info/tests.rs:12-19`, `frame/frame_peek.rs:69-110`). Move `DecoderBytes` out of
  `io` (`drizzle/drizzle_shape.rs:16` imports it).

### 3. imaginarium — the codec boundary

Files: `imaginarium/src/codec/mod.rs`, `interleaved/interleaved_image/mod.rs`, `error/`,
`lumos/src/io/raster.rs`.

- [ ] `codec/mod.rs:116-155` — `load_tiff` hands `read_image()`'s buffer to
  `InterleavedImage::from_samples`, whose `assert_eq!` panics on a 1-bit or 12-bit TIFF, a
  `PlanarConfiguration = 2` file, or RGB with an unspecified extra sample. Check the decoded
  length and layout against the desc first and return a `CodecError` case; test each layout.
- [ ] `codec/mod.rs:63-66,117-119` — `no_limits()` / `Limits::unlimited()` let a ~100-byte
  header ask for 64 GiB. Derive the limit from `common::SystemMemory` and refuse a larger header
  with a `CodecError` case, before lumos's `check_side` (`lumos/src/io/raster.rs:71-74`).
- [ ] `interleaved_image/mod.rs:3-4` ↔ `codec/mod.rs:20-21` — cycle: the pixel type imports the
  codec layer for two one-line forwards. Make decode and encode methods of `RasterFormat`.
- [ ] `error/mod.rs:54-55` — `CodecError::Io` is never produced; delete it and its row in
  `error/tests.rs:28-32`.

### 4. lumos — calibration, background mesh, noise

Files: `lumos/src/calibration/{prepared_flat,bundle,dark_scale.rs,calibration_masters,defect_map,master_role.rs,calibration_set.rs}`,
`bg_mesh/{mod.rs,color_mesh,tile_stats,mesh_surface}`, `noise/mrs_noise.rs`,
`math/statistics/{clipped_stats.rs,spread}`.

- [ ] `calibration/prepared_flat/mod.rs:49` — `from_divisor` trusts a bundle's flat
  (`bundle/mod.rs:128` checks only BITPIX / LUMROLE / LUMPREP); a 0, negative or 0.01 sample
  gives ±inf or a 100× gain in `calibration_pass/mod.rs:132`. Return a `BundleError` case
  unless each measured sample is finite and ≥ `MIN_NORMALIZED_FLAT`; test it.
- [ ] `calibration/dark_scale.rs:67,133,153` — OLS through the origin on a noisy regressor,
  selected at `d > 5σ`, is attenuated by ≈ `n·σ_d²/Σd²` (≈ 4 % for excesses near 5σ_d); the doc
  claims it is unbiased. Correct the attenuation (`Σd² − n·σ_d²`, or Deming) and the doc. Add
  table tests on `Pair` slices: exact slope and `standard_error`, slope ≤ 0 / NaN fallback,
  `MAX_FIT_PASSES` exhaustion, a noisy-dark fixture. Fix `:197`'s "300 s over 120 s" (750 / 300).
- [ ] `calibration/calibration_masters/mod.rs:161` — flat-darks are decoded, stacked and cached
  with no flats, then dropped silently. Decided: refuse before any decode with a new error case
  (`FlatDarksWithoutFlats { count }`); test it.
- [ ] `defect_map/ring_reference.rs:76,163`, `bg_mesh/tile_stats/mod.rs:104`, `dark_scale.rs:153`
  — three "fit, clip at κσ of residual MAD, refit" loops and two plane solvers (Cramer on raw
  moments vs exact centred `i128`). One clipped-regression helper in `math::statistics` over a
  small model trait, with the centred plane solve.
- [ ] `defect_map/mod.rs:376,390` — re-derives `Spread::resolution` with a different floor; call
  it.
- [ ] `bg_mesh/color_mesh/mod.rs:75,103` — its own centre search and bilinear lerp beside the
  bicubic `MeshSurface`, so the hot-pixel cut and cosmic-ray noise read another background than
  detection does. Hold one `MeshSurface` per colour and read rows through `SurfaceReader`. Fix
  the `defect_map` doc ("tile medians" → Pearson-mode skies).
- [ ] `bg_mesh/mod.rs:145,156,163` — bad-tile fill scans the full (2r+1)² square per radius per
  bad tile: O(bad · R³), serial (≈ 6·10⁹ steps on a 94×63 grid with one good tile). Multi-source
  Chebyshev BFS over the tile grid, then gather the ring perimeter, parallel over bad tiles.
- [ ] `bg_mesh/mesh_surface/mod.rs:218,255` — `at` re-solves its row on every call, though
  `star_detection/.../detect/mod.rs:~145`'s `COMPONENTS_PER_TASK` doc relies on row reuse.
  `RowScratch` keeps the last solved `y`. Measure with the detect bench before and after.
- [ ] `noise/mrs_noise.rs:20,31,285,367,431,438` — `WHITE_NOISE_RESPONSE` was measured for 4
  scales but divides out below 4 (frames < 16 px a side); σ₁ appears as 0.889 and 0.8907. Take
  σ_j from `wavelet::white_noise_sigma`; derive the response per scale count with a reference
  script, as `mad_consistency.py` does.
- [ ] `math/statistics/clipped_stats.rs` — `TileStats.sigma` is documented as SExtractor's
  background RMS but is a MAD-σ of survivors. Fix the doc or the estimator.
- [ ] `calibration_masters/tests/cosmic_ray_scratch.rs:1-110` → `cosmic_ray/tests.rs`; delete
  its constant-arithmetic asserts (`754_974_720`, `14_155_776`). Move the bundle round trips in
  `calibration_masters/tests/mod.rs` (2,539 lines) to `bundle/tests.rs`, sensor refusals beside
  `sensor.rs`, dark matching beside `dark_match/`.
- [ ] `calibration_masters/mod.rs:317` names a `Size2us` `desc` → `size`; `:676` gated module
  mid-file → end of file; "in calibration order" at `master_role.rs:30`,
  `calibration_set.rs:42`, `calibration_masters/mod.rs:193,207` → "bundle order".
- [ ] `noise/bg_split` and `image/metadata/bg_split` share a name, and `noise::bg_split::fit`
  builds the other module's `BgSplit`. Rename one.

### 5. lumos — star detection

Files: `lumos/src/star_detection/**`, `noise/mrs_noise.rs`.

- [ ] `star_detector/stages/prepared_frame/mod.rs:232,255` — `inverse_variance_weights` gives a
  σ = 0 channel the whole weight, and `NoisePlane::sigma` returns 0 for a constant channel
  (`noise/mrs_noise.rs:257`): RGB with G = B = 0 gives weights `[0, .5, .5]` and zero stars.
  The estimator tells "no pixels", "constant" and "noiseless" apart; a constant channel weighs 0.
  Replace the test that pins the current weights (`prepared_frame/tests.rs:142`).
- [ ] `prepared_frame/mod.rs:30,186`, `centroid/measure_grid.rs:81-86` — with
  `fwhm.mode = None` the grid assumes FWHM 0: a 9×9 stamp and a 5–9 px annulus that sits at
  0.83 FWHM of a FWHM-6 star, lifting the sky and lowering flux, SNR and FWHM. Size the
  measurement from a width measured on the frame, independent of the matched filter. Fix
  `star_detection/tests/mod.rs`'s `synthetic_config` comment.
- [ ] `centroid/mod.rs:124` — one `sky.noise.reader()` per star (two `ScratchPool` locks each);
  one reader per task.
- [ ] `duplicates.rs:55-56,73-74` — `duplicate_min_separation = 1e-30` saturates the cell index
  and overflows `home.x + dx`. Bound it in `FilterConfig::validate`.
- [ ] Cycles: move `Prefilter` into `detection_filter` (cut `detection_filter/mod.rs:19`); move
  `MatchedFilter` and the FWHM bounds into `convolution` (cut
  `convolution/filter_kernels.rs:9`, `convolution/mod.rs:15`).
- [ ] `star_detection_config/extraction_config.rs:104` — label `"deblend n_thresholds"` →
  `"deblend threshold_count"`; rename `n_thresholds_effect` (`deblend/multi_threshold/tests.rs:453`)
  and the rows at `star_detection_config/tests.rs:255,262,363`.
- [ ] `star_detection/mod.rs:60,62` — `detection_filter`, `duplicates` → private; fold the gated
  `buffer_room` (`deblend/multi_threshold/mod.rs:377`) and `plane_ring`
  (`convolution/simd/mod.rs:320`) into each file's `internals`.
- [ ] `star.rs:58` — `expect(dead_code)` → the right gate.
- [ ] `centroid/moffat_fit/tests.rs:304-401` — one strategy table (β → strategy, `n`,
  `int_part`), one accuracy sweep, `assert_eq!` for exact powers (2.5¹⁰ = 9765625/1024).
- [ ] `centroid/tests/stamps.rs:66-122` — five one-field tests → one table over
  (pos, radius) → (origin, local_pos, peak, len | None).

### 6. lumos — registration

Files: `lumos/src/registration/**`, `pipeline/frame_registrar.rs`, `math/{dmat3,lstsq,linear_system}`,
`frame_store/frame_buffers.rs`, `lib.rs`.

- [ ] `triangle/triangle_config.rs:17-19`, `ransac/mod.rs:94-101`, `estimate/moments.rs:83-111` —
  `check_orientation: false` only adds mirrored votes: RANSAC refuses every mirror and
  Procrustes cannot reflect. Decided: delete the knob and the mirrored-vote path; orientation is
  always checked, as Astroalign does, and mixed ROWORDER stays refused at admission. Update
  `tests/public_api.rs:135`; add a test that a mirrored frame fails with a clear error.
- [ ] `estimate/dlt.rs:36-50` — `Dlt::solve` returns `v_t.row(8)` with no rank rule, breaking
  `estimate`'s "None when degenerate" (`estimate/mod.rs:4-6,89-92`); the final fit is
  unshielded. Take the null vector from `Lstsq`'s factorization (rank rule, QR first) and refuse
  rank < 8.
- [ ] `register.rs:41-96,186-194`, `final_fit/fit_catalogs.rs:68-86`,
  `pipeline/frame_registrar.rs:126` — the reference is validated, tree-built, triangulated and
  copied again for every light. `Registrar::reference(&[Star]) -> PreparedReference` once per
  run; `register(&PreparedReference, &[Star])`. Fix the stale tree-reuse comment at
  `triangle/triangle_config.rs:26-27`.
- [ ] `register.rs:78-87`, `registrar.rs:26-28` — `take(max_stars)` assumes flux order without a
  check. Select the brightest inside (`select_nth_unstable_by`).
- [ ] `error.rs:74-80,146-157` — `FailedModel.error: Box<RegistrationError>`; four cases occur
  (`model_search.rs:153-208`). A `ModelFailure` enum, unboxed. Give `NoMatchingPatterns`
  (`register.rs:133`) `found` / `required`.
- [ ] `triangle/voting.rs:25-33,42-44,75` — sorted `Vec<u64>` votes, estimated ~32 MB at 500 stars
  against a 1 MB dense `u32` matrix. Bench `registration/triangle/match/500` against a dense
  matrix scanned in order; keep the winner and fix the doc.
- [ ] `resample/mod.rs:10-11,100-130` — `impl FrameBuffers for WarpResult` makes the resampler
  import `frame_store` and `frame::frame_quality`. Move the impl to `frame_store/frame_buffers.rs`.
  At `:208-219`, zip one more `par::chunks_mut` instead of `UnsafeSendPtr`.
- [ ] `resample/warp_kernel/mod.rs:332-339` — `largest_singular_value` re-derives
  `LocalGeometry::of(j).stretch[0]` with a cancelling formula; call it. `ransac/mod.rs:163` →
  `point_normalization::centroid`. `warp_kernel/mod.rs:259` → `abs()` (batch 7).
- [ ] `registration_result/mod.rs:31-46` — `new` takes `rms_error` and asserts it equals its own
  derivation; derive it inside.
- [ ] `warp_transform/inverse_warp/mod.rs:35-63` — `InverseWarp::apply` / `InverseMapped` have
  no production caller: drizzle calls only `pos` (`drizzle/accumulator/input_map.rs:69`).
  Delete both and their `lib.rs:166` export; point `stack_product/run_report.rs:61`'s link at
  `pos`; `drizzle/tests/accumulation.rs:417` uses `pos`.
- [ ] `math/dmat3/mod.rs:22-100` → a registration-owned `Homography(glam::DMat3)` keeping the
  horizon rule and singularity threshold. `math/linear_system` `Lu` (one caller,
  `distortion/tps/mod.rs:128`) → nalgebra `LU` plus a pivot check; fix `tps/mod.rs:3`'s
  `expect(dead_code)`. Record the statrs 5e-11 reason in `math/error_function.rs`'s module doc.
- [ ] `distortion/sip/mod.rs:577-751` — test fixtures build SIP through a second, test-only solver
  (`fit_under`, `conformal`, `coupled`). Build fixtures from explicit coefficients or `fit_with`;
  keep one `required_points` assert (`sip/sip_order.rs:64-91`).
- [ ] `estimate/tests.rs:9-32` and `ransac/tests/mod.rs:33-57` — one shared helper module for
  `make_grid`, `apply_all`, `exact_fit_tolerance`.
- [ ] `tests/input.rs:15-82` — four `median_fwhm` tests → one table with `assert_eq!`.

### 7. simd — contracts and API, and the kernels held to them

Files: `simd/src/**`, `lumos/src/internals/simd_check/mod.rs`, `lumos/src/image_ops/stretch/simd/`,
`lumos/src/image/cfa/demosaic/bayer/rcd/mod.rs`, `lumos/src/math/{sum,wavelet}`,
`imaginarium/src/interleaved/conversion/elements.rs`, `imaginarium/src/ops/affine_warp/cpu/gather_bilinear.rs`.

- [ ] `backend/avx2_fma/f64x4.rs:78` — `pow2i` rounds (`_mm256_cvtpd_epi32`) where NEON and
  Portable truncate: 1.5 → 4 vs 2. Use `_mm256_cvttpd_epi32`; Portable `debug_assert`s an
  integer in [−1022, 1023].
- [ ] `math.rs:118-141,173-190` — `ln_f32` says `x > 0` but gives −88.02 for 1e-40, −88.03 for
  0, 88.72 for +∞; `asinh_f32(+∞)` = 89.42. State "positive normal, finite" and assert it in
  Portable.
- [ ] `f32x8.rs:43-55` — `div_scale` takes a divisor and its reciprocal that only debug Portable
  checks. `div_full_scale(values, FullScale::{U8, U16})`; callers at
  `imaginarium/src/interleaved/conversion/elements.rs:130,160`.
- [ ] Multi-call closures under ops the crate requires inline (`lib.rs:24-27`, simd/AGENTS.md):
  `backend/neon/f32x8.rs:21-44`, `backend/neon/mod.rs:387-400,426-439,501-523`,
  `backend/avx2_fma/mod.rs:386-396`, and callers `imaginarium/.../elements.rs:187,198`,
  `affine_warp/cpu/gather_bilinear.rs:184`. Private `#[inline(always)]` fns. Re-bench the five
  NEON regressions in `.notes/ISSUES.md` on the M2 after.
- [ ] `backend/avx2_fma/mod.rs:404-416`, `isa.rs:153-156,162-182` — AVX2 falls back to the scalar
  lane loop for `store_interleaved_u16` and the f32 C = 3 store. Override both; move the lane
  loop to a `pub(crate)` fn so the C = 3 branch stops copying the default.
- [ ] Add `F32x8::copysign` (bitwise, Portable models `f32::copysign`); use it in
  `rcd/mod.rs:94-98` instead of a vector divide.
- [ ] Every vector trait requires `store` and provides `to_array` (F64x4 included); delete
  `isa.rs:343-356` (`lanes_of`, `u8_lanes`) and `tests.rs:409-424,782-796`.
- [ ] Delete `Isa::run` (three identical `kernel.run(self)` impls); `IsaTier::run` calls
  `kernel.run(isa)`. This removes the `isa → kernel → isa_tier → backend → isa` cycle.
- [ ] Naming: one scheme for `round_clamped*`, `narrow_*` and `store_interleaved*`
  (`store_interleaved_u8::<C, G>` like its siblings).
- [ ] `isa_tier.rs:21-33` — `IsaTier` is a public enum whose variants change per arch; a public
  struct over a private enum.
- [ ] Battery (`tests.rs:53,102-136,247-258`): ±∞ and −0.0 for `round_clamped*`; a per-lane
  `high` and document which lane bounds which value (`isa.rs:100-103`); `high` near `i32::MAX`;
  f64-only rows (subnormal, 1e300, ±∞, −0). Move an `ln_f32` / `asinh_f32` sweep beside the
  exp / ln tests. Merge the frexp tests (`:628-641` with `:705-727`); reuse `ROUNDED` (`:815-826`).
- [ ] `lumos/src/internals/simd_check/mod.rs:18-86,99-150` — the harness takes a tolerance and
  `DATA_SHAPES` has no NaN, ±∞, −0.0 or subnormal. Make it bit-exact (`to_bits`, NaN-aware), add
  the special shapes, delete `of_sums` / `relative`.
- [ ] `lumos/src/image_ops/stretch/simd/` — the asinh kernels are held at 4ε / 10ε to libm. The
  scalar reference runs the kernel's steps per lane; `AsinhCurve::eval` uses them; test bit
  equality.
- [ ] `lumos/src/math/sum/simd.rs:66` — `HalfSums<V>` re-declares `simd::Halves<V>`; use it.
- [ ] `lumos/src/math/sum/bench.rs:29` — `{tier}` → `tier.id()`; `math/wavelet/bench.rs:15-28`
  runs `SmoothRow` on each supported tier.

### 8. lumos — one I/O layer

Files: `lumos/src/io/**`, `image/{linear_image.rs,cfa/cfa_image.rs,cfa/tests.rs}`,
`frame_store/{file_set.rs,run_storage.rs}`, `ingest/`, `pipeline/**`, `combine/`, `drizzle/`,
`calibration/`, `stack_product/`, `lens` call sites, `lumos/examples`. Needs batch 2.

- [ ] Decided: the bytes/readers/writers rule binds lumos's codecs — `io`'s FITS, raster,
  bundle, stack-product and ONNX code decode and encode over `Read + Seek`, bytes and `Write`.
  Paths stay at one edge: `LoadCtx`, `pipeline`, `FileSet` and the decode cache, and the save
  fns that call `common::fs::publish`. Record the scope in `lumos/AGENTS.md` (practice: the
  `image` crate's `ImageReader::new(reader)` / `write_to` core under `open` / `save`).
- [ ] `image/linear_image.rs:18-19,39-45`, `image/cfa/cfa_image.rs:29-30,110` — cut `image → io`:
  saving moves to io, mirroring `LoadCtx`, over a `Write`; a path wrapper at the edge.
- [ ] `io/fits/decode/{selected_fits.rs,fits_file.rs,pixels/mod.rs,plan.rs}`, `tile_parallel.rs`
  — every read names `StreamReader<File>`. Generic over fits-well's `Source`; only
  `FitsFile::open` opens a path.
- [ ] Tests from bytes: `io/tests/synthetic.rs` (41 temp files, `sync_all` at `:75`),
  `io/fits/decode/tests.rs` (22), `io/tests/mod.rs` (13), and the FITS round trips in
  `image/cfa/tests.rs:108-506,571-726` move beside `io/fits` with one helper set. DATAMAX
  (`synthetic.rs:769,1102`, `decode/tests.rs:605`) → one table. Delete the demosaic copies
  (`synthetic.rs:1016-1099`); move the frame-set test (`:447-568`) to `ingest`/`combine` and
  `metadata_default` to `image`.
- [ ] `frame_store/file_set.rs:19,154`, `run_storage.rs:8,38` — cut `frame_store → ingest`: a
  storage config (`cache_dir`, `keep_cache`, `ctx`) that `IngestRun` holds.
- [ ] `combine/frame_cache/loader/mod.rs:29-30`, `drizzle/drizzle_run.rs:51-53`,
  `StackConfig::stack_paths`, `DrizzleConfig::drizzle_paths`, `CalibrationMasters::build` —
  path-driven entry points move to `pipeline`; combine and drizzle take stored frames.
- [ ] `pipeline/{align_stack_config.rs:180,203,213,light_source.rs:12,light_files.rs:24,align_stack_run.rs:122,129,277}`
  — drop `P: AsRef<Path> + Sync`: convert to `Vec<&Path>` at each public entry.
- [ ] `stack_product/weight_kind.rs:3` imports `WeightUnits` from `io::fits::lumos_extension`;
  move the type to its owner.
- [ ] `io/fits/decode/pixels/mod.rs:186-253` re-implements `NullPolicy::resolve`
  (`io/null_policy.rs:37-80`). One entry point with optional precomputed summaries and
  saturation level.
- [ ] `io/raw/raw_files/mod.rs:18` and its `lib.rs` re-export — only examples and
  `internals/real_data` call it; move it there.

### 9. imaginarium — planar and interleaved as siblings

Files: `imaginarium/src/{planar,interleaved,ops/preview,ops/contrast_brightness,ops/blend}`,
`common/src/buffer2.rs`, `lens/src/image/mod.rs`, `lumos/src/{io/raster.rs,image/linear_image.rs,image/cfa/demosaic/mod.rs}`,
`darkroom/src/gui/state/preview_store/preview_converter.rs`.

- [ ] `planar_image/mod.rs:234-266` — interleaved → planar converts to `RGB_F32` interleaved,
  then `Buffer2::deinterleave` reads the whole buffer once per channel (24 MP: 288 MB written,
  864 MB re-read, 648 MB peak vs 72 MB and 360 MB). One row-parallel kernel from any format into
  `f32` planes, with a borrowing alpha-dropping named method. Callers
  `lens/src/image/mod.rs:117-123,157-171` (clone + two conversions) and
  `lumos/src/io/raster.rs:134` use it. `common::Buffer2::deinterleave` writes all planes in one
  pass.
- [ ] Add owned `From<PlanarImage> for InterleavedImage` that hands a mono plane over; use it in
  `lens/src/image/mod.rs:107-111`.
- [ ] `ops/preview/mod.rs:34-36` — `Preview` takes `&PlanarImage` too; remove the full
  interleave in `darkroom/.../preview_converter.rs:161,179` and `lens/src/image/mod.rs:149-154`.
- [ ] `ops/preview/cpu/mod.rs:171-191` — colour averaged without alpha weight: opaque red beside
  transparent black gives `(128,0,0,128)`. Weight by alpha; table-test it with the upscale safety
  net. Merge `preview/cpu/tests.rs:14-141` into one table.
- [ ] `planar_image/mod.rs:191-218`, `interleaved_image/mod.rs:136-146,183-187` — three `From`s
  panic (0×0 plane, mismatched sizes, empty planes). `new_mono` / `new_rgb` (panicking) with
  `checked` forms; callers `lumos/src/image/linear_image.rs:386-390,476-504`,
  `image/cfa/demosaic/mod.rs:57`.
- [ ] Symmetry: one name for the zeroed constructor (`new_black` / `new_zeroed` / `new_empty`);
  the same `new` shape and accessor set (`width`, `height`, `pixel_count`, `sample_count`) on
  `PlanarDesc` and `InterleavedDesc`; one failure contract for wrong-length data
  (`PlanarImage::from_interleaved` panics, `InterleavedImage::new_with_data` returns).
- [ ] Decided: `f32` is nominal [0, 1] and unbounded — 1.0 is full scale for the mid-point and
  brightness offset, no op clamps `f32`, integer formats clamp on store (GIMP float mode,
  lumos's `SampleDomain`). Remove the `f32` clamp from contrast/brightness
  (`contrast_brightness/channel_affine.rs:22-30`) and blend (`blend/cpu/mod.rs:94-97`) on the
  CPU, every tier and the shaders; test that −0.2 and 1.5 pass through; record the rule in
  `imaginarium/AGENTS.md`.
- [ ] `lumos/src/image/linear_image.rs:85-135` — ~10 forwarded read accessors → one
  `pixels(&self) -> &PlanarImage`; one `From<PlanarImage>`; the lockstep RGB block split
  (`map_rgb`, `remap_intensity`, `stretch::apply_color_preserving_asinh`) → one helper.
- [ ] `lumos/src/image/cfa/cfa_image.rs:190-193` — `ram_bytes` omits a `FrameGain::Own`; use
  `LinearImage::ram_bytes`'s expression.
- [ ] `lumos/src/image/cfa/cfa_pattern.rs` — `CfaPattern` is Bayer-only beside `XTransPattern`
  → `BayerPattern` in `bayer_pattern.rs`.
- [ ] `contrast_brightness/cpu/tests.rs:34-100` → one table; delete the change-only asserts
  (`:205-231`, `mod.rs:103-122`). `blend/cpu/tests.rs:93-173` → rows of `blend_mode_formulas`.
- [ ] Names: `interleaved_image/mod.rs:116` cites a nonexistent `ImageBuffer`; `bench/mod.rs:5`
  "transform's kernel"; `affine_warp/cpu/mod.rs:163` `out_row_byte_len` → `out_row`.

### 10. imaginarium — GPU backend

Files: `imaginarium/src/gpu/**`, `imaginarium/src/ops/*/{pipeline.rs,gpu}`, `imaginarium/examples`.

- [ ] `gpu/mod.rs:68-74` — on Metal `max_buffer_byte_len` exceeds 4 GiB; `PackedLayout::new`
  then panics (`packed_layout.rs:180-181`) and the warp shader's `u32` byte index wraps
  (`affine_warp/shader.wgsl:24-37`). Cap at `u32::MAX` there, so `GpuError::BufferTooLarge`
  refuses up front; test the cap.
- [ ] `gpu/gpu_ctx.rs:1-47` — a `HashMap<TypeId, Box<dyn GpuPipeline>>` for three fixed
  pipelines forces a cloned `Gpu` and two extra arguments per call; `Device` / `Queue` are
  already `Clone`. One `GpuCtx` owning device, queue and an `OnceLock<ComputeKernel>` per op;
  delete `GpuPipeline`, the `Gpu*Pipeline` types and the `Arc`s; parameters named `gpu`, not
  `ctx`.
- [ ] `contrast_brightness/mod.rs:65-83` — `apply_gpu` needs a second image where the CPU works
  in place; one read_write binding and `apply_gpu(&mut GpuImage)`. `gpu_image.rs:19-22` — a
  public `desc()`.

### 11. lens

Files: `lens/src/**`, `lens/Cargo.toml`, `lumos/src/combine/normalization/mod.rs`,
`common/src/task`, `darkroom/src/gui/app/session/bench.rs`.

- [ ] `astro/nodes/calibration/mod.rs:262-303` — the master and its `.source` marker publish
  separately; a failure between them leaves master B under marker A, which a later run reuses
  silently. Put the key in the master's FITS header and compare it on load; test the failed
  store.
- [ ] `astro/nodes/ml.rs:35-37,66-79` — every run reads the ONNX file and builds a session. Keep
  it in `Invocation::state`, keyed by path and `FileIdentity`.
- [ ] `image/nodes/io.rs:96-107` — Save Image clones a shared input (`take_interleaved`) though
  saving borrows. Move the `DynamicValue` into the blocking closure and save from the borrow.
- [ ] `config/stacking.rs:253-264` — `CombineKnobs::default` hard-codes σ 3.0 beside
  `StackConfig::light()`'s 2.5 (`preset/tests.rs:146-160` pins it). Decided: derive method and σ
  from `StackConfig::light().combine` (2.5, lumos's measured rate); fix the doc and the test.
- [ ] `config/stacking.rs:200-218` — delete `NormalizationChoice`; derive `IntrospectEnum` with a
  `type_id` on `lumos::Normalization` (new UUID via `uuidgen`).
- [ ] `config/stacking.rs:93-109,147-158` — lossy `From<StarDetectionConfig>` /
  `From<RegistrationConfig>` → named projection methods; drop the dead `.or_else` fallback.
- [ ] `image/nodes/io.rs:67-69,99-107`, `astro/nodes/runtime.rs:33-36,46-48` — a `JoinError`
  becomes a node error while `fs_watch/mod.rs:174-178` lets panics through. The release profile
  has `panic = "abort"`, so a node panic already ends the process in the shipped build; the
  conversion only makes dev and tests differ from it. One helper in `common::task`:
  `try_into_panic` → `resume_unwind`, a cancelled join → the node's cancellation error.
- [ ] `astro/nodes/processing.rs:102-141` — `FrameOp` and six impls only rename `apply`; pass
  `fn(&C, &mut LinearImage) -> Result<(), OpError>`.
- [ ] `image/mod.rs:69-78` — `Image::from_planes` exists for a darkroom bench; delete it and
  build through `LinearImage` + `Image::from`.
- [ ] `fs_watch/mod.rs:22`, `astro/nodes/ml.rs:16-17` — `from_u128` ids → `FuncId::literal`.
- [ ] `astro/nodes/calibration/mod.rs:24-38` — fold the one-variant `FrameSetKeyError` into
  `CacheError::FrameMetadata`.
- [ ] `lens/Cargo.toml:25-29` — dev-dependency on lumos with `internals`.
- [ ] Tests: the six "is registered" tests (`astro/nodes/tests.rs:39-48,106-133,229-283,327-366`)
  → one table; `fs_watch/tests.rs:270-289` waits 300 ms and can pass without the write — use
  `enable()` before the write or a counter; add Save Image's conversion branch, the Convert node
  (pass-through and conversion), and one tiny invocation per astro op family.

### 12. lumos — combine and pipeline simplification

Files: `lumos/src/combine/**`, `drizzle/**`, `pipeline/**`, `memory/run_plan.rs`,
`bit_buffer2/mod.rs`, `math/lm_controller`.

- [ ] `combine/frame_cache/mod.rs:494-504` — each gathered sample rebuilds its noise model with
  three divisions (≈ 5·10¹⁰ samples at 300 × 60 MP × 3). A `NoiseAt` per (frame, slot) at gain
  1 with `1/electrons` precomputed. Add a gather bench first.
- [ ] `frame_cache/frame_weights/mod.rs:51-66` and `sample_noise/mod.rs:126-156` derive the frame
  noise variance twice; `stack/mod.rs:105-151` builds `NormalizedStats` twice. Noise weights from
  `SampleNoise`, built once.
- [ ] `combine/rejection/sigma_clip_config.rs:355-359` — the doc attributes MAD clipping and a
  5.5 % / 3-pass figure to Siril's sigma clip, which uses the standard deviation with no pass
  cap. Correct it with the rate measured for the right method.
- [ ] Names: `combine::stack::FrameOrigin` and `drizzle::drizzle_shape::FrameOrigin` →
  `CombineSource` / `DrizzleSource`; `enum Drop<T>` (`drizzle/accumulator/frame_source.rs:67`) →
  `DropOutcome`.
- [ ] `stack::combine(&FrameCache, plan)` → `FrameCache::combine`; delete
  `StackConfig::run_stacking` (tests use it to skip validation); `loader::load_tiered` →
  `LoadedCache::load`.
- [ ] `pipeline/frame_registrar.rs:44-58` — `Parked` duplicates `FrameRegistration` +
  `Option<StoredFrame>`, rebuilt in `registered_set.rs:39-60`; one `ParkedLight`.
  `light_files.rs:24-41` — `Preparation` mirrors `LightFiles`; two constructors instead.
- [ ] `memory/run_plan.rs:9,85,92` — `width(usize)` by stage index (`PREPARE = 0` and
  `PASS = 0` differ). `RunPlan::of::<N>(…, [Stage; N]) -> [Width; N]`; drop the constants,
  `MAX_STAGES` and `ArrayVec`.
- [ ] `pipeline/align_stack_run.rs:404-411` — the detector config is validated twice behind an
  unreachable error; a panicking `StarDetector::new` for checked configs.
- [ ] `bit_buffer2/mod.rs:76` — `pub(crate) size` can desync `stride` / `words`; private with an
  accessor. `combine/normalization/common_domain.rs:46-117` uses it as a 1-D bitset.
- [ ] `math/lm_controller/mod.rs:43-53` — `LmFit::iterations` → `#[cfg(test)]`.
- [ ] Tests: `combine/tests/tier_equivalence.rs:1-111` and `stack/tests.rs:2078-2106` are subsets
  of `disk_tier_output_is_bit_identical_to_memory_tier` with stale references — fold the path
  case into its table, delete both. `combine/tests/mod.rs:185-268,341-396` — a `plain(Combine)`
  helper and one table over every method, with each exclusion's reason. Move
  `each_plane_carries_its_own_range_and_label` (`combine/error.rs`) beside `frame_plane`, and
  `frame_cache/loader/tests.rs:118-214` (source identity, 40 ms sleeps) to `frame_store`.
  `pipeline/tests/mod.rs:935,1110` → one table over entry × tier × schedule.

### 13. lumos — hermetic tests and fixtures

Files: `lumos/src/internals/**`, `lumos/src/bench/**`, every test using `IngestConfig::default()`,
`lumos/src/memory/tests.rs`, `common/src` (SystemMemory test), `lumos/tests/memory_budget/budget.rs`,
`lumos/Cargo.toml`, `lumos/scripts/clone-refs.sh`, `lumos/src/pipeline/bench.rs`.

- [ ] One internals `IngestConfig` fixture with a fixed `memory_override` and a `TempDir`
  cache; replace the 69 `IngestConfig::default()` test sites (21 in `pipeline/tests/mod.rs`).
- [ ] `memory/tests.rs:34` tests `common::SystemMemory` with a 25 % stability assert that a
  parallel allocation breaks; move it to common without that assert.
- [ ] One seeded normal source in `internals` (ChaCha8); build `TestRng` on it; delete
  `bench/fixture.rs:9`'s and `combine/rejection/tests.rs:88,967`'s copies. Recheck
  `math/statistics/spread/tests.rs:85` and `noise/tests.rs:12` on it.
- [ ] `pipeline/bench.rs:158` — `("resident", None)` plans against free RAM; pass
  `Some(u64::MAX)` and assert `spilled_frames == 0` at setup.
- [ ] `tests/memory_budget/budget.rs:5-6` copies `MEMORY_PERCENT`; call `memory::usable` through
  the gated `lumos::bench`.
- [ ] `lumos/Cargo.toml:30,44-51` — dev-dependency on fits-well with `internals`; drop bytemuck's
  unused `extern_crate_alloc`.
- [ ] `scripts/clone-refs.sh:27-29` — drop the LibRaw 0.20.1 clone (the submodule is 0.22.2);
  cfitsio is a reference, not a native dependency.
- [ ] Names: `math/lanczos/lanczos_lut.rs:46` `num_entries` → `entry_count`;
  `internals/synthetic/metrics.rs:67-68` `n_truth` / `n_recovered` → `truth_count` /
  `recovered_count`. Imports through `lib.rs` re-exports (`bench/scene/mod.rs:23`,
  `bench/scene/tests.rs:4`, `internals/real_data/{mod.rs:25,milky_way.rs:10}`,
  `calibration/*/tests`, `io/tests/synthetic.rs:54,917`) → each item's home path.
- [ ] Validation table `image_ops/tests.rs:92` covers about half its bounds: add
  `target_background`, `black_point`, `shadow_sigmas`, GHS `sp/lp/hp/b`, `ExtractBg`
  `tile_size/rejection_sigma/divide_floor`, local-contrast `strength`, denoise `scales`.
- [ ] `image_ops/bench.rs:100-104` — bench `ExtractBg` on `stacked_rgb`, its contract input.
- [ ] `image_ops/stretch/tests/mod.rs:113-120,561-587,589-608` — fold the direction-only d, β and
  lift checks into the reference tables as exact rows.
- [ ] `image/cfa/demosaic/xtrans/mod.rs:6` — 22.5 % → 22.2 % (8/36).
- [ ] `image/cfa/cfa_image.rs:333-338` — the demosaic balances both Bayer greens by G1's
  multiplier though `camera_white_balance` (`io/raw/mod.rs:33`) keeps G2. LibRaw scales the
  second green by its own `pre_mul[3]` before it merges the greens
  (`libraw/LibRaw/src/postprocessing/postprocessing_utils_dcrdefs.cpp:195-196`,
  `src/demosaic/misc_demosaic.cpp:63-75`). Gb reads G2's gain; `CfaLattice` already tells Gr
  and Gb apart. Test with `G1 ≠ G2`.
