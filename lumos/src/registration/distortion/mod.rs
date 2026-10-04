//! Distortion modeling for optical corrections.
//!
//! This module provides a polynomial distortion correction in the form of SIP, and non-parametric
//! thin-plate spline (TPS) interpolation for correcting optical distortions in astronomical images.
//!
//! ## SIP Polynomial (Parametric)
//!
//! The polynomial form of the Simple Imaging Polynomial convention:
//!
//! ```text
//! u' = u + Σ A_pq × u^p × v^q  (for 2 ≤ p+q ≤ order)
//! v' = v + Σ B_pq × u^p × v^q
//! ```
//!
//! Used in the registration pipeline after RANSAC to refine transformation accuracy. It maps the
//! reference frame onto a target frame, not pixels onto the sky, and holds its coefficients in
//! coordinates normalized about the matched stars' centroid, with no inverse (`AP`/`BP`): it is no
//! FITS WCS SIP header, and none is written.
//!
//! ## Thin-Plate Spline (Non-Parametric)
//!
//! Smooth RBF interpolation that minimizes "bending energy":
//!
//! ```text
//! f(x,y) = a₀ + a₁x + a₂y + Σᵢ wᵢ U(||(x,y) - (xᵢ,yᵢ)||)
//! ```
//!
//! where U(r) = r² log(r). Use when distortion is non-radial or non-uniform.

pub(crate) mod sip;
mod tps;

/// Pivot magnitude below which a matrix is considered singular in LU/Cholesky solvers.
const SINGULAR_THRESHOLD: f64 = 1e-12;
