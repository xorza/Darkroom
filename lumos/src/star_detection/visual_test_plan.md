# Visual Test Plan for Star Detection Module

## Overview

This document outlines a comprehensive visual testing strategy for the star detection module. Visual tests complement unit tests by providing:
1. **Human-inspectable outputs** - PNG images showing algorithm results
2. **Ground truth comparison** - Overlay of expected vs detected stars
3. **Algorithm stage visualization** - Intermediate pipeline outputs
4. **Edge case coverage** - Challenging astronomical scenarios

All tests use **synthetic data with known ground truth** for reproducible validation.

---

## Test Categories

### 1. Algorithm Stage Tests

Visualize each stage of the detection pipeline independently.

| Test | Input | Output | Purpose |
|------|-------|--------|---------|
| `test_vis_background_estimation` | Stars on gradient bg | Background map + residual | Verify smooth background removal |
| `test_vis_median_filter` | CFA pattern + stars | Before/after median | Show Bayer artifact removal |
| `test_vis_matched_filter` | Faint stars in noise | Before/after convolution | Show SNR improvement |
| `test_vis_threshold_mask` | Mixed SNR stars | Binary threshold mask | Show detection regions |
| `test_vis_connected_components` | Overlapping stars | Labeled components (colored) | Show segmentation |
| `test_vis_dilation` | Star mask | Before/after dilation | Show morphological growth |
| `test_vis_centroid_refinement` | Single star | Iteration path overlay | Show convergence |
| `test_vis_deblending` | Blended pair | Deblend tree/regions | Show separation |
| `test_vis_cosmic_ray_rejection` | Stars + cosmic rays | Laplacian SNR map | Show CR detection |

---

### 2. Full Pipeline Tests - Standard Cases

| Test | Configuration | Expected Result |
|------|--------------|-----------------|
| `test_vis_sparse_field` | 20 well-separated stars | All detected, <0.1px centroid error |
| `test_vis_dense_field` | 100 stars, some close | Most detected, deblending active |
| `test_vis_faint_stars` | SNR 5-20 stars | Detection threshold validation |
| `test_vis_bright_saturated` | Mix of normal + saturated | Saturated flagged correctly |
| `test_vis_variable_fwhm` | FWHM 2-8 pixels | Adaptive stamp sizing works |
| `test_vis_variable_background` | Gradient + vignette | Local threshold adaptation |

---

### 3. Full Pipeline Tests - Challenging Cases

#### 3.1 Crowded Fields
| Test | Description | Challenge |
|------|-------------|-----------|
| `test_vis_crowded_uniform` | 500+ stars, uniform density | Deblending at scale |
| `test_vis_crowded_clustered` | Dense cluster + sparse halo | Local vs global crowding |
| `test_vis_touching_stars` | Stars separated by 1 FWHM | Deblend vs merge decision |
| `test_vis_chain_of_stars` | 5 stars in a line, touching | Multiple overlaps |

#### 3.2 Elliptical Stars (Tracking Errors)
| Test | Description | Challenge |
|------|-------------|-----------|
| `test_vis_elliptical_uniform` | All stars e=0.4, same angle | Consistent elongation |
| `test_vis_elliptical_varying` | e=0.2-0.8, random angles | Eccentricity filtering |
| `test_vis_trailed_stars` | Aspect ratio 3:1 | Extreme elongation rejection |
| `test_vis_field_rotation` | Elongation varies with radius | Typical tracking error pattern |

#### 3.3 Noise and Artifacts
| Test | Description | Challenge |
|------|-------------|-----------|
| `test_vis_high_noise` | SNR ~ 3-5 range | False positive rejection |
| `test_vis_hot_pixels` | Random hot pixels | DefectMap masking |
| `test_vis_bad_columns` | Vertical bad columns | Column masking |
| `test_vis_cosmic_rays` | Sharp single-pixel spikes | Sharpness/Laplacian filtering |
| `test_vis_satellite_trail` | Linear streak | Roundness filtering |

