# Star Detection Pipeline Redesign

## Reference Implementations

Three reference implementations inform this redesign:

- **SExtractor / SEP**: flat function API (`sep.Background()` -> `sep.extract()` -> `sep.sum_circle()`), single `extract()` call with all config as parameters, returns structured array. Background is a separate object computed first. Deblending is internal to `extract()` (32 sub-thresholds, min_contrast=0.005). No separate segmentation step exposed to the caller.

- **photutils / DAOStarFinder**: class-based API with `find_stars(data, mask)` entry point. Internal pipeline: kernel construction -> convolution -> peak finding -> property extraction (lazy) -> filtering. Catalog classes compute metrics lazily via `@lazyproperty`. Filtering is a separate `apply_all_filters()` method on the catalog. Clean separation between detection (positions) and measurement (properties).

- **IRAF DAOFIND**: convolve with Gaussian kernel -> find density maxima above threshold -> compute centroid by marginal Gaussian fits -> compute sharpness and roundness -> filter. Linear pipeline, each step produces clear output for next step.

### Key patterns across all three:
1. Background estimation is always a separate, independent first step
2. Detection (finding positions) is separate from measurement (computing properties)
3. Filtering happens after measurement, not interleaved with detection
4. Configuration is flat parameters, not nested config trees
5. Deblending is part of the segmentation/detection step, not a separate user-facing stage

---

## Current Problems

### 1. StarDetector is a god object

`StarDetector::detect()` is a 100-line method that orchestrates preprocessing, background estimation, FWHM estimation, matched filtering, candidate detection, centroid computation, quality filtering, FWHM outlier removal, duplicate removal, and statistics. Each "step" interleaves buffer pool management with actual logic. The method is hard to follow, hard to test in isolation, and hard to extend.

### 2. Buffer pool threading is invasive

`BufferPool` is threaded through every layer. `BackgroundMap::from_pool`, `LabelMap::from_pool`, pool acquire/release pairs are scattered across the detector, candidate detection, and background modules. This couples allocation strategy to algorithm logic. Every function that touches a buffer needs to know about the pool.

### 3. Candidate detection does too many things

`candidate_detection::detect_stars()` creates threshold masks, dilates them, runs connected component labeling, extracts candidates, filters by size/edge, and deblends -- all in one function. The mask creation, labeling, and deblending are conceptually separate stages forced into one module.

### 4. Centroid module mixes centroid computation with quality metrics

`centroid::compute_centroid()` computes the weighted-moments position, optionally runs Gaussian/Moffat fitting, then computes flux/FWHM/eccentricity/SNR/sharpness/roundness/laplacian_snr. Centroid refinement and quality measurement are separate concerns jammed together.

### 5. Filtering logic is split across three locations

Size/edge filtering happens in `candidate_detection::extract_and_filter_candidates`. Quality filtering (SNR, eccentricity, sharpness, roundness, saturation) happens in `detector::apply_quality_filters`. FWHM outlier removal and duplicate removal are separate functions called later. Three separate filter passes on different data representations.

### 6. Too many config structs for a flat pipeline

`StarDetectionConfig` nests `BackgroundConfig`, `FilteringConfig`, `DeblendConfig`, `CentroidConfig`, `PsfConfig`, plus optional `NoiseModel` and `DefectMap`. The nesting creates awkward field paths (`config.filtering.min_snr`, `config.background.refinement`, `config.centroid.local_background_method`). Presets modify fields across multiple sub-configs, requiring knowledge of the full tree.

### 7. Data flows through inconsistent representations

The pipeline passes through: `AstroImage` -> `Buffer2<f32>` -> `BitBuffer2` (mask) -> `LabelMap` -> `Vec<StarCandidate>` -> `Vec<Star>`. `StarCandidate` has {bbox, peak, peak_value, area}. `Star` has {pos, flux, fwhm, eccentricity, snr, peak, sharpness, roundness1, roundness2, laplacian_snr}. The transition from candidate to star happens inside `compute_centroid` which builds a `Star` from scratch, discarding the candidate's bbox and area.

### 8. FWHM estimation is a recursive mini-pipeline

`estimate_fwhm_from_bright_stars` constructs a modified `StarDetectionConfig`, calls `detect_stars` (which runs thresholding + labeling + deblending), then calls `compute_centroids`, then passes stars to `estimate_fwhm()`. This is a full detection pipeline inside the detection pipeline. Works correctly but is complex and hard to reason about.

### 9. Deblend requires LabelMap which couples stages

