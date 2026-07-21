# Lumos

Astronomical image-processing library: RAW/FITS decoding, master-frame calibration, star detection, star-pattern registration, frame stacking, drizzle reconstruction, and non-linear display stretching. CPU-bound with hand-written SIMD (AVX2 / SSE4.1 / NEON) hot paths and rayon parallelism; no GPU backend. Pixels are stored **planar** (one `imaginarium::Buffer2<f32>` per channel) and normalized to `[0, 1]`.

## Mission & scope

Lumos aims to be the **most precise and the fastest** astrophotography stacking pipeline there is, and is growing from "produce a good-looking image" toward a **science data product**: the calibrated, registered, **stacked** deep-sky image *plus* the ancillary per-pixel quality planes (coverage, weight, variance/noise) that let a downstream tool **measure** the result — photometry, source extraction, error bars — instead of merely viewing it.

The core deliverable is still that stacked master — load → calibrate → detect → register → combine — and it always comes first. **Science-metadata extras are welcome alongside it, but only when they stay low-complexity and don't derail the core**: they should ride cheaply on data the pipeline already computes (e.g. drizzle's `weight`/`linear_variance` maps fall straight out of the `Σwᵢ`/`Σwᵢ²` the accumulator already tracks). Anything that adds significant machinery without serving either the image or its measurability is still **out of scope** and should be removed rather than carried.

**Precision and correctness outrank speed.** Both are first-class goals — the hot paths are aggressively optimized — but when the two conflict, the numerically-correct choice wins; never trade accuracy of the stacked result for throughput.

## Pipeline

A stack of telescope exposures → one calibrated, aligned, combined deep-sky image. The modules below are stages in that flow:

1. **Load / decode** (`io::astro_image`, `io::raw`) — FITS (pure-Rust `fits-well`), camera RAW (libraw → RCD/Markesteijn demosaic), or standard formats into a planar `AstroImage`. The calibration path keeps RAW as single-channel `CfaImage` (correct before demosaic).
2. **Calibrate** (`stacking::calibration_masters`) — stack calibration frames into master dark/flat/bias/flat-dark + defect map; hot detection thresholds per-color residuals after a robust 64×64-tile dark-background fit, while cold detection reads the subtracted unfloored flat. Per light frame: dark-subtract → flat-divide → defect-correct, plus optional single-frame cosmic-ray rejection (L.A.Cosmic) on the calibrated `CfaImage` before demosaic.
3. **Detect stars** (`stacking::star_detection`) — six-stage detector → flux-sorted `Star`s with sub-pixel centroids and shape/quality metrics.
4. **Register** (`stacking::registration`) — triangle matching → RANSAC/MAGSAC++ transform fit → match recovery → optional SIP distortion → image warp into a common frame.
5. **Combine** — `stacking::combine` (statistical per-pixel combine with rejection/normalization/weighting, memory-tiered) **or** `stacking::drizzle` (Fruchter & Hook variable-pixel reconstruction for dithered/super-resolution sets).
6. **Stretch** (`stretching`, *display-domain, optional*) — map the linear stacked master to a viewable image with a non-linear tone curve (MTF/STF auto-stretch or color-preserving arcsinh), parameters auto-derived from the background. The science deliverable is the linear master from step 5; stretching is display-prep that runs strictly after all linear-domain work.

`math` (SIMD sums, robust statistics, transforms) and `concurrency` (bounded Rayon mapping, pointer safety, and reusable per-job scratch) support all stages. `lib.rs` defines the entire public surface.

## Crate layout

`src/lib.rs` is the only place that `pub use`s — no intermediate re-exports. Source is organized as **features** (`stacking/`, `image_ops/`) over shared **foundation** modules (`io/`, `math/`, `background_mesh/`, `concurrency`), so `src/` reads as a short list of top-level concerns and new features drop in as siblings:

```
src/
├── stacking/   feature: load → calibrate → detect → register → combine into a stacked master
│   ├── calibration_masters/   star_detection/   registration/
│   └── frame_store/   combine/   drizzle/   pipeline/   progress.rs
├── image_ops/  feature: in-place display/processing ops on a linear f32 master (imaginarium Image)
│   ├── mod.rs (par_map_pixels / intensity / interleave helpers)   rgb/   op.rs (OpError contract)   wavelet/
│   ├── stretching/ (post-stack display: MTF/STF, arcsinh)   denoise/   hdr/   local_contrast/
│   └── color_calibration/ (Scnr, NeutralizeBackground)   background_extraction/   ml/ (feature-gated)
├── io/         astro_image (container + FITS/standard load) · raw (libraw decode + demosaic)
├── math/       robust stats, SIMD sum, DMat3, bbox, Vec2us
├── background_mesh/  shared SExtractor-style TileGrid (used by star_detection + background_extraction)
├── bit_buffer2/  packed boolean masks with 128-bit row padding
├── concurrency/  bounded mapping + UnsafeSendPtr + reusable Rayon job scratch leases
└── testing/    #[cfg(test)] forward-model synthetic generator + real_data fixtures
```

