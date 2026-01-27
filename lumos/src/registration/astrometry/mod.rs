//! Astrometric plate solving module.
//!
//! Computes World Coordinate System (WCS) solutions by matching detected stars
//! against astronomical catalogs like Gaia DR3.
//!
//! # Overview
//!
//! Plate solving determines the celestial coordinates corresponding to each pixel
//! in an astronomical image. The solution includes:
//!
//! - Reference pixel and sky coordinates
//! - Pixel scale and rotation (CD matrix)
//! - Optional polynomial distortion (SIP convention)
//!
//! # Algorithm
//!
//! This module implements a quad-based geometric hashing approach similar to
//! Astrometry.net and ASTAP:
//!
//! 1. Form tetrahedron patterns from the brightest stars
//! 2. Compute scale/rotation-invariant hash codes
//! 3. Match against pre-computed catalog hashes
//! 4. Use RANSAC to find the best transformation
//! 5. Refine and output WCS solution
//!
//! # Example
//!
//! ```ignore
//! use lumos::astrometry::{PlateSolver, PlateSolverConfig, CatalogSource};
//!
//! // Configure solver with Gaia catalog
//! let config = PlateSolverConfig {
//!     catalog: CatalogSource::gaia_vizier(),
//!     search_radius: 2.0, // degrees
//!     ..Default::default()
//! };
//!
//! let solver = PlateSolver::new(config);
//!
//! // Solve using detected stars and approximate center
//! let image_stars: Vec<(f64, f64)> = /* from star detection */;
//! let approx_center = (180.0, 45.0); // RA, Dec in degrees
//! let pixel_scale = 1.0; // arcsec/pixel estimate
//!
//! let wcs = solver.solve(&image_stars, approx_center, pixel_scale)?;
//!
//! // Convert pixel to sky coordinates
//! let (ra, dec) = wcs.pixel_to_sky(512.0, 512.0);
//! ```

mod catalog;
mod quad_hash;
mod solver;
mod wcs;

#[cfg(test)]
mod tests;

// Re-export public API
pub use catalog::{CatalogError, CatalogSource, CatalogStar};
pub use quad_hash::{QuadHash, QuadHasher};
pub use solver::{PlateSolution, PlateSolver, PlateSolverConfig, SolveError};
pub use wcs::{PixelSkyMatch, Wcs, WcsBuilder};
