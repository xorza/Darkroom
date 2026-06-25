# Lumos

Astronomical image-processing library: RAW/FITS decoding, master-frame calibration, star detection, star-pattern registration, frame stacking, drizzle reconstruction, and non-linear display stretching. CPU-bound with hand-written SIMD (AVX2 / SSE4.1 / NEON) hot paths and rayon parallelism; no GPU backend. Pixels are stored **planar** (one `common::Buffer2<f32>` per channel) and normalized to `[0, 1]`.

## Mission & scope

Lumos aims to be the **most precise and the fastest** astrophotography stacking pipeline there is, and is growing from "produce a good-looking image" toward a **science data product**: the calibrated, registered, **stacked** deep-sky image *plus* the ancillary per-pixel quality planes (coverage, weight, variance/noise) that let a downstream tool **measure** the result — photometry, source extraction, error bars — instead of merely viewing it.

The core deliverable is still that stacked master — load → calibrate → detect → register → combine — and it always comes first. **Science-metadata extras are welcome alongside it, but only when they stay low-complexity and don't derail the core**: they should ride cheaply on data the pipeline already computes (e.g. drizzle's `weight`/`variance` maps fall straight out of the `Σwᵢ`/`Σwᵢ²` the accumulator already tracks). Anything that adds significant machinery without serving either the image or its measurability is still **out of scope** and should be removed rather than carried.

**Precision and correctness outrank speed.** Both are first-class goals — the hot paths are aggressively optimized — but when the two conflict, the numerically-correct choice wins; never trade accuracy of the stacked result for throughput.

## Pipeline

A stack of telescope exposures → one calibrated, aligned, combined deep-sky image. The modules below are stages in that flow:

1. **Load / decode** (`io::astro_image`, `io::raw`) — FITS (pure-Rust `fits-well`), camera RAW (libraw → RCD/Markesteijn demosaic), or standard formats into a planar `AstroImage`. The calibration path keeps RAW as single-channel `CfaImage` (correct before demosaic).
2. **Calibrate** (`stacking::calibration_masters`) — stack calibration frames into master dark/flat/bias/flat-dark + defect map, then per light frame: dark-subtract → flat-divide → defect-correct, plus optional single-frame cosmic-ray rejection (L.A.Cosmic) on the calibrated `CfaImage` before demosaic.
3. **Detect stars** (`stacking::star_detection`) — six-stage detector → flux-sorted `Star`s with sub-pixel centroids and shape/quality metrics.
4. **Register** (`stacking::registration`) — triangle matching → RANSAC/MAGSAC++ transform fit → match recovery → optional SIP distortion → image warp into a common frame.
5. **Combine** — `stacking::combine` (statistical per-pixel combine with rejection/normalization/weighting, memory-tiered) **or** `stacking::drizzle` (Fruchter & Hook variable-pixel reconstruction for dithered/super-resolution sets).
6. **Stretch** (`stretching`, *display-domain, optional*) — map the linear stacked master to a viewable image with a non-linear tone curve (MTF/STF auto-stretch or color-preserving arcsinh), parameters auto-derived from the background. The science deliverable is the linear master from step 5; stretching is display-prep that runs strictly after all linear-domain work.

`math` (SIMD sums, robust statistics, transforms) and `concurrency` (`UnsafeSendPtr`) support all stages. `lib.rs` defines the entire public surface.

## Crate layout

`src/lib.rs` is the only place that `pub use`s — no intermediate re-exports. Source is organized as **features** (`stacking/`, `stretching/`) over shared **foundation** modules (`io/`, `math/`, `concurrency`), so `src/` reads as a short list of top-level concerns and new features drop in as siblings:

```
src/
├── stacking/   feature: load → calibrate → detect → register → combine into a stacked master
│   ├── calibration_masters/   star_detection/   registration/
│   └── combine/   drizzle/   pipeline/
├── stretching/ feature: post-stack display — linear master → viewable image (MTF/STF, arcsinh)
├── io/         astro_image (container + FITS/standard load) · raw (libraw decode + demosaic)
├── math/       robust stats, SIMD sum, DMat3, bbox
├── concurrency.rs  UnsafeSendPtr (send raw pointers across rayon closures)
└── testing/    #[cfg(test)] forward-model synthetic generator + real_data fixtures
```

