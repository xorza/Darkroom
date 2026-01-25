# Lumos - AI Implementation Notes

## Project Structure

```
lumos/src/
├── lib.rs                    # Main library exports
├── math.rs                   # SIMD-optimized math operations
├── common/
│   └── parallel.rs          # Parallel iteration utilities
├── astro_image/
│   ├── mod.rs               # AstroImage struct, calibration, metadata
│   ├── demosaic/            # Bayer and X-Trans demosaicing
│   │   ├── bayer/           # SIMD Bayer demosaic (with bench.rs)
│   │   └── xtrans/          # SIMD X-Trans demosaic (with bench.rs)
│   ├── hot_pixels.rs        # Hot pixel detection and correction (with bench.rs)
│   ├── libraw.rs            # LibRaw bindings for raw file loading
│   └── sensor.rs            # Sensor type detection (Bayer/X-Trans/Mono)
├── calibration_masters.rs   # Bias/Dark/Flat master loading
├── stacking/
│   ├── mod.rs               # ImageStack API
│   ├── cache.rs             # Memory/disk caching with mmap
│   ├── cache_config.rs      # Memory allocation configuration
│   ├── mean/                # Mean stacking (with bench.rs)
│   ├── median/              # Median stacking (with bench.rs)
│   ├── sigma_clipped/       # Sigma-clipped mean stacking (with bench.rs)
│   └── error.rs             # Error types
└── star_detection/
    ├── mod.rs               # Main API: find_stars(), Star, StarDetectionConfig
    ├── notes.md             # Detailed algorithm review and optimization plan
    ├── background/          # Tiled sigma-clipped background estimation
    │   ├── mod.rs           # estimate_background() function
    │   ├── simd/            # SIMD implementations (AVX2/SSE/NEON)
    │   │   ├── mod.rs       # Runtime dispatch
    │   │   ├── sse.rs       # x86_64 implementations
    │   │   └── neon.rs      # aarch64 implementations
    │   ├── tests.rs         # Unit tests
    │   └── bench.rs         # Benchmarks
    ├── convolution/         # Gaussian convolution / matched filtering
    │   ├── mod.rs           # gaussian_convolve(), matched_filter()
    │   ├── simd/            # SIMD implementations (AVX2/SSE/NEON)
    │   │   ├── mod.rs       # Runtime dispatch for row convolution
    │   │   ├── sse.rs       # x86_64: AVX2+FMA and SSE4.1 implementations
    │   │   └── neon.rs      # aarch64: NEON implementations
    │   ├── tests.rs         # Unit tests
    │   └── bench.rs         # Benchmarks
    ├── detection/           # Connected component star detection
    │   ├── mod.rs           # detect_stars(), dilate_mask()
    │   ├── tests.rs         # Unit tests
    │   └── bench.rs         # Benchmarks
    ├── deblend/             # Star deblending algorithms
    │   ├── mod.rs           # Public interface, DeblendConfig
    │   ├── local_maxima.rs  # Simple deblending via local maxima
    │   ├── multi_threshold.rs # SExtractor-style tree-based deblending
    │   ├── tests.rs         # Unit tests
    │   └── bench.rs         # Benchmarks
    ├── centroid/            # Sub-pixel centroid and profile fitting
    │   ├── mod.rs           # compute_centroid(), quality metrics
    │   ├── gaussian_fit.rs  # 2D Gaussian L-M fitting
    │   ├── moffat_fit.rs    # 2D Moffat L-M fitting
    │   ├── linear_solver.rs # Shared 5x5/6x6 Gaussian elimination
    │   ├── tests.rs         # Unit tests
    │   └── bench.rs         # Benchmarks
    ├── cosmic_ray/          # L.A.Cosmic cosmic ray detection
    │   ├── mod.rs           # Public interface
    │   ├── laplacian.rs     # Laplacian computation and SNR
    │   ├── fine_structure.rs# Median-based fine structure
    │   ├── tests.rs         # Unit tests
    │   └── bench.rs         # Benchmarks
    ├── median_filter/       # 3x3 median filtering
    │   ├── mod.rs           # median_filter_3x3()
    │   ├── tests.rs         # Unit tests
    │   └── bench.rs         # Benchmarks
    └── visual_tests/        # Visual debugging and validation
        ├── mod.rs
        ├── debug_steps.rs
        ├── subpixel_accuracy.rs
        └── synthetic.rs
```

