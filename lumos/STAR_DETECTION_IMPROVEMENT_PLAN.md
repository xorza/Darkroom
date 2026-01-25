# Star Detection Module - Comprehensive Improvement Plan

## Executive Summary

Based on thorough analysis of the star detection module (`lumos/src/star_detection/`) and research into
state-of-the-art astronomical source detection algorithms, this document outlines identified improvements
categorized by priority.

**Key Findings:**
- The implementation is well-structured and follows established algorithms (DAOFIND, L.A.Cosmic)
- Test coverage is comprehensive with 358 passing tests
- Benchmarks exist for all major components
- Several SIMD implementations are incomplete (empty files)
- Code duplication exists across modules
- Some magic numbers should be extracted as constants

## Research Summary

### Industry Standard Algorithms

1. **DAOFIND (Stetson 1987)** - Our implementation follows this closely:
   - Gaussian convolution (matched filtering) ✓
   - Peak detection with sharpness/roundness metrics ✓
   - Sub-pixel centroiding ✓

2. **SExtractor** - Additional techniques we could adopt:
   - Multi-threshold deblending (already implemented as option) ✓
   - Layered detection for faint objects near bright ones
   - More sophisticated background mesh filtering

3. **photutils** (Python) - Modern reference implementation:
   - DAOStarFinder with elliptical Gaussian kernels ✓
   - IRAFStarFinder with image moments ✓
   - Local background subtraction in annuli

