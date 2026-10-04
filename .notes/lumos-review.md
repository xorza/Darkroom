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

## 2. Absolute `EPSILON` floors on quantities that scale with the data

`Spread::resolution` and `Spread::floored` (`math/statistics/spread.rs`) are the scale-free form. The rejection and `sigma_clip_iteration` use them; the items below do not yet.

**Failure scenario:** 16-bit data in a 32-bit integer FITS normalizes by 2³²−1, so σ ≈ 2e-9. Every floor below then trips.


## 8. Missing-data masks are dropped after decode

- [ ] `8.2` **Star detection does not handle `nulls` in its stamps** — `star_detection/centroid/`
  - The mesh leaves the pixels with no data out, and the threshold clears them. A stamp that holds them still reads their fill as a measurement. `[C]`

## 15. Spill and memory planning disagree with the machine

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

## 22. Run-to-run determinism

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
- [ ] `24.14` **Cosmic-ray noise and background are computed again in every iteration, serially** — `calibration_masters/cosmic_ray/mono.rs:114-129`, `calibration_masters/cosmic_ray/xtrans.rs:248-271`, `calibration_masters/cosmic_ray/masks.rs:74-92` `[C]`
- [ ] `24.15` **GHS uses scalar `ln_1p`/`exp_m1` per pixel** — `image_ops/stretching/mod.rs:478-496`
  - It is the only curve without a vector or LUT path. `[C]`
- [ ] `24.16` **Star detection allocates per frame** — `noise_floor_from`, `from_stars`, `filter_fwhm_outliers`, the dedup `HashMap`, the median buffer, and the kernel `Vec`
  - `labels.fill(0)` writes 96 MB per 24 MP frame. Clear only the previous runs. `[C]`
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
- [ ] `25.18` `background_mesh/mod.rs:87-89`: the doc and `#[inline]` of `find_lower_tile_y` sit on `sigma_range`.

## 26. One fact in two places, wide signatures, and style deviations

- [ ] `26.4` **`StackConfig::bias()` and `dark()` are identical** — `combine/config/mod.rs:265-282` `[C]`
- [ ] `26.13` **`KernelPlan` is rebuilt per frame, although the doc says "once per run"** — `drizzle/accumulator/mod.rs:182`, `drizzle/accumulator/output_band.rs:38-42`
  - It has two `impl` blocks with `OutputBand` between them (`:92`, `:122`). `[C]`
- [ ] `26.16` **`lib.rs:95-131` has 12 renamed re-exports** (`Config as StarDetectionConfig`, `Error as StackError`, …)
  - Rename the types, so rustc and docs show the public names. `[C]`
- [ ] `26.19` **TPS defects** — `registration/distortion/tps/mod.rs`
  - The default regularization 0 interpolates centroid noise exactly.
  - `DistortionMap::interpolate` returns 0 past the grid.
  - `compute_residuals` duplicates `transform`. `[C]`
- [ ] `26.20` **Dead code**
  - `DMat3` `IndexMut` (test-only, `math/dmat3/mod.rs:141-146`)
  - the bounds check in `resolve_matches` (`registration/triangle/voting.rs:222-223`)
  - `mask.fill(false)` (`star_detection/detector/stages/detect/mod.rs:83`)
  - `.filter(|d| d.area > 0)` (`:178`)
  - the 8-bit LibRaw branch (group 16)
  - `let planar = image;` (`image_ops/ml/backend/mod.rs:282`)
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

Built in `math/statistics/spread.rs`. Consumers today: the rejection driver (S4) and the floor of `sigma_clip_iteration`. The frame statistics, the background mesh and the detection noise move onto it in phases 5 and 7.

