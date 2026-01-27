# lumos - Implementation Notes (AI)

Astrophotography image processing library for loading, calibrating, and stacking astronomical images.

## Key Modules

| Module | Description |
|--------|-------------|
| `astro_image/mod.rs` | `AstroImage` for loading FITS and RAW camera files |
| `stacking/mod.rs` | Image stacking algorithms (mean, median, sigma-clipped mean) |
| `calibration_masters.rs` | Master dark/flat/bias frame management |
| `math.rs` | SIMD-accelerated math utilities (ARM NEON, x86 SSE4) |
| `star_detection/` | Star detection and centroid computation |
| `registration/` | Image registration and alignment |

## Key Types

```rust
AstroImage         // Astronomical image wrapper around imaginarium::Image with metadata
AstroImageMetadata // FITS metadata (object, instrument, exposure, bitpix, etc.)
ImageDimensions    // { width, height, channels } - helper struct
BitPix             // FITS pixel type enum (UInt8, Int16, Int32, Int64, Float32, Float64)
StackingMethod     // Mean | Median | SigmaClippedMean(SigmaClipConfig)
SigmaClipConfig    // { sigma, max_iterations }
CacheConfig        // { cache_dir, keep_cache, available_memory, progress }
StackError         // Error type for stacking operations
FrameType          // Dark | Flat | Bias | Light
CalibrationMasters // Container for master dark/flat/bias frames
```

## Stacking Module Structure

| Module | Description |
|--------|-------------|
| `stacking/mod.rs` | `StackingMethod`, `FrameType`, `stack_frames()` dispatch |
| `stacking/error.rs` | `StackError` enum |
| `stacking/cache.rs` | `ImageCache` with memory-mapped binary cache |
| `stacking/cache_config.rs` | `CacheConfig` with adaptive chunk sizing |
| `stacking/mean/` | Mean stacking (SIMD: NEON/SSE/scalar) |
| `stacking/median/` | Median stacking via mmap (SIMD sorting networks) |
| `stacking/sigma_clipped/` | Sigma-clipped mean via mmap |

## Demosaic Module (astro_image/demosaic/)

Bayer and X-Trans demosaicing with SIMD acceleration.

| Module | Description |
|--------|-------------|
| `bayer/scalar.rs` | Scalar bilinear demosaicing |
| `bayer/simd_sse3.rs` | x86_64 SSE3 SIMD |
| `bayer/simd_neon.rs` | ARM aarch64 NEON SIMD |
| `xtrans/` | X-Trans (Fujifilm) demosaicing |

CFA Patterns: RGGB, BGGR, GRBG, GBRG

## Star Detection Module (star_detection/)

| Module | Description |
|--------|-------------|
| `mod.rs` | `Star`, `StarDetectionConfig`, `find_stars()` |
| `constants.rs` | Astronomical constants (FWHM_TO_SIGMA, MAD_TO_SIGMA, etc.) |
| `background/` | Tile-based sigma-clipped background estimation |
| `detection/` | Connected components with threshold detection |
| `centroid/` | WeightedMoments, GaussianFit, MoffatFit algorithms |
| `convolution/` | Gaussian kernel convolution (SIMD) |
| `deblend/` | Multi-threshold star deblending |
| `cosmic_ray/` | Laplacian-based cosmic ray detection |
| `median_filter/` | 3x3 median filter for Bayer artifacts |

### Star Detection Key Types

```rust
Star               // { x, y, flux, fwhm, eccentricity, snr, peak, sharpness }
StarDetectionConfig // Detection parameters with validate() and builder pattern
StarCandidate      // { centroid_x, centroid_y, area, bbox }
BackgroundMap      // { background, noise } per-pixel estimates
CentroidMethod     // WeightedMoments | GaussianFit | MoffatFit { beta }
LocalBackgroundMethod // GlobalMap | LocalAnnulus
```

### Detection Pipeline

1. Median filter - 3x3 removes Bayer artifacts
2. Background estimation - Tile-based sigma-clipped median
3. Star detection - Threshold + connected components
4. Deblending - Multi-peak separation
5. Centroid refinement - Configurable method
6. Quality filtering - SNR, eccentricity, saturation, sharpness, FWHM
7. Duplicate removal - Spatial deduplication

## Registration Module (registration/)

| Module | Description |
|--------|-------------|
| `types/` | `TransformMatrix`, `TransformType`, `RegistrationConfig`, `RegistrationResult` |
| `spatial/` | K-d tree for O(n log n) spatial queries |
| `triangle/` | Triangle matching with geometric hashing |
| `ransac/` | RANSAC with LO-RANSAC (SIMD accelerated) |
| `phase_correlation/` | FFT-based coarse alignment |
| `interpolation/` | Lanczos-3/4, bicubic, bilinear, nearest |
| `pipeline/` | Full registration pipeline with `Registrator` |
| `quality/` | Quality metrics and quadrant consistency |

### Registration Key Types

```rust
TransformMatrix    // 3x3 homogeneous matrix with apply(), inverse(), compose()
TransformType      // Translation | Euclidean | Similarity | Affine | Homography
RegistrationConfig // Builder pattern with validation
RegistrationResult // { transform, matched_stars, residuals, rms_error, quality_score }
StarMatch          // { ref_idx, target_idx, votes, confidence }
KdTree             // 2D k-d tree for k-NN and radius queries
ThinPlateSpline    // Smooth non-rigid transformation
DistortionMap      // Grid-based distortion visualization
```

### Registration Pipeline

1. Coarse alignment (optional) - Phase correlation for initial translation
2. Triangle matching - Geometric hashing (scale/rotation invariant)
3. RANSAC - LO-RANSAC for robust transform estimation
4. Refinement - Least-squares on all inliers
5. Warping - High-quality Lanczos-3 interpolation

## SIMD Math Utilities (math.rs)

- `sum_f32()` - SIMD-accelerated sum
- `mean_f32()` - SIMD-accelerated mean
- `sum_squared_diff()` - SIMD-accelerated sum of squared differences

Platforms: ARM NEON (aarch64), x86 SSE4, scalar fallback

## RAW File Loading

Priority: rawloader (pure Rust) → libraw (C library fallback)

Supported: RAF, CR2, CR3, NEF, ARW, DNG

## Examples

- `full_pipeline.rs` - Complete calibration workflow: master frames, light calibration, star detection, registration

## Dependencies

common, imaginarium, fitsio, rawloader, libraw-rs, anyhow, rayon, strum_macros
