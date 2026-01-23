//! Demosaicing module for CFA (Color Filter Array) sensors.
//!
//! This module provides demosaicing algorithms for different sensor types:
//! - Bayer CFA patterns (RGGB, BGGR, GRBG, GBRG)
//! - X-Trans 6x6 patterns (Fujifilm sensors)
//!
//! Re-exports the main types and functions from the submodules.

mod bayer;
pub(crate) mod xtrans;

pub use bayer::{BayerImage, CfaPattern, demosaic_bilinear};

#[cfg(feature = "bench")]
pub mod bench {
    pub use super::bayer::bench as bayer;
    pub use super::xtrans::bench as xtrans;
}