| Module | Vis | Role |
|--------|-----|------|
| `stacking` | `pub(crate)` | Umbrella for the stacked-master feature; declares the six stage modules below (no re-exports). |
| `stacking::calibration_masters` | `pub(crate)` (types re-exported) | Master dark/flat/bias/flat-dark creation, defect maps, `calibrate()`. |
| `stacking::star_detection` | `pub(crate)` | Six-stage stellar detection + sub-pixel centroiding. |
| `stacking::registration` | `pub(crate)` | Triangle + RANSAC/MAGSAC++ star-pattern alignment, SIP distortion, image warp. |
| `stacking::combine` | `pub(crate)` | Multi-frame combination with rejection / normalization / weighting + cache tiers. (Was the old top-level `stacking`.) |
| `stacking::drizzle` | `pub(crate)` | Fruchter & Hook variable-pixel reconstruction. |
| `stacking::pipeline` | `pub(crate)` | End-to-end orchestration: `align_and_stack`, `calibrate_align_stack`. |
| `stretching` | `pub(crate)` (types re-exported) | Post-stack non-linear display stretch: MTF/STF and color-preserving arcsinh. |
| `io::astro_image` | `pub(crate)` (types re-exported) | `AstroImage` container, FITS/standard loading, metadata, CFA, sensor detection. |
| `io::raw` | `pub(crate)` | libraw RAW decode + Bayer (RCD) / X-Trans (Markesteijn) demosaicing. |
| `math` | `pub(crate)` | `DMat3`, `Aabb`/`BBox`, compensated SIMD `sum`, robust statistics. (`Vec2us` lives in the workspace `common` crate.) |
| `concurrency` | `pub(crate)` | `UnsafeSendPtr` (send raw pointers across rayon closures). |
| `testing` | `#[cfg(test)]` | Forward-model synthetic generator (`synthetic/`: `Scene` → `Camera` → `observe::render` → `SimFrame{image, truth}`, graded by `metrics`) + `real_data/` fixtures, for tests/benches. |

`common::{Buffer2, BitBuffer2, cpu_features}` (the workspace `common` crate, distinct from the in-crate `concurrency` module) underpin pixel storage and SIMD dispatch. This file is the crate-level map; read the code in each module for algorithm specifics.

## io/astro_image — image container & loading

- `AstroImage` (`io/astro_image/mod.rs:248`): `metadata: AstroImageMetadata` + `dimensions: ImageDimensions` + `pixels: PixelData`.
- `PixelData` (`mod.rs:154`): `L(Buffer2<f32>)` or `Rgb([Buffer2<f32>; 3])` — **planar**, one buffer per channel.
- `BitPix` (`mod.rs:31`, FITS pixel type + `normalization_max()`), `ImageDimensions` (`mod.rs`, `size: Vec2us` + channels ∈ {1,3}), `AstroImageMetadata` (`mod.rs:103`, full FITS/EXIF header set + CFA/filter/gain/exposure/coords).
- Entry points: `from_file` (`mod.rs:270`, dispatches FITS → `fits::load_fits`, RAW exts → `io::raw::load_raw`, else imaginarium), `from_pixels` (`mod.rs:300`, interleaved → planar), `from_planar_channels` (`mod.rs:329`). `mean()` (parallel Kahan).
- The `Rgb` value struct (`mod.rs:216`, `.intensity()` / `.scale()`). Per-pixel display transforms now live in `image_ops` (`par_map_pixels` / `intensity_plane` / `apply_intensity_remap` / `deinterleave_f32` / `interleave_f32`) over the interleaved `imaginarium::Image`, not on `AstroImage`.
- `cfa` (`CfaType` = `Mono | Bayer(CfaPattern) | XTrans([[u8;6];6])`; `CfaImage` un-demosaiced sensor data with in-place `subtract`/`divide_by_normalized` and `demosaic()` → `AstroImage`). Flat division uses **per-color-channel means** so non-white flats don't shift color.
- `fits` (`fits-well` I/O, `physical()` BSCALE/BZERO scaling, NaN/Inf sanitization, ROWORDER/XBAYROFF flips), `sensor` (`detect_sensor_type(filters, colors)` from libraw metadata), `error` (`ImageError`).

