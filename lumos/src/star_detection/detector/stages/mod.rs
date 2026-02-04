//! Pipeline stages for star detection.
//!
//! Each stage is a pure function that transforms data, with all buffer
//! management contained within.

pub(crate) mod background;
pub(crate) mod detect;
pub(crate) mod filter;
pub(crate) mod fwhm;
pub(crate) mod measure;
pub(crate) mod prepare;
