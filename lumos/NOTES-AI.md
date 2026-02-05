# lumos - Implementation Notes (AI)

> **Last Updated**: 2026-01-27

Astrophotography image processing library for loading, calibrating, and stacking astronomical images.

## Key Modules

| Module | Description |
|--------|-------------|
| `astro_image/mod.rs` | `AstroImage` for loading FITS and RAW camera files |
| `stacking/mod.rs` | Image stacking algorithms (see `stacking/NOTES-AI.md`) |
| `calibration_masters.rs` | Master dark/flat/bias frame management |
| `math.rs` | SIMD-accelerated math utilities (ARM NEON, x86 SSE4) |
| `star_detection/` | Star detection and centroid computation |
| `registration/` | Image registration, alignment, and astrometry |

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

See `stacking/NOTES-AI.md` for detailed documentation.

| Module | Description |
|--------|-------------|
| `stacking/mod.rs` | `StackingMethod`, `FrameType`, `ImageStack` dispatch |
| `stacking/mean/` | Mean stacking (SIMD: NEON/SSE/scalar) |
| `stacking/median/` | Median stacking via mmap (SIMD sorting networks) |
| `stacking/sigma_clipped/` | Sigma-clipped mean via mmap |
| `stacking/weighted/` | Weighted mean with quality-based frame weights |
| `stacking/local_normalization.rs` | Tile-based local normalization (PixInsight-style) |
| `stacking/live.rs` | Live/real-time stacking with incremental updates |
| `stacking/comet.rs` | Comet/asteroid dual-stack stacking |
| `stacking/session.rs` | Multi-session integration with weighted stacking |
| `stacking/gradient_removal.rs` | Post-stack gradient removal (polynomial/RBF) |
| `stacking/gpu/` | GPU-accelerated sigma clipping and batch pipeline |

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
| `mod.rs` | Re-exports public API (`Star`, `StarDetectionConfig`, `StarDetector`) |
| `star.rs` | `Star` struct with quality metric methods |
| `config.rs` | All configuration structs with validation |
| `detector/` | `StarDetector` - main pipeline orchestrator |
| `background/` | Tile-based sigma-clipped background estimation (SIMD) |
| `candidate_detection/` | Threshold mask + connected component labeling |
| `threshold_mask/` | Bit-packed threshold mask creation (SIMD) |
| `mask_dilation/` | Separable morphological dilation |
| `centroid/` | WeightedMoments, GaussianFit (AVX2 SIMD), MoffatFit (AVX2 SIMD) |
| `convolution/` | Separable + elliptical Gaussian convolution (SIMD) |
| `deblend/` | Local maxima + SExtractor-style multi-threshold deblending |
| `cosmic_ray/` | L.A.Cosmic Laplacian-based cosmic ray detection (SIMD) |
| `median_filter/` | 3x3 median filter for Bayer artifacts (SIMD) |
| `fwhm_estimation.rs` | Auto FWHM estimation from bright stars |
| `buffer_pool.rs` | Buffer reuse pool for batch processing |
| `defect_map.rs` | Sensor defect (hot/dead pixel) masking |

### Star Detection Key Types

```rust
Star                  // { pos: DVec2, flux, fwhm, eccentricity, snr, peak, sharpness, roundness1, roundness2, laplacian_snr }
StarDetector          // Main detector with buffer pool for batch processing
StarDetectionResult   // { stars: Vec<Star>, diagnostics: StarDetectionDiagnostics }
StarDetectionConfig   // Groups BackgroundConfig, PsfConfig, FilteringConfig, CentroidConfig, DeblendConfig
StarCandidate         // { bbox, peak, peak_value, area }
BackgroundMap         // { background, noise, adaptive_sigma } per-pixel estimates
CentroidMethod        // WeightedMoments | GaussianFit | MoffatFit { beta }
BackgroundRefinement  // None | Iterative { iterations } | AdaptiveSigma(config)
LocalBackgroundMethod // GlobalMap | LocalAnnulus
NoiseModel            // { gain, read_noise } for CCD noise equation
```

### Detection Pipeline

1. Defect masking - Replace hot/dead pixels with local median (optional)
2. Median filter - 3x3 removes Bayer artifacts (CFA sensors only)
3. Background estimation - Tile-based sigma-clipped median + MAD, bilinear interpolation
4. FWHM estimation - Auto-estimate from bright stars (optional)
5. Matched filtering - Gaussian convolution matched to PSF (optional)
6. Threshold mask - Bit-packed mask (pixel > background + sigma * noise)
7. Mask dilation - Connect fragmented detections
8. Connected component labeling - RLE-based with lock-free union-find
9. Candidate extraction + deblending - Local maxima or multi-threshold
10. Centroid refinement - WeightedMoments, GaussianFit, or MoffatFit
11. Quality filtering - SNR, eccentricity, saturation, sharpness, roundness, Laplacian SNR
12. FWHM outlier rejection - MAD-based
13. Duplicate removal - Spatial hashing deduplication

