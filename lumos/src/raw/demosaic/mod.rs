//! Demosaicing module for CFA (Color Filter Array) sensors.
//!
//! This module provides demosaicing algorithms for different sensor types:
//! - Bayer CFA patterns (RGGB, BGGR, GRBG, GBRG)
//! - X-Trans 6x6 patterns (Fujifilm sensors)
//!
//! Re-exports the main types and functions from the submodules.

pub(crate) mod bayer;
pub(crate) mod xtrans;

pub use bayer::CfaPattern;
pub(crate) use bayer::{BayerImage, demosaic_bayer};
