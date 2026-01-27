# Lumos Public API Review

## Executive Summary

The lumos crate has a **well-organized public API** with clear separation between user-facing types and internal implementation. The API follows Rust conventions consistently with some areas for improvement.

**Overall Assessment**: Good - minor refinements needed, no breaking changes required.

---

## 1. Items That Should NOT Be Public

### 1.1 Registration Module - Implementation Details Exposed

**Issue**: GPU-specific functions exposed when users should use high-level API.

```rust
// Currently public but should be internal:
pub fn warp_to_reference_gpu(...)      // Implementation detail
pub fn warp_rgb_to_reference_gpu(...)  // Implementation detail
pub fn warp_multichannel_parallel(...) // Low-level utility
```

**Recommendation**: 
- Keep `GpuWarper` public for advanced users who need explicit GPU control
- Remove standalone `warp_*_gpu` functions from public API - use `warp_to_reference` which auto-selects backend
- Mark `warp_multichannel_parallel` as `pub(crate)` - it's a utility function

### 1.2 Triangle Matching - Algorithm Name in API

**Issue**: `match_stars_triangles_kdtree` exposes implementation detail (kdtree) in function name.

```rust
// Current:
pub fn match_stars_triangles_kdtree(...)

// Better:
pub fn match_triangles(...)  // Algorithm choice is internal
```

**Recommendation**: Rename to `match_triangles` or keep internal (users should use `Registrator` which calls this automatically).

### 1.3 Internal Types in Public API

**Issue**: These types are implementation details that don't need public exposure:

```rust
// registration/triangle/mod.rs
pub enum Orientation { ... }           // Internal to triangle algorithm
pub struct Triangle { ... }            // Internal data structure  
pub struct TriangleHashTable { ... }   // Internal data structure

// registration/spatial/mod.rs  
pub struct KdTree { ... }              // Internal spatial index
pub fn form_triangles_from_neighbors(...) // Internal utility
```

**Recommendation**: Mark as `pub(crate)`.

### 1.4 SIMD/Scalar Implementation Functions

**Issue**: Low-level SIMD functions exposed that users shouldn't call directly:

```rust
// registration/ransac/simd/mod.rs
pub fn count_inliers_simd(...)
pub fn count_inliers_scalar(...)

// registration/interpolation/simd/mod.rs
pub fn warp_row_bilinear_simd(...)
pub fn warp_row_bilinear_scalar(...)
pub fn warp_row_lanczos3(...)
pub fn warp_row_lanczos3_scalar(...)
pub fn bilinear_sample(...)
pub fn warp_image_lanczos3(...)

// star_detection/convolution/simd/mod.rs
pub fn convolve_row_simd(...)
pub fn simd_available()
pub fn simd_implementation_name()

// star_detection/cosmic_ray/simd/mod.rs
pub fn compute_laplacian_row_simd(...)
pub fn compute_laplacian_simd(...)
```

**Recommendation**: All should be `pub(crate)`. Users should use high-level functions that auto-dispatch.

---

## 2. Consistency Issues

### 2.1 TransformMatrix Constructor Parameter Order

**Issue**: Inconsistent parameter ordering across similar functions:

```rust
// Pattern A: (dx, dy, ...) - translation first
pub fn translation(dx: f64, dy: f64) -> Self
pub fn euclidean(dx: f64, dy: f64, angle: f64) -> Self
pub fn similarity(dx: f64, dy: f64, angle: f64, scale: f64) -> Self

// Pattern B: (angle, cx, cy) - different order
pub fn from_rotation_around(angle: f64, cx: f64, cy: f64) -> Self

// Pattern C: (sx, sy) - scale components
pub fn from_scale(sx: f64, sy: f64) -> Self
```

**Recommendation**: Standardize on `(dx, dy, angle, scale)` order where applicable. Consider:
```rust
pub fn from_rotation_around(cx: f64, cy: f64, angle: f64) -> Self  // Center first, then angle
```

### 2.2 Alias Functions - Redundant API Surface

**Issue**: Multiple ways to do the same thing:

```rust
// TransformMatrix
pub fn translation(dx, dy)       // Canonical
pub fn from_translation(dx, dy)  // Alias - adds confusion

pub fn apply(x, y)               // Canonical  
pub fn transform_point(x, y)     // Alias - adds confusion
```

**Recommendation**: 
- Remove `from_translation` alias (use `translation` only)
- Remove `transform_point` alias (use `apply` only)
- Document removed aliases in CHANGELOG if this is a breaking change

### 2.3 Star Method Naming

**Issue**: `is_usable` combines multiple checks but naming is inconsistent with other `is_*` predicates:

```rust
pub fn is_saturated(&self) -> bool
pub fn is_cosmic_ray(&self, max_sharpness: f32) -> bool
pub fn is_cosmic_ray_laplacian(&self, max_laplacian_snr: f32) -> bool
pub fn is_round(&self, max_roundness: f32) -> bool
pub fn is_usable(&self, min_snr, max_ecc, max_sharp, max_round) -> bool  // Different pattern
```

**Recommendation**: Rename to `passes_quality_filters` or `is_valid_for_registration` to distinguish from simple predicates.

### 2.4 Phase Correlation Functions - Internal Utilities Public

**Issue**: Low-level FFT utilities exposed:

```rust
pub fn hann_window(size: usize) -> Vec<f32>        // Should be internal
pub fn transpose_inplace(data: &mut [Complex<f32>], n: usize)  // Should be internal
```

