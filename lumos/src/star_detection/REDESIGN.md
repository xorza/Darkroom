# Star Detection Pipeline Redesign

## Current Problems

### 1. StarDetector is a god object

`StarDetector::detect()` is a 100-line method that orchestrates preprocessing, background estimation, FWHM estimation, matched filtering, candidate detection, centroid computation, quality filtering, FWHM outlier removal, duplicate removal, and statistics. Each "step" interleaves buffer pool management with actual logic. The method is hard to follow, hard to test in isolation, and hard to extend.

### 2. Buffer pool threading is invasive

`BufferPool` is threaded through every layer. `BackgroundMap::from_pool`, `LabelMap::from_pool`, pool acquire/release pairs are scattered across the detector, candidate detection, and background modules. This couples allocation strategy to algorithm logic. Every function that touches a buffer needs to know about the pool.

### 3. Candidate detection does too many things

`candidate_detection::detect_stars()` creates threshold masks, dilates them, runs connected component labeling, extracts candidates, filters by size/edge, and deblends — all in one function. The mask creation, labeling, and deblending are conceptually separate stages forced into one module.

### 4. Centroid module mixes centroid computation with quality metrics

`centroid::compute_centroid()` computes the weighted-moments position, optionally runs Gaussian/Moffat fitting, then computes flux/FWHM/eccentricity/SNR/sharpness/roundness/laplacian_snr. Centroid refinement and quality measurement are separate concerns jammed together.

### 5. Filtering logic is split across two locations

Size/edge filtering happens in `candidate_detection::extract_and_filter_candidates`. Quality filtering (SNR, eccentricity, sharpness, roundness, saturation) happens in `detector::apply_quality_filters`. FWHM outlier removal and duplicate removal are separate functions called later. Three separate filter passes on different data representations.

### 6. Too many config structs for a flat pipeline

`StarDetectionConfig` nests `BackgroundConfig`, `FilteringConfig`, `DeblendConfig`, `CentroidConfig`, `PsfConfig`, plus optional `NoiseModel` and `DefectMap`. The nesting creates awkward field paths (`config.filtering.min_snr`, `config.background.refinement`, `config.centroid.local_background_method`). Presets modify fields across multiple sub-configs, requiring knowledge of the full tree.

### 7. Data flows through inconsistent representations

The pipeline passes through: `AstroImage` -> `Buffer2<f32>` -> `BitBuffer2` (mask) -> `LabelMap` -> `Vec<StarCandidate>` -> `Vec<Star>`. `StarCandidate` has {bbox, peak, peak_value, area}. `Star` has {pos, flux, fwhm, eccentricity, snr, peak, sharpness, roundness1, roundness2, laplacian_snr}. The transition from candidate to star happens inside `compute_centroid` which builds a `Star` from scratch, discarding the candidate's bbox and area.

---

## Proposed Redesign

### Design Principles

- **Flat pipeline**: each stage takes input, produces output, no hidden side effects
- **Stages are independent functions**: can be tested and benchmarked alone
- **One data type per transition**: clear what goes in and comes out
- **Config is flat**: one struct, fields grouped by comments, no nesting
- **Buffer management is separate from algorithms**: pool lives at the top, algorithms take slices

### Data Structures

```
// The only pixel-level intermediate — replaces BackgroundMap's 3 fields
struct ImageStats {
    background: Buffer2<f32>,
    noise: Buffer2<f32>,
    adaptive_sigma: Option<Buffer2<f32>>,  // only with adaptive thresholding
}

// Connected region — replaces StarCandidate
struct Region {
    bbox: Aabb,
    peak: Vec2us,
    peak_value: f32,
    area: usize,
}

// Measurement — replaces Star
// All fields computed at once in the measurement stage
struct Star {
    pos: DVec2,
    flux: f32,
    fwhm: f32,
    eccentricity: f32,
    snr: f32,
    peak: f32,
    sharpness: f32,
    roundness1: f32,
    roundness2: f32,
    laplacian_snr: f32,
}

// Final output — unchanged
struct DetectionResult {
    stars: Vec<Star>,
    diagnostics: Diagnostics,
}
```

### Pipeline Stages

