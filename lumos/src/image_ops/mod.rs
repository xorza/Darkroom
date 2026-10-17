//! Run the display/processing ops on the crate's planar [`crate::LinearImage`] — one
//! `Buffer2<f32>` per channel, the same storage stacking and star detection use.
//!
//! - ops with genuine per-channel 2D structure ([`crate::image_ops::denoise`]'s wavelets,
//!   [`crate::image_ops::extract_background`]'s surface fit) take their plane straight from the
//!   image and need no adapter at all;
//! - per-pixel ops reach the samples through the image's own mutators: `planes_mut` for work that
//!   treats every sample alike, `map_rgb` for work that needs a whole pixel at once (SCNR, the
//!   color-preserving stretch), and `remap_intensity` for the display enhancers.
//!
//! Every path to the samples goes through the image's mutators, which drop the metadata's
//! quantization σ and mosaic noise: neither describes the samples an op leaves.
//!
//! The submodules below are the image operations themselves (each an op-named config struct with an
//! in-place `apply`), plus their shared support, [`error`] (the `OpError` contract). The
//! multiscale primitive `denoise` and `hdr` build on is [`crate::math::wavelet`].
//!
//! A convention rather than a trait, deliberately. A trait would turn nine inherent `apply`
//! methods into trait methods, so every downstream call site would need it in scope, to save the
//! one-line prologue and express a composability nothing uses — `lens` drives each op from its own
//! node with its own deserialized config type, and would keep doing so. `NeutralizeBackground`
//! takes no parameters and so has no `validate` to call; that is the contract met, not skipped.
//!
//! The two `ml`-gated ops (`MlDenoise`, `RemoveStars`) hold the `OnnxModel` their caller loaded,
//! so they `apply` through `&mut self`, and report `MlError`: their failures are a model that does
//! not run or an image smaller than one tile, neither of which is a config range that
//! `InvalidConfigField` describes.

pub(crate) mod color_calibration;
pub(crate) mod denoise;
pub(crate) mod error;
pub(crate) mod extract_background;
pub(crate) mod hdr;
pub(crate) mod local_contrast;
#[cfg(feature = "ml")]
pub(crate) mod ml;
pub(crate) mod stretch;

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