## Key Modules

### Star Detection (`star_detection/`)

Detects stars in astronomical images with sub-pixel accuracy.

**API:**
```rust
pub fn find_stars(pixels: &[f32], width: usize, height: usize, config: &StarDetectionConfig) -> StarDetectionResult;

pub struct StarDetectionResult {
    pub stars: Vec<Star>,                  // Detected stars sorted by flux
    pub diagnostics: StarDetectionDiagnostics, // Pipeline statistics
}

pub struct Star {
    pub x: f32,           // Sub-pixel X position
    pub y: f32,           // Sub-pixel Y position
    pub flux: f32,        // Total flux (background-subtracted)
    pub fwhm: f32,        // Full Width at Half Maximum
    pub eccentricity: f32,// 0=circular, 1=elongated
    pub snr: f32,         // Signal-to-noise ratio
    pub peak: f32,        // Peak pixel value
    pub sharpness: f32,   // peak/core_flux - cosmic rays have high values
    pub roundness1: f32,  // GROUND: marginal Gaussian fit roundness
    pub roundness2: f32,  // SROUND: bilateral symmetry roundness
    pub laplacian_snr: f32, // L.A.Cosmic Laplacian SNR metric
}

pub struct StarDetectionDiagnostics {
    pub candidates_after_filtering: usize,
    pub stars_after_centroid: usize,
    pub rejected_low_snr: usize,
    pub rejected_high_eccentricity: usize,
    pub rejected_cosmic_rays: usize,
    pub rejected_saturated: usize,
    pub rejected_fwhm_outliers: usize,
    pub rejected_duplicates: usize,
    pub final_star_count: usize,
    pub median_fwhm: f32,
    pub median_snr: f32,
    pub background_stats: (f32, f32, f32),  // (min, max, mean)
    pub noise_stats: (f32, f32, f32),       // (min, max, mean)
}
```

**Algorithm:**
1. **Preprocessing**: 3x3 median filter to remove Bayer pattern artifacts from CFA sensors
2. **Background estimation** (`background/`): Tiled sigma-clipped median with 3x3 tile median filter for robustness to bright stars, then bilinear interpolation. Optional iterative refinement with object masking (SExtractor-style).
3. **Matched filtering** (`convolution/`): Gaussian convolution matching expected PSF to boost SNR for faint stars (SIMD-accelerated)
4. **Detection** (`detection/`): Threshold mask + morphological dilation (radius 1) + connected component labeling (union-find) + deblending for star pairs
5. **Deblending** (`deblend/`): Local maxima detection or multi-threshold tree-based deblending (SExtractor-style)
6. **Centroid** (`centroid/`): Iterative Gaussian-weighted centroid refinement with adaptive stamp radius (~3.5× FWHM)
7. **Profile fitting** (`centroid/gaussian_fit.rs`, `moffat_fit.rs`): Optional 2D Gaussian or Moffat fitting for ~0.01 pixel accuracy
8. **Quality metrics**: DAOFIND-style roundness (GROUND/SROUND), L.A.Cosmic Laplacian SNR
9. **Filtering**: SNR, eccentricity, sharpness (cosmic ray rejection), area constraints, FWHM outlier removal (MAD-based), deduplication

**SIMD Acceleration:**
- **Convolution**: Separable Gaussian convolution with AVX2+FMA, SSE4.1, or NEON
- **Background**: Sum and sum-of-squares computation with SIMD (prepared, not yet integrated)
- Runtime detection with automatic fallback to scalar code

**Profile Fitting (new):**
- **Gaussian fitting**: 6-parameter 2D Gaussian via Levenberg-Marquardt
- **Moffat fitting**: 5 or 6 parameter (fixed or variable beta) via Levenberg-Marquardt
- **Shared linear solver**: Gaussian elimination with partial pivoting for 5x5 and 6x6 systems

**Key Implementation Details:**
- **Median filter**: Removes alternating-row sensitivity differences from Bayer CFA patterns that cause horizontal striping in threshold masks
- **Tile median filter**: 3x3 median on background tiles rejects outliers from bright stars contaminating tiles
- **Dilation (radius 1)**: Connects fragmented star regions while minimizing merging of close stars
- **Deblending**: Local maxima or multi-threshold tree deblending (configurable)
- **Adaptive stamp radius**: Centroid window sized at ~3.5× expected FWHM to capture >99% of PSF flux
- **Sharpness metric**: peak/core_flux ratio distinguishes cosmic rays (>0.7) from real stars (0.2-0.5)
- **L.A.Cosmic**: Laplacian edge detection for sharp cosmic ray artifacts
- **Centroid uses original pixels**: Smoothed data for detection, original for sub-pixel accuracy