#### 3.4 Background Challenges
| Test | Description | Challenge |
|------|-------------|-----------|
| `test_vis_nebula_background` | Non-uniform bright region | Local threshold needed |
| `test_vis_gradient_strong` | 50% intensity gradient | Background subtraction |
| `test_vis_vignette` | Radial falloff pattern | Edge detection unchanged |
| `test_vis_amp_glow` | Corner brightening | Background tile adaptation |

#### 3.5 Edge Cases
| Test | Description | Challenge |
|------|-------------|-----------|
| `test_vis_star_at_edge` | Star partially off image | Edge rejection |
| `test_vis_star_at_corner` | Star in corner | Stamp extraction |
| `test_vis_small_image` | 64x64 with 5 stars | Minimum viable size |
| `test_vis_large_image` | 4096x4096 performance | Memory/threading |

---

## Synthetic Data Generators

### 4.1 Core Star Profiles

```rust
/// Gaussian star profile (good seeing)
fn render_gaussian_star(
    pixels: &mut [f32], width: usize,
    x: f32, y: f32, sigma: f32, amplitude: f32
);

/// Moffat profile (realistic atmospheric PSF)
fn render_moffat_star(
    pixels: &mut [f32], width: usize,
    x: f32, y: f32, alpha: f32, beta: f32, amplitude: f32
);

/// Elliptical Gaussian (tracking error)
fn render_elliptical_star(
    pixels: &mut [f32], width: usize,
    x: f32, y: f32, sigma_major: f32, sigma_minor: f32, 
    angle: f32, amplitude: f32
);

/// Saturated star (flat-topped)
fn render_saturated_star(
    pixels: &mut [f32], width: usize,
    x: f32, y: f32, sigma: f32, saturation_level: f32
);
```

### 4.2 Field Generators

```rust
/// Random star field with configurable parameters
struct StarFieldConfig {
    width: usize,
    height: usize,
    num_stars: usize,
    fwhm_range: (f32, f32),
    magnitude_range: (f32, f32),  // Converted to flux
    background_level: f32,
    noise_sigma: f32,
    // Optional modifiers
    crowding: CrowdingType,       // Uniform, Clustered, Gradient
    elongation: ElongationType,   // None, Uniform, Varying, FieldRotation
    saturation_fraction: f32,
    cosmic_ray_count: usize,
    hot_pixel_count: usize,
}

fn generate_star_field(config: &StarFieldConfig) -> (Vec<f32>, Vec<GroundTruthStar>);
```

### 4.3 Artifact Generators

```rust
/// Add CFA pattern (Bayer RGGB)
fn add_bayer_pattern(pixels: &mut [f32], width: usize, strength: f32);

/// Add cosmic ray hits
fn add_cosmic_rays(pixels: &mut [f32], width: usize, count: usize) -> Vec<(usize, usize)>;

/// Add hot/dead pixels
fn add_defects(pixels: &mut [f32], width: usize, defect_map: &DefectMap);

/// Add satellite/airplane trail
fn add_linear_trail(
    pixels: &mut [f32], width: usize,
    start: (f32, f32), end: (f32, f32), width_px: f32, amplitude: f32
);

/// Add nebula-like background structure
fn add_nebula_background(pixels: &mut [f32], width: usize, config: &NebulaConfig);
```

---

## Output Format

### 5.1 Single-Stage Output

Each stage test produces:
```
test_output/
  vis_background_estimation/
    input.png              # Original synthetic image
    background_map.png     # Estimated background (stretched)
    residual.png           # Background-subtracted image
    overlay.png            # Side-by-side comparison
    report.txt             # Statistics and metrics
```

### 5.2 Full Pipeline Output

Each pipeline test produces:
```
test_output/
  vis_crowded_uniform/
    input.png              # Synthetic star field
    ground_truth.png       # True star positions (blue circles)
    detected.png           # Detected stars (green circles)
    comparison.png         # Overlay: blue=truth, green=detected, red=missed
    metrics.txt            # Detection rate, centroid errors, etc.
```

