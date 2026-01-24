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
│   │   ├── bayer/           # SIMD Bayer demosaic
│   │   └── xtrans/          # SIMD X-Trans demosaic
│   ├── hot_pixels.rs        # Hot pixel detection and correction
│   ├── libraw.rs            # LibRaw bindings for raw file loading
│   └── sensor.rs            # Sensor type detection (Bayer/X-Trans/Mono)
├── calibration_masters.rs   # Bias/Dark/Flat master loading
├── stacking/
│   ├── mod.rs               # ImageStack API
│   ├── cache.rs             # Memory/disk caching with mmap
│   ├── cache_config.rs      # Memory allocation configuration
│   ├── mean.rs              # Mean stacking
│   ├── median.rs            # Median stacking (chunked)
│   ├── sigma_clipped.rs     # Sigma-clipped mean stacking
│   └── error.rs             # Error types
└── star_detection/
    ├── mod.rs               # Main API: find_stars(), Star, StarDetectionConfig
    ├── background.rs        # Tiled sigma-clipped background estimation
    ├── detection.rs         # Connected component star detection
    └── centroid.rs          # Sub-pixel centroid and quality metrics
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
1. **Background estimation** (`background.rs`): Tiled sigma-clipped median with bilinear interpolation
2. **Detection** (`detection.rs`): Threshold mask + connected component labeling (union-find)
3. **Centroid** (`centroid.rs`): Iterative Gaussian-weighted centroid refinement

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
cargo test && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings
```

Many tests require environment variables pointing to test data (marked as `#[ignore]`).
