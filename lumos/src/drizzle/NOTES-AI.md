# drizzle Module - Implementation Notes

## Overview
Implements Variable-Pixel Linear Reconstruction (Drizzle algorithm, Fruchter & Hook 2002). Takes dithered input images with geometric transforms, shrinks input pixels into "drops" (controlled by pixfrac), maps them onto a higher-resolution output grid (controlled by scale), and accumulates weighted contributions. Supports four kernels: Square, Point, Gaussian, Lanczos.

## Architecture
- `DrizzleAccumulator`: Accumulate contributions from multiple frames, then finalize
- Correct axis-aligned rectangle overlap computation for weighted accumulation
- Homography support via full projective division
- f64 intermediates for coordinate transform precision
- Per-channel weight tracking
- Builder pattern for configuration with validation

## Known Issues

### Critical: Drop Size Formula is Inverted
**Location:** `mod.rs:181`

```rust
let drop_size = pixfrac / scale;  // WRONG
```

Should be `pixfrac * scale`.

With defaults (pixfrac=0.8, scale=2.0):
- Current: 0.4 output pixels
- Correct: 1.6 output pixels

Current behavior makes drops cover < 1 output pixel, acting like an aggressive point kernel. Self-consistent normalization hides the error, but spatial flux distribution is fundamentally wrong. This defeats drizzle's key benefit: proper flux distribution for resolution recovery.

### Critical: Square Kernel is Actually Turbo Kernel
**Location:** `mod.rs:196-243`

Current implementation:
- Only transforms the CENTER point
- Creates axis-aligned bounding box in output space
- This is the "Turbo" approximation, not true Square kernel

True Square kernel should:
- Transform all 4 corners of input pixel
- Compute polygon-polygon overlap using Sutherland-Hodgman clipping

For images with field rotation, axis-aligned approximation has systematic errors.

**Fix:** Rename to `Turbo`, implement proper `Square` with corner transformation and polygon clipping.

### Major: Gaussian Kernel FWHM is Wrong
**Location:** `mod.rs:275`

```rust
sigma = drop_size / 2.355
```

FWHM should be pixfrac in input pixel units, converted to output pixels. Current formula compounds the inverted drop_size error.

### Major: Lanczos Used Without Required Constraints
**Location:** `mod.rs:309`, config

Per STScI DrizzlePac and Siril documentation:
- Lanczos requires pixfrac=1.0
- Not recommended with scale!=1.0

Current implementation:
- No validation or warnings for incompatible parameters
- `x3()` preset sets pixfrac=0.7, violating the constraint

### Moderate: No Context/Contribution Image
STScI DrizzlePac produces:
- `outsci`: science data
- `outwht`: weight map
- `outcon`: bitmask of contributing frames

Current implementation only produces data + weights.

Context image is important for:
- Artifact identification
- Error estimation
- Debugging alignment issues

### Moderate: No Per-Pixel Input Weights / Bad Pixel Masks
**Location:** `mod.rs:167`

Current: Only scalar weight per frame, no per-pixel weight maps.

Cannot mask:
- Bad pixels
- Cosmic rays
- Satellite trails in individual frames

### Moderate: No Jacobian / Geometric Distortion Correction
**Location:** `mod.rs:181-182`

Current: Constant drop_size for all pixels across the image.

For proper handling of non-uniform transforms (especially homographies):
- Should compute local Jacobian determinant
- Drop sizes should vary spatially to conserve flux

Only critical for transforms with significant spatial variation in magnification.

### Moderate: min_coverage Compared Against Raw Accumulated Weight
**Location:** `mod.rs:338`

Documented as 0-1 threshold but compared against raw weight sum (depends on frame count). Should compare against normalized coverage fraction instead.

### Minor: No Parallelism
Per-pixel loop is single-threaded. STScI DrizzlePac and Siril both process rows in parallel for better performance.

### Minor: into_interleaved_pixels Allocation on Every Frame
**Location:** `mod.rs:184`

Could work with planar channel data directly to avoid allocation overhead.

### Minor: Coverage Map Averages Across Channels
**Location:** `mod.rs:328-332`

Standard practice: coverage is per-pixel spatial coverage, not per-channel average.

## Correct Kernels
- Point kernel: Correct implementation
- Lanczos kernel: Correct function with singularity handling at x=0

## References
- Fruchter & Hook 2002: Original drizzle paper
- STScI DrizzlePac: Reference implementation
- Siril: Alternative implementation with documented constraints