The deblend functions (`deblend_local_maxima`, `deblend_multi_threshold`) take `ComponentData` which iterates pixels by checking `labels[idx] == self.label`. This means the `LabelMap` must remain alive through deblending and cannot be freed after segmentation. This is an inherent data dependency, not a design flaw -- but the current code obscures it by bundling everything into `candidate_detection::detect_stars()`.

---

## Proposed Redesign

### Design Principles

- **Flat pipeline**: each stage takes input, produces output, no hidden side effects
- **Detection separate from measurement**: finding positions (stages 1-6) is independent from computing properties (stage 7)
- **Stages are independent functions**: can be tested and benchmarked alone
- **One data type per transition**: clear what goes in and comes out
- **Config is flat**: one struct, fields grouped by comments, no nesting
- **Buffer management is separate from algorithms**: pool lives at the top level

### Data Structures

```rust
// Per-pixel background and noise estimates.
// Data-only struct, no methods. Replaces BackgroundMap.
struct ImageStats {
    background: Buffer2<f32>,
    noise: Buffer2<f32>,
    adaptive_sigma: Option<Buffer2<f32>>,
}

// Connected region from segmentation + deblending.
// Replaces StarCandidate. Identical fields.
struct Region {
    bbox: Aabb,
    peak: Vec2us,
    peak_value: f32,
    area: usize,
}

// Measured source. Unchanged from current Star.
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

// Final output.
struct DetectionResult {
    stars: Vec<Star>,
    diagnostics: Diagnostics,
}
```

### Pipeline

```
AstroImage
   |
   v
[1. Prepare]          grayscale, defect correction, median filter (CFA)
   |
   v
Buffer2<f32>          (clean grayscale pixels)
   |
   v
[2. Background]       tile stats, interpolation, optional iterative refinement
   |
   v
ImageStats            (per-pixel background + noise)
   |
   v
[3. FWHM Estimate]    optional: fast first-pass detect -> robust median FWHM
   |
   v
Option<f32>           (effective FWHM, or None if disabled)
   |
   v
[4. Detect]           matched filter (opt) -> threshold -> dilate -> label -> deblend -> filter regions
   |
   v
Vec<Region>           (candidate source regions)
   |
   v
[5. Measure]          centroid refinement + all quality metrics per region
   |
   v
Vec<Star>             (measured sources with positions and metrics)
   |
   v
[6. Filter]           SNR, eccentricity, sharpness, roundness, saturation,
   |                   FWHM outliers, duplicate removal
   v
Vec<Star>             (final catalog, sorted by flux)
```

### Why 6 stages instead of 9

The initial proposal split detection into threshold/segment/deblend/filter-regions as 4 separate stages. After reviewing SExtractor, SEP, and photutils:

- **SExtractor/SEP** bundles thresholding, segmentation, deblending, and region filtering into a single `extract()` call. These steps are tightly coupled: the LabelMap must survive through deblending, and region filtering (min_area, edge_margin) is a trivial retain-pass that doesn't warrant a separate stage function.

- **photutils** similarly has a single `_find_stars()` internal method that does convolution + peak finding as one step.

- **Deblending needs the LabelMap and raw pixels**: `ComponentData::iter_pixels` checks `labels[idx] == self.label` for every pixel. Exposing `LabelMap` in the public pipeline would leak an implementation detail. Better to keep it internal to the detect stage.

The detect stage is internally structured as substeps (threshold -> dilate -> label -> extract_components -> deblend -> filter_regions), but the caller sees one function: `pixels + stats + config -> Vec<Region>`.

### Stage Signatures

```rust
// Stage 1: Prepare
fn prepare(
    image: &AstroImage,
    defects: Option<&DefectMap>,
    pool: &mut BufferPool,
) -> Buffer2<f32>;

// Stage 2: Background estimation
fn estimate_background(
    pixels: &Buffer2<f32>,
    config: &Config,
    pool: &mut BufferPool,
) -> ImageStats;

// Stage 3: FWHM estimation (optional)
// Runs a lightweight first-pass detection internally.
// Returns None when both auto_estimate_fwhm=false and expected_fwhm=0.
fn estimate_fwhm(
    pixels: &Buffer2<f32>,
    stats: &ImageStats,
    config: &Config,
    pool: &mut BufferPool,
) -> Option<f32>;

// Stage 4: Detection
// Internally: matched_filter -> threshold_mask -> dilate -> label -> deblend -> filter_regions
// LabelMap is created and consumed internally, never exposed.
fn detect(
    pixels: &Buffer2<f32>,
    stats: &ImageStats,
    fwhm: Option<f32>,
    config: &Config,
    pool: &mut BufferPool,
) -> Vec<Region>;

// Stage 5: Measurement
// Centroid refinement (weighted moments, optional Gaussian/Moffat fitting)
// plus all quality metrics (flux, FWHM, eccentricity, SNR, sharpness, roundness, laplacian).
fn measure(
    regions: &[Region],
    pixels: &Buffer2<f32>,
    stats: &ImageStats,
    config: &Config,
) -> Vec<Star>;

// Stage 6: Filtering
// All filtering in one pass: saturation, SNR, eccentricity, sharpness,
// roundness, FWHM outliers, duplicate removal. Returns sorted by flux.
fn filter(stars: Vec<Star>, config: &Config) -> Vec<Star>;
```