```
AstroImage
   |
   v
[1. Prepare]  ——  grayscale, defect correction, median filter (CFA)
   |
   v
Buffer2<f32>   (clean grayscale)
   |
   v
[2. Background]  ——  tile stats, interpolation, optional refinement
   |
   v
ImageStats     (background + noise per pixel)
   |
   v
[3. FWHM Estimate]  ——  optional, first-pass detection + robust median
   |
   v
f32            (effective FWHM for matched filter)
   |
   v
[4. Threshold]  ——  matched filter (optional), threshold mask, dilation
   |
   v
BitBuffer2     (above-threshold pixels)
   |
   v
[5. Segment]  ——  connected component labeling
   |
   v
(LabelMap, Vec<Region>)   (labeled regions with bbox/area/peak)
   |
   v
[6. Deblend]  ——  split blended regions (local maxima or multi-threshold)
   |
   v
Vec<Region>    (individual source regions)
   |
   v
[7. Filter Regions]  ——  min/max area, edge margin
   |
   v
Vec<Region>    (valid regions)
   |
   v
[8. Measure]  ——  centroid refinement + quality metrics per region
   |
   v
Vec<Star>      (measured stars)
   |
   v
[9. Filter Stars]  ——  SNR, eccentricity, sharpness, roundness, saturation,
   |                     FWHM outliers, duplicate removal
   v
Vec<Star>      (final catalog)
```

### Stage Signatures

```rust
// Stage 1
fn prepare(image: &AstroImage, defects: Option<&DefectMap>, pool: &mut BufferPool) -> Buffer2<f32>;

// Stage 2
fn estimate_background(pixels: &Buffer2<f32>, config: &Config, pool: &mut BufferPool) -> ImageStats;

// Stage 3 (optional)
fn estimate_fwhm(
    pixels: &Buffer2<f32>,
    stats: &ImageStats,
    config: &Config,
    pool: &mut BufferPool,
) -> f32;

// Stage 4
fn threshold(
    pixels: &Buffer2<f32>,
    stats: &ImageStats,
    fwhm: Option<f32>,
    config: &Config,
    pool: &mut BufferPool,
) -> BitBuffer2;

// Stage 5
fn segment(mask: &BitBuffer2, connectivity: Connectivity, pool: &mut BufferPool) -> (LabelMap, Vec<Region>);

// Stage 6
fn deblend(
    regions: Vec<Region>,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    config: &Config,
) -> Vec<Region>;

// Stage 7
fn filter_regions(regions: Vec<Region>, width: usize, height: usize, config: &Config) -> Vec<Region>;

// Stage 8
fn measure(
    regions: &[Region],
    pixels: &Buffer2<f32>,
    stats: &ImageStats,
    config: &Config,
) -> Vec<Star>;

// Stage 9
fn filter_stars(stars: Vec<Star>, config: &Config) -> Vec<Star>;
```

### Config

Flatten into a single struct with grouped fields:

```rust
struct Config {
    // -- Background --
    tile_size: usize,
    sigma_clip_iterations: usize,
    refinement: BackgroundRefinement,
    mask_dilation: usize,
    min_unmasked_fraction: f32,

    // -- Detection threshold --
    sigma_threshold: f32,

    // -- PSF / matched filter --
    expected_fwhm: f32,        // 0 = disabled
    auto_estimate_fwhm: bool,
    min_stars_for_fwhm: usize,
    fwhm_estimation_sigma_factor: f32,
    psf_axis_ratio: f32,
    psf_angle: f32,

    // -- Segmentation --
    connectivity: Connectivity,

    // -- Deblending --
    deblend_min_separation: usize,
    deblend_min_prominence: f32,
    deblend_n_thresholds: usize,   // 0 = local maxima only
    deblend_min_contrast: f32,

    // -- Region filtering --
    min_area: usize,
    max_area: usize,
    edge_margin: usize,

    // -- Centroid --
    centroid_method: CentroidMethod,
    local_background: LocalBackgroundMethod,

    // -- Star filtering --
    min_snr: f32,
    max_eccentricity: f32,
    max_sharpness: f32,
    max_roundness: f32,
    max_fwhm_deviation: f32,
    duplicate_min_separation: f32,

    // -- Noise model (optional) --
    noise_model: Option<NoiseModel>,
}
```

Presets become associated functions that return a fully-populated `Config`:

```rust
impl Config {
    fn default() -> Self { ... }
    fn wide_field() -> Self { ... }
    fn high_resolution() -> Self { ... }
    fn crowded_field() -> Self { ... }
    fn nebulous_field() -> Self { ... }
    fn precise_ground() -> Self { ... }
}
```

