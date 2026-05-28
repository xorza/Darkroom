# Lumos

Astronomical image processing library: RAW/FITS decoding, calibration, star detection, image registration, frame stacking, and drizzle integration. CPU-bound with SIMD (AVX2/SSE4.1/NEON) hot paths; rayon for parallelism. No GPU backend in the current tree.

## Crate layout

Public surface is defined in `src/lib.rs` (the only place that `pub use`s — no intermediate re-exports). Seven domain modules plus support:

| Module | Vis | Role |
|--------|-----|------|
| `astro_image` | private (types re-exported) | Core `AstroImage` container, FITS/RAW/standard loading, metadata, CFA. |
| `calibration_masters` | private | Master dark/flat/bias creation, defect maps, `calibrate()`. |
| `raw` | `pub` | Camera RAW decoding (libraw) + Bayer/X-Trans demosaicing. |
| `star_detection` | `pub(crate)` | Multi-stage stellar detection + sub-pixel centroiding. |
| `registration` | `pub(crate)` | Triangle/RANSAC star-pattern alignment + warping. |
| `stacking` | `pub(crate)` | Multi-frame combination with rejection/normalization/weighting. |
| `drizzle` | `pub` | Fruchter & Hook variable-pixel reconstruction. |
| `math` | `pub(crate)` | `DMat3`, `BBox`, `Vec2us`, statistics, SIMD `sum`. |
| `common` | `pub(crate)` | CPU feature detection, shared helpers. |
| `prelude` | `pub` | Convenience re-exports of the main API. |
| `testing` | `#[cfg(test)]` | Synthetic + real-data fixtures for tests/benches. |

This file is the crate-level map; consult the code in each domain module for algorithm specifics.

## Modules

### `astro_image` — image container
- `AstroImage` (`astro_image/mod.rs:251`): `metadata` + `ImageDimensions` + `PixelData`.
- `PixelData` (`mod.rs:186`): `L(Buffer2)` or `Rgb([Buffer2; 3])` — **planar** layout, one `Buffer2` per channel for cheap per-channel ops.
- `BitPix` (`mod.rs:27`), `ImageDimensions` (`mod.rs:85`), `AstroImageMetadata` (`mod.rs:134`, full FITS/EXIF header set).
- Entry points: `AstroImage::from_file` (`mod.rs:268`, format-detects fits/raw/tiff/png/jpg), `from_pixels` (`mod.rs:291`, interleaved→planar), `from_planar_channels` (`mod.rs:320`).
- Submodules: `fits` (cfitsio I/O), `cfa` (`CfaImage`/`CfaType`, `demosaic()` → `AstroImage`), `sensor` (camera auto-detect from headers), `error` (`ImageError`).

### `calibration_masters` — master frames & defects
- `CalibrationMasters` (`mod.rs:31`): master dark/flat/bias/flat-dark + `DefectMap`.
- `DefectMap` (`defect_map.rs`): hot/cold-pixel flags with same-color-CFA-neighbor interpolation.
- `from_files` (`mod.rs:118`) / `from_images` (`mod.rs:87`) build masters; `calibrate(&mut image)` (`mod.rs:144`) applies dark-subtract → flat-divide → defect-correct in place on `CfaImage`.
- Stacking rule: ≥8 frames → sigma-clipped mean, else median. Defect threshold `DEFAULT_SIGMA_THRESHOLD = 5.0`.

### `raw` — RAW decode & demosaic
- `load_raw` / `load_raw_cfa` (`raw/mod.rs`): libraw wrapper → `CfaImage`. Replicates libraw `adjust_bl()` black-level folding.
- `demosaic/bayer` (`CfaPattern`: RGGB/BGGR/GRBG/GBRG), `demosaic/xtrans` (Fuji 6×6), `normalize` (u16→f32, rayon).

### `star_detection` — detection pipeline
- `StarDetector` (`detector/mod.rs:89`) holds a reusable `BufferPool`; `detect(&image)` (`detector/mod.rs:129`) → `DetectionResult` (stars sorted by flux + `Diagnostics`).
- `Star` (`star.rs`): pos, flux, fwhm, eccentricity, snr, peak, sharpness, roundness1/2.
- Six stages in `detector/stages/`: `prepare` (grayscale/defect/CFA median) → `background` (tiled sigma-clipped + bilinear interp, optional refine/adaptive) → `fwhm` (auto PSF estimate) → `detect` (threshold + connected-component labeling + deblend) → `measure` (moments or Gaussian/Moffat fit centroids) → `filter` (SNR/eccentricity/sharpness/roundness/dup cuts).
- Supporting submodules: `convolution` (matched filter), `threshold_mask`, `labeling` (8-connectivity), `deblend` (`local_maxima` | `multi_threshold`), `centroid` (Gaussian/Moffat fits via Levenberg-Marquardt), `median_filter`, `mask_dilation`, `buffer_pool`.
- SIMD: SSE/NEON in background/convolution/threshold_mask/median_filter; AVX2/NEON in centroid fits.

