# Star Detection

The detector turns an `AstroImage` into flux-sorted `Star` values plus diagnostics for every
pipeline stage. `StarDetector` validates its composed configuration once at construction and reuses
its buffer pool across images.

## Public API

All public types are exported from the crate root:

```rust
use lumos::{StarDetectionConfig, StarDetector};

let mut detector = StarDetector::from_config(StarDetectionConfig::default())?;
let result = detector.detect(&image);

for star in &result.stars {
    println!(
        "({:.2}, {:.2}) flux={:.1} SNR={:.1} FWHM={:.2}",
        star.pos.x, star.pos.y, star.flux, star.snr, star.fwhm,
    );
}
```

`StarDetector::default()` is the infallible default-config shortcut. Use `from_config` for custom
settings; it returns `StarDetectionConfigError` before allocating detection buffers when any nested
component is invalid.

## Composed configuration

`StarDetectionConfig` owns one configuration value per processing responsibility:

| Field | Type | Responsibility |
|-------|------|----------------|
| `background` | `StarDetectionBackgroundConfig` | Tile size, sigma clipping, masking refinement |
| `detection` | `StarDetectionCandidateConfig` | Thresholding, PSF shape, deblending, area and edge gates |
| `fwhm` | `StarDetectionFwhmConfig` | Fixed or automatically estimated matched-filter FWHM |
| `measurement` | `StarDetectionMeasurementConfig` | Centroid method, local background, sensor noise model |
| `filter` | `StarDetectionFilterConfig` | SNR, shape, FWHM-outlier, and duplicate rejection |

Customize only the stage that owns a setting:

```rust
use lumos::{
    BackgroundRefinement, CentroidMethod, NoiseModel, StarDetectionConfig, StarDetector,
};

let mut config = StarDetectionConfig::crowded_field();
config.background.refinement = BackgroundRefinement::Iterative { iterations: 2 };
config.detection.sigma_threshold = 3.5;
config.fwhm.auto_estimate = true;
config.measurement.centroid_method = CentroidMethod::MoffatFit { beta: 2.5 };
config.measurement.noise_model = Some(NoiseModel::new(1.5, 5.0));
config.filter.min_snr = 15.0;

let mut detector = StarDetector::from_config(config)?;
```

Presets are `wide_field`, `high_resolution`, `crowded_field`, and `precise_ground`.

## Pipeline

The detector has six processing responsibilities. Background estimation stays in `background/`
because image operations reuse its mesh; the other five stage boundaries live in
`detector/stages/`. Each receives only its stage-owned configuration:

1. `prepare` builds a linear detection plane. Grayscale is copied; RGB channels are combined with
   noise weighting. CFA input receives a 3×3 median filter.
2. `background` uses the shared `background_mesh::TileGrid`: sigma-clipped tile statistics,
   crowding-aware Pearson mode, a 3×3 tile median filter, and natural bicubic-spline interpolation.
   Iterative mode masks sources and recomputes the mesh.
3. `fwhm` uses `fwhm.expected` or estimates a robust median FWHM from a stricter first pass.
4. `detect` applies the optional matched filter, threshold mask, connected-component labeling,
   deblending, and area/edge filtering.
5. `measure` constructs the canonical `Star` directly from weighted moments or a Gaussian/Moffat
   Levenberg–Marquardt fit. It also computes flux, SNR, FWHM, eccentricity, sharpness, and DAOFIND
   roundness metrics.
6. `filter` applies saturation and quality gates, robust FWHM-outlier rejection, duplicate removal,
   and the final descending flux sort.

`deblend_n_thresholds = 0` selects local-maxima deblending; values from 2 through 256 select the
SExtractor-style multi-threshold tree.

## Results and diagnostics

`StarDetectionResult` contains:

- `stars`: accepted `Star` values sorted brightest first.
- `diagnostics`: counts for threshold pixels, connected components, candidate filtering,
  deblending, measurement, estimated FWHM, and final output.

Quality-stage rejection counts are kept as one `StarDetectionQualityFilterDiagnostics` component
inside the main diagnostics value: saturation, low SNR, eccentricity, cosmic-ray sharpness,
roundness, FWHM outliers, and duplicates.

## Module layout

| Path | Ownership |
|------|-----------|
| `config.rs` | Composed public configuration and validation |
| `detector/mod.rs` | Orchestration, results, diagnostics, reusable pool |
| `detector/stages/` | Prepare, FWHM, detection, measurement, and filtering boundaries |
| `background/` | Background/noise estimation over the shared mesh |
| `convolution/` | Circular and elliptical matched filtering |
| `threshold_mask/` | Threshold-mask SIMD kernels |
| `labeling/` | RLE connected components and union-find |
| `deblend/` | Local-maxima and multi-threshold separation |
| `centroid/` | Moments, profile fits, and stellar measurements |
| `star.rs` | Canonical detected-star value |
| `buffer_pool.rs` | Allocation reuse across detections |

## Performance and accuracy

The tile mesh, convolution, thresholding, median filtering, and profile fitting have runtime SIMD
dispatch with scalar fallbacks. The buffer pool retains full-frame allocations between calls, so a
detector should be reused for a batch.

For registration, auto-FWHM plus weighted moments is the normal throughput/accuracy balance.
Gaussian fitting suits well-sampled symmetric PSFs; Moffat fitting better models atmospheric wings.
Use `LocalBackgroundMethod::LocalAnnulus` when rapidly varying nebulosity makes the global mesh a
poor local estimate.

When detection is sparse, first adjust `detection.sigma_threshold`, then verify
`fwhm.expected`/`fwhm.auto_estimate`, `detection.min_area`, and the background tile size. When false
positives dominate, raise the threshold or `filter.min_snr` and tighten the shape gates.
