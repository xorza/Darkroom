# Lumos

Astronomical image-processing library: RAW/FITS decoding, master-frame calibration, star detection, star-pattern registration, frame stacking, and drizzle reconstruction. CPU-bound with hand-written SIMD (AVX2 / SSE4.1 / NEON) hot paths and rayon parallelism; no GPU backend. Pixels are stored **planar** (one `common::Buffer2<f32>` per channel) and normalized to `[0, 1]`.

## Crate layout

`src/lib.rs` is the only place that `pub use`s — no intermediate re-exports. Seven domain modules plus support:

| Module | Vis | Role |
|--------|-----|------|
| `astro_image` | private (types re-exported) | `AstroImage` container, FITS/RAW/standard loading, metadata, CFA, sensor detection. |
| `calibration_masters` | private (types re-exported) | Master dark/flat/bias/flat-dark creation, defect maps, `calibrate()`. |
| `raw` | `pub` | libraw RAW decode + Bayer (RCD) / X-Trans (Markesteijn) demosaicing. |
| `star_detection` | `pub(crate)` | Six-stage stellar detection + sub-pixel centroiding. |
| `registration` | `pub(crate)` | Triangle + RANSAC/MAGSAC++ star-pattern alignment, SIP distortion, image warp. |
| `stacking` | `pub(crate)` | Multi-frame combination with rejection / normalization / weighting + cache tiers. |
| `drizzle` | `pub` | Fruchter & Hook variable-pixel reconstruction. |
| `math` | `pub(crate)` | `DMat3`, `Aabb`/`BBox`, `Vec2us`, compensated SIMD `sum`, robust statistics. |
| `common` | `pub(crate)` | `UnsafeSendPtr` + thin re-use of the `common` crate's CPU feature detection. |
| `prelude` | `pub` | Convenience re-exports of the loading / detection / registration / stacking API. |
| `testing` | `#[cfg(test)]` | Synthetic star-field + real-data fixtures for tests/benches. |

`common::{Buffer2, BitBuffer2, cpu_features}` (the workspace `common` crate, not the in-crate `common` module) underpin pixel storage and SIMD dispatch. This file is the crate-level map; read the code in each domain module for algorithm specifics.

## astro_image — image container & loading

- `AstroImage` (`astro_image/mod.rs:251`): `metadata: AstroImageMetadata` + `dimensions: ImageDimensions` + `pixels: PixelData`.
- `PixelData` (`mod.rs:186`): `L(Buffer2<f32>)` or `Rgb([Buffer2<f32>; 3])` — **planar**, one buffer per channel.
- `BitPix` (`mod.rs:27`, FITS pixel type + `normalization_max()`), `ImageDimensions` (`mod.rs:85`, width/height/channels ∈ {1,3}), `AstroImageMetadata` (`mod.rs:134`, full FITS/EXIF header set + CFA/filter/gain/exposure/coords).
- Entry points: `from_file` (`mod.rs:268`, dispatches FITS → `fits::load_fits`, RAW exts → `raw::load_raw`, else imaginarium), `from_pixels` (`mod.rs:291`, interleaved → planar), `from_planar_channels` (`mod.rs:320`). `mean()` (parallel Kahan), `into_grayscale()` (Rec.709 luminance).
- `cfa` (`CfaType` = `Mono | Bayer(CfaPattern) | XTrans([[u8;6];6])`; `CfaImage` un-demosaiced sensor data with in-place `subtract`/`divide_by_normalized` and `demosaic()` → `AstroImage`). Flat division uses **per-color-channel means** so non-white flats don't shift color.
- `fits` (cfitsio I/O, header parsing, NaN/Inf sanitization, ROWORDER/XBAYROFF flips), `sensor` (`detect_sensor_type(filters, colors)` from libraw metadata), `error` (`ImageError`).

## calibration_masters — master frames & defects

- `CalibrationMasters` (`mod.rs:66`): optional master dark/flat/bias/flat-dark `CfaImage`s + `DefectMap`.
- `from_files` (`mod.rs:146`) stacks raw CFA frames through the full stacking pipeline (sigma-clipped mean at ≥8 frames, else median); `from_images` (`mod.rs:117`) builds from pre-stacked frames and derives a `DefectMap` from the dark.
- `calibrate(&mut CfaImage)` (`mod.rs:171`): **order = dark-subtract (or bias) → flat-divide (flat-dark priority over bias) → defect-correct**, in place.
- `DefectMap` (`defect_map.rs:40`): hot/cold flat-index lists from per-color MAD thresholds (`from_master_dark`, adaptive sampling above 200K px); `correct()` replaces defects with same-color CFA-neighbor medians. `DEFAULT_SIGMA_THRESHOLD = 5.0`.

