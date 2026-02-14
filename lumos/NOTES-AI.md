# lumos - Implementation Notes (AI)

> **Last Updated**: 2026-02-14

Astrophotography image processing library for loading, calibrating, and stacking astronomical images.

## Key Modules

| Module | Description |
|--------|-------------|
| `astro_image/mod.rs` | `AstroImage` type, file loading dispatch, image operations |
| `raw/` | RAW file loading via libraw (RAF, CR2, CR3, NEF, ARW, DNG) |
| `stacking/mod.rs` | Image stacking algorithms (see `stacking/NOTES-AI.md`) |
| `calibration_masters.rs` | Master dark/flat/bias frame management |
| `math.rs` | SIMD-accelerated math utilities (ARM NEON, x86 SSE4) |
| `star_detection/` | Star detection and centroid computation |
| `registration/` | Image registration, alignment, and astrometry |
| `drizzle/` | Variable-Pixel Linear Reconstruction (Fruchter & Hook 2002) |

## Key Types

```rust
AstroImage           // Astronomical image wrapper around imaginarium::Image with metadata
AstroImageMetadata   // FITS metadata (object, instrument, exposure, bitpix, etc.)
ImageDimensions      // { width, height, channels } - helper struct
BitPix               // FITS pixel type enum (UInt8, Int16, Int32, Int64, Float32, Float64)
StackConfig          // Builder for stacking parameters (combine, rejection, normalization, weights)
CombineMethod        // Mean(Rejection) | Median
Normalization        // None | Global | Multiplicative
Rejection            // SigmaClip | Winsorized | LinearFit | Percentile | Gesd | None
FrameType            // Dark | Flat | Bias | Light
CacheConfig          // { cache_dir, keep_cache, available_memory, progress }
CalibrationMasters   // Container for master dark/flat/bias frames
DrizzleConfig        // Builder for drizzle parameters (kernel, scale, pixfrac, etc.)
DrizzleAccumulator   // Accumulate contributions from multiple dithered frames
DrizzleResult        // { image: AstroImage, coverage: Buffer2<f32> }
```

## Stacking Module Structure

See `stacking/NOTES-AI.md` for detailed documentation.

| Module | Description |
|--------|-------------|
| `stacking/mod.rs` | Public API, `FrameType` enum, re-exports |
| `stacking/stack.rs` | `stack()`/`stack_with_progress()` entry points, normalization, dispatch |
| `stacking/config.rs` | `StackConfig`, `CombineMethod`, `Normalization`, presets, validation |
| `stacking/rejection.rs` | Six rejection algorithms with config structs, `Rejection` enum dispatch |
| `stacking/cache.rs` | `ImageCache<I>` (in-memory or mmap), chunked parallel processing |
| `stacking/cache_config.rs` | `CacheConfig`, adaptive chunk sizing, system memory queries |
| `stacking/error.rs` | `Error` enum (thiserror), I/O and dimension errors |
| `stacking/progress.rs` | `ProgressCallback`, `StackingStage` (Loading/Processing) |
| `stacking/bench.rs` | Benchmark tests |
| `stacking/tests/` | Integration tests, real data tests |

## Demosaic Module (raw/demosaic/)

Both X-Trans (Markesteijn 1-pass) and Bayer (RCD) demosaicing are implemented.

| Module | Description |
|--------|-------------|
| `bayer/mod.rs` | `CfaPattern` enum, `BayerImage` struct, `demosaic_bayer()` entry point |
| `bayer/rcd.rs` | RCD algorithm (5 steps, rayon row-parallel, ~650 lines) |
| `bayer/tests.rs` | 20 tests (11 CFA pattern + 9 RCD correctness) |
| `xtrans/mod.rs` | X-Trans entry point, `XTransPattern`, `XTransImage` |
| `xtrans/markesteijn.rs` | Markesteijn 1-pass orchestrator, `DemosaicArena` preallocation |
| `xtrans/markesteijn_steps.rs` | Algorithm steps: green interp, R/B, derivatives, homogeneity, blend |
| `xtrans/hex_lookup.rs` | Pre-computed hexagonal neighbor lookup tables |

CFA Patterns: RGGB, BGGR, GRBG, GBRG

### Bayer Demosaic — RCD

