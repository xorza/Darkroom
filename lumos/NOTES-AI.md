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
    ├── background/          # Tiled sigma-clipped background estimation
    │   ├── mod.rs           # estimate_background() function
    │   ├── tests.rs         # Unit tests
    │   └── bench.rs         # Benchmarks
    ├── detection/           # Connected component star detection
    │   ├── mod.rs           # detect_stars(), dilate_mask()
    │   └── tests.rs         # Unit tests
    └── centroid/            # Sub-pixel centroid and quality metrics
        ├── mod.rs           # compute_centroid()
        └── tests.rs         # Unit tests
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
2. **Background estimation** (`background.rs`): Tiled sigma-clipped median with 3x3 tile median filter for robustness to bright stars, then bilinear interpolation
3. **Detection** (`detection.rs`): Threshold mask + morphological dilation (radius 1) + connected component labeling (union-find) + deblending for star pairs
4. **Centroid** (`centroid.rs`): Iterative Gaussian-weighted centroid refinement on original (unfiltered) pixels with adaptive stamp radius (~3.5× FWHM)
5. **Filtering**: SNR, eccentricity, sharpness (cosmic ray rejection), area constraints, FWHM outlier removal (MAD-based), deduplication (configurable separation)

**Key Implementation Details:**
- **Median filter**: Removes alternating-row sensitivity differences from Bayer CFA patterns that cause horizontal striping in threshold masks
- **Tile median filter**: 3x3 median on background tiles rejects outliers from bright stars contaminating tiles
- **Dilation (radius 1)**: Connects fragmented star regions while minimizing merging of close stars
- **Deblending**: Detects local maxima within connected components to separate star pairs (configurable min_separation and min_prominence)
- **Adaptive stamp radius**: Centroid window sized at ~3.5× expected FWHM to capture >99% of PSF flux
- **Sharpness metric**: peak/core_flux ratio distinguishes cosmic rays (>0.7) from real stars (0.2-0.5)
- **Centroid uses original pixels**: Smoothed data for detection, original for sub-pixel accuracy

**Configurable Parameters (StarDetectionConfig):**
- `deblend_min_separation`: Minimum separation between peaks for deblending (default: 3 pixels)
- `deblend_min_prominence`: Minimum peak prominence as fraction of primary (default: 0.3)
- `duplicate_min_separation`: Minimum separation for duplicate removal (default: 8.0 pixels)
- `max_sharpness`: Maximum sharpness for cosmic ray rejection (default: 0.7)

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
cargo test && cargo fmt && cargo check && cargo clippy --all-targets --features bench -- -D warnings
```

Many tests require environment variables pointing to test data (marked as `#[ignore]`).

## Benchmarks

Benchmarks are gated behind the `bench` feature. Run with:
```bash
# All benchmarks
cargo bench --features bench

# Specific benchmark
cargo bench --features bench --bench background
cargo bench --features bench --bench demosaic_bayer
cargo bench --features bench --bench hot_pixels
cargo bench --features bench --bench stack_mean
cargo bench --features bench --bench stack_median
cargo bench --features bench --bench stack_sigma_clipped
```

Available benchmarks in `benches/`:
- `background.rs` - Background estimation (synthetic data, various image/tile sizes)
- `demosaic_bayer.rs` - Bayer demosaic (requires LUMOS_CALIBRATION_DIR)
- `demosaic_xtrans.rs` - X-Trans demosaic (requires LUMOS_CALIBRATION_DIR)
- `hot_pixels.rs` - Hot pixel detection
- `math.rs` - SIMD math operations
- `stack_*.rs` - Stacking operations