Every op in `image_ops/` is an op-named config struct (`Stretch`, `Denoise`, `Hdr`, `LocalContrast`, `ExtractBackground`, `Scnr`, `NeutralizeBackground`, ML denoise/star-removal) with `Default` + builder methods + an in-place `apply(&mut Image) -> Result<(), OpError>`; they share the `op` contract (format/config validation) and the `mod.rs` per-pixel helpers, and the spatial ones lean on `wavelet` (à-trous starlet). The public types are `pub use`d from `lib.rs` unchanged (`lumos::Stretch`, `lumos::Denoise`, …).

| Module | Vis | Role |
|--------|-----|------|
| `stacking` | `pub(crate)` | Umbrella for the stacked-master feature; declares its stage and support modules without re-exports. |
| `stacking::calibration_masters` | `pub(crate)` (types re-exported) | Master dark/flat/bias/flat-dark creation, defect maps, `calibrate()`. |
| `stacking::star_detection` | `pub(crate)` | Six-stage stellar detection + sub-pixel centroiding. |
| `stacking::registration` | `pub(crate)` | Triangle + RANSAC/MAGSAC++ star-pattern alignment, SIP distortion, image warp. |
| `stacking::frame_store` | `pub(crate)` | Shared memory planning, RAM/mmap planes, spill lifetime, and stored frame records. |
| `stacking::combine` | `pub(crate)` | Multi-frame combination with rejection / normalization / weighting + cache tiers. (Was the old top-level `stacking`.) |
| `stacking::drizzle` | `pub(crate)` | Fruchter & Hook variable-pixel reconstruction. |
| `stacking::pipeline` | `pub(crate)` | End-to-end orchestration: `align_and_stack`, `calibrate_align_stack`. |
| `stacking::progress` | `pub(crate)` (types re-exported) | Progress reporting shared by combine, drizzle, calibration, and pipeline. |
| `image_ops` | `pub(crate)` (types re-exported) | Umbrella for in-place display/processing ops on a linear f32 master; holds the `op` contract, per-pixel/interleave helpers, and `wavelet`. |
| `image_ops::stretching` | `pub(crate)` (types re-exported) | Post-stack non-linear display stretch: MTF/STF and color-preserving arcsinh. |
| `image_ops::{denoise, hdr, local_contrast, color_calibration, background_extraction}` | `pub(crate)` (types re-exported) | Wavelet denoise, HDR tone-compression, local contrast, SCNR/background neutralization, gradient background extraction. |
| `image_ops::ml` | `pub(crate)`, `#[cfg(feature = "ml")]` | ONNX-backed ML denoise + star removal (tiled inference). |
| `io::astro_image` | `pub(crate)` (types re-exported) | `AstroImage` container, FITS/standard loading, metadata, CFA, sensor detection. |
| `io::raw` | `pub(crate)` | libraw RAW decode + Bayer (RCD) / X-Trans (Markesteijn) demosaicing; owns the authoritative `RAW_EXTENSIONS` policy. |
| `math` | `pub(crate)` | `Vec2us`, `DMat3`, half-open `Rect`/`URect`, compensated SIMD `sum`, robust statistics. `Vec2us` is re-exported as part of Lumos's public image-dimension API. |
| `bit_buffer2` | `pub(crate)` | Packed boolean masks with checked layout arithmetic and 128-bit row padding for word kernels. |
| `concurrency` | `pub(crate)` | Input-order-preserving fallible bounded Rayon mapping, `UnsafeSendPtr`, and reusable per-job scratch leases. |
| `testing` | `#[cfg(test)]` | Forward-model synthetic generator (`synthetic/`: `Scene` → `Camera` → `observe::render` → `SimFrame{image, truth}`, graded by `metrics`) + `real_data/` fixtures, for tests/benches. |

`imaginarium::Buffer2` underpins planar pixel storage, its shared `cpu_features` detector gates SIMD dispatch, and Lumos owns its packed `BitBuffer2` masks. The workspace `common` crate supplies cross-crate contracts such as serialization and cancellation. This file is the crate-level map; read the code in each module for algorithm specifics.

## io/astro_image — image container & loading

