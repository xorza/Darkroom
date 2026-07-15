# Image Registration

Registration matches two flux-sorted star catalogs, estimates a robust geometric transform,
optionally refines it with a SIP distortion polynomial, and resamples the target image into the
reference frame.

## Public API

Public registration types and functions are exported from the crate root:

```rust
use lumos::{RegistrationConfig, register, warp};

let config = RegistrationConfig::default();
let registration = register(&reference_stars, &target_stars, &config)?;
let warped = warp(
    &target_image,
    &registration.warp_transform(),
    &config.warp,
);
```

`register` returns `Result<RegistrationResult, RegistrationError>`. Configuration validation,
insufficient catalogs, pattern matching failure, RANSAC failure, SIP failure, and the final RMS gate
are typed errors.

`warp` returns `WarpResult { image, coverage }`. Coverage is the fraction of interpolation support
that came from in-bounds source pixels. The stacking pipeline stores and applies it as a per-pixel
weight, excluding extrapolated borders from rejection and combination.

## Registration flow

1. Validate `RegistrationConfig` and derive the RANSAC noise scale from the catalogs' median FWHM.
2. Take the brightest `matching.max_stars` entries from each catalog.
3. Form local k-nearest-neighbor triangles, query the reference triangle invariants with a 2D
   k-d tree, vote for point pairs, and resolve a deterministic one-to-one match set.
4. Estimate the requested transform with progressive sampling, continuous MAGSAC-style scoring,
   plausibility gates, LO-RANSAC refinement, and adaptive termination.
5. For `TransformType::Auto`, try Euclidean, Similarity, and Affine in order; accept the first model
   at or below 0.5 px RMS, otherwise use Homography.
6. Recover additional matches by nearest-neighbor search under the fitted transform and refit until
   the match set stabilizes or five passes complete.
7. If configured, fit a sigma-clipped SIP polynomial to the remaining residuals.
8. Apply `max_rms_error` and return the transform, matches, residuals, fit diagnostics, and timing.

The fitted `Transform` maps reference coordinates to target coordinates. `WarpTransform` combines
that linear mapping with the optional SIP correction and supplies the output-to-input sampling path
used by resampling.

## Composed configuration

`RegistrationConfig` is split by stage:

| Field | Type | Responsibility |
|-------|------|----------------|
| `transform_type` | `TransformType` | Translation through Homography, or `Auto` |
| `matching` | `RegistrationMatchingConfig` | Star-count gates and `TriangleConfig` |
| `ransac` | `RansacConfig` | Iterations, confidence, LO, rotation/scale bounds, seed |
| `max_rms_error` | `f64` | Final registration acceptance gate |
| `sip` | `Option<SipConfig>` | Optional order 2–5 distortion refinement |
| `warp` | `WarpParams` | Interpolation kernel and border value |

Presets are `fast`, `precise`, `wide_field`, `precise_wide_field`, and `mosaic`.

```rust
use lumos::{InterpolationMethod, RegistrationConfig, TransformType};

let mut config = RegistrationConfig::wide_field();
config.transform_type = TransformType::Homography;
config.matching.max_stars = 500;
config.ransac.max_iterations = 5_000;
config.warp.method = InterpolationMethod::Lanczos3 { deringing: 0.3 };
```

## Transform models

| Model | Degrees of freedom | Minimum points | Typical use |
|-------|--------------------|----------------|-------------|
| Translation | 2 | 1 | Dither offsets |
| Euclidean | 3 | 2 | Translation and rotation |
| Similarity | 4 | 2 | Rotation plus uniform scale |
| Affine | 6 | 3 | Shear and differential scale |
| Homography | 8 | 4 | Projective/wide-field mapping |

Euclidean and Similarity use Procrustes fits. Affine uses Hartley-normalized least squares;
Homography uses normalized DLT with SVD.

## Resampling

`resample.rs` owns the public image-level `warp` orchestration and `WarpResult`.
`interpolation/` owns kernels and row execution:

- nearest, bilinear, bicubic, and Lanczos-2/3/4;
- a 4096-sample-per-unit Lanczos LUT;
- incremental coordinate stepping and interior fast paths;
- x86 AVX2/SSE4.1 and aarch64 NEON bilinear paths;
- 128-bit x86 FMA and NEON Lanczos interior kernels with soft-clamp deringing.

Nearest and bilinear have non-negative kernels, so zero-border partial pixels are divided by their
coverage to recover the in-bounds weighted average. Bicubic and Lanczos retain their raw edge value
because negative-lobe kernels require signed in-sampler weight accounting for exact normalization.

## Module layout

| Path | Ownership |
|------|-----------|
| `mod.rs` | Catalog registration and match recovery |
| `config.rs` | Composed public configuration and validation |
| `result.rs` | Public registration result and errors |
| `transform.rs` | Linear transforms and `WarpTransform` |
| `resample.rs` | Public image warp and coverage orchestration |
| `triangle/` | Pattern formation, invariant matching, and voting |
| `spatial/` | Flat implicit 2D k-d tree |
| `ransac/` | Robust estimation and transform solvers |
| `distortion/sip/` | Live polynomial residual correction |
| `distortion/tps/` | Tested implementation reserved for later integration |
| `interpolation/` | Sampling kernels and optimized row warps |

TPS is not called by production registration. It remains intentionally parked for a future
post-RANSAC distortion mode; SIP is the only nonlinear correction in the current pipeline.