### 5.3 Comparison Image Encoding

| Color | Meaning |
|-------|---------|
| Blue circle | Ground truth star position |
| Green circle | Correctly detected star |
| Red circle | Missed star (false negative) |
| Yellow circle | False positive detection |
| Cyan cross | Detected centroid position |
| Magenta cross | True centroid position |

---

## Metrics and Validation

### 6.1 Detection Metrics

```rust
struct DetectionMetrics {
    // Counts
    true_positives: usize,    // Detected within 2 FWHM of truth
    false_positives: usize,   // No matching truth star
    false_negatives: usize,   // Truth star not detected
    
    // Rates
    detection_rate: f32,      // TP / (TP + FN)
    precision: f32,           // TP / (TP + FP)
    f1_score: f32,            // Harmonic mean
    
    // Positional accuracy
    centroid_errors: Vec<f32>,     // Distance from truth
    mean_centroid_error: f32,
    max_centroid_error: f32,
    
    // Property accuracy
    fwhm_errors: Vec<f32>,         // (detected - true) / true
    flux_errors: Vec<f32>,
}
```

### 6.2 Pass/Fail Criteria

| Metric | Standard Case | Crowded Case | Faint Stars |
|--------|---------------|--------------|-------------|
| Detection rate | > 98% | > 90% | > 80% |
| False positive rate | < 2% | < 5% | < 10% |
| Mean centroid error | < 0.1 px | < 0.2 px | < 0.5 px |
| FWHM error | < 15% | < 25% | < 40% |

---

## Implementation Plan

### Phase 1: Infrastructure (Priority: High)

| Task | Description | Effort |
|------|-------------|--------|
| 1.1 | Create `visual_tests/generators/` module | 2h |
| 1.2 | Implement core star profile renderers | 3h |
| 1.3 | Implement `StarFieldConfig` and generator | 4h |
| 1.4 | Implement artifact generators (CR, defects, trails) | 3h |
| 1.5 | Create `visual_tests/output/` image saving utilities | 2h |
| 1.6 | Implement comparison image renderer | 2h |
| 1.7 | Implement `DetectionMetrics` computation | 2h |

### Phase 2: Algorithm Stage Tests (Priority: High)

| Task | Description | Effort |
|------|-------------|--------|
| 2.1 | `test_vis_background_estimation` | 1h |
| 2.2 | `test_vis_median_filter` | 1h |
| 2.3 | `test_vis_matched_filter` | 1h |
| 2.4 | `test_vis_threshold_mask` | 1h |
| 2.5 | `test_vis_connected_components` | 1h |
| 2.6 | `test_vis_centroid_refinement` | 2h |
| 2.7 | `test_vis_deblending` | 2h |
| 2.8 | `test_vis_cosmic_ray_rejection` | 1h |

### Phase 3: Standard Pipeline Tests (Priority: High)

| Task | Description | Effort |
|------|-------------|--------|
| 3.1 | `test_vis_sparse_field` | 1h |
| 3.2 | `test_vis_dense_field` | 1h |
| 3.3 | `test_vis_faint_stars` | 1h |
| 3.4 | `test_vis_bright_saturated` | 1h |
| 3.5 | `test_vis_variable_fwhm` | 1h |
| 3.6 | `test_vis_variable_background` | 1h |

### Phase 4: Challenging Case Tests (Priority: Medium)

| Task | Description | Effort |
|------|-------------|--------|
| 4.1 | Crowded field tests (4 tests) | 3h |
| 4.2 | Elliptical star tests (4 tests) | 2h |
| 4.3 | Noise/artifact tests (5 tests) | 3h |
| 4.4 | Background challenge tests (4 tests) | 2h |
| 4.5 | Edge case tests (4 tests) | 2h |

### Phase 5: Regression and CI Integration (Priority: Medium)

| Task | Description | Effort |
|------|-------------|--------|
| 5.1 | Create reference image set | 2h |
| 5.2 | Implement image diff comparison | 2h |
| 5.3 | Add `#[ignore]` for slow visual tests | 1h |
| 5.4 | Document test output location and usage | 1h |