4. **Astrometry.net image2xy** - Benchmark comparison shows:
   - ~40% detection rate (we're more conservative)
   - 0.265 pixel centroid accuracy (excellent)

### Sources Consulted
- [DAOStarFinder - photutils](https://photutils.readthedocs.io/en/stable/api/photutils.detection.DAOStarFinder.html)
- [DAOFIND - IRAF](https://iraf.readthedocs.io/en/latest/tasks/noao/digiphot/apphot/daofind.html)
- [Background Estimation - photutils](https://photutils.readthedocs.io/en/stable/user_guide/background.html)
- [SExtractor Background Modeling](https://sextractor.readthedocs.io/en/latest/Background.html)
- [Review of source detection approaches - MNRAS](https://academic.oup.com/mnras/article/422/2/1674/1040345)
- [Improved star detection - IEEE](https://ieeexplore.ieee.org/document/8707438/)

---

## Priority 1: Code Quality & Maintainability

### 1.1 Extract Shared Constants
**Files affected:** Multiple across all submodules
**Estimated effort:** Small

Create `constants.rs` with:
```rust
/// FWHM to Gaussian sigma conversion factor (2√(2ln2))
pub const FWHM_TO_SIGMA: f32 = 2.3548201;

/// MAD to standard deviation conversion (for normal distribution)
pub const MAD_TO_SIGMA: f32 = 1.4826022;

/// Default saturation threshold (95% of dynamic range)
pub const DEFAULT_SATURATION_THRESHOLD: f32 = 0.95;

/// Rows per parallel chunk for false sharing reduction
pub const ROWS_PER_CHUNK: usize = 8;

/// Stamp radius as fraction of FWHM (captures 99%+ of PSF flux)
pub const STAMP_RADIUS_FWHM_FACTOR: f32 = 1.75;
```

Currently these values are hardcoded in:
- `convolution/mod.rs:59` (2.355)
- `centroid/mod.rs:332` (2.355)
- `background/mod.rs:328,364` (1.4826)
- `median_filter/mod.rs:32` (ROWS_PER_CHUNK=8)
- Multiple other locations

### 1.2 Consolidate Duplicate Functions
**Files affected:** `detection/mod.rs`, `background/mod.rs`, `visual_tests/debug_steps.rs`
**Estimated effort:** Small

**`dilate_mask()` duplication:**
- Main: `detection/mod.rs:129-145`
- Duplicate: `background/mod.rs:580-603`
- Local copy: `visual_tests/debug_steps.rs:56-76`

**Action:** Move to common utilities, re-export from all modules.

**Median computation duplication:**
- Optimized: `median_filter/mod.rs:167-316`
- Simple: `cosmic_ray/fine_structure.rs:9-14`
- Wrapper: `background/mod.rs:138-147`

**Action:** Consolidate into single utility with variants for different use cases.

### 1.3 Improve Error Handling
**Files affected:** `deblend/local_maxima.rs`, `deblend/multi_threshold.rs`, benchmark files
**Estimated effort:** Small

Replace `.unwrap()` on `partial_cmp` with:
```rust
// Bad: panics on NaN
.partial_cmp(&b.0).unwrap()

// Good: handles NaN gracefully
.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
```

Add `.expect("message")` to test assertions for better debugging.

---

## Priority 2: Complete Missing SIMD Implementations

### 2.1 Detection SIMD (Empty Files)
**Files:** `detection/simd/*.rs` (all 0 bytes)
**Benchmark baseline:** 2.5ms for 1024x1024 @ 200 stars

Opportunity: SIMD threshold comparison and mask operations.
```rust
// Threshold comparison can process 8 f32s per AVX2 instruction
// Connected component labeling is memory-bound, less benefit expected
```

### 2.2 Median Filter SIMD (Empty Files)
**Files:** `median_filter/simd/*.rs` (all 0 bytes)
**Benchmark baseline:** 500µs for 1024x1024

Opportunity: SIMD sorting networks for 9-element median.
Reference: Existing optimized `median9` in scalar already uses sorting networks.
Potential speedup: 2-4x for the median computation portion.

### 2.3 Cosmic Ray SIMD (Stub Files)
**Files:** `cosmic_ray/simd/*.rs` (9 lines total)
**Benchmark baseline:** 70ms for 2048x2048

Opportunity: SIMD Laplacian computation (currently 6ms for compute_laplacian alone).
```rust
// Laplacian kernel is separable: can use same techniques as convolution
// Fine structure computation also benefits from SIMD
```

---

## Priority 3: Algorithm Enhancements

### 3.1 Iterative Background Estimation
**Status:** Implemented but marked `#[allow(dead_code)]`
**File:** `background/mod.rs:465-604`

Enable and expose `estimate_background_iterative()` which:
1. Estimates initial background
2. Masks detected objects
3. Dilates masks to cover PSF wings
4. Re-estimates background excluding masked pixels
5. Iterates for improved accuracy in crowded fields

This is the SExtractor approach and would improve background accuracy near bright stars.

### 3.2 Elliptical Gaussian Kernel Support
**Current:** Circular Gaussian only for matched filtering
**Reference:** DAOStarFinder supports elliptical kernels via `ratio` and `theta` parameters

Add to `StarDetectionConfig`:
```rust
pub ellipticity_ratio: f32,  // b/a axis ratio (1.0 = circular)
pub ellipticity_angle: f32,  // Position angle in radians
```

This would improve detection of stars with tracking errors or field rotation.

### 3.3 Local Background Subtraction in Centroid
**Current:** Uses global background map
**Reference:** photutils `LocalBackground` uses circular annuli

Add option to compute local background in annulus around each star during centroid refinement.
This improves accuracy in regions with variable nebulosity.

### 3.4 PSF Fitting Integration
**Status:** Gaussian and Moffat fitting exist but aren't used in main pipeline
**Files:** `centroid/gaussian_fit.rs`, `centroid/moffat_fit.rs`

Add configuration option to use PSF fitting for high-precision centroiding:
```rust
pub enum CentroidMethod {
    WeightedMoments,      // Current default (fast)
    GaussianFit,          // ~0.01 pixel accuracy
    MoffatFit { beta: f32 }, // Best for atmospheric seeing
}
```

Benchmarks show:
- Weighted moments: ~9µs per star
- Gaussian fit 21x21: ~78µs per star (8x slower but more accurate)

---

## Priority 4: Performance Optimizations

### 4.1 Background Estimation Parallelization
**Benchmark baseline:** ~11ms for 2048x2048 (matched filter)

The tile-based background estimation is already parallelized.
Opportunity: The bilinear interpolation loop could benefit from SIMD.

### 4.2 Connected Component Labeling
**Current:** Union-find with path compression (good algorithm)

Consider: Block-based labeling for better cache locality on large images.
This would mainly help images >4K resolution.

### 4.3 Reduce Allocation in Hot Paths
**Observation:** Some functions allocate Vec per call where reuse is possible.

Example in `compute_metrics()`:
```rust
// Could accept pre-allocated scratch buffers
fn compute_metrics(..., scratch: &mut MetricsScratch) -> Option<StarMetrics>
```

---

## Priority 5: Test & Benchmark Improvements

### 5.1 Enable Ignored Visual Tests
**Status:** 60 tests marked `#[ignore]`
**Reason:** Require `LUMOS_TEST_OUTPUT_DIR` or generate large files

Consider:
- Add CI configuration to run ignored tests
- Document how to run full test suite
- Add fixture generation for reproducible visual tests

### 5.2 Add Regression Benchmarks
**Current:** Benchmarks exist but no automated regression tracking

Consider integrating with:
- `criterion`'s baseline comparison (already used)
- GitHub Actions for benchmark CI
- Performance dashboard

### 5.3 Add Survey Benchmark Integration
**Files:** `survey_benchmark/` module exists
**Status:** Requires network access to catalog servers

Document and enable benchmarking against:
- SDSS catalog
- Pan-STARRS catalog
- Gaia catalog

This provides real-world validation of detection rates.

---

## Priority 6: API & Configuration Improvements

### 6.1 Split StarDetectionConfig
**Current:** 17 fields in single struct
**Issue:** Difficult to understand which parameters affect which stage

Proposed structure:
```rust
pub struct StarDetectionConfig {
    pub detection: DetectionParams,    // threshold, area bounds
    pub quality: QualityFilters,       // SNR, eccentricity, sharpness
    pub camera: CameraParams,          // gain, read_noise, is_cfa
    pub deblending: DeblendParams,     // separation, prominence
}
```

Benefits:
- Clearer API
- Easier to add stage-specific options
- Better documentation

### 6.2 Builder Pattern for Config
Add builder for common use cases:
```rust
let config = StarDetectionConfig::builder()
    .for_wide_field()  // Sets appropriate defaults for wide-field imaging
    .with_fwhm(3.5)
    .with_cosmic_ray_rejection(true)
    .build();
```

---

## Implementation Order Recommendation

1. **Week 1-2: Code Quality**
   - Extract constants (1.1)
   - Consolidate duplicates (1.2)
   - Fix error handling (1.3)

2. **Week 3-4: SIMD Completion**
   - Median filter SIMD (2.2) - most impactful
   - Cosmic ray SIMD (2.3)
   - Detection SIMD (2.1) - lower priority

3. **Week 5-6: Algorithm Enhancements**
   - Enable iterative background (3.1)
   - Add centroid method option (3.4)

4. **Future: API Improvements**
   - Config restructuring (6.1)
   - Builder pattern (6.2)

---

## Benchmark Validation Requirements

Before merging any optimization:
1. Run full benchmark suite: `cargo bench -p lumos --features bench`
2. Ensure no performance regression in unmodified code paths
3. Document speedup in PR description
4. Add benchmark for new code paths

Benchmark commands:
```bash
# Run all star detection benchmarks
cargo bench -p lumos --features bench --bench star_detection_background
cargo bench -p lumos --features bench --bench star_detection_convolution
cargo bench -p lumos --features bench --bench star_detection_centroid
cargo bench -p lumos --features bench --bench star_detection_detection
cargo bench -p lumos --features bench --bench star_detection_cosmic_ray
cargo bench -p lumos --features bench --bench star_detection_deblend
cargo bench -p lumos --features bench --bench median_filter

# Run astrometry benchmark for accuracy validation
cargo test -p lumos --features bench -- --ignored test_local_astrometry_benchmark --nocapture
```

---

## Conclusion

The star detection module is a well-engineered implementation following established astronomical algorithms.
The identified improvements focus on:
1. Code maintainability (extracting constants, reducing duplication)
2. Completing SIMD implementations for ~2-4x speedup in specific components
3. Enabling already-implemented but unused features (iterative background, PSF fitting)
4. Improving API ergonomics

The 40% detection rate vs image2xy is expected behavior - our detector is more conservative with
quality filtering (SNR, eccentricity, sharpness) to reject artifacts. The 0.265 pixel centroid
accuracy is excellent and matches state-of-the-art.