## stacking/calibration_masters — master frames & defects

- `CalibrationMasters` (`mod.rs:70`): optional master dark/flat/bias/flat-dark `CfaImage`s + `DefectMap`.
- `from_files` (`mod.rs:161`) stacks raw CFA frames through the full stacking pipeline (sigma-clipped mean at ≥8 frames, else median); `from_images` (`mod.rs:124`) builds from pre-stacked frames and derives a `DefectMap` (hot from the dark, cold/dead from the flat).
- `calibrate(&mut CfaImage)` (`mod.rs:186`): **order = dark-subtract (or bias) → flat-divide (flat-dark priority over bias) → defect-correct**, in place.
- `DefectMap` (`defect_map.rs`): hot/cold flat-index lists, built fluently from `DefectMap::default().detect_hot(&dark, σ).detect_cold(&flat)` — **hot** from the dark via per-color MAD threshold (adaptive sampling above 200K px), **cold/dead** from the flat via a same-color local-neighbour ratio (`< DEAD_PIXEL_FRACTION × local median`, robust to vignetting where a global cut can't be); `correct()` replaces defects with same-color CFA-neighbor medians. `DEFAULT_SIGMA_THRESHOLD = 5.0`.
- `cosmic_ray.rs`: `reject_cosmic_rays(&mut CfaImage, &CosmicRayConfig) -> usize` (count removed) — single-frame **L.A.Cosmic** (van Dokkum 2001) Laplacian CR/streak rejection on the calibrated, linear `CfaImage` *before* demosaic/registration (warping/demosaic would smear a hit); flagged pixels in-painted with unflagged-neighbour medians, detect→replace looped. CFA dispatch: Mono = subsampled L.A.Cosmic, Bayer = per-2×2-phase dense same-color planes, X-Trans = same-color stencils via `color_at`. `CosmicRayConfig` / `NoiseEstimation` are `pub use`d in `lib.rs`.

## io/raw — RAW decode & demosaic

- `load_raw` (`io/raw/mod.rs:722`): libraw unpack → black-level consolidation (replicates libraw `adjust_bl()`) → white-balance normalization (min multiplier = 1.0) → sensor-type dispatch: Mono (no demosaic) / Bayer (RCD) / X-Trans (Markesteijn) / Unknown (libraw fallback). Returns `AstroImage`.
- `load_raw_cfa` (`io/raw/mod.rs:792`): un-demosaiced single-channel `CfaImage` for the calibration path (defect correction before demosaic).
- `normalize.rs`: `normalize_u16_to_f32_parallel` (`clamp((v-black).max(0) * inv_range)`), SIMD SSE4.1/NEON + scalar.
- `demosaic/bayer`: `CfaPattern` (RGGB/BGGR/GRBG/GBRG, `from_bayerpat`/`flip_*`/`color_at`), `BayerImage`, `demosaic_bayer` → RCD (Ratio-Corrected Demosaicing v2.3, fused V/H direction detection, 4-px border, rayon row-parallel).
- `demosaic/xtrans`: `XTransImage` with dual `PixelSource::{U16, F32}` (u16 path normalizes on the fly to save a buffer), Markesteijn 1-pass (`markesteijn.rs` + `markesteijn_steps.rs` + precomputed `hex_lookup.rs`) into a single `DemosaicArena` (~10·P f32, RGB recomputed on-the-fly rather than materialized).

## stacking/star_detection — detection pipeline

`StarDetector` (`detector/mod.rs:82`) holds a reusable `BufferPool` (`buffer_pool.rs:14`); `detect(&AstroImage)` → `DetectionResult` (`stars: Vec<Star>` flux-sorted + `Diagnostics`).

`Star` (`star.rs:8`): `pos: DVec2`, `flux`, `fwhm`, `eccentricity`, `snr`, `peak`, `sharpness`, `roundness1`/`roundness2` (DAOFIND GROUND/SROUND), with `is_saturated`/`is_cosmic_ray`/`is_round`.

`Config` (`config.rs:184`, re-exported as `StarDetectionConfig`) is a flat struct grouped by stage, with presets `wide_field` / `high_resolution` / `crowded_field` / `precise_ground`. Notable knobs: `CentroidMethod` (`WeightedMoments | GaussianFit | MoffatFit{beta}`, `config.rs:44`), `Connectivity` (`Four | Eight`, default 8), `BackgroundRefinement` (`None | Iterative{iterations}`), `LocalBackgroundMethod` (`GlobalMap | LocalAnnulus`), optional `NoiseModel{gain, read_noise}`, deblend selector `deblend_n_thresholds` (0 = local-maxima, ≥1 = multi-threshold).

**Six stages** in `detector/stages/` (each a pure function; buffers come from the pool):
1. **prepare** (`prepare.rs:20`) — reduce to a single detection plane (copy for grayscale; inverse-variance / noise-weighted channel combination for RGB — the linear analogue of the SExtractor χ² detection image, kept linear so downstream flux/centroid stay valid); 3×3 median filter for CFA images.
2. **background** (`background/mod.rs:30`; the tiled estimator itself lives in the shared top-level `background_mesh::TileGrid`, promoted out of `star_detection` so `background_extraction` reuses it) — tiled (default 64-px, 3 clip iterations) SExtractor crowding-aware sky per tile (Pearson mode `2.5·median − 1.5·mean` of the clip survivors when `|mean − median| < 0.3σ`, median fallback when strongly skewed) + MAD σ, 3×3 tile median filter, natural bicubic-spline interpolation; optional iterative object-masking refinement for crowded fields → `BackgroundEstimate{background, noise}`.
3. **fwhm** (`fwhm.rs:50`) — manual `expected_fwhm` if set, else auto-estimate via a 2σ first pass + robust median/MAD of bright stars.
4. **detect** (`detect.rs:51`) — optional matched-filter Gaussian convolution (separable, elliptical PSF) → threshold mask → connected-component labeling (RLE + union-find, 8-connectivity) → deblend (`local_maxima` Voronoi-by-peak, ArrayVec for ≤8 peaks; or `multi_threshold` SExtractor-style tree with contrast criterion) → area/edge region filter.
5. **measure** (`measure.rs:18`, `centroid/mod.rs:286`) — stamp extraction → iterative weighted-moment centroid, then optional Levenberg-Marquardt profile fit: `gaussian_fit` (6 params) or `moffat_fit` (5–6 params) via the generic `lm_optimizer.rs` + `linear_solver.rs`; computes flux/FWHM/eccentricity/SNR/sharpness/roundness. FWHM/eccentricity come from **windowed/adaptive second moments** (`windowed_covariance`: Gaussian window iterated to match the source, then deconvolved — SExtractor WIN style) so wing noise can't inflate them; falls back to plain moments if it can't converge.
6. **filter** (`filter.rs:41`) — saturation/SNR/eccentricity/sharpness/roundness cuts, FWHM-outlier removal (median+MAD of brightest half), duplicate removal (O(n²) under 100 stars, else spatial hash), flux sort.

## stacking/registration — alignment & warp

`register(ref_stars, target_stars, config)` (`stacking/registration/mod.rs:105`) → `Result<RegistrationResult, RegistrationError>`:
1. derive `max_sigma = (median_fwhm·0.5).max(0.5)` from input FWHM,
2. select brightest ≤`max_stars`,
3. **triangle matching** (`triangle/`: k-NN invariant triangles via `spatial::KdTree`, ratio-space voting in `voting.rs`, greedy conflict resolution) → `Vec<PointMatch>`,
4. **RANSAC/MAGSAC++** (`ransac/`); `Auto` (`auto_ladder`) walks Euclidean → Similarity → Affine → Homography and takes the first model within 0.5 px RMS (simplest fit, no overfitting), falling through to Homography,
5. iterative **match recovery** (`mod.rs:376`, kd-tree NN within `~3.03·max_sigma`, ≤5 passes),
6. optional **SIP** polynomial fit, then `max_rms_error` gate.

- `Transform` (`transform.rs:70`) / `TransformType` (`transform.rs:15`): `Translation | Euclidean | Similarity | Affine | Homography | Auto` over a row-major `DMat3`; `apply`/`apply_inverse`/`inverse`/`compose`. `WarpTransform` (`transform.rs:323`) bundles a `Transform` + optional `SipPolynomial` (applies SIP first).
- `RegistrationConfig` (`config.rs:109`): star-matching, RANSAC (iterations/confidence/inlier-ratio/rotation+scale bounds/LO), SIP, and warp settings; presets `default`/`fast`/`precise`/`wide_field`/`precise_wide_field`/`mosaic`.
- `RegistrationResult` (`result.rs:104`): transform, optional `SipFitResult`, matched pairs, residuals, RMS/max error, inlier count, `quality_score`, `elapsed_ms`; `warp_transform()` builds the `WarpTransform`. `RegistrationError` (`result.rs:39`) covers insufficient stars / no patterns / RANSAC failure / accuracy.
- `ransac/`: `transforms.rs` (Procrustes for Euclidean/Similarity, Hartley-normalized least-squares for Affine, DLT-SVD for Homography), `magsac.rs` (continuous MAGSAC++ loss, no binary threshold), LO-RANSAC + adaptive iteration count.
- `distortion/`: `sip` (FITS WCS SIP polynomial, order 2–5, MAD sigma-clipped fit) is live; `tps` (thin-plate spline, `DistortionMap`) is **fully implemented but `#![allow(dead_code)]` and not wired into `register()`**.
- `warp(image, output, warp_transform, config)` (`mod.rs:243`): per-channel inverse-mapping. `InterpolationMethod` (`config.rs:24`) = `Nearest | Bilinear | Bicubic | Lanczos2/3/4{deringing}`. `interpolation/warp/` has AVX2/SSE4.1 + NEON bilinear (no-SIP, border 0) and a const-generic Lanczos path (4096-sample LUT, incremental f64 stepping, interior fast path, PixInsight-style soft-clamp deringing) whose 128-bit interior kernel is x86_64 AVX2/FMA (`warp/sse.rs`) and aarch64 NEON (`warp/neon.rs`) for all Lanczos sizes — both vectorize the deringing soft-clamp; bicubic is scalar.
- `spatial::KdTree` (`spatial/mod.rs`): flat implicit 2D k-d tree (`k_nearest_into`/`nearest_one`/`radius_indices_into`) with a stack-allocated bounded max-heap for k ≤ 32.

## stacking/combine — frame combination

`stack(paths, config, progress, cancel)` (`stack.rs:79`, from disk) and `stack_images(frames, config, progress, cancel)` (`stack.rs`, in-memory) → `AstroImage`. The trailing `cancel: common::CancelToken` is a cooperative cancel token (also on `stack_cfa_master` and `calibrate_align_stack`; pass `CancelToken::never()` to opt out): it rides on `CacheCore` and is polled at every heavy loop — RAW-decode load (per frame), the **demosaic** (`CfaImage::demosaic` threads it into the RCD + Markesteijn kernels, which check between their full-image stages and bail with `Cancelled` — so a single in-flight demosaic stops within ~one stage), star detection + registration/warp (per frame), the combine (`process_chunked{,_weighted}`, per row, since an in-memory stack is a single chunk), and defect-map detection (`DefectMap::detect_hot`/`detect_cold`, per pixel — it dominates a *cached*-master `build_masters`, so `from_images` takes a `cancel` too). `calibrate_align_stack` also caps how many lights decode+demosaic at once (`MAX_CONCURRENT_LIGHTS`, via `try_par_map_limited`): the libraw decode is the one *uninterruptible* step, so bounding it caps the work a cancel must drain and peak memory. When set, an op bails early and returns `Error::Cancelled` (or, for `from_images`, a partial map the caller must turn into an error by re-checking the token). The single libraw decode is the only step a cancel can't interrupt — at most ~`MAX_CONCURRENT_LIGHTS` in-flight decodes finish first. A `StackFrame { image, coverage: Option<Buffer2<f32>> }` bundles each in-memory frame with optional per-pixel coverage (e.g. from `warp`); plain `AstroImage`s convert via `.into()` (coverage `None` = fully covered).

- `StackConfig` (`config.rs:71`): `method: CombineMethod` (`Mean(Rejection) | Median`, `config.rs:13`), `weighting: Weighting` (`Equal | Noise | Manual(Vec<f32>)`, `config.rs:22`), `normalization: Normalization` (`None | Global | Multiplicative`, `config.rs:36`), `cache: CacheConfig`. Presets `sigma_clipped`/`winsorized`/`linear_fit`/`median`/`mean`/`gesd`/`percentile`/`weighted` + frame presets `light`/`flat`/`dark`/`bias`.
- `Rejection` (`rejection.rs:831`): `None | SigmaClip | Winsorized | LinearFit | Percentile | Gesd` (each with its own config struct).
- Two concrete caches (`cache.rs`) share the tiered store + combine engine `CacheCore` by **composition** (`{ frames, core: CacheCore }`): `CfaCache` (`CfaImage` calibration frames, `Frame`s — channels only, **no coverage**, plain combine → `CfaImage`) and `LightCache` (`AstroImage` light frames, `WeightedFrame`s — channels + optional coverage, coverage-weighted combine → `AstroImage`). Each frame is planar `Plane`s, `Memory` (RAM) or `Mapped` (mmap), chosen per stack by whether the set fits ~75% RAM; `CacheCore::process_chunks` is the one chunked-read path. Per-frame `FrameStats` (per-channel median + MAD). Coverage is type-true: it exists only on `WeightedFrame`, so `CfaCache` cannot carry it.
- Pipeline (`stack.rs`): `stack` (from paths → `LightCache`, coverage `None`) / `stack_images` (in-memory `StackFrame`s → `LightCache`) / calibration (`CfaCache::from_paths`). Each: tier-select + load → pick lowest-MAD reference → Global/Multiplicative norms → resolve weights → chunked per-pixel combine applying normalize → reject → accumulate. `run_stacking` (`CfaCache` → `CfaImage`) / `run_stacking_weighted` (`LightCache` → `AstroImage`).
- **Coverage weighting** (`LightCache::process_chunked_weighted`): a frame contributes at a pixel only where its coverage > `COVERAGE_EPSILON`, weighted by `coverage × per-frame weight`; `0` where no frame covers. Excluding sub-ε coverage keeps warp border-fill out of the rejection set, so `align_and_stack` leaves no dark warped-edge ring. Coverage is a `Plane` (so it's mmap-capable like channels; disk-backed coverage awaits the streaming-warp producer — roadmap Tier 4).

## stacking/drizzle — variable-pixel reconstruction

`drizzle_stack(paths, transforms, weights?, pixel_weight_maps?, config, progress)` (`stacking/drizzle/mod.rs`) → `DrizzleResult`: output `AstroImage` + normalized `coverage` `[0,1]`, absolute `weight` map (`Σwᵢ`, the WHT), and `variance` map (`Σwᵢ²/(Σwᵢ)²` = output variance per unit input variance — the true per-pixel noise the correlation-suppressed image RMS understates).

- `DrizzleConfig` (`mod.rs:86`): `scale`, `pixfrac`, `kernel`, `fill_value`, `min_coverage`.
- `DrizzleKernel` (`mod.rs:61`): `Square` (exact polygon clipping via Green's theorem `boxer`/`sgarea`), `Turbo` (axis-aligned, default), `Point`, `Gaussian`, `Lanczos` (valid only at pixfrac=scale=1).
- `DrizzleAccumulator` (`mod.rs:169`): per-channel flux `Buffer2` (`Σ fluxᵢ·wᵢ`) plus a single shared `weight` (`Σwᵢ`) and `weight_sq` (`Σwᵢ²`) `Buffer2` — the weight is geometric, so it's channel-independent. `add_image` maps each input pixel via `Transform`, distributes flux over the drop footprint with a local Jacobian; `finalize` normalizes `data/weight` against `min_coverage` and emits the coverage/weight/variance maps. Pure CPU + rayon-parallel finalize.

## stretching — non-linear display stretch

`Stretch { method, color }.apply(&mut imaginarium::Image) -> Result<(), OpError>` (`stretching/mod.rs`) maps a *linear* stacked master to a display image. Every display/processing op follows this imaginarium-style shape — an op-named config struct (`Stretch`, `Denoise`, `Hdr`, `LocalContrast`, `ExtractBackground`, `Scnr`, `NeutralizeBackground`) with `Default` + builder methods + an `apply(&mut Image)` that validates config + format and operates in place via `image_ops`. `apply` returns [`OpError`] (`UnsupportedFormat` unless `L_F32`/`RGB_F32`; `InvalidConfig` on out-of-range params) instead of panicking. Input may exceed `[0,1]` (a raw stack's bright stars do); every curve clamps, so output is always `[0,1]`. Runs strictly after the linear-domain work (background extraction, color calibration). Algorithm derivations live in `docs/image-stretching.md`.

- `StretchMethod`: `AutoStf{shadow_sigmas, target_background}` (MTF/STF screen-stretch — black point `median − k·σ`, midtones from the MTF self-inverse identity so the rescaled median lands on the target), `AutoAsinh{target_background}` (normalized arcsinh, `β` solved by log-space bisection so the background median maps to the target), `Asinh{beta}` (explicit); constructors `Stretch::{auto_stf, auto_asinh, ghs}`. `Stretch::apply` validates and returns `OpError::InvalidConfig` on out-of-range params.
- `ColorMode`: `ColorPreserving` (default — stretch the combined intensity `I=(r+g+b)/3`, scale every channel by `f(I)/I` with a hue-preserving highlight cap, so star color survives) or `PerChannel` (independent per-channel auto-stretch — auto-grays the background but ties color to brightness). No effect on grayscale.
- Curves are monomorphized behind a `ToneCurve` trait — `StfCurve` (clip-rescale + `MTF(m,x) = (m−1)x/((2m−1)x − m)`) and `AsinhCurve` (`asinh(x/β)/asinh(1/β)`), both clamping to `[0,1]`. `stretch` resolves the `Curve` enum **once** and runs a monomorphized loop, so the variant is never re-decided per pixel. Statistics come from `intensity_plane()` (color) or each channel (per-channel); explicit-`β` skips them.

## math — primitives

- `sum/`: `sum_f32` / `mean_f32` / `weighted_mean_f32` — hybrid compensated summation (per-lane Kahan in the SIMD inner loop, Neumaier horizontal reduction + remainder). AVX2 (8-wide) / SSE4.1 (4-wide) / NEON / scalar.
- `statistics/`: `median_f32_mut` (quickselect, NaN-safe `total_cmp`), `median_f32_fast` (NaN-free intermediate), `mad_f32_fast`, `mad_to_sigma` (`MAD_TO_SIGMA = 1.4826022`), iterative `sigma_clipped_median_mad` → `ClippedStats {median, sigma, mean}` (the survivors' mean exposes residual skew for the background's Pearson-mode estimator); `FWHM_TO_SIGMA ≈ 2.3548` lives in `mod.rs`.
- `dmat3.rs` (`DMat3`, row-major f64 homogeneous transforms + inverse/determinant), `bbox.rs` (`Aabb`, several methods test-gated). (`Vec2us` pixel indexing now lives in `common`.)

## SIMD dispatch

Runtime feature detection via the `common` crate (`cpu_features::has_avx2()` / `has_sse4_1()`); scalar fallback everywhere. Hand-written backends:

| Area | AVX2 | SSE4.1 | NEON |
|------|------|--------|------|
| `math/sum` | ✓ | ✓ | ✓ |
| `io/raw/normalize` | | ✓ | ✓ |
| `stacking/star_detection/background` | ✓ | ✓ | ✓ |
| `stacking/star_detection/convolution` | ✓ | ✓ | ✓ |
| `stacking/star_detection/threshold_mask` | | ✓ | ✓ |
| `stacking/star_detection/median_filter` | ✓ | ✓ | ✓ |
| `stacking/star_detection/centroid/{gaussian,moffat}_fit` | ✓ | | ✓ |
| `stacking/registration/interpolation/warp` (bilinear; Lanczos 128-bit FMA + deringing) | ✓ | ✓ | ✓ |

## WIP / notes

- `stacking/registration/distortion/tps` (thin-plate spline) is implemented and tested but `#![allow(dead_code)]` and **not wired** into `register()` — an alternate post-RANSAC distortion model.
- Test-only constructors/APIs are gated and kept minimal (e.g. `math/bbox`, warp params). Don't widen them for production use.
- The `real-data` feature flag (empty) gates real-data tests/benches that read the bundled `test_data/lumos_data` dataset (gitignored; present locally).
- **Test layout:** per-file unit tests are the `foo/{mod.rs, tests.rs}` split, beside the code. Module-level integration tests sit beside the implementation as `<module>/synthetic_tests[.rs|/]` (forward-model tests) and `<module>/real_data_tests.rs` (`#[cfg_attr(not(feature="real-data"), ignore)]`). `stacking::star_detection`'s shared test-output/metric infra is `stacking/star_detection/test_common/`; its synthetic tree groups `stacking/star_detection/synthetic_tests/{stage_tests, pipeline_tests}/` + `metric_curves.rs` / `subpixel_accuracy.rs`. The shared generator + graders live in `testing::synthetic`.

## Reference docs & upstream sources

- **`docs/pipeline/`** — best-practices reference for each pipeline stage, grounded in upstream source + cross-checked web research (≥2 sources per load-bearing claim). One doc per stage: `01-load-decode.md`, `02-calibration.md`, `03-star-detection.md`, `04-registration.md`, `05-stacking-drizzle.md`, plus `README.md` (index + cross-cutting "stay linear, stay calibrated" principle). These are *descriptive* references (how the field does each stage well, what to avoid) — **not** a prescriptive change list. `README.md` also tracks a table of findings flagged against lumos source (claims to verify, with file pointers; none acted on yet) — consult it before working a stage, but treat each row as unverified.
- **`docs/image-stretching.md`** — the stretch-stage algorithm reference: MTF/STF, Lupton arcsinh, Generalized Hyperbolic Stretch, CLAHE, color preservation, and workflow ordering, with the math behind the `stretching` module. Multi-source-verified; descriptive (how the field stretches well), like `docs/pipeline/`.
- **`scripts/clone-refs.sh`** — shallow-clones the upstream software whose functionality overlaps lumos into `.tmp/refs/<name>/` for source investigation (Read/Grep without per-file registry prompts; nothing is built or linked). `--list` prints the set, `--all` adds the large suites (RawTherapee, OpenCV, astropy, kstars, …), no arg clones the core set. Idempotent — an existing clone is skipped; delete its dir to refresh. Native deps (LibRaw, cfitsio) are pinned to `Cargo.lock` versions; everything else tracks upstream HEAD. Each entry's comment names the lumos module it informs (e.g. `sep`/`sextractor`/`photutils` → `stacking::star_detection`, `magsac`/`astroalign` → `stacking::registration`, `drizzle` → `stacking::drizzle`).
- **`.tmp/refs/`** is gitignored and persists across sessions. Run the script before reading upstream source; the `docs/pipeline/` docs were built from these clones.

## Commands

```bash
cargo build -p lumos --release
cargo test -p lumos
cargo test -p lumos --release <bench_name> -- --ignored --nocapture   # benches (e.g. bench_rcd, bench_bayer)

scripts/clone-refs.sh          # clone core upstream refs into .tmp/refs/ (--all for large suites, --list to preview)
```

## Benchmarks (quickbench)

Benches are `#[quick_bench]` fns (expand to `#[test] #[ignore]`) in per-module `bench.rs` files — e.g. `stacking/registration/interpolation/bench.rs`, plus RCD/Bayer/stacking. No `cargo bench` target; they run through `cargo test`.

- **Run** — always `--release` (debug numbers are meaningless and print a warning):
  ```bash
  cargo test -p lumos --release <filter> -- --ignored --nocapture
  ```
  `<filter>` is a substring of the test path: a bench name (`bench_warp_lanczos3_1k`) or a whole bench module (`interpolation::bench`). Omit it to run every bench.
- **Auto-comparison is the baseline mechanism.** Each bench writes `bench-results/<name>.txt` (gitignored); the next run prints a coloured `faster`/`SLOWER` diff against it (±5% threshold). To measure an optimization: run once (baseline) → make the change → run again, the diff is automatic. The file is overwritten each run, so to keep a baseline across several iterations, copy `bench-results/` aside first (into `.tmp/`).
- For SIMD/kernel work, prefer the **single-thread** variants (e.g. `bench_warp_lanczos3_1k_single_thread`) to isolate per-thread throughput from rayon + memory effects; the multi-thread benches show realistic end-to-end time. **aarch64 is the profiled target** (NEON mandatory; x86 SIMD is runtime-detected — see the SIMD dispatch table above).
