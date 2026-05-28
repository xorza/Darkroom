pub(crate) mod cache;
pub(crate) mod cache_config;
pub(crate) mod config;
pub(crate) mod error;
pub mod progress;
pub mod rejection;
pub(crate) mod stack;

use strum_macros::Display;

/// Type of calibration frame being stacked.
///
/// Used for logging and error messages. Does not affect stacking algorithm
/// behavior — stacking method, rejection, and normalization are controlled
/// by [`StackConfig`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Display)]
#[strum(serialize_all = "lowercase")]
pub enum FrameType {
    /// Dark frames - thermal noise calibration
    Dark,
    /// Flat frames - vignetting and dust correction
    Flat,
    /// Bias frames - readout noise calibration
    Bias,
    /// Light frames - actual image data
    Light,
}

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;