**Recommendation**: Mark as `pub(crate)`.

---

## 3. Missing from Public API

### 3.1 Prelude Gaps

**Issue**: Some commonly-used types not in prelude:

```rust
// Currently NOT in prelude but should be:
// - WarpConfig (needed for custom warping)
// - RansacConfig (needed for tuning registration)
// - StarDetectionDiagnostics (useful for debugging)
```

**Recommendation**: Add to prelude:
```rust
pub use crate::{
    // ... existing ...
    WarpConfig,           // Common customization point
    StarDetectionDiagnostics,  // Useful for understanding results
};
```

---

## 4. Module Organization Issues

### 4.1 Heavy Re-exports from pub(crate) Modules

**Issue**: Modules marked `pub(crate)` but many items re-exported, creating confusion about what's "public":

```rust
// registration/mod.rs
pub(crate) mod distortion;  // But DistortionMap, ThinPlateSpline are re-exported
pub(crate) mod gpu;         // But GpuWarper, warp_*_gpu are re-exported
pub(crate) mod interpolation; // But InterpolationMethod, warp_image are re-exported
```

**Recommendation**: This pattern is acceptable but document it clearly. Consider using `#[doc(hidden)]` for items that are technically public but not intended for general use.

### 4.2 Benchmark Module Visibility

**Issue**: `bench` module requires `feature = "bench"` but exposes many internal paths:

```rust
#[cfg(feature = "bench")]
pub mod bench {
    pub use crate::star_detection::background::BackgroundMap;  // Leaks internal path
    pub use crate::star_detection::detection::{create_threshold_mask, scalar};
}
```

**Recommendation**: This is acceptable for benchmarks. Document that bench module is unstable.

---

## 5. Documentation Gaps

### 5.1 Missing Stability Markers

**Issue**: No indication of which APIs are stable vs experimental:

**Recommendation**: Add doc attributes:
```rust
/// # Stability
/// This API is stable and will follow semver.
pub fn find_stars(...) { }

/// # Stability  
/// This is an advanced/experimental API subject to change.
pub fn fit_moffat_2d(...) { }
```

### 5.2 GPU Functions Need Clearer Docs

**Issue**: Not clear when to use GPU vs CPU variants:

**Recommendation**: Add docs explaining:
- `warp_to_reference` auto-selects best backend
- Use `GpuWarper` only when you need explicit control over GPU resources
- GPU variants require GPU availability (obviously)

---

## 6. Summary of Recommended Changes

### High Priority (Visibility Fixes)

| Item | Current | Recommended | Reason |
|------|---------|-------------|--------|
| `warp_to_reference_gpu` | `pub` | `pub(crate)` | Implementation detail |
| `warp_rgb_to_reference_gpu` | `pub` | `pub(crate)` | Implementation detail |
| `warp_multichannel_parallel` | `pub` | `pub(crate)` | Low-level utility |
| `match_stars_triangles_kdtree` | `pub` | rename or `pub(crate)` | Algorithm detail in name |
| `Triangle`, `TriangleHashTable` | `pub` | `pub(crate)` | Internal data structures |
| `KdTree`, `form_triangles_from_neighbors` | `pub` | `pub(crate)` | Internal utilities |
| `hann_window`, `transpose_inplace` | `pub` | `pub(crate)` | FFT utilities |
| SIMD functions (all) | `pub` | `pub(crate)` | Implementation details |

### Medium Priority (Consistency)

| Item | Current | Recommended | Reason |
|------|---------|-------------|--------|
| `from_translation` | alias | remove | Redundant |
| `transform_point` | alias | remove | Redundant |
| `from_rotation_around` params | `(angle, cx, cy)` | `(cx, cy, angle)` | Consistency |
| `is_usable` | method | rename to `passes_quality_filters` | Clarity |

### Low Priority (Polish)

| Item | Action | Reason |
|------|--------|--------|
| Prelude | Add `WarpConfig` | Commonly needed |
| Docs | Add stability markers | User guidance |
| GPU docs | Clarify when to use | User guidance |

---

## 7. Items Correctly Public

The following are correctly exposed as public API:

**Core Types** (lib.rs exports):
- `AstroImage`, `AstroImageMetadata`, `BitPix`, `HotPixelMap`, `ImageDimensions`
- `CalibrationMasters`
- `Star`, `StarDetectionConfig`, `StarDetectionResult`, `find_stars`
- `Registrator`, `RegistrationConfig`, `RegistrationResult`, `TransformMatrix`
- `ImageStack`, `SigmaClipConfig`, `StackingProgress`

**Advanced but Valid Public**:
- `GpuWarper` - explicit GPU control
- `RansacEstimator`, `RansacConfig` - advanced registration tuning
- `BackgroundMap`, `estimate_background` - advanced star detection
- `InterpolationMethod`, `warp_image` - custom image warping
- `ThinPlateSpline`, `DistortionMap` - distortion correction
- `PhaseCorrelator` - coarse alignment

---

## 8. Migration Path

If implementing breaking changes:

1. **v0.x.y (current)**: Add deprecation warnings to items being removed
2. **v0.x+1.0**: Remove deprecated items
3. **Document** all changes in CHANGELOG.md

For non-breaking changes (visibility restrictions on unused items):
- Can be done in minor version if items weren't actually used externally
- Check if any downstream crates depend on these items first