**Configurable Parameters (StarDetectionConfig):**
- `deblend_min_separation`: Minimum separation between peaks for deblending (default: 3 pixels)
- `deblend_min_prominence`: Minimum peak prominence as fraction of primary (default: 0.3)
- `multi_threshold_deblend`: Enable SExtractor-style multi-threshold deblending
- `deblend_nthresh`: Number of sub-thresholds for multi-threshold deblending (default: 32)
- `deblend_min_contrast`: Minimum contrast for branch splitting (default: 0.005)
- `duplicate_min_separation`: Minimum separation for duplicate removal (default: 8.0 pixels)
- `max_sharpness`: Maximum sharpness for cosmic ray rejection (default: 0.7)
- `max_roundness`: Maximum roundness for non-circular source rejection (default: 1.0 = disabled)

### Hot Pixel Detection (`hot_pixels.rs`)

Uses per-channel MAD-based outlier detection:
- Samples ~100K pixels for efficiency
- 1.4826 constant converts MAD to standard deviation
- Corrections applied via neighbor interpolation

### Stacking (`stacking/`)

Memory-efficient image stacking with automatic disk caching:
- Uses 75% of available memory threshold
- Mmap for disk-backed storage
- Supports mean, median, and sigma-clipped mean

### Demosaic (`demosaic/`)

SIMD-optimized demosaicing:
- Bayer: Bilinear interpolation with SSE/AVX
- X-Trans: Pattern-aware neighbor interpolation

## Constants and Magic Numbers

- **1.4826**: MAD to sigma conversion factor (1/Φ⁻¹(0.75))
- **2.355**: FWHM = 2.355 × σ for Gaussian profiles
- **75% memory**: Conservative threshold for system stability
- **3σ clipping**: Standard for outlier rejection in astronomy

## Constraints

- **Background tile_size**: Must be 16-256, image must be at least tile_size × tile_size

## Testing

Run full validation:
```bash
cargo test && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings
```

Many tests require environment variables pointing to test data (marked as `#[ignore]`).

## Benchmarks

Benchmarks are gated behind the `bench` feature. Run with:
```bash
# All benchmarks
cargo bench --features bench

# Specific benchmark
cargo bench --features bench --bench star_detection_background
cargo bench --features bench --bench demosaic_bayer
cargo bench --features bench --bench hot_pixels
cargo bench --features bench --bench median_filter
cargo bench --features bench --bench stack_mean
cargo bench --features bench --bench stack_median
cargo bench --features bench --bench stack_sigma_clipped
```

Available benchmarks in `benches/`:
- `star_detection_background.rs` - Background estimation (synthetic data, various image/tile sizes)
- `demosaic_bayer.rs` - Bayer demosaic (requires LUMOS_CALIBRATION_DIR)
- `demosaic_xtrans.rs` - X-Trans demosaic (requires LUMOS_CALIBRATION_DIR)
- `hot_pixels.rs` - Hot pixel detection
- `math.rs` - SIMD math operations
- `median_filter.rs` - 3x3 median filtering
- `stack_*.rs` - Stacking operations

## Performance Notes

### Background Estimation
- 4K image (4096×4096) with 64-pixel tiles: ~31ms (~540 Melem/s)
- Parallelized tile computation with rayon
- Row-chunked interpolation to reduce false sharing

### Convolution (SIMD-accelerated)
- Separable Gaussian convolution: O(n×k) instead of O(n×k²)
- AVX2+FMA: Processes 8 pixels per iteration with fused multiply-add
- SSE4.1: Processes 4 pixels per iteration
- NEON: Processes 4 pixels per iteration with FMA
- Automatic fallback to scalar for edge pixels and small images

### Profile Fitting
- Levenberg-Marquardt optimization typically converges in 10-20 iterations
- Gaussian fitting: ~0.01 pixel centroid accuracy
- Moffat fitting: Better for atmospheric seeing (extended wings)