## raw — RAW decode & demosaic

- `load_raw` (`raw/mod.rs:722`): libraw unpack → black-level consolidation (replicates libraw `adjust_bl()`) → white-balance normalization (min multiplier = 1.0) → sensor-type dispatch: Mono (no demosaic) / Bayer (RCD) / X-Trans (Markesteijn) / Unknown (libraw fallback). Returns `AstroImage`.
- `load_raw_cfa` (`raw/mod.rs:792`): un-demosaiced single-channel `CfaImage` for the calibration path (defect correction before demosaic).
- `normalize.rs`: `normalize_u16_to_f32_parallel` (`clamp((v-black).max(0) * inv_range)`), SIMD SSE4.1/NEON + scalar.
- `demosaic/bayer`: `CfaPattern` (RGGB/BGGR/GRBG/GBRG, `from_bayerpat`/`flip_*`/`color_at`), `BayerImage`, `demosaic_bayer` → RCD (Ratio-Corrected Demosaicing v2.3, fused V/H direction detection, 4-px border, rayon row-parallel).
- `demosaic/xtrans`: `XTransImage` with dual `PixelSource::{U16, F32}` (u16 path normalizes on the fly to save a buffer), Markesteijn 1-pass (`markesteijn.rs` + `markesteijn_steps.rs` + precomputed `hex_lookup.rs`) into a single `DemosaicArena` (~10·P f32, RGB recomputed on-the-fly rather than materialized).

## star_detection — detection pipeline

`StarDetector` (`detector/mod.rs:82`) holds a reusable `BufferPool` (`buffer_pool.rs:14`); `detect(&AstroImage)` (`detector/mod.rs:122`) → `DetectionResult` (`stars: Vec<Star>` flux-sorted + `Diagnostics`). `detect_file` (`mod.rs:228`) loads then detects; `detection_file::save_detection_result` writes results.

`Star` (`star.rs:8`): `pos: DVec2`, `flux`, `fwhm`, `eccentricity`, `snr`, `peak`, `sharpness`, `roundness1`/`roundness2` (DAOFIND GROUND/SROUND), with `is_saturated`/`is_cosmic_ray`/`is_round`.

`Config` (`config.rs:184`, re-exported as `StarDetectionConfig`) is a flat struct grouped by stage, with presets `wide_field` / `high_resolution` / `crowded_field` / `precise_ground`. Notable knobs: `CentroidMethod` (`WeightedMoments | GaussianFit | MoffatFit{beta}`, `config.rs:44`), `Connectivity` (`Four | Eight`, default 8), `BackgroundRefinement` (`None | Iterative{iterations}`), `LocalBackgroundMethod` (`GlobalMap | LocalAnnulus`), optional `NoiseModel{gain, read_noise}`, deblend selector `deblend_n_thresholds` (0 = local-maxima, ≥1 = multi-threshold).

**Six stages** in `detector/stages/` (each a pure function; buffers come from the pool):
1. **prepare** (`prepare.rs:20`) — reduce to a single detection plane (copy for grayscale; inverse-variance / noise-weighted channel combination for RGB — the linear analogue of the SExtractor χ² detection image, kept linear so downstream flux/centroid stay valid); 3×3 median filter for CFA images.
2. **background** (`background/mod.rs:30`, `estimate.rs`) — tiled sigma-clipped mean/σ (default 64-px tiles, 3 iterations) + bilinear interpolation; optional iterative object-masking refinement for crowded fields → `BackgroundEstimate{background, noise}`.
3. **fwhm** (`fwhm.rs:50`) — manual `expected_fwhm` if set, else auto-estimate via a 2σ first pass + robust median/MAD of bright stars.
4. **detect** (`detect.rs:51`) — optional matched-filter Gaussian convolution (separable, elliptical PSF) → threshold mask → connected-component labeling (RLE + union-find, 8-connectivity) → deblend (`local_maxima` Voronoi-by-peak, ArrayVec for ≤8 peaks; or `multi_threshold` SExtractor-style tree with contrast criterion) → area/edge region filter.
5. **measure** (`measure.rs:18`, `centroid/mod.rs:286`) — stamp extraction → iterative weighted-moment centroid, then optional Levenberg-Marquardt profile fit: `gaussian_fit` (6 params) or `moffat_fit` (5–6 params) via the generic `lm_optimizer.rs` + `linear_solver.rs`; computes flux/FWHM/eccentricity/SNR/sharpness/roundness.
6. **filter** (`filter.rs:41`) — saturation/SNR/eccentricity/sharpness/roundness cuts, FWHM-outlier removal (median+MAD of brightest half), duplicate removal (O(n²) under 100 stars, else spatial hash), flux sort.

