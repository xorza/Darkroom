//! Demosaicing module for CFA (Color Filter Array) sensors.
//!
//! This module provides demosaicing algorithms for different sensor types:
//! - Bayer CFA patterns (RGGB, BGGR, GRBG, GBRG)
//!
//! Re-exports the main types and functions from the bayer submodule.

mod bayer;

pub use bayer::{BayerImage, CfaPattern, demosaic_bilinear};

#[cfg(feature = "bench")]
pub use bayer::bench;
