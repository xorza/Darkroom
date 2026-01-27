# Distortion Module - AI Implementation Notes

## Overview

This module provides parametric and non-parametric distortion models for correcting optical aberrations in astronomical images.

## Distortion Types

### 1. Radial Distortion (`radial.rs`)
**Brown-Conrady model** for barrel/pincushion distortion:
```
r' = r(1 + k₁r² + k₂r⁴ + k₃r⁶)
```

- **Barrel** (k₁ > 0): Edges bow outward, common in wide-angle lenses
- **Pincushion** (k₁ < 0): Edges bow inward, common in telephoto lenses
- Supports up to 3 radial coefficients (r², r⁴, r⁶ terms)
- Newton-Raphson iteration for undistortion
- Coefficient estimation from matched point pairs

### 2. Tangential Distortion (`tangential.rs`)
**Brown-Conrady model** for lens decentering:
```
x' = x + [2p₁xy + p₂(r² + 2x²)]
y' = y + [p₁(r² + 2y²) + 2p₂xy]
```

- **p₁**: Vertical decentering (y-dependent x shift)
- **p₂**: Horizontal decentering (x-dependent y shift)
- Compatible with OpenCV camera calibration conventions
- Newton-Raphson iteration with Jacobian for undistortion

### 3. Field Curvature (`field_curvature.rs`)
**Petzval field curvature** model for curved focal plane:
```
r' = r × (1 + c₁r² + c₂r⁴)
```

- **c₁ > 0**: Outward curvature (magnification increases with radius)
- **c₁ < 0**: Inward curvature (magnification decreases with radius)
- Related to Petzval surface sag: z ≈ r²/(2R)
- `from_petzval_radius()` for creating config from physical parameters
- `sag_at()` for computing defocus distance at any position

### 4. Thin-Plate Spline (`mod.rs`)
**Non-parametric** smooth interpolation:
```
f(x,y) = a₀ + a₁x + a₂y + Σᵢ wᵢ U(||(x,y) - (xᵢ,yᵢ)||)
```

- U(r) = r² log(r) radial basis function
- Minimizes bending energy for smooth interpolation
- LU decomposition for system solution
- Best for complex, non-radial distortions

## API Design

All parametric models follow a consistent API:
- `new(config)` / `identity()` - Construction
- `distort(x, y)` / `apply(x, y)` - Forward transformation
- `undistort(x, y)` / `correct(x, y)` - Inverse transformation
- `estimate(points_a, points_b, center, num_coefficients)` - Calibration
- `rms_error(points_a, points_b)` - Error assessment
- `coefficients()` / `center()` - Getters
- `*_points(&[])` - Batch operations

## Implementation Details

### Newton-Raphson Iteration
All inverse transformations use Newton-Raphson:
- Max iterations: 10-15
- Convergence threshold: 1e-10
- Step size limiting to prevent divergence
- Early exit on convergence

### Coefficient Estimation
Uses least-squares fitting:
- Cholesky decomposition for symmetric positive definite systems
- LU decomposition with partial pivoting for general systems
- Minimum point requirements for stable estimation

## File Structure

```
distortion/
├── mod.rs              # ThinPlateSpline, DistortionMap, TpsConfig
├── radial.rs           # RadialDistortion, RadialDistortionConfig
├── tangential.rs       # TangentialDistortion, TangentialDistortionConfig
├── field_curvature.rs  # FieldCurvature, FieldCurvatureConfig
├── tests.rs            # Integration tests
└── NOTES-AI.md         # This file
```

## Test Coverage

- radial.rs: 20 tests
- tangential.rs: 20 tests
- field_curvature.rs: 24 tests
- tests.rs: Integration tests (TPS)

## Usage in Astrophotography

1. **Camera calibration**: Estimate coefficients from calibration target or star field
2. **Image correction**: Apply undistortion before stacking
3. **Combined correction**: Chain radial + tangential + field curvature for complete model

Typical workflow:
```rust
use lumos::{RadialDistortion, TangentialDistortion, FieldCurvature};

// Apply corrections in order: radial -> tangential -> field curvature
let (x1, y1) = radial.undistort(x, y);
let (x2, y2) = tangential.undistort(x1, y1);
let (x3, y3) = field_curvature.correct(x2, y2);
```

## References

- [Petzval Field Curvature - Wikipedia](https://en.wikipedia.org/wiki/Petzval_field_curvature)
- [Brown-Conrady Distortion Model](https://en.wikipedia.org/wiki/Distortion_(optics))
- [OpenCV Camera Calibration](https://docs.opencv.org/4.x/dc/dbb/tutorial_py_calibration.html)
- [Telescope Optics - Field Curvature](https://www.telescope-optics.net/curvature.htm)
