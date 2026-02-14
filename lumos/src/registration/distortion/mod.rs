//! Distortion modeling for optical corrections.
//!
//! This module provides SIP polynomial distortion correction (FITS WCS standard)
//! and non-parametric thin-plate spline (TPS) interpolation for correcting
//! optical distortions in astronomical images.
//!
//! ## SIP Polynomial (Parametric)
//!
//! Simple Imaging Polynomial used in the FITS WCS standard:
//!
//! ```text
//! u' = u + Σ A_pq × u^p × v^q  (for 2 ≤ p+q ≤ order)
//! v' = v + Σ B_pq × u^p × v^q
//! ```
//!
//! Used in the registration pipeline after RANSAC to refine transformation
//! accuracy. Compatible with Astrometry.net, Siril, ASTAP.
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

mod sip;
mod tps;

pub use sip::{SipConfig, SipFitResult, SipPolynomial};
pub(crate) use tps::tps_kernel;
pub use tps::{DistortionMap, ThinPlateSpline, TpsConfig};
