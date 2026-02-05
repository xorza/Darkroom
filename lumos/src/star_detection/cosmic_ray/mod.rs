//! Cosmic ray detection using L.A.Cosmic algorithm.
//!
//! Implementation based on van Dokkum 2001, PASP 113, 1420:
//! "Cosmic-Ray Rejection by Laplacian Edge Detection"
//!
//! The key insight is that cosmic rays have very sharp edges compared to
//! astronomical sources, which are smoothed by the PSF. The Laplacian
//! (second derivative) responds strongly to sharp edges.
//!
//! This module provides `compute_laplacian_snr` for per-star cosmic ray
//! classification during star detection.

mod laplacian;

pub use laplacian::compute_laplacian_snr;