### Top-level Orchestrator

```rust
struct StarDetector {
    config: Config,
    pool: BufferPool,
}

impl StarDetector {
    fn detect(&mut self, image: &AstroImage) -> DetectionResult {
        let pixels = prepare(image, self.config.defect_map.as_ref(), &mut self.pool);
        let stats = estimate_background(&pixels, &self.config, &mut self.pool);
        let fwhm = if self.config.auto_estimate_fwhm {
            Some(estimate_fwhm(&pixels, &stats, &self.config, &mut self.pool))
        } else if self.config.expected_fwhm > 0.0 {
            Some(self.config.expected_fwhm)
        } else {
            None
        };
        let mask = threshold(&pixels, &stats, fwhm, &self.config, &mut self.pool);
        let (labels, regions) = segment(&mask, self.config.connectivity, &mut self.pool);
        let regions = deblend(regions, &pixels, &labels, &self.config);
        let regions = filter_regions(regions, pixels.width(), pixels.height(), &self.config);
        let stars = measure(&regions, &pixels, &stats, &self.config);
        let stars = filter_stars(stars, &self.config);
        // ... diagnostics ...
        DetectionResult { stars, diagnostics }
    }
}
```

### Module Structure

```
star_detection/
    mod.rs              -- re-exports, StarDetector
    config.rs           -- Config, enums (Connectivity, CentroidMethod, etc.)
    star.rs             -- Star struct
    region.rs           -- Region struct (was StarCandidate)
    pipeline/
        prepare.rs      -- stage 1: grayscale, defects, median filter
        background.rs   -- stage 2: tile estimation, interpolation, refinement
        threshold.rs    -- stage 4: matched filter, thresholding, dilation
        segment.rs      -- stage 5: connected component labeling
        deblend.rs      -- stage 6: local maxima + multi-threshold
        measure.rs      -- stage 8: centroid + quality metrics
        filter.rs       -- stages 7 + 9: region filtering + star filtering
    fwhm_estimation.rs  -- stage 3: robust FWHM from bright stars
    buffer_pool.rs      -- BufferPool
    defect_map.rs       -- DefectMap
    image_stats.rs      -- ImageStats (was BackgroundMap)

    // Low-level building blocks (unchanged internally)
    background/         -- tile grid, SIMD interpolation
    threshold_mask/     -- SIMD threshold mask creation
    mask_dilation/      -- morphological dilation
    median_filter/      -- 3x3 median filter, sorting networks
    convolution/        -- separable/elliptical Gaussian convolution
    labeling/           -- connected component labeling (was inside candidate_detection)
    centroid/           -- weighted moments, Gaussian/Moffat fitting, SIMD
    cosmic_ray/         -- L.A.Cosmic laplacian SNR
    deblend/            -- local maxima, multi-threshold (internal algorithms)
```

### What Changes vs What Stays

**Changes:**
- `StarDetector::detect()` becomes a flat sequence of stage calls
- `StarDetectionConfig` flattened into `Config`
- `StarCandidate` renamed to `Region`
- `BackgroundMap` becomes `ImageStats` (data-only, no methods)
- Background estimation/refinement becomes a free function in `pipeline/background.rs`
- `candidate_detection` module split: labeling goes to `labeling/`, detection logic becomes `pipeline/threshold.rs` + `pipeline/segment.rs`
- `centroid::compute_centroid` split: centroid part stays, metrics part becomes `pipeline/measure.rs`
- Filtering consolidated into `pipeline/filter.rs`
- Buffer pool acquire/release only happens in `StarDetector::detect()` and stage functions, not inside algorithms

**Stays the same:**
- All SIMD code (centroid, convolution, threshold mask, background interpolation, median filter, mask dilation)
- Gaussian/Moffat fitting
- L.A.Cosmic laplacian
- Connected component labeling algorithm
- Deblending algorithms (local maxima, multi-threshold)
- `Star` struct fields
- `DefectMap`
- `BufferPool` (interface unchanged, just used more cleanly)

### Migration Notes

- `BackgroundConfig` fields merge into `Config` — callers using `StarDetectionConfig` get a simpler API
- External users of `Star`, `StarDetector`, `StarDetectionConfig` need import path changes
- `BackgroundMap` users (testing module, registration tests) switch to `ImageStats`
- Presets change from builder-style (`.wide_field()`) to constructor-style (`Config::wide_field()`) — eliminates chaining ambiguity