### Config

Flatten into a single struct. Group fields by comments. No nesting.

```rust
#[derive(Debug, Clone)]
struct Config {
    // -- Background estimation --
    tile_size: usize,                       // default 64
    sigma_clip_iterations: usize,           // default 3
    refinement: BackgroundRefinement,       // None / Iterative / AdaptiveSigma
    bg_mask_dilation: usize,               // dilation for background refinement masks, default 3
    min_unmasked_fraction: f32,            // default 0.3

    // -- Detection --
    sigma_threshold: f32,                   // default 4.0
    connectivity: Connectivity,             // Four / Eight, default Four

    // -- PSF / matched filter --
    expected_fwhm: f32,                     // 0 = no matched filter, default 4.0
    auto_estimate_fwhm: bool,              // default false
    min_stars_for_fwhm: usize,            // default 10
    fwhm_estimation_sigma_factor: f32,     // default 2.0
    psf_axis_ratio: f32,                   // default 1.0 (circular)
    psf_angle: f32,                        // default 0.0

    // -- Deblending --
    deblend_min_separation: usize,         // default 3
    deblend_min_prominence: f32,           // default 0.3
    deblend_n_thresholds: usize,           // 0 = local maxima, 32 = SExtractor-style, default 0
    deblend_min_contrast: f32,             // default 0.005

    // -- Region filtering (applied during detection) --
    min_area: usize,                        // default 5
    max_area: usize,                        // default 500
    edge_margin: usize,                     // default 10

    // -- Centroid --
    centroid_method: CentroidMethod,        // WeightedMoments / GaussianFit / MoffatFit
    local_background: LocalBackgroundMethod, // GlobalMap / LocalAnnulus

    // -- Star quality filtering --
    min_snr: f32,                           // default 10.0
    max_eccentricity: f32,                 // default 0.6
    max_sharpness: f32,                    // default 0.7
    max_roundness: f32,                    // default 0.5
    max_fwhm_deviation: f32,               // default 3.0
    duplicate_min_separation: f32,          // default 8.0

    // -- Noise model --
    noise_model: Option<NoiseModel>,        // optional, for accurate SNR

    // -- Defect map --
    defect_map: Option<DefectMap>,          // optional, for sensor defects
}
```

Presets are constructors, not builder methods. Each returns a complete Config. No chaining.

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

Current builder-style chaining (`StarDetectionConfig::default().wide_field().crowded_field()`) is eliminated. Chaining was confusing because later presets only override some fields, creating hard-to-predict combinations. With constructors, each preset is self-contained and predictable. Users who need customization modify fields after construction:

```rust
let mut config = Config::crowded_field();
config.min_snr = 20.0;
config.centroid_method = CentroidMethod::MoffatFit { beta: 2.5 };
```

### Top-level Orchestrator

```rust
#[derive(Debug)]
struct StarDetector {
    config: Config,
    pool: BufferPool,
}

impl StarDetector {
    fn new() -> Self { ... }
    fn from_config(config: Config) -> Self { ... }

    fn detect(&mut self, image: &AstroImage) -> DetectionResult {
        let pixels = prepare(image, self.config.defect_map.as_ref(), &mut self.pool);
        let stats = estimate_background(&pixels, &self.config, &mut self.pool);
        let fwhm = estimate_fwhm(&pixels, &stats, &self.config, &mut self.pool);
        let regions = detect(&pixels, &stats, fwhm, &self.config, &mut self.pool);
        let stars = measure(&regions, &pixels, &stats, &self.config);
        let stars = filter(stars, &self.config);
        // compute diagnostics ...
        DetectionResult { stars, diagnostics }
    }
}
```

The orchestrator is ~10 lines with no buffer management interleaved. Each stage function manages its own pool usage internally.

### Module Structure