- `Spread { centre, sigma }` of a sorted window: the median, and `b_n`·1.4826·MAD. The `b_n` for n ≤ 20 are measured again for the midpoint median (`internals/reference/mad_consistency.py`, 10⁷ trials each), because Croux & Rousseeuw's table does not fit it at even n (n = 6: 1.200 against 1.1895). Above 20, `n/(n − 0.8)` is within 0.1%.
- `sigma_eff = max(sigma, background, resolution(centre))`, where `resolution` is `|centre|·ε`, or the smallest subnormal at 0. `sigma_eff` is never 0, so a band about a tied majority holds the tied samples alone when no background is known.
- In the combine, `background` is the RMS over the pixel's samples of `gain²·max(noise, quantization σ)² / confidence`, from `SampleNoise`. The confidence is the warp's inverse variance factor, so a warped sample's floor is its own noise and not the source's. The quantization σ holds the floor where `DifferenceNoise` reads 0 on integer data.

## S4. One rejection driver

Built in `combine/rejection`. Open: `RejectionScale::CcdModel` needs `CcdNoise` (S2) and moves to phase 5.

- Every pixel's samples are sorted once, on `u64` keys (the sample's order bits, then its gather position), into `SortedSamples`. Each method narrows a window of them. The `F32x8` sorting network was not built: the key sort is about 6% of the combine bench's samples, and the light preset is faster than before without it (see the phase 4 bench below).
- The driver loops to a fixed point under each method's pass cap. When a pass proposes fewer than `min_survivors`, it keeps the `min_survivors` samples nearest that pass's centre, ties to the lower position, and stops. A consequence: with the default of 3, a stack of 3 frames rejects nothing.
- The methods:
  - **Sigma clip:** a band about the median in units of `sigma_eff`. The shortcut is gone.
  - **Winsorized:** the PCL form. The start is the robust σ. Each step clamps the working copy at ±1.5σ, takes the mean as the centre and 1.13339 × the standard deviation as σ (the exact clamped-Gaussian factor; PCL rounds it to 1.134). The clamp is cumulative, as in Siril and PCL: clamping the samples themselves at each step is Huber's proposal 2, and it breaks down at 3 outliers in 10. Passes repeat until one rejects nothing.
  - **Linear fit:** the first pass is the median clip, as a robust start. The fitted passes regress the kept samples on their Blom scores among all n samples (a censored Q-Q regression) and clip about the intercept in units of the slope. A first pass that rejects nothing does not end the method. Clean-data rejection at k = 3: 1.16%, 0.61% and 0.38% at 20, 50 and 200 samples, falling with n (Siril's form: 4.4%, 6.3%, 7.8%).
  - **GESD:** the cap is `⌊0.3·n⌋`, limited by `n − min_survivors` and by `n − 2`, and the sample deviation is floored. The critical values are one table by live count.
  - **Trim** (was percentile): exact integer counts.
- Every method only rejects, and survivors are named by frame (`PixelSamples::frame_ids`), so quantization tracking works for winsorized, under any coverage, and per channel.

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

Built in phase 6, as shared parts rather than one function. As built:

- `IngestConfig` carries the FITS policy, the memory override and the cache. `StackConfig::ingest` and `DrizzleConfig::ingest` hold it. `IngestRun` reads the machine once per run and builds the one `LoadContext` every decode takes, so every entry honours the caller's FITS policy and cancel token.
- `FrameAdmission` checks every decoded frame in one order (geometry, facts against frame 0, samples) and measures its statistics, inside the parallel closure.
- The combine-direct entries (`stack`, `stack_cfa_master`) load through the loader. A flat's subtraction is a `FrameStep`, and a stepped frame never enters the kept cache.
- The registered entries (`align_and_stack`, `calibrate_align_stack`) prepare and detect through `LightSource::detect`. A held frame is charged net of what the caller already holds (`RunShape::held_bytes`).
- With `Reference::Index`, raw lights go through `RawLights::stack_in_one_pass`: the reference first, then each light through its preparation, detection, registration, warp and store, written once, under `MemoryPlan::single_pass`. `FrameRegistrar` and `RegisteredSet` are the register and combine steps both paths share.
- The loader and the detection stage stay two functions. One parks `StoredFrame`s for the combine and reuses the kept cache, and the other parks detected frames for registration. A single function over both would need a generic park step and a kept-cache branch that only one side reads.

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

## C2. Calibration as typed masters

Built in phase 6. As built:

- `CalibrationMasters` holds `bias: Option<CfaImage>`, `dark: Option<MasterDark>`, `flat: Option<PreparedFlat>` and the `DefectMap`. `MasterDark` records `DarkBias::Included` or `DarkBias::Removed`. `PreparedFlat` owns the divisor and the count of pixels raised to the floor. The flat-dark is spent on the flats and is not kept.
- `calibrate` checks the masters against the light before it changes a pixel. A check that fails is an error, and a fact that one side does not declare is counted as unverified:
  - A flat or a light with no subtractor is an error, unless the light holds no offset: a removed pedestal, or a synthetic frame with no domain.
  - A dark must match the light's exposure within 1%, and the temperature within 1 °C when both frames declare one. A bias-removed dark with a bias is scaled by `t_light / t_dark` and is not refused. Any other mismatch is an error.
  - `calibrate` returns a `CalibrationOutcome`. The pipeline adds the outcomes into `RunReport`: unverified exposures and temperatures, scaled darks, and floored flat pixels.
- `stack_cfa_master` takes the subtractor and removes it from each frame before the normalization. The prepared frames spill to the run's cache and are never cached between runs.

Still open, after S1 and S2: each master carries its noise, so the subtraction adds its variance. Defect repair sets `DEFECT | REPAIRED`, and the flat floor sets `FLAT_FLOOR`.

## C3. Detection: one plane to threshold, one plane to measure

Built in phase 7. As built:

- `PreparedFrame::new` combines the channels, measures the sky on the combined plane (the no-data pixels masked), refines it around the sources when asked, and takes it out: `measure`, never filtered. It holds the sky σ, the saturation mask, the no-data mask and the refined sources.
- `PreparedFrame::detection_plane` copies `measure`, takes the 3×3 median when the frame was demosaiced and the matched filter when a FWHM is known, and measures the result's noise with the same mesh, around the sources and the no-data pixels. That holds under any correlation (9.2).
- The threshold and both deblenders read the detection plane in its own σ (9.4). Every measurement reads `measure` (9.1).
- The refinement thresholds the detection plane at `mask_sigma` and dilates by a disk (9.8).
- The matched-filter PSF lives in `FwhmConfig` (26.11).

- The multi-threshold tree follows SEP `deblend.c`: every object of a level splits into its regions above the next level that hold `min_area` pixels (9.5), an object is significant by its flux above its own level (9.7), and a bottom-up pass propagates `ok` (9.6).
- Both deblenders index the component's own pixels by row, so their scratch follows its pixels, not its box (15.8).

## C4. Measurement with a convergence contract and an error output

Closes groups 10, 11 and 12, 2.3, 2.4, 2.6, 26.6 to 26.10 and 24.13. It uses S2, S3 and S6.

- A `MeasureGrid` holds what follows from the expected FWHM: the window σ, the stamp radius and the annulus radii (26.6, 26.9). The inner annulus radius encloses a stated flux fraction of a Moffat with β = 2.5 (11.7).
- The windowed centroid subtracts the local sky first (11.3). It uses signed values (11.4) and the adaptive-moments Newton step `σ_w² / (σ_w² − C_obs)` (11.1). It stops on the bound `c/(1 − c)·‖Δ‖` of the remaining error.
- The PSF models are integrated over the pixel. The Gaussian uses erf differences, which is exact (11.5). The Moffat uses Gauss–Legendre quadrature, with an order that keeps its error below 1% of the centroid noise at the minimum FWHM.
  - As built: a rotated Gaussian has no closed-form pixel integral, so both models take a tensor Gauss–Legendre rule. Each fit chooses the order from its own width by a rigorous bound (Trefethen, ATAP theorem 19.3, on the Bernstein ellipse), so the quadrature errs under 2⁻²⁵ of the amplitude, half an ulp of the f32 samples. It starts at the order its seed needs and fits again at a higher order when it converges narrower. A FWHM-4 star takes order 5 with either model; the narrowest admitted profiles take 7 (Gaussian) and 11 (Moffat at β just above 1).
  - Every width the detector reports is the PSF's before the pixel integrates it. The windowed moments remove the box's variance 1/12; the box's kurtosis leaves `1/(240·(σ_w² + s²))` px² per axis to first order, 0.0009 at σ 1.5. A source the moments place inside one pixel reads FWHM 0. The matched filter's kernels are the PSF's pixel means.
  - Both fits admit the same narrowest profile, the FWHM of a Gaussian of σ 0.5 (1.18 px). The Gaussian now bounds its principal widths, not only the diagonal of its inverse covariance, which admitted a principal σ of 0.35. The Moffat β validates in (1, 10]: at β ≤ 1 the flux diverges.
  - `erf` and `erfc` are a port of fdlibm's (`math/error_function.rs`): `statrs` 0.19.1 errs by up to 5e-11 on [0.5, 1], which a difference of two erfs amplifies.
  - The synthetic fixtures render pixel means: the closed form for an axis-aligned Gaussian, otherwise a rule chosen by the same bound at 1e-12 of the peak.
  - Cost, release: the Gaussian fit of a 17×17 stamp takes 133 µs (15 µs before), the Moffat 256 µs (10 µs before). The fits are opt-in; the default centroid is the windowed moment.
- The fits run on `LmController` (S6), with fixed constants (26.7). A failure returns `None` (26.8). The IRLS pass runs only after a successful fit (11.9). The fit weight floor and the amplitude seed floor are fractions of the stamp's sky σ (2.4, 2.6).
- A failed fit falls back to the converged windowed centroid (11.2). A fit that moves more than half the stamp radius is stamped again, once (11.6).
- Every star gets `position_sigma` (12.4):
  - after a fit, from `(JᵀWJ)⁻¹·χ²/(n − p)`,
  - after the centroid fallback, from the windowed-moment error, as SExtractor `ERRX2WIN` computes it.
  Registration needs a σ for every star, so no star leaves without one.
- The SNR is the CCD equation (Merline & Howell 1995) with the sky-estimate term `n_pix(1 + n_pix/n_B)` (12.2), on the background σ held to the threshold's floor (12.3). The background σ already holds the read noise, so it is counted once (12.1). The variance floor is the S3 rule (2.3).
  - As built: the equation is `StarNoise` in `centroid/`, not `CcdNoise`. Measurement reads the residual, so the sky level that `CcdNoise` carries is gone, and the sums are f64. `MeasurementConfig::electrons_per_unit` replaces the `NoiseModel`; without it, the source term is absent and the fits are unweighted.
- Shape metrics follow DAOFIND and photutils: `roundness1` from the pinwheel quadrant sum (10.1), `roundness2` from 1-D Gaussian fits to the marginals (10.2), and sharpness from the star's own peak (10.3). `max_fwhm_deviation` multiplies 1.4826·MAD (10.4).
  - As built: both roundness metrics read DAOFIND's cutout, of radius `max(2, ⌊1.5σ⌋)` for the expected PSF. On the whole stamp, a neighbour 8 px away read −0.66 and rejected both stars of a pair. Both read the unconvolved samples: DAOFIND's convolved `roundness1` reads a round FWHM-2 star's phase up to 0.45, the unconvolved one up to 0.36. Each metric lies in `[−2, 2]`, so `max_roundness` validates in `(0, 2]`.
  - Sharpness stays the peak over the 3×3 core, not DAOFIND's `(peak − mean of the rest)/convolved peak`: the cosmic-ray cut at 0.7 is calibrated on it, and the fix 10.3 asks for is the star's own peak.
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

## Phase 4. Results

The combine bench (30 frames, `combine::bench`, release, one machine, same session):

| Preset | Before | After |
|---|---|---|
| light (sigma clip, noise weights) | 178 ms | 135 ms |
| median | 63 ms | 77 ms |
| winsorized | 98 ms | 115 ms |

- The light preset is faster: the old path ran the shortcut screen and then sorted anyway on most pixels.
- Winsorized is slower because it now loops to a fixed point (the old code rejected once), and each pass estimates again.
- The median code did not change. Run alone, it is 87 ms before and 88 ms after. In the sequence after the light preset it measures slower, which is an order effect of the allocator, not of the median path.

## Phase 7. Results

Phase 7 is done. Two open points remain from it:
- The detection plane's σ is measured per 64-px tile. On a plane correlated over about 36 px (FWHM 4) a tile holds some 110 independent samples, so its σ errs by about 11%, and a 2σ threshold passes about 11% more than it states. A larger tile for the detection plane's noise, or an error-weighted σ, would narrow it.
- The `max_area` filter stays on the regions the deblenders make, not on the components before them, against the plan: a crowded group larger than `max_area` still splits into its stars (`a_crowded_group_splits_into_every_star`). The memory reason for the move is gone, because the deblenders' scratch follows the component's pixels.

Found on the way and fixed: the SNR floored its variance at `f32::EPSILON`, an absolute value that lost every star of a frame scaled by 2⁻¹⁶; it now holds σ to the frame's own floor and computes in f64. The bounded parallel map now returns the failure at the lowest index, so a set with two bad frames always reports the first.

## Phase 8. Results

1. Done: `LmController` (Nielsen's λ update, a Cholesky solve of the Marquardt-scaled system with a relative pivot, and stop tests that hold at any scale — a negligible accepted or Gauss–Newton step in the scaled norm, a Gauss–Newton decrease of χ² under 1e-12 of it, or a gradient orthogonal to the residuals) carries the centroid fits, which now return `None` when they fail and reweight only after a success. `Lstsq` holds the one SVD rank rule the SIP fit and the background extraction share. `Irls` waits for its first robust consumer, the final registration fit (phase 9). Items 2.3, 2.5, 11.8, 11.9, 24.13, 26.7 and 26.8 are closed.
2. Done: `MeasureGrid` holds the stamp, the window σ and the annulus (from where a β = 2.5 Moffat holds 99% of its flux). `WindowedCentroid` subtracts the local sky, weights the signed signal, takes the adaptive-moments Newton step and stops on the bound `c/(1 − c)·‖Δ‖`; a failed fit falls back to it, and a fit that moves more than half the stamp radius is stamped again once. Every star carries `position_sigma`, from the fit's `(JᵀWJ)⁻¹·χ²/(n − p)` or the windowed centroid's propagated noise; both match the scatter of 1000 noise draws. The fits' weight and amplitude seed floors are relative. Items 2.4, 2.6, 11.1 to 11.4, 11.6, 11.7, 12.4, 26.6, 26.9 and 26.10 are closed.
   The integrated models (11.5) are built as C4 records. The characterization snapshots moved with the fixtures, which now render pixel means; detection on the characterization field finds 34 stars, and registration 32 inliers with a worst corner error of 3.5e-3 px.
3. Done: the SNR is the CCD equation with the sky-estimate term, on the background σ held to the frame's floor, and the fits weigh by the same equation when the gain is known. GROUND and SROUND are photutils' `roundness2` and `roundness1` on DAOFIND's cutout, and the tests pin them to photutils' formulas reproduced in numpy. Sharpness reads the star's own centre pixel. `validate_catalog` checks every float field of a star. Items 10.1 to 10.4, 11.10 and 12.1 to 12.3 are closed. 24.12 needs a SIMD `ln`, so it moves to phase 14 with the rest of group 24.
   Effect on the characterization field: 34 stars pass, against 30. The 30 keep their positions, fluxes, FWHMs and eccentricities exactly. The 4 new ones are blended neighbours that the whole-stamp roundness rejected. Registration finds 32 inliers, against 27, and its worst corner error falls from 4.6e-3 px to 3.4e-3 px.

## Phase 9. Results

1. Done: no rotation limit by default, and a fixed default seed (`RansacConfig::seed: u64`, 0). LO-RANSAC takes a refit on its score alone. A minimal sample is degenerate when any pair is closer than the scorer's noise scale, or any triplet stands lower than it over its longest side. Near-isosceles triangles vote in every vertex order their tied sides admit, with the orientation test reversed for an odd permutation. Items 3.1, 13.7 to 13.10 and 22.1 are closed.
   Deviation (13.9): a triangle is kept while it stands at least the noise scale high over its longest side, not by a share of that side. The invariants `s₀/L` and `s₁/L` move by about σ/L whatever the shape, so Groth's side-ratio limit, which protects invariants built on the shortest side, does not apply; a relative height limit cut real matches in a dense field. A flat triangle loses its orientation, and that is what the noise-scale test guards.
2. Done: `registration/final_fit/` matches every unsaturated star of both full catalogs through the hypothesis, weights each pair by `1/(|det J|·σ_ref² + σ_target²)` times a Cauchy weight (Holland and Welsch's 2.3849) on its normalized residual, and gates at the recovery radius first, then at `√χ²₀.₉₉(2)` of the robust scale (median over the Rayleigh median, at least 1). It reaches the Cramér–Rao bound of 2000 stars, times the Cauchy weights' variance ratio of 1.062. The homography refines its weighted DLT by LM on the reprojection error. Each linear part is fitted with its SIP correction at the joint optimum in every pass: translation, similarity and affine map in one linear solve with the linear part factored out (astrometry.net's `fit_sip_wcs`), the rotation by a secant iteration on the envelope derivative. `Auto` fits every model and takes the lowest GRIC (Torr 1998) over the union of their pairs, among those the caller's `max_rms_error` accepts. Items 13.1 to 13.6, 26.17 and 26.18 are closed.
3. Done: `AlignmentSummary::frames` holds each light's registration in input order — the reference, the warp it was resampled through with its pair count and RMS, or the error that dropped it — and `registered()` and `dropped()` derive from it. `Reference::Auto` takes the lowest median FWHM among the frames with the stars registration needs, ties to the lowest index. Two default runs stack bit for bit. Items 21.1 and 21.4 are closed; phase 9 is complete.
   Deviations: (a) A homography takes no SIP correction: its perspective terms act to first order as the correction's quadratic ones, so the two are not determined together (the joint solve is rank-deficient), which is why the SIP convention puts an affine map under the polynomial. Validation refuses the pairing, `Auto` with SIP stops at the affine map, and the wide-field presets use `Auto`. (b) The SIP origin defaults to the reference catalog's bounding-box centre, not the image centre: `register` does not see the image, and the reference catalog is the same for every frame of a run. (c) Sigma clipping in the SIP fit is gone; the final fit's robust weights and gate carry outliers for the transform and the correction alike.

## Phase 10. Results

1. Done: a SIP row collapses to two polynomials in `u` (`SipPolynomial::row`, Horner per pixel), and the 2k RGB SIP warp bench goes from 49.6 ms to 44.5 ms. A Newton step takes the correction and its Jacobian from one set of powers (`SipPolynomial::local`), and `InverseWarp::position` skips the final Jacobian that drizzle's placement discarded. Items 24.5 and 24.6 are closed.
2. Done: `resample/frame_sampler` builds each output pixel's window once — taps, weights, weight sums — and every channel and both quality values come from it, so the old `row` and `quality` passes are gone. Bicubic and the Lanczos family take PixInsight's ringing clamp (`WarpParams::clamping_threshold`, default `Some(0.3)`). Every filter but Nearest is stretched by the largest singular value of the output-to-source Jacobian, held to at least 1 (DeForest 2004, astropy `reproject`'s adaptive mode). Where the source edge or a null cuts the window, its taps with data are normalized while `(Σ L)² ≥ Σ L²` and `Σ L > 0`, that is while the normalized sample is no noisier than one pixel; otherwise Bilinear at the same stretch averages its own taps with data. Coverage is now the share of the kernel's magnitude `Σ|L|` on data, so a lost negative tap costs it too, and confidence is the Kish size of the coefficients the sample used. Windows wholly inside the source with no null near and at most 8 taps per axis run in registers (`InteriorWindow`); every Isa gives `Portable`'s bits. Items 14.1 to 14.4 and 24.4 are closed; phase 10 is complete.
   Deviations: (a) The clamp splits the taps by the sign of the weight `L`, not of `L·f`, and reads the light `max(f, 0)`, with the part below zero through the plain kernel. For data at or above zero this is PCL's rule exactly. PCL's split, on calibrated data that dips below zero, can put a negative weight in the positive lobe's sum and divide by nearly nothing. (b) The stretch is one per frame, the largest local scale on a 32 px grid (one Jacobian for an affine map), not per pixel: the flag propagation reads one window reach per frame. It is isotropic, so a warp that shrinks one axis also blurs the other. (c) Where the clamp acts, confidence describes the unclamped coefficients, which understates it beside bright stars. (d) Cost: the 4k mono Lanczos3 warp goes from 71.3 ms to 91.7 ms, 2k RGB homography from 39.8 to 47.9 ms, 2k RGB SIP from 44.2 to 51.3 ms. Without the clamp the new kernel matches the old one; the clamp is the 23%. The 2k Bilinear plane goes from 3.35 ms to 10.8 ms, its quality maps now included.

## Phase 11. Results

1. Done: the memory reading is the least of the host's `MemAvailable` and what every level of the process's own cgroup chain leaves (`memory/cgroup_memory.rs`), v2 and v1, with the mount's root taken off the group path so a container's view resolves. One run's spills go to `RunScratch`: each file is unlinked on Unix as soon as it is created, before any data goes in, and opened delete-on-close on Windows, so a crash leaves nothing, no run sees another's files, and a frame's disk space returns when its planes drop; the marker file, the run directories and the pid scan are gone. `keep_cache` writes only content-keyed frames, to `DecodeCache` (`<root>/decode-cache`); a prepared frame and every pipeline frame stay in the run's scratch. The default root is the user cache directory (`$XDG_CACHE_HOME/lumos`, `~/.cache/lumos`, `~/Library/Caches/lumos`, `%LOCALAPPDATA%\lumos`, else `/var/tmp/lumos`), and a root on tmpfs or ramfs per `/proc/self/mountinfo` is refused with an error that names it. `RunReport` gains `spilled_frames` and `parked_lights`, which the pipeline test now reads instead of file names. Test temp directories moved from `/tmp` to the workspace's `.tmp/tests`. Items 15.1, 15.2, 15.6 and 15.7 are closed.
   Deviations: (a) The cgroup figure is `limit − (use − inactive_file)` per level, Kubernetes' working set, not sysinfo's `limit − use`: a group's use counts its page cache, which on this host's session was 10.8 of 11.8 GB, and the kernel reclaims the inactive part before the limit; v2's `memory.high` caps as `memory.max` does. sysinfo reads only the root cgroup, which misses a systemd unit's limit. (b) The Unix scratch file is created and unlinked, not opened `O_TMPFILE`: that needs `libc` or a per-architecture constant, and the window between the two calls can leave only an empty file. (c) On Windows a delete-on-close file keeps its name until its last handle closes, so the scratch directory is not empty while a run is in progress there.
2. Done: the warp reads a parked frame through `SourceImage`, its planes in place from the map rather than copied back, and a parked reference becomes its stored frame without a second write of its channels. Both register-and-warp paths drop their warp buffers before the combine. Items 15.4 and 15.5 are closed; phase 11 is complete but for the host tests under Pending.
   Note: the memory plan still charges a warp worker its source frame on the spill tier. A parked source is now mapped page cache, which the kernel reclaims, so the charge is conservative rather than wrong.

## Phase 12. RAW and FITS paths (C7)

1. Replace the preview path, and remove the list in 16.2.
2. Add the integer `BlackLevel`, the one-pass normalize and checked file arithmetic.
3. Add the white-balanced demosaic and the LibRaw fallback settings. Refuse SuperCCD and odd pitch. Widen the extension list.
4. Merge the FITS entry points. Add the alias table and the streamed checksum. Write and read the `LUMFLAGS` extension, and give the stack product's flags a public view (`PixelFlags` and `LinearImage::flags` are crate-internal today).
5. Add the RCD golden cross-check (16.13).
- **Tests:** preview and science agree pixel for pixel in the former margin band. A Canon black level with `cblack` is exact against a hand sum in integers. A portrait frame through the fallback keeps W×H.
- **Closes:** group 16 except 16.12, 15.9, 17.4 to 17.7.

1. Done: `load_raw` is `load_raw_cfa`'s frame demosaicked and clamped, so the preview and the science path cannot disagree, the masked margins included; the reader crops as it normalizes, and both demosaics take cropped frames alone, which removes `BlackRepeat::at_raw`, `raw_filter_color`, `apply_bayer_black_corrections`, the clamped normalize, `CfaPattern::at_raw_origin`, `raw_xtrans_pattern`, `XTransNormalization`, `PixelSource`, `XTransImage::with_margins`, `process_xtrans` and the SIMD normalize. RCD hands back its working planes instead of copying them. `BlackLevel` holds ADU as LibRaw's `adjust_bl` folds them, in f64, and normalizes in one pass, `(v − black(x, y)) / span` rounded once; it uses the unrounded optical-black mean (`black_stat`) and a DNG's float levels where LibRaw's integers are their roundings, refuses a black that reaches `maximum` in any channel or cell, and has no `u32` sums of file data left. The LibRaw fallback sets `adjust_maximum_thr = 0`, `user_flip = 0` and `use_fuji_rotate = 0`, and accepts only 16-bit output. A SuperCCD (`fuji_width`, read through the shim) and a `raw_pitch` other than two bytes per sample are refused. Items 16.1 to 16.10 are closed.
   Deviation (16.10): the DNG float levels are used only when each lies within one ADU of LibRaw's integer for it. LibRaw folds `BlackLevelDeltaH/V` into one rounded mean, and a float that strays further was not where LibRaw's black came from.

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
- The optional `dispersion` plane of C1: the weighted scatter of the survivors.
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

## Pending

1. **The FWHM convention (phase 8).** Every width is now the PSF's before the pixel integrates it, as DAOPHOT and PSFEx report it. PixInsight and Siril fit point-sampled models, so their FWHM includes the pixel: about `√(σ² + 1/12)` in σ, 3% wider at FWHM 2.5. A `Fixed` FWHM in the config is read the same way. The alternative is to report the width with the pixel included and keep the integrated models inside.
2. **Run scratch on macOS and Windows (phase 11).** The plan asks for the scratch deletion to be tested on the macOS laptop and on Windows. Unix unlink-after-create is POSIX and runs the same code on macOS, but it is untested there; the Windows path (`FILE_FLAG_DELETE_ON_CLOSE`, the handle held with the map) compiles only under `cfg(windows)` and is untested. I need the laptop's tmux session for the first, and a Windows host for the second.

No phase needs a new dependency. `statrs` gives `Φ⁻¹` and `erf`. The cgroup limits are read from `/sys/fs/cgroup` directly (phase 11). `std` gives the Windows delete-on-close flags. `/proc/self/mountinfo` gives the file system type.