RCD (Ratio Corrected Demosaicing) v2.3 by Luis Sanz Rodriguez. Rayon-parallel, 111ms/24MP
(216 MP/s). Buffer triple-reuse, 11 rayon dispatches. See `demosaic/bayer/NOTES-AI.md`.

### X-Trans Markesteijn 1-Pass Algorithm

Custom implementation of the Markesteijn demosaic for Fujifilm X-Trans 6x6 CFA sensors:

1. **Green min/max** — Bound green interpolation using hexagonal neighbors
2. **Green interpolation (4 directions)** — Weighted hexagonal with clamping
3. **YPbPr derivatives** — Spatial Laplacian (RGB recomputed on-the-fly from green_dir)
4. **Homogeneity maps** — Count consistent pixels per direction in 3x3 window
5. **Final blend** — Sum homogeneity in 5x5 window, recompute RGB for best directions, average

**Memory**:
- Single preallocated arena (`DemosaicArena`): 10P f32 (P = width × height)
- Layout: `[green_dir 4P | drv/output 4P | gmin/homo P | gmax/threshold P]`
- Region A: green_dir (written Step 2, read through Step 5)
- Region B: drv (Steps 3–4), then output (3P) written in Step 5
- Region C: gmin reinterpreted as homo u8 after Step 2
- Region D: gmax reused as threshold in Step 4
- RGB never materialized — recomputed on-the-fly in Steps 3 and 5 via `compute_rgb_pixel`
- On-the-fly normalization: `XTransImage` stores raw `&[u16]` data, normalizes per-read via `read_normalized()`
- Sequential SATs: blend step builds one SAT at a time (~1P), stores scores in `hm_buf: Vec<[u32; 4]>` (~4P)
- Raw u16 data copied to `Vec<u16>` (P×2 = ~47 MB) instead of `Vec<f32>` (P×4 = ~93 MB)
- libraw guard + file buffer dropped before demosaicing starts

**Optimizations**:
- Precomputed `ColorInterpLookup` avoids per-pixel pattern search in R/B interpolation
- Uninitialized buffer allocation via `alloc_uninit_vec` avoids kernel page zeroing
- On-the-fly u16→f32 normalization eliminates ~93 MB pre-normalized buffer
- Sequential SAT construction reduces peak blend memory from ~4P to ~1P + hm_buf

**Performance** (6032x4028 X-Trans, AMD CPU):
- Our Markesteijn: ~620ms demosaic, ~1.49s total load
- libraw: ~2.5s total load
- Quality: MAE ~0.000521 vs libraw reference (after linear regression normalization)

**Parallelization**: Row-parallel via rayon `par_chunks_mut` for all steps. Step 3 uses chunked direction×row parallelism with sliding 3-row YPbPr cache (64-row chunks, ~252 tasks). Steps 3+4 previously used (direction × row) flattening.

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
| `median_filter/` | 3x3 median filter for Bayer artifacts (SIMD) |
| `fwhm_estimation.rs` | Auto FWHM estimation from bright stars |
| `buffer_pool.rs` | Buffer reuse pool for batch processing |
| `defect_map.rs` | Sensor defect (hot/dead pixel) masking |

### Star Detection Key Types

```rust
Star                  // { pos: DVec2, flux, fwhm, eccentricity, snr, peak, sharpness, roundness1, roundness2 }
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
11. Quality filtering - SNR, eccentricity, saturation, sharpness, roundness
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

Platforms: ARM NEON (aarch64), x86 SSE4/AVX2, scalar fallback

## RAW File Loading (`raw/`)

| File | Description |
|------|-------------|
| `raw/mod.rs` | `load_raw()`, RAII guards, sensor dispatch (mono/Bayer/X-Trans/fallback) |
| `raw/normalize.rs` | SIMD u16-to-f32 normalization (SSE4.1, SSE2, NEON, scalar) |
| `raw/tests.rs` | Unit tests for loading, guards, normalization |
| `raw/benches.rs` | Benchmarks, libraw quality comparison |

Supported: RAF, CR2, CR3, NEF, ARW, DNG

## Examples

- `full_pipeline.rs` - Complete calibration workflow: master frames, light calibration, star detection, registration

## Dependencies

common, imaginarium, fitsio, rawloader, libraw-rs, anyhow, rayon, strum_macros
