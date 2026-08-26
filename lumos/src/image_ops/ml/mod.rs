//! ML-based filters via an ONNX Runtime backend (`ort`), gated behind the `ml` feature.
//!
//! These wrap a pre-trained convolutional network that the **caller supplies** — lumos bundles **no
//! model weights**. The best astro star-removal / denoise models (StarNet2, the *XTerminator*
//! suite) are proprietary and non-redistributable, so the backend is generic and the user points it
//! at their own legally-obtained `.onnx` file (see `ml/README.md` for the licensing rationale and
//! the StarNet2 I/O contract).
//!
//! Currently: [`star_removal`] (StarNet-style) and [`denoise`] (DeepSNR-style), both on the shared
//! ONNX-Runtime [`backend`]. Each is a config type with an `apply`, matching the in-place op
//! contract the rest of [`crate::image_ops`] follows.

pub(crate) mod backend;
pub(crate) mod denoise;
pub(crate) mod star_removal;

#[cfg(test)]
mod tests;