```
star_detection/
    mod.rs                -- re-exports, StarDetector orchestrator
    config.rs             -- Config, enums (Connectivity, CentroidMethod, NoiseModel, etc.)
    star.rs               -- Star struct
    region.rs             -- Region struct
    stages/
        prepare.rs        -- stage 1: grayscale, defects, median filter
        background.rs     -- stage 2: estimate + optional refine, returns ImageStats
        fwhm.rs           -- stage 3: first-pass detect + robust FWHM estimation
        detect.rs         -- stage 4: threshold -> label -> deblend -> filter regions
        measure.rs        -- stage 5: centroid + quality metrics
        filter.rs         -- stage 6: all star filtering + sorting
    image_stats.rs        -- ImageStats struct (data only)
    buffer_pool.rs        -- BufferPool (unchanged)
    defect_map.rs         -- DefectMap (unchanged)

    // Low-level building blocks (unchanged internally)
    background/           -- tile grid, SIMD interpolation
    threshold_mask/       -- SIMD threshold mask creation
    mask_dilation/        -- morphological dilation
    median_filter/        -- 3x3 median filter, sorting networks
    convolution/          -- separable/elliptical Gaussian convolution
    labeling/             -- connected component labeling (moved from candidate_detection/)
    centroid/             -- weighted moments, Gaussian/Moffat fitting, SIMD
    cosmic_ray/           -- L.A.Cosmic laplacian SNR
    deblend/              -- local maxima, multi-threshold (unchanged)
```

The `pipeline/` directory from the initial proposal is renamed to `stages/` -- clearer intent, avoids confusion with the registration pipeline module.

### Internal Structure of `stages/detect.rs`

This is the most complex stage. Internally it runs:

```rust
pub fn detect(
    pixels: &Buffer2<f32>,
    stats: &ImageStats,
    fwhm: Option<f32>,
    config: &Config,
    pool: &mut BufferPool,
) -> Vec<Region> {
    // 1. Matched filter convolution (optional)
    let filtered = if let Some(fwhm) = fwhm {
        let mut out = pool.acquire_f32();
        let mut scratch1 = pool.acquire_f32();
        let mut scratch2 = pool.acquire_f32();
        matched_filter(pixels, &stats.background, fwhm, ..., &mut out, &mut scratch1, &mut scratch2);
        pool.release_f32(scratch2);
        pool.release_f32(scratch1);
        Some(out)
    } else {
        None
    };

    // 2. Threshold mask
    let mut mask = pool.acquire_bit();
    create_threshold_mask(..., &mut mask);

    // 3. Dilate
    let mut dilated = pool.acquire_bit();
    dilate_mask(&mask, 1, &mut dilated);
    pool.release_bit(mask);

    // 4. Label (connected components)
    let label_map = LabelMap::from_pool(&dilated, config.connectivity, pool);
    pool.release_bit(dilated);

    // 5. Extract component data
    let components = collect_component_data(&label_map, pixels.width(), config.max_area);

    // 6. Deblend
    let regions = deblend_components(components, pixels, &label_map, config);
    label_map.release_to_pool(pool);

    // 7. Filter regions (min_area, edge_margin)
    regions.retain(|r| r.area >= config.min_area && /* edge check */);

    if let Some(buf) = filtered { pool.release_f32(buf); }

    regions
}
```

Buffer pool operations are contained within this one function. The deblend algorithms (local_maxima, multi_threshold) remain unchanged -- they receive `ComponentData`, `pixels`, and `labels` as before.

---

## What Changes vs What Stays

### Changes

| Current | Proposed | Reason |
|---|---|---|
| `StarDetectionConfig` with 7 nested sub-configs | Flat `Config` with grouped fields | Simpler API, clearer field access |
| `StarDetector::detect()` 100-line god method | 10-line orchestrator calling 6 stage functions | Each stage testable in isolation |
| `BackgroundMap` (struct with methods) | `ImageStats` (data-only struct) + `stages::background::estimate()` free function | Separates data from computation |
| `candidate_detection::detect_stars()` does everything | `stages::detect()` with clear internal substeps | Still one function externally, but internal flow is explicit |
| `centroid::compute_centroid()` mixes position + metrics | `stages::measure()` calls centroid then metrics separately | Clear separation of concerns |
| 3 filter locations (region, quality, FWHM/dupes) | Single `stages::filter()` | All filtering in one place |
| Builder-style presets (`.wide_field().crowded_field()`) | Constructor presets (`Config::crowded_field()`) | No confusing partial overrides from chaining |
| `StarCandidate` | `Region` | Better name for what it represents |
| `candidate_detection/` module | Split into `labeling/` + `stages/detect.rs` | Labeling is a reusable building block |
| Buffer pool acquire/release scattered everywhere | Pool only touched in stage functions and orchestrator | Algorithms receive pre-allocated buffers |