### `registration` — alignment
- `register(ref_stars, target_stars, config)` (`registration/mod.rs:104`) → `RegistrationResult` (`result.rs:104`): derives max σ from median FWHM, matches triangles, runs RANSAC, recovers extra matches, optionally fits SIP.
- `warp(...)` (`registration/mod.rs:244`): applies `Transform` (+ optional SIP) through the interpolation pipeline.
- `Transform` (`transform.rs:70`) / `TransformType` (`transform.rs:15`): Translation/Euclidean/Similarity/Affine/Homography/Auto.
- Submodules: `triangle` (k-NN invariant triangles, k-d-tree match, vote correspondences), `ransac` (RANSAC + MAGSAC++, progressive sampling, LO-RANSAC, physical plausibility), `spatial` (implicit-array 2D k-d tree), `distortion` (`sip` fitting; `tps` is **WIP**), `interpolation` (`warp/`: nearest/bilinear/bicubic/Lanczos3, SSE4.1 row paths).

### `stacking` — frame combination
- `stack(paths, frame_type, config)` (`stack.rs:88`) and `stack_with_progress(...)` (`stack.rs:123`) → `AstroImage`.
- `StackConfig` (`config.rs:71`) bundles `CombineMethod` (`config.rs:13`: `Mean(Rejection)` | `Median`), `Weighting` (`config.rs:22`: Equal/Noise/Manual), `Normalization` (`config.rs:36`: None/Global/Multiplicative), `CacheConfig`.
- `Rejection` (`rejection.rs:831`): SigmaClip/Winsorized/LinearFit/Percentile/GESD/None.
- `ImageCache` (`cache.rs:153`) auto-selects in-memory (fits in ~75% RAM) vs disk-backed memory-mapped tiers; `FrameStats` (`cache.rs:38`) per-channel median+MAD. Pipeline picks lowest-MAD reference, normalizes per frame, combines in memory-bounded chunks.

### `drizzle` — variable-pixel reconstruction
- `drizzle_stack(paths, transforms, weights?, pixel_weight_maps?, config, progress)` (`drizzle/mod.rs:876`) → `DrizzleResult` (output `AstroImage` + coverage map).
- `DrizzleAccumulator` (`mod.rs:170`): paired flux + weight `Buffer2` per channel.
- `DrizzleConfig` (`mod.rs:86`): scale, pixfrac, kernel, fill_value, min_coverage.
- `DrizzleKernel` (`mod.rs:62`): Square (exact polygon clipping), Turbo (axis-aligned, default), Point, Gaussian, Lanczos. Pure CPU + rayon.

### `math` — primitives
- `DMat3` (`dmat3.rs:22`, row-major f64 homogeneous transforms), `BBox` (`bbox.rs`), `Vec2us` (`vec2us.rs`).
- `sum/`: SIMD `sum_f32`/`weighted_mean_f32` — AVX2 (8-wide Kahan) / SSE4.1 (4-wide Kahan) / NEON, scalar Neumaier fallback.
- `statistics/`: quickselect median, MAD (`MAD_TO_SIGMA = 1.4826022`), iterative sigma-clipped stats, fast NaN-free variants for hot paths.

## SIMD dispatch

Runtime feature detection via `common` (`has_avx2()` / `has_sse4_1()`), scalar fallback everywhere. Backends present: `math/sum`, `star_detection/{background,convolution,threshold_mask,median_filter,centroid}`, `registration/interpolation/warp`.

## WIP / notes

- `registration/distortion/tps` (thin-plate spline) is implemented but `#![allow(dead_code)]` and **not wired** into `register()` — intended as an alternate post-RANSAC distortion model.
- Test-only constructors/APIs are gated and kept minimal (see recent commits limiting test-only `bbox`/warp-params surface). Don't widen them for production use.

## Commands

```bash
cargo build -p lumos --release
cargo nextest run -p lumos
cargo test -p lumos --release <bench_name> -- --ignored --nocapture   # benches
```