## registration — alignment & warp

`register(ref_stars, target_stars, config)` (`registration/mod.rs:105`) → `Result<RegistrationResult, RegistrationError>`:
1. derive `max_sigma = (median_fwhm·0.5).max(0.5)` from input FWHM,
2. select brightest ≤`max_stars`,
3. **triangle matching** (`triangle/`: k-NN invariant triangles via `spatial::KdTree`, ratio-space voting in `voting.rs`, greedy conflict resolution) → `Vec<PointMatch>`,
4. **RANSAC/MAGSAC++** (`ransac/`) with `Auto` upgrading Similarity→Homography when RMS > 0.5 px,
5. iterative **match recovery** (`mod.rs:376`, kd-tree NN within `~3.03·max_sigma`, ≤5 passes),
6. optional **SIP** polynomial fit, then `max_rms_error` gate.

- `Transform` (`transform.rs:70`) / `TransformType` (`transform.rs:15`): `Translation | Euclidean | Similarity | Affine | Homography | Auto` over a row-major `DMat3`; `apply`/`apply_inverse`/`inverse`/`compose`. `WarpTransform` (`transform.rs:323`) bundles a `Transform` + optional `SipPolynomial` (applies SIP first).
- `RegistrationConfig` (`config.rs:109`): star-matching, RANSAC (iterations/confidence/inlier-ratio/rotation+scale bounds/LO), SIP, and warp settings; presets `default`/`fast`/`precise`/`wide_field`/`precise_wide_field`/`mosaic`.
- `RegistrationResult` (`result.rs:104`): transform, optional `SipFitResult`, matched pairs, residuals, RMS/max error, inlier count, `quality_score`, `elapsed_ms`; `warp_transform()` builds the `WarpTransform`. `RegistrationError` (`result.rs:39`) covers insufficient stars / no patterns / RANSAC failure / accuracy.
- `ransac/`: `transforms.rs` (Procrustes for Euclidean/Similarity, Hartley-normalized least-squares for Affine, DLT-SVD for Homography), `magsac.rs` (continuous MAGSAC++ loss, no binary threshold), LO-RANSAC + adaptive iteration count.
- `distortion/`: `sip` (FITS WCS SIP polynomial, order 2–5, MAD sigma-clipped fit) is live; `tps` (thin-plate spline, `DistortionMap`) is **fully implemented but `#![allow(dead_code)]` and not wired into `register()`**.
- `warp(image, output, warp_transform, config)` (`mod.rs:243`): per-channel inverse-mapping. `InterpolationMethod` (`config.rs:24`) = `Nearest | Bilinear | Bicubic | Lanczos2/3/4{deringing}`. `interpolation/warp/` has AVX2/SSE4.1 bilinear (no-SIP, border 0) and a const-generic Lanczos path (4096-sample LUT, incremental f64 stepping, interior fast path, PixInsight-style soft-clamp deringing, Lanczos3/AVX2 FMA); other methods are scalar.
- `spatial::KdTree` (`spatial/mod.rs`): flat implicit 2D k-d tree (`k_nearest`/`nearest_one`/`radius_indices_into`) with a stack-allocated bounded max-heap for k ≤ 32.

## stacking — frame combination

`stack(paths, frame_type, config)` (`stack.rs:79`) and `stack_with_progress(...)` (`stack.rs:91`) → `AstroImage`. `FrameType` = `Dark | Flat | Bias | Light`.