### Stays the Same

- All SIMD code (centroid, convolution, threshold mask, background interpolation, median filter, mask dilation)
- Gaussian and Moffat profile fitting
- L.A.Cosmic laplacian computation
- Connected component labeling algorithm (union-find, RLE, parallel blocks)
- Deblending algorithms (local maxima, multi-threshold tree)
- `Star` struct fields and methods
- `DefectMap` struct and implementation
- `BufferPool` interface
- `BackgroundRefinement` enum
- `CentroidMethod`, `Connectivity`, `LocalBackgroundMethod` enums

---

## Migration

### External API changes

| Before | After |
|---|---|
| `StarDetectionConfig::default()` | `Config::default()` |
| `StarDetectionConfig::default().wide_field()` | `Config::wide_field()` |
| `config.filtering.min_snr` | `config.min_snr` |
| `config.background.tile_size` | `config.tile_size` |
| `config.centroid.method` | `config.centroid_method` |
| `config.psf.expected_fwhm` | `config.expected_fwhm` |
| `config.deblend.n_thresholds` | `config.deblend_n_thresholds` |
| `BackgroundMap::new_uninit(w, h, bg_config)` | `estimate_background(&pixels, &config, &mut pool)` |
| `BackgroundMap` fields (`.background`, `.noise`) | `ImageStats` fields (same names) |

### Internal callers

- `registration/pipeline/mod.rs`: uses `Star` (unchanged)
- `stacking/weighted/mod.rs`: uses `Star` and `StarDetectionResult` -> `DetectionResult` (rename)
- `testing/` modules: use `BackgroundMap` -> switch to `ImageStats` + `estimate_background()`
- `registration/tests/`: use `StarDetectionConfig` -> switch to `Config`

### lib.rs re-exports

Current re-exports from `star_detection`: `Star`, `StarDetector`, `StarDetectionConfig`, `StarDetectionResult`, `StarDetectionDiagnostics`, `BackgroundConfig`, `BackgroundMap`, `CentroidMethod`, `DefectMap`, `FilteringConfig`, `LocalBackgroundMethod`, `NoiseModel`, `PsfConfig`, `GaussianFitConfig`, `GaussianFitResult`, `MoffatFitConfig`, `MoffatFitResult`, `fit_gaussian_2d`, `fit_moffat_2d`, `alpha_beta_to_fwhm`, `fwhm_beta_to_alpha`.

After: `Star`, `StarDetector`, `Config`, `DetectionResult`, `Diagnostics`, `ImageStats`, `CentroidMethod`, `DefectMap`, `LocalBackgroundMethod`, `NoiseModel`, `BackgroundRefinement`, `Connectivity`, `GaussianFitConfig`, `GaussianFitResult`, `MoffatFitConfig`, `MoffatFitResult`, `fit_gaussian_2d`, `fit_moffat_2d`, `alpha_beta_to_fwhm`, `fwhm_beta_to_alpha`.

Removed: `BackgroundConfig`, `FilteringConfig`, `PsfConfig`, `DeblendConfig`, `CentroidConfig` (all merged into `Config`). `StarDetectionConfig` -> `Config`. `StarDetectionResult` -> `DetectionResult`. `StarDetectionDiagnostics` -> `Diagnostics`. `BackgroundMap` -> `ImageStats`.

---

## Implementation Order

1. **Create `Region` struct** and `ImageStats` struct (trivial, no behavior change)
2. **Flatten `Config`** from nested sub-configs, update all callers. Change presets from builders to constructors. This is the largest mechanical change.
3. **Extract `stages/prepare.rs`** -- move grayscale/defect/median logic out of `StarDetector::detect()`
4. **Extract `stages/background.rs`** -- move `BackgroundMap` methods into free function returning `ImageStats`
5. **Extract `stages/fwhm.rs`** -- move FWHM estimation logic
6. **Extract `stages/detect.rs`** -- consolidate threshold/label/deblend/region-filter. Move `labeling/` out of `candidate_detection/`
7. **Extract `stages/measure.rs`** -- split centroid from metrics in current `compute_centroid()`
8. **Extract `stages/filter.rs`** -- consolidate all star filtering
9. **Rewrite `StarDetector::detect()`** as flat orchestrator calling stage functions
10. **Delete `candidate_detection/mod.rs`** and `detector/mod.rs`** -- logic has been redistributed
11. **Update tests** -- stage functions are independently testable, add unit tests per stage

Each step compiles and passes tests before proceeding to the next.
