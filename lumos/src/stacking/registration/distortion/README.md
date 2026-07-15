# Registration Distortion Models

The distortion layer contains one live registration correction and one reserved implementation.

## SIP

`sip/` implements the FITS Simple Imaging Polynomial convention for residual distortion after the
linear transform:

```text
u' = u + Σ A[p,q] u^p v^q
v' = v + Σ B[p,q] u^p v^q       where 2 ≤ p+q ≤ order
```

`RegistrationConfig::sip: Option<SipConfig>` enables fitting. The fit uses normalized coordinates,
Cholesky with LU fallback, and iterative MAD sigma clipping. `RegistrationResult::sip_fit` stores
the polynomial and fit diagnostics, and `RegistrationResult::warp_transform()` includes it in image
resampling.

The reference point defaults to the match centroid or can be set explicitly for CRPIX-compatible
coefficients. Polynomial order is 2–5; radial barrel/pincushion residuals require at least order 3.

## Thin-plate spline

`tps/` implements and tests non-parametric thin-plate-spline interpolation and distortion maps. It
has no production caller and is intentionally retained for a planned post-RANSAC distortion mode.
SIP is the only nonlinear correction selected by the current registration pipeline.

## Layout

```text
distortion/
├── mod.rs
├── sip/
│   ├── mod.rs
│   └── tests.rs
└── tps/
    ├── mod.rs
    └── tests.rs
```

The shared `SINGULAR_THRESHOLD` is used by both solvers.