- `AstroImage` (`io/astro_image/mod.rs:248`): `metadata: AstroImageMetadata` + `dimensions: ImageDimensions` + `pixels: PixelData`.
- `PixelData` (`mod.rs:154`): `L(Buffer2<f32>)` or `Rgb([Buffer2<f32>; 3])` — **planar**, one buffer per channel.
- `BitPix` (`mod.rs:31`, FITS pixel type + `normalization_max()`), opaque `ImageDimensions` (`mod.rs`, non-zero size + channels ∈ {1,3}, exposed through immutable accessors), `AstroImageMetadata` (`mod.rs:103`, full FITS/EXIF header set + CFA/filter/gain/exposure/coords).
- Entry points: `AstroImage::from_file` admits linear non-mosaic FITS and float TIFF, `CfaImage::from_file` admits camera RAW and mosaic FITS, and `PreviewImage::from_file` owns the public `PREVIEW_IMAGE_EXTENSIONS` display policy; `AstroImage::from_pixels` and `from_planar_channels` build explicit in-memory scientific products. `mean()` uses parallel Kahan summation.
- The crate-private `image_ops::rgb::Rgb` value struct (`image_ops/rgb/mod.rs`, `.intensity()` / `.scale()`) supports display transforms over the interleaved `imaginarium::Image`: per-pixel operations use `par_map_pixels`, intensity operations use `intensity_plane` / `apply_intensity_remap`, and spatial operations stream through `process_channels`. Full planar conversion is private to the optional ML backend's model boundary.
- `cfa` (`CfaType` = `Mono | Bayer(CfaPattern) | XTrans([[u8;6];6])`; `CfaImage` un-demosaiced sensor data with in-place `subtract` and `demosaic()` → `AstroImage`). Calibration-master construction prepares the reusable flat divisor with **per-color-channel means** so non-white flats don't shift color.
- `fits` (`fits-well` I/O, `physical_f32()` BSCALE/BZERO scaling, null rejection, physical float preservation, ROWORDER/XBAYROFF flips), `sensor` (`detect_sensor_type(filters, colors)` from libraw metadata), `error` (`ImageError`).

## stacking/calibration_masters — master frames & defects

