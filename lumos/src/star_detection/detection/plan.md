# Detection Module Improvement Plan

## Completed Optimizations

### 1. RLE-based Connected Component Labeling ✓
**Achieved:** ~50% faster (15ms → 8ms → 5.5ms on 6K globular cluster)

- Run extraction using word-level bit scanning (64-bit fast-paths)
- CTZ-based scanning for mixed words (10x faster for sparse masks)
- Label runs and merge overlapping runs from previous row
- Parallel strip processing with atomic boundary merging
- Lock-free CAS-based union-find for thread safety

**Location:** `labeling/mod.rs`

### 2. 8-Connectivity Option ✓
**Impact:** Fewer fragmented detections for undersampled PSFs

- `Connectivity` enum (`Four` default, `Eight`) in `config.rs`
- `Run::search_window()` method handles connectivity-specific overlap
- `runs_connected()` function for adjacency checks

### 3. Code Organization ✓
Clean module structure with comprehensive tests (59 tests).

### 4. Adaptive Local Thresholding ✓
**Impact:** Better detection in variable nebulosity, dust lanes, gradients

Implemented tile-adaptive sigma thresholds based on local contrast:

**Algorithm:**
- Computes contrast metric (CV = sigma/median) per tile during background estimation
- Interpolates per-pixel adaptive sigma alongside background/noise
- Higher sigma in high-contrast regions (nebulae), lower in uniform sky
- SIMD-accelerated threshold mask creation (SSE4.1/NEON)

**Configuration:**
```rust
// Enable with default settings
let config = StarDetectionConfig::default()
    .with_adaptive_threshold();

// Or use preset for nebulous fields
let config = StarDetectionConfig::for_nebulous_field();

// Or customize
let config = StarDetectionConfig::default()
    .with_adaptive_threshold_config(AdaptiveThresholdConfig {
        base_sigma: 3.5,    // Low-contrast regions
        max_sigma: 6.0,     // High-contrast regions
        contrast_factor: 2.0,
    });
```

**Files modified:**
- `background/tile_grid.rs` - Added `adaptive_sigma` to TileStats, contrast computation
- `background/mod.rs` - Added `new_with_adaptive_sigma()`, third interpolation channel
- `common/threshold_mask/mod.rs` - Added `create_adaptive_threshold_mask()`
- `common/threshold_mask/sse.rs` - SIMD implementation for x86_64
- `common/threshold_mask/neon.rs` - SIMD implementation for aarch64
- `config.rs` - Added `AdaptiveThresholdConfig`
- `detection/mod.rs` - Integrated adaptive thresholding into `detect_stars()`
- `mod.rs` - Uses `new_with_adaptive_sigma()` when configured

---

## Investigated but Not Beneficial

These optimizations were tested and showed no improvement:

| Optimization | Why Not Beneficial |
|--------------|-------------------|
| Atomic path compression | Strip-based processing keeps trees shallow |
| SIMD run extraction | Most words are zeros (scalar fast-path), dispatch overhead |
| Precomputed lookup tables | CTZ already 10x faster for sparse masks |

---

## Pending Improvements

### 1. Auto-Estimate FWHM
**Impact:** Medium (usability)
**Effort:** Medium

Currently `expected_fwhm` is manual config. Auto-estimation:
1. Detect brightest N stars with conservative threshold
2. Fit Gaussian/Moffat to estimate FWHM
3. Use median FWHM for matched filter

### 2. SIMD Label Flattening
**Impact:** Low (performance)
**Effort:** Low

The final label mapping pass could use SIMD for vectorized lookup.

### 3. Adaptive Threshold with Matched Filter
**Impact:** Medium (quality)
**Effort:** Medium

Currently adaptive thresholding is disabled when matched filter is used
because the filter changes noise characteristics. Could be enabled by
scaling the adaptive sigma based on filter kernel size.

---

## Priority Matrix

| Improvement | Effort | Impact | Status |
|-------------|--------|--------|--------|
| RLE-based CCL | High | High (perf) | **Done** (~50% faster) |
| CTZ run extraction | Medium | High (perf) | **Done** (10x for sparse) |
| 8-connectivity option | Low | Medium (quality) | **Done** |
| Code organization | Low | Medium (maintainability) | **Done** |
| Adaptive thresholding | High | High (quality) | **Done** |
| Auto-estimate FWHM | Medium | Medium (usability) | Pending |
| SIMD label flattening | Low | Low (perf) | Pending |
| Adaptive + matched filter | Medium | Medium (quality) | Pending |

---

## API Summary

### Adaptive Thresholding

```rust
// Method 1: Builder pattern
let config = StarDetectionConfig::default()
    .with_adaptive_threshold();

// Method 2: Preset
let config = StarDetectionConfig::for_nebulous_field();

// Method 3: Custom configuration
let config = StarDetectionConfig::default()
    .with_adaptive_threshold_config(AdaptiveThresholdConfig {
        base_sigma: 3.5,
        max_sigma: 6.0,
        contrast_factor: 2.0,
    });

// Direct BackgroundMap usage
let adaptive = AdaptiveSigmaConfig {
    base_sigma: 3.5,
    max_sigma: 6.0,
    contrast_factor: 2.0,
};
let bg = BackgroundMap::new_with_adaptive_sigma(&pixels, &bg_config, adaptive);
```

### Performance Notes

- Memory: +24MB for 6K image (one `Buffer2<f32>` for adaptive sigma)
- Compute: ~30% overhead in background estimation (third channel interpolation)
- Threshold mask: ~20% slower due to per-pixel sigma load (still SIMD accelerated)