## Registration Module (registration/)

| Module | Description |
|--------|-------------|
| `types/` | `TransformMatrix`, `TransformType`, `RegistrationConfig`, `RegistrationResult` |
| `spatial/` | K-d tree for O(n log n) spatial queries |
| `triangle/` | Triangle matching with geometric hashing |
| `ransac/` | RANSAC with LO-RANSAC (SIMD accelerated) |
| `phase_correlation/` | FFT-based coarse alignment |
| `interpolation/` | Lanczos-3/4, bicubic, bilinear, nearest (SIMD) |
| `pipeline/` | Full registration pipeline with `Registrator` |
| `quality/` | Quality metrics and quadrant consistency |
| `gpu/` | GPU-accelerated warping via imaginarium |
| `distortion/` | Radial, tangential, field curvature correction |
| `astrometry/` | Plate solving (WCS coordinates from star patterns) |

### Registration Key Types

```rust
TransformMatrix         // 3x3 homogeneous matrix with apply(), inverse(), compose()
TransformType           // Translation | Euclidean | Similarity | Affine | Homography
RegistrationConfig      // Builder pattern with validation
RegistrationResult      // { transform, matched_stars, residuals, rms_error, quality_score }
PointMatch              // { ref_idx, target_idx, votes, confidence }
KdTree                  // 2D k-d tree for k-NN and radius queries
ThinPlateSpline         // Smooth non-rigid transformation
DistortionMap           // Grid-based distortion visualization
RadialDistortion        // Brown-Conrady barrel/pincushion correction
TangentialDistortion    // Brown-Conrady tangential (decentering) correction
FieldCurvature          // Petzval field curvature correction
GpuWarper               // GPU-accelerated image warping context
Wcs                     // World Coordinate System (plate solution)
PlateSolver             // Astrometric plate solving via quad hashing
CatalogStar             // Star from Gaia/UCAC4 catalog
```

### Distortion Correction

See `registration/distortion/NOTES-AI.md` for detailed documentation.

1. **RadialDistortion** (parametric): Brown-Conrady model `r' = r(1 + k₁r² + k₂r⁴ + k₃r⁶)`
   - `distort()` / `undistort()` - Forward/inverse transform
   - `estimate()` - Estimate coefficients from matched points
   - Barrel (k1>0), pincushion (k1<0) support

2. **TangentialDistortion**: Brown-Conrady tangential model
   - Corrects decentering distortion from misaligned lens elements
   - p1/p2 coefficients, OpenCV-compatible

3. **FieldCurvature**: Petzval field curvature correction
   - Corrects radial magnification variation from curved focal plane
   - c1/c2 coefficients for even-order polynomial model

4. **ThinPlateSpline** (non-parametric): For complex non-radial distortion
   - Fitted from star correspondences
   - Smooth interpolation minimizing bending energy

### Astrometry (Plate Solving)

See `registration/astrometry/NOTES-AI.md` for detailed documentation.

- **Wcs**: World Coordinate System - gnomonic projection pixel↔sky transforms
- **PlateSolver**: Quad-based geometric hashing for star pattern matching
- **CatalogSource**: Gaia DR3 via VizieR or preloaded star lists
- Uses RANSAC for robust transformation estimation

### Registration Pipeline

1. Coarse alignment (optional) - Phase correlation for initial translation
2. Triangle matching - Geometric hashing (scale/rotation invariant)
3. RANSAC - LO-RANSAC for robust transform estimation with early termination
4. Refinement - Least-squares on all inliers
5. Warping - Parallel channel processing (CPU) or GPU-accelerated

### GPU Warping

```rust
// Single-use convenience functions
warp_to_reference_gpu(image, w, h, transform) -> Vec<f32>
warp_rgb_to_reference_gpu(image, w, h, transform) -> Vec<f32>

// Reusable context for multiple warps
let mut warper = GpuWarper::new();
warper.warp_channel(image, w, h, transform)
warper.warp_rgb(image, w, h, transform)

// Parallel CPU warping for multi-channel images
warp_multichannel_parallel(image, w, h, channels, transform, method) -> Vec<f32>
```

**Performance Note:** CPU parallel warping is ~32% faster than GPU for 24+ megapixel images due to memory transfer overhead. Use CPU bilinear for speed, Lanczos3 for quality. GPU warping is available but recommended only when keeping data on GPU across multiple operations.

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
