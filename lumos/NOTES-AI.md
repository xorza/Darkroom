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
pub fn find_stars(pixels: &[f32], width: usize, height: usize, config: &StarDetectionConfig) -> Vec<Star>;

pub struct Star {
    pub x: f32,           // Sub-pixel X position
    pub y: f32,           // Sub-pixel Y position
    pub flux: f32,        // Total flux (background-subtracted)
    pub fwhm: f32,        // Full Width at Half Maximum
    pub eccentricity: f32,// 0=circular, 1=elongated
    pub snr: f32,         // Signal-to-noise ratio
    pub peak: f32,        // Peak pixel value
}
```

**Algorithm:**
1. **Preprocessing**: 3x3 median filter to remove Bayer pattern artifacts from CFA sensors
2. **Background estimation** (`background.rs`): Tiled sigma-clipped median with bilinear interpolation
3. **Detection** (`detection.rs`): Threshold mask + morphological dilation (radius 2) + connected component labeling (union-find)
4. **Centroid** (`centroid.rs`): Iterative Gaussian-weighted centroid refinement on original (unfiltered) pixels
5. **Filtering**: SNR, eccentricity, area constraints + deduplication (8px separation)

**Key Implementation Details:**
- **Median filter**: Removes alternating-row sensitivity differences from Bayer CFA patterns that cause horizontal striping in threshold masks
- **Dilation (radius 2)**: Connects fragmented star regions that may have gaps at faint signal levels
- **Centroid uses original pixels**: Smoothed data for detection, original for sub-pixel accuracy

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
