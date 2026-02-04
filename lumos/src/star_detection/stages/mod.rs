//! Pipeline stages for star detection.
//!
//! Each stage is a pure function that transforms data, with all buffer
//! management contained within.

pub mod background;
pub mod detect;
pub mod fwhm;
pub mod prepare;
