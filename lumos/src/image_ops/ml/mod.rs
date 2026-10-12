//! ML-based filters via an ONNX Runtime backend (`ort`), gated behind the `ml` feature.
//!
//! These wrap a pre-trained convolutional network that the **caller supplies** — lumos bundles **no
//! model weights**. The best astro star-removal / denoise models (`StarNet2`, the *`XTerminator`*
//! suite) are proprietary and non-redistributable, so the backend is generic and the user points it
//! at their own legally-obtained `.onnx` file.
//!
//! Currently: [`remove_stars`] (StarNet-style) and [`ml_denoise`] (DeepSNR-style), both on the
//! shared ONNX-Runtime [`backend`]. Each is a config type with an `apply`, matching the in-place op
//! contract the rest of [`crate::image_ops`] follows.

pub(crate) mod backend;
pub(crate) mod ml_denoise;
pub(crate) mod remove_stars;

#[cfg(test)]
mod tests;