- `StackConfig` (`config.rs:71`): `method: CombineMethod` (`Mean(Rejection) | Median`, `config.rs:13`), `weighting: Weighting` (`Equal | Noise | Manual(Vec<f32>)`, `config.rs:22`), `normalization: Normalization` (`None | Global | Multiplicative`, `config.rs:36`), `cache: CacheConfig`. Presets `sigma_clipped`/`winsorized`/`linear_fit`/`median`/`mean`/`gesd`/`percentile`/`weighted` + frame presets `light`/`flat`/`dark`/`bias`.
- `Rejection` (`rejection.rs:831`): `None | SigmaClip | Winsorized | LinearFit | Percentile | Gesd` (each with its own config struct).
- `ImageCache<I>` (`cache.rs:152`): selects **in-memory** (fits in ~75% RAM) vs **disk-backed mmap** tiers; computes per-frame `FrameStats` (`cache.rs:37`, per-channel median + MAD).
- Pipeline (`stack.rs`): tier-select + load → pick lowest-MAD reference → compute Global/Multiplicative norms → resolve weights → chunked per-pixel combine (adaptive row chunks bounded by memory) applying normalize → reject → accumulate.

## drizzle — variable-pixel reconstruction

`drizzle_stack(paths, transforms, weights?, pixel_weight_maps?, config, progress)` (`drizzle/mod.rs:937`) → `DrizzleResult` (output `AstroImage` + normalized coverage map).

- `DrizzleConfig` (`mod.rs:86`): `scale`, `pixfrac`, `kernel`, `fill_value`, `min_coverage`.
- `DrizzleKernel` (`mod.rs:61`): `Square` (exact polygon clipping via Green's theorem `boxer`/`sgarea`), `Turbo` (axis-aligned, default), `Point`, `Gaussian`, `Lanczos` (valid only at pixfrac=scale=1).
- `DrizzleAccumulator` (`mod.rs:169`): paired flux + weight `Buffer2` per channel; `add_image` maps each input pixel via `Transform`, distributes flux over the drop footprint with a local Jacobian, `finalize` normalizes `data/weights` against `min_coverage`. Pure CPU + rayon-parallel finalize.

## math — primitives

- `sum/`: `sum_f32` / `mean_f32` / `weighted_mean_f32` — hybrid compensated summation (per-lane Kahan in the SIMD inner loop, Neumaier horizontal reduction + remainder). AVX2 (8-wide) / SSE4.1 (4-wide) / NEON / scalar.
- `statistics/`: `median_f32_mut` (quickselect, NaN-safe `total_cmp`), `median_f32_fast` (NaN-free intermediate), `mad_f32_fast`, `mad_to_sigma` (`MAD_TO_SIGMA = 1.4826022`), iterative sigma-clipped stats; `FWHM_TO_SIGMA ≈ 2.3548` lives in `mod.rs`.
- `dmat3.rs` (`DMat3`, row-major f64 homogeneous transforms + inverse/determinant), `bbox.rs` (`Aabb`, several methods test-gated), `vec2us.rs` (`Vec2us` pixel indexing).

## SIMD dispatch

Runtime feature detection via the `common` crate (`cpu_features::has_avx2()` / `has_sse4_1()`); scalar fallback everywhere. Hand-written backends:

| Area | AVX2 | SSE4.1 | NEON |
|------|------|--------|------|
| `math/sum` | ✓ | ✓ | ✓ |
| `raw/normalize` | | ✓ | ✓ |
| `star_detection/background` | | ✓ | ✓ |
| `star_detection/convolution` | | ✓ | ✓ |
| `star_detection/threshold_mask` | | ✓ | ✓ |
| `star_detection/median_filter` | | ✓ | ✓ |
| `star_detection/centroid/{gaussian,moffat}_fit` | ✓ | | ✓ |
| `registration/interpolation/warp` (bilinear; Lanczos3 FMA) | ✓ | ✓ | scalar |

## WIP / notes

- `registration/distortion/tps` (thin-plate spline) is implemented and tested but `#![allow(dead_code)]` and **not wired** into `register()` — an alternate post-RANSAC distortion model.
- Test-only constructors/APIs are gated and kept minimal (e.g. `math/bbox`, warp params). Don't widen them for production use.
- The `real-data` feature flag (empty) gates real-data tests/benches that read a calibration directory from the environment.

## Commands

```bash
cargo build -p lumos --release
cargo test -p lumos
cargo test -p lumos --release <bench_name> -- --ignored --nocapture   # benches (e.g. bench_rcd, bench_bayer)
```