---

## File Structure

```
lumos/src/star_detection/
├── visual_tests/
│   ├── mod.rs                    # Test module declarations
│   ├── generators/
│   │   ├── mod.rs
│   │   ├── star_profiles.rs      # Gaussian, Moffat, elliptical
│   │   ├── star_field.rs         # StarFieldConfig, generate_star_field
│   │   ├── artifacts.rs          # Cosmic rays, defects, trails
│   │   └── backgrounds.rs        # Gradients, nebulae, vignette
│   ├── output/
│   │   ├── mod.rs
│   │   ├── image_writer.rs       # PNG output utilities
│   │   ├── comparison.rs         # Overlay renderer
│   │   └── metrics.rs            # DetectionMetrics computation
│   ├── stage_tests/
│   │   ├── mod.rs
│   │   ├── background.rs         # Background estimation visual tests
│   │   ├── filtering.rs          # Median, matched filter tests
│   │   ├── detection.rs          # Threshold, components tests
│   │   ├── centroid.rs           # Centroid refinement tests
│   │   └── cosmic_ray.rs         # CR rejection tests
│   └── pipeline_tests/
│       ├── mod.rs
│       ├── standard.rs           # Sparse, dense, faint, saturated
│       ├── crowded.rs            # Crowded field tests
│       ├── elliptical.rs         # Tracking error simulation
│       ├── artifacts.rs          # Noise, CR, hot pixels
│       ├── background.rs         # Nebula, gradient, vignette
│       └── edge_cases.rs         # Boundary and size tests
```

---

## Running Visual Tests

```bash
# Run all visual tests (slow, generates images)
cargo test -p lumos --features visual-tests -- visual_tests --nocapture

# Run specific category
cargo test -p lumos --features visual-tests -- test_vis_crowded

# Run single test
cargo test -p lumos --features visual-tests -- test_vis_crowded_uniform --nocapture

# Output location
ls target/test_output/star_detection/
```

---

## Example Test Implementation

```rust
#[test]
#[cfg(feature = "visual-tests")]
fn test_vis_crowded_uniform() {
    let config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 500,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (8.0, 14.0),
        background_level: 0.1,
        noise_sigma: 0.02,
        crowding: CrowdingType::Uniform,
        elongation: ElongationType::None,
        ..Default::default()
    };
    
    let (pixels, ground_truth) = generate_star_field(&config);
    
    let detection_config = StarDetectionConfig {
        multi_threshold_deblend: true,  // Enable for crowded field
        ..Default::default()
    };
    
    let result = find_stars(&pixels, config.width, config.height, &detection_config);
    
    // Compute metrics
    let metrics = compute_detection_metrics(&result.stars, &ground_truth, config.fwhm_range.1);
    
    // Save outputs
    let output_dir = test_output_path("vis_crowded_uniform");
    save_grayscale_png(&pixels, config.width, &output_dir.join("input.png"));
    save_comparison_image(
        &pixels, config.width,
        &ground_truth, &result.stars,
        &output_dir.join("comparison.png")
    );
    save_metrics(&metrics, &output_dir.join("metrics.txt"));
    
    // Assertions
    assert!(metrics.detection_rate > 0.90, "Detection rate {} < 90%", metrics.detection_rate);
    assert!(metrics.mean_centroid_error < 0.2, "Centroid error {} > 0.2px", metrics.mean_centroid_error);
}
```

---

## Summary

| Category | Test Count | Priority |
|----------|------------|----------|
| Algorithm stages | 9 | High |
| Standard pipeline | 6 | High |
| Crowded fields | 4 | Medium |
| Elliptical stars | 4 | Medium |
| Noise/artifacts | 5 | Medium |
| Background challenges | 4 | Medium |
| Edge cases | 4 | Low |
| **Total** | **36** | |

Estimated total effort: **~45 hours** (including infrastructure)