- `CalibrationSet<T>` names dark/flat/bias/flat-dark once for raw path slices and prebuilt master inputs. `CalibrationMasters` stores the source masters, a derived `DefectMap`, and its flat `CfaImage` in the already-prepared state; `from_images`/`from_files` keep them synchronized. `components()` and `defect_summary()` expose read-only derived state; `Default` is the valid empty bundle.
- `from_files` (`mod.rs`) stacks raw CFA frames through the full stacking pipeline (sigma-clipped mean at ≥8 frames, else median); `from_images` (`mod.rs`) derives hot defects from background-subtracted dark residuals, subtracts flat-dark/bias from the flat, detects cold defects from that unfloored response, then consumes it into a per-color normalized, clamped divisor. `save`/`load` persist this prepared bundle in a versioned cache, so cache hits repeat neither defect detection nor flat preparation.
- `calibrate(&mut CfaImage) -> Result<(), CalibrationError>` (`mod.rs`): validates matching light/master CFA metadata before mutation, then **order = dark-subtract (or bias) → prepared-flat divide → defect-correct**, in place.
- `DefectMap` (`defect_map/mod.rs`): hot/cold flat-index lists, built fluently from `DefectMap::default().detect_hot(&dark, σ).detect_cold(&flat)` — **hot** from per-color residuals after a robust 64×64-tile broad-dark fit, with adaptive sampling capped at 100K residuals per color. Its scale is the maximum of MAD σ, the Gaussian-calibrated 99th absolute-residual percentile (protecting against broad/column model error), and the RAW quantization σ propagated through master stacking (with f32 resolution fallback); **cold/dead** comes from the flat via a same-color local-neighbour ratio (`< DEAD_PIXEL_FRACTION × local median`, robust to vignetting where a global cut can't be). `correct()` replaces defects with same-color CFA-neighbor medians. `DEFAULT_SIGMA_THRESHOLD = 5.0`.
- `cosmic_ray.rs`: `reject_cosmic_rays(&mut CfaImage, &CosmicRayConfig) -> usize` (count removed) — single-frame **L.A.Cosmic** (van Dokkum 2001) Laplacian CR/streak rejection on the calibrated, linear `CfaImage` *before* demosaic/registration (warping/demosaic would smear a hit); flagged pixels in-painted with unflagged-neighbour medians, detect→replace looped. CFA dispatch: Mono = subsampled L.A.Cosmic, Bayer = per-2×2-phase dense same-color planes, X-Trans = same-color stencils via `color_at`. `CosmicRayConfig` / `NoiseEstimation` are `pub use`d in `lib.rs`.

## io/raw — RAW decode & demosaic

- `load_raw` (`io/raw/mod.rs`): libraw unpack → black-level consolidation and subtraction (including residual spatial repeats in visible coordinates) → sensor-type dispatch: Mono (no demosaic) / Bayer (RCD) / X-Trans (Markesteijn) / Unknown (libraw fallback). Camera white balance is deliberately disabled on every path, keeping channels raw-linear for calibration, stacking, and later explicit color calibration; canonical `[R, G1, B, G2]` camera multipliers are retained as metadata only. Demosaic kernels always emit unclipped linear RGB; this direct-load entry point clamps the completed image to `[0,1]` once.
- `load_raw_cfa` (`io/raw/mod.rs:792`): un-demosaiced single-channel `CfaImage` for the calibration path (defect correction before demosaic). The CFA image retains quantization σ from LibRaw's actual black-to-maximum range; CFA stacking propagates each frame's uncertainty through normalization, weighting, and actual rejection survivors for zero-MAD defect detection.
- `normalize.rs`: `normalize_u16_to_f32_parallel` (`clamp((v-black).max(0) * inv_range)`), SIMD SSE4.1/NEON + scalar.
- `demosaic/bayer`: `CfaPattern` (RGGB/BGGR/GRBG/GBRG, `from_bayerpat`/`flip_*`/`color_at`), `BayerImage`, `demosaic_bayer` → unclipped RCD (Ratio-Corrected Demosaicing v2.3, fused V/H direction detection, 4-px border, rayon row-parallel). Nonnegative and well-conditioned signed low-pass neighborhoods use canonical ratio correction exactly; near signed denominator cancellation, a smoothstep transition reaches the denominator-free additive midpoint estimate `G₁ + (LPF₀−LPF₂)/8`.
- `demosaic/xtrans`: `XTransImage` reads raw u16 data through plain or spatial-repeat-corrected sources (normalizing on demand to save a buffer), or calibrated f32 data directly. Unclipped Markesteijn 1-pass (`markesteijn.rs` + `markesteijn_steps.rs` + precomputed `hex_lookup.rs`) uses a single 18·P-f32 `DemosaicArena`. The four directional green and `[red, blue]` candidates are materialized through Markesteijn's solitary-green, opposite-color, and 2×2-green reconstruction stages before derivative voting and blending. Equivalent u16/f32 inputs follow identical interpolation and homogeneity selection.

## stacking/star_detection — detection pipeline

`StarDetector` holds reusable `DetectionResources` (pooled image buffers plus background mesh/interpolation workspaces); fallible `from_config` validates once through `StarDetectionConfigError`, then `detect(&AstroImage)` → `DetectionResult` (`stars: Vec<Star>` flux-sorted + `Diagnostics`). Measurement constructs `Star` directly, while filtering produces the `QualityFilterDiagnostics` component stored unchanged in `Diagnostics`. Alignment pipelines propagate invalid detection configuration through `AlignStackError` before doing useful work.

`Star` (`star.rs:8`): `pos: DVec2`, `flux`, `fwhm`, `eccentricity`, `snr`, `peak`, `sharpness`, `roundness1`/`roundness2` (DAOFIND GROUND/SROUND), with `is_saturated`/`is_cosmic_ray`/`is_round`.

`Config` (re-exported as `StarDetectionConfig`) composes `BackgroundConfig`, `DetectionConfig`, `FwhmConfig`, `MeasurementConfig`, and `FilterConfig`, with presets `wide_field` / `high_resolution` / `crowded_field` / `precise_ground`. `validate()` delegates to every component and reports `StarDetectionConfigError`. Notable knobs: `CentroidMethod` (`WeightedMoments | GaussianFit | MoffatFit{beta}`), `Connectivity` (`Four | Eight`, default 8), `BackgroundRefinement` (`None | Iterative{iterations}`), `LocalBackgroundMethod` (`GlobalMap | LocalAnnulus`), optional normalized-domain `NoiseModel{electrons_per_normalized_unit, read_noise_electrons}`, and `detection.deblend_n_thresholds` (0 = local-maxima, ≥2 = multi-threshold).

**Six processing stages** (background stays in `background/`; the other five boundaries live in
`detector/stages/`; working memory comes from the detector resources):
1. **prepare** (`prepare.rs:20`) — reduce to a single detection plane (copy for grayscale; inverse-variance / noise-weighted channel combination for RGB — the linear analogue of the SExtractor χ² detection image, kept linear so downstream flux/centroid stay valid); 3×3 median filter for CFA images.
2. **background** (`background/mod.rs:30`; the tiled estimator itself lives in the shared top-level `background_mesh::TileGrid`, promoted out of `star_detection` so `background_extraction` reuses it) — tiled (default 64-px, 3 clip iterations) SExtractor crowding-aware sky per tile (Pearson mode `2.5·median − 1.5·mean` of the clip survivors when `|mean − median| < 0.3σ`, median fallback when strongly skewed) + MAD σ, 3×3 tile median filter, natural bicubic-spline interpolation; optional iterative object-masking refinement for crowded fields → `BackgroundEstimate{background, noise}`. Masked tiles use a two-pass bitset count plus deterministic direct sampling, retaining at most 1,024 values per Rayon scratch.
3. **fwhm** (`fwhm.rs`) — fixed `fwhm.expected` if set, or auto-estimate via a stricter first pass + robust median/MAD of bright stars; the resulting effective FWHM is shared by detection and final measurement.
4. **detect** (`detect.rs:51`) — optional matched-filter Gaussian convolution (separable, elliptical PSF) → threshold mask → connected-component labeling (RLE + union-find, 8-connectivity) → deblend (`local_maxima` Voronoi-by-peak, ArrayVec for ≤8 peaks; or `multi_threshold` SExtractor-style tree with contrast criterion over the full configured ladder) → area/edge region filter.
5. **measure** (`measure.rs:18`, `centroid/mod.rs:286`) — stamp extraction and weighting seeded by the effective FWHM → iterative weighted-moment centroid, then optional Levenberg-Marquardt profile fit: `gaussian_fit` (6 params) or `moffat_fit` (5–6 params) via the generic `lm_optimizer.rs` + `linear_solver.rs`; computes flux/FWHM/eccentricity/SNR/sharpness/roundness. FWHM/eccentricity come from **windowed/adaptive second moments** (`windowed_covariance`: Gaussian window iterated to match the source, then deconvolved — SExtractor WIN style) so wing noise can't inflate them; falls back to plain moments if it can't converge.
6. **filter** (`filter.rs:41`) — saturation/SNR/eccentricity/sharpness/roundness cuts, FWHM-outlier removal (median+MAD of brightest half), duplicate removal (O(n²) under 100 stars, else spatial hash), flux sort.

## stacking/registration — alignment & warp

`register(ref_stars, target_stars, config)` (`stacking/registration/mod.rs`) → `Result<RegistrationResult, RegistrationError>`:
1. validate finite catalog positions/FWHM and configuration limits, then derive `max_sigma = (median_fwhm·0.5).max(0.5)` from input FWHM,
2. select brightest ≤`max_stars`,
3. **triangle matching** (`triangle/`: k-NN invariant triangles via `spatial::KdTree`, ratio-space voting in `voting.rs`, greedy conflict resolution) → `Vec<PointMatch>`,
4. **RANSAC/MAGSAC++** (`ransac/`); `Auto` (`auto_ladder`) walks Euclidean → Similarity → Affine → Homography and takes the first model within 0.5 px RMS (simplest fit, no overfitting), falling through to Homography,
5. iterative **match recovery** (`mod.rs:376`, kd-tree NN within `~3.03·max_sigma`, ≤5 passes),
6. optional **SIP** polynomial fit, then `max_rms_error` gate.

- `Transform` (`transform.rs:70`) / `TransformType` (`transform.rs:15`): `Translation | Euclidean | Similarity | Affine | Homography | Auto`; model-specific constructors preserve the private matrix/model invariant, with immutable row-major coefficients available through `matrix()`. `apply`/`apply_inverse`/`inverse`/`compose`. `WarpTransform` (`transform.rs:323`) bundles a `Transform` + optional `SipPolynomial` (applies SIP first).
- `RegistrationConfig`: composes `RegistrationMatchingConfig`, `RansacConfig`, optional `SipConfig`, and `WarpParams`; presets `default`/`fast`/`precise`/`wide_field`/`precise_wide_field`/`mosaic`.
- `RegistrationResult` (`result/`): immutable transform, optional `SipFitResult`, and one coherent `Vec<StarMatch>` whose records carry correspondence indices plus final residuals; RMS/max error, inlier count, and `quality_score` are derived from those records. `warp_transform()` builds the `WarpTransform`. Match recovery returns its private transform-plus-matches state as `RecoveredMatches`. `RegistrationError` covers insufficient stars / no patterns / RANSAC failure / SIP fit failure / accuracy.
- `ransac/`: `transforms.rs` (Procrustes for Euclidean/Similarity, Hartley-normalized least-squares for Affine, DLT-SVD for Homography), `magsac.rs` (continuous MAGSAC-inspired loss with finite support), LO-RANSAC + adaptive iteration count.
- `distortion/`: `sip` (FITS WCS SIP polynomial, order 2–5, MAD sigma-clipped fit) is live; `tps` (thin-plate spline, `DistortionMap`) is **fully implemented but `#![allow(dead_code)]` and not wired into `register()`**.
- `resample.rs` owns public `warp(image, warp_transform, params) -> WarpResult { image, coverage, confidence }`; coverage is the in-bounds fraction of kernel magnitude and gates geometric inclusion, while confidence is the inverse white-noise variance of the normalized interpolation coefficients. All methods return `WarpParams::border_value` and zero quality outside the closed source pixel footprint; partial support inside uses only real source pixels. `InterpolationMethod` = `Nearest | Bilinear | Bicubic | Lanczos2/3/4`. `interpolation/warp/` has AVX2/SSE4.1 + NEON bilinear (no-SIP, border 0) and a const-generic linear Lanczos path (4096-sample LUT, incremental f64 stepping, normalized interior fast path, edge-extended bilinear for partial kernels) whose interior kernel is x86_64 FMA (`warp/sse.rs`) and aarch64 NEON (`warp/neon.rs`) for all Lanczos sizes; bicubic is scalar.
- `spatial::KdTree` (`spatial/mod.rs`): flat implicit 2D k-d tree (`k_nearest_into`/`nearest_one`/`radius_indices_into`) with a stack-allocated bounded max-heap for k ≤ 32.

## stacking/combine — frame combination

`stack(paths, config, progress, cancel)` (`stack.rs`, from disk) and `stack_images(frames, config, progress, cancel)` (`stack.rs`, in-memory) → `StackProduct`. The trailing `cancel: common::CancelToken` is cooperative (also on `stack_cfa_master`, `CalibrationMasters::from_images`, and both alignment entry points; pass `CancelToken::never()` to opt out): it is polled between RAW loads, within demosaic stages, between detection/registration/warp frames, during cache validation and registered normalization measurement, per combine row or chunk, and during defect-map scans. Cancellation returns a typed `Cancelled` error and partial images, masters, maps, and stack products do not escape. `calibrate_align_stack` caps concurrent decode+demosaic work at `MAX_CONCURRENT_LIGHTS`; libraw decode is the one uninterruptible unit, so at most the bounded in-flight decodes finish before cancellation completes. A `StackFrame { image, coverage: Option<Buffer2<f32>>, confidence: Option<Buffer2<f32>>, source_stats: FrameStats }` bundles each in-memory frame with optional warp quality and pre-warp statistics; plain `AstroImage`s convert via `.into()` (full support and unit confidence).

- `StackConfig` (`config.rs`): `method: CombineMethod` (`Mean(Rejection) | Median`), `weighting: Weighting` (`Equal | Noise | Manual(Vec<f32>)`), `normalization: Normalization` (`None | Global | Multiplicative`), `small_n: SmallN`, `cache: CacheConfig`. Presets `sigma_clipped`/`winsorized`/`linear_fit`/`median`/`mean`/`gesd`/`percentile`/`weighted` + frame presets `light`/`flat`/`dark`/`bias`. `validate()` reports `StackConfigError`; every stack entry point validates before loading or allocating. `gesd()` falls back to median below 15 frames.
- `Rejection` (`rejection.rs`): `None | SigmaClip | Winsorized | LinearFit | Percentile | Gesd` (each with its own config struct). GESD is the textbook two-sided Rosner test using mean/sample standard deviation and accurate Student-t critical values cached per combine worker.
- `frame_store/mod.rs` owns memory-budget arithmetic, RAM/mmap `StoredPlane`s, spill-directory cleanup, full-frame statistics for unregistered inputs, collision-resistant BLAKE3 source-path cache names, and the `StoredFrame`/`StoredLightFrame` records consumed by combine. Each `StoredLightFrame` owns its source statistics alongside channels and optional coverage/confidence planes. `combine/cache/loader/mod.rs` owns tier loading and the source-identity sidecar (canonical path, byte length, nanosecond-resolution mtime, published last as the cache commit record); `combine/cache/mod.rs` owns the chunked combine engine; `combine/normalization/mod.rs` owns reference selection, temporary common-domain measurements, and affine fitting.
- Pipeline (`stack.rs`): `stack` (from paths → `LightCache`, quality planes `None`) / `stack_images` (in-memory `StackFrame`s → `LightCache`) / calibration (`CfaCache::from_paths`). Each: tier-select + load → optionally compute normalization → resolve weights → chunked per-pixel combine applying normalize → reject → accumulate. Registered common-domain statistics are measured in parallel across independent frame/channel pairs after the shared mask is fixed. `LightCache` retains only final frame norms; common-domain measurements are discarded after fitting, and `Normalization::None` skips them. `run_stacking` (`CfaCache` → `CfaImage`) / `run_stacking_weighted` (`LightCache` → `StackProduct`). The latter accumulates each channel's effective `Σwᵢ` and conditional linear factor `Σwᵢ²/(Σwᵢ)²` from the reducer's actual survivor indices. RGB `StackProduct.weight` and mean `linear_variance` use `QualityMap::PerChannel`; monochrome output uses `QualityMap::Shared`, and median output has no linear variance. `coverage` remains one channel-independent geometric-support fraction. `align_and_stack` composes that product with an `AlignmentSummary` in `AlignStackResult`.
- `pipeline/` is split by ownership: `config.rs` defines stage composition, `result.rs` defines outcomes and errors, `align.rs` owns calibrated-image detection/registration/warping, and `streaming.rs` owns RAW calibration plus RAM/disk tier selection. `pipeline::align::DetectedFrame<I>` keeps each image (resident or `StoredImage`) with its detected stars, so image/star indices cannot drift between tiers.
- **Warp quality** (`LightCache::process_chunked_weighted`): a frame contributes only where coverage > `MIN_CONTRIBUTING_COVERAGE` and confidence is positive. Coverage is only a geometric gate; effective inverse-variance weight is `confidence × per-frame weight`. Excluding sub-ε support keeps warp border-fill out of the rejection set without treating signed kernel sums as geometry. Both quality maps are `StoredPlane`s, memory-mapped with the channels in the streaming tier.

## stacking/drizzle — variable-pixel reconstruction

`drizzle_stack(Vec<DrizzleFrame<Path>>, config, progress)` (`stacking/drizzle/stack.rs`) → `StackProduct`: output `AstroImage` + normalized `coverage` `[0,1]`, shared absolute `weight` plane (`Σwᵢ`, the geometric WHT), and shared `Some(linear_variance)` (`Σwᵢ²/(Σwᵢ)²`). `drizzle_images` accepts the same coherent records with in-memory `AstroImage` sources.

- `config.rs` owns `DrizzleConfig` (`scale`, `pixfrac`, `kernel`, `fill_value`, `min_coverage`) and `DrizzleKernel`: `Square` (exact polygon clipping via Green's theorem `boxer`/`sgarea`), `Turbo` (axis-aligned, default), `Point`, `Gaussian`, and `Lanczos` (valid only at pixfrac=scale=1). `validate()` requires `0 < pixfrac <= 1` and reports `DrizzleConfigError`; builders remain freely composable and drizzle entry points validate before loading or allocating.
- `accumulator.rs` owns `DrizzleFrame<T>`, which keeps one source, transform, frame weight, and optional per-pixel weight map coherent, plus `DrizzleAccumulator`. Fallible construction validates configuration and input dimensions. The accumulator stores per-channel flux `Buffer2` (`Σ fluxᵢ·wᵢ`) plus shared `weight` (`Σwᵢ`) and `weight_sq` (`Σwᵢ²`) planes; `add_frame` validates before mapping each input pixel and `finalize` emits the normalized image and quality planes. Pure CPU + rayon-parallel finalize.
- `geometry.rs` owns the pure Jacobian, rectangle overlap, Lanczos, and polygon clipping helpers. `stack.rs` owns disk loading and the path/in-memory entry points; `mod.rs` only declares the ownership modules and tests.

## image_ops::stretching — non-linear display stretch

`Stretch { method, color }.apply(&mut imaginarium::Image) -> Result<(), OpError>` (`image_ops/stretching/mod.rs`) maps a *linear* stacked master to a display image. Every display/processing op follows this imaginarium-style shape — an op-named config struct (`Stretch`, `Denoise`, `Hdr`, `LocalContrast`, `ExtractBackground`, `Scnr`, `NeutralizeBackground`) with `Default` + builder methods + an `apply(&mut Image)` that validates config + format and operates in place via `image_ops`. `apply` returns [`OpError`] (`UnsupportedFormat` unless `L_F32`/`RGB_F32`; `InvalidConfig` on out-of-range params; `RankDeficient` when background sample geometry cannot determine its polynomial) instead of panicking. Input may exceed `[0,1]` (a raw stack's bright stars do); every curve clamps, so output is always `[0,1]`. Runs strictly after the linear-domain work (background extraction, color calibration).

- `StretchMethod`: `AutoStf{shadow_sigmas, target_background}` (MTF/STF screen-stretch — black point `median − k·σ`, midtones from the MTF self-inverse identity so the rescaled median lands on the target), `AutoAsinh{target_background}` (normalized arcsinh, `β` solved by log-space bisection so the background median maps to the target), `Asinh{beta}` (explicit); constructors `Stretch::{auto_stf, auto_asinh, ghs}`. `Stretch::apply` validates and returns `OpError::InvalidConfig` on out-of-range params.
- `ColorMode`: `ColorPreserving` (default — stretch the combined intensity `I=(r+g+b)/3`, scale every channel by `f(I)/I` with a hue-preserving highlight cap, so star color survives) or `PerChannel` (independent per-channel auto-stretch — auto-grays the background but ties color to brightness). No effect on grayscale.
- Curves are monomorphized behind a `ToneCurve` trait — `StfCurve` (clip-rescale + `MTF(m,x) = (m−1)x/((2m−1)x − m)`) and `AsinhCurve` (`asinh(x/β)/asinh(1/β)`), both clamping to `[0,1]`. `stretch` resolves the `Curve` enum **once** and runs a monomorphized loop, so the variant is never re-decided per pixel. Statistics come from `intensity_plane()` (color) or each channel (per-channel); explicit-`β` skips them.

## math — primitives

- `sum/`: `sum_f32` / `mean_f32` / `weighted_mean_f32` — hybrid compensated summation (per-lane Kahan in the SIMD inner loop, Neumaier horizontal reduction + remainder). AVX2 (8-wide) / SSE4.1 (4-wide) / NEON / scalar.
- `statistics/`: `median_f32_mut` (quickselect, NaN-safe `total_cmp`), `median_f32_fast` (NaN-free intermediate), `mad_f32_fast`, `mad_to_sigma` (`MAD_TO_SIGMA = 1.4826022`), iterative `sigma_clipped_median_mad` → `ClippedStats {median, sigma, mean}` (the survivors' mean exposes residual skew for the background's Pearson-mode estimator); `FWHM_TO_SIGMA ≈ 2.3548` lives in `mod.rs`.
- `vec2us/` (`Vec2us`, public pixel coordinates/dimensions with index conversion), `dmat3.rs` (`DMat3`, row-major f64 homogeneous transforms + inverse/determinant), and `rect/` (`Rect` for continuous geometry and `URect` for pixel regions; both use half-open min/max bounds).

## SIMD dispatch

Runtime feature detection reuses Imaginarium's cached detector (`imaginarium::cpu_features::{has_avx2, has_avx2_fma, has_sse4_1}`); scalar fallback everywhere. Hand-written backends:

| Area | AVX2 | SSE4.1 | NEON |
|------|------|--------|------|
| `math/sum` | ✓ | ✓ | ✓ |
| `io/raw/normalize` | | ✓ | ✓ |
| `stacking/star_detection/background` | ✓ | ✓ | ✓ |
| `stacking/star_detection/convolution` | ✓ | ✓ | ✓ |
| `stacking/star_detection/threshold_mask` | | ✓ | ✓ |
| `stacking/star_detection/median_filter` | ✓ | ✓ | ✓ |
| `stacking/star_detection/centroid/{gaussian,moffat}_fit` | ✓ | | ✓ |
| `stacking/registration/resample/row` (bilinear; linear Lanczos FMA) | ✓ | ✓ | ✓ |
| `image_ops/stretching` (color-preserving arcsinh; Cephes `logf`/`asinh`) | ✓ | | ✓ |

## WIP / notes

- `stacking/registration/distortion/tps` (thin-plate spline) is implemented and tested but `#![allow(dead_code)]` and **not wired** into `register()` — an alternate post-RANSAC distortion model.
- Test-only constructors/APIs are gated and kept minimal (e.g. warp params). Don't widen them for production use.
- The `real-data` feature flag (empty) gates real-data tests/benches that read the bundled `test_data/lumos_data` dataset (gitignored; present locally).
- **Test layout:** per-file unit tests are the `foo/{mod.rs, tests.rs}` split, beside the code. Module-level integration tests sit beside the implementation as `<module>/synthetic_tests[.rs|/]` (forward-model tests) and `<module>/real_data_tests.rs` (`#[ignore = "real-data integration test; run explicitly with --ignored"]`). `stacking::star_detection`'s shared test-output/metric infra is `stacking/star_detection/test_common/`; its synthetic tree groups `stacking/star_detection/synthetic_tests/{stage_tests, pipeline_tests}/` + `metric_curves.rs` / `subpixel_accuracy.rs`. The shared generator + graders live in `testing::synthetic`.

## Reference docs & upstream sources

- **`src/stacking/docs/`** — best-practices reference for each pipeline stage, grounded in upstream source + cross-checked research. One doc per stage: `01-load-decode.md`, `02-calibration.md`, `03-star-detection.md`, `04-registration.md`, `05-stacking-drizzle.md`, plus `README.md`. These are descriptive references rather than the module contract; their source-comparison sections can lag implementation, while the README status table records which findings have since been resolved.
- **`scripts/clone-refs.sh`** — shallow-clones the upstream software whose functionality overlaps lumos into `.tmp/refs/<name>/` for source investigation (Read/Grep without per-file registry prompts; nothing is built or linked). `--list` prints the set, `--all` adds the large suites (RawTherapee, OpenCV, astropy, kstars, …), no arg clones the core set. Idempotent — an existing clone is skipped; delete its dir to refresh. Native deps (LibRaw, cfitsio) are pinned to `Cargo.lock` versions; everything else tracks upstream HEAD. Each entry's comment names the lumos module it informs (e.g. `sep`/`sextractor`/`photutils` → `stacking::star_detection`, `magsac`/`astroalign` → `stacking::registration`, `drizzle` → `stacking::drizzle`).
- **`.tmp/refs/`** is gitignored and persists across sessions. Run the script before reading upstream source; the `src/stacking/docs/` references were built from these clones.

## Commands

```bash
cargo build -p lumos --release
cargo test -p lumos
cargo test -p lumos --release <bench_name> -- --ignored --nocapture   # benches (e.g. bench_rcd, bench_bayer)

scripts/clone-refs.sh          # clone core upstream refs into .tmp/refs/ (--all for large suites, --list to preview)
```

## Benchmarks (quickbench)

Benches are `#[quick_bench]` fns (expand to `#[test] #[ignore]`) in per-module `bench.rs` files — e.g. `stacking/registration/resample/bench.rs`, plus RCD/Bayer/stacking. No `cargo bench` target; they run through `cargo test`.

- **Run** — always `--release` (debug numbers are meaningless and print a warning):
  ```bash
  cargo test -p lumos --release <filter> -- --ignored --nocapture
  ```
  `<filter>` is a substring of the test path: a bench name (`bench_warp_lanczos3_1k`) or a whole bench module (`interpolation::bench`). Omit it to run every bench.
- **Auto-comparison is the baseline mechanism.** Each bench writes `bench-results/<name>.txt` (gitignored); the next run prints a coloured `faster`/`SLOWER` diff against it (±5% threshold). To measure an optimization: run once (baseline) → make the change → run again, the diff is automatic. The file is overwritten each run, so to keep a baseline across several iterations, copy `bench-results/` aside first (into `.tmp/`).
- For SIMD/kernel work, prefer the **single-thread** variants (e.g. `bench_warp_lanczos3_1k_single_thread`) to isolate per-thread throughput from rayon + memory effects; the multi-thread benches show realistic end-to-end time. **aarch64 is the profiled target** (NEON mandatory; x86 SIMD is runtime-detected — see the SIMD dispatch table above).
