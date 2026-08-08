//! Image denoising via a caller-supplied ONNX model (e.g. DeepSNR). See `ml/README.md`.
//!
//! Runs the model through the shared `ort` [`backend`](crate::image_ops::ml::backend) (overlapping 512² tiles,
//! feather-blended). A **display-domain** operation: these CNN denoisers are trained on stretched
//! data, so feed the stretched `[0,1]` image — mirroring how NoiseXTerminator / GraXpert AI denoise
//! are applied (after the stretch / channel combination).

use std::path::PathBuf;

use crate::image_ops::ml::backend::{MlError, TiledOnnxConfig};
use crate::io::image::linear::LinearImage;

/// Denoise a *stretched* (display-domain, `[0, 1]`) image with a caller-supplied ONNX denoiser.
///
/// The learned counterpart to the wavelet [`Denoise`](crate::image_ops::denoise::Denoise).
#[derive(Debug, Clone)]
pub struct MlDenoise {
    /// Backend settings: which model to run and at what tile stride.
    pub onnx: TiledOnnxConfig,
}

impl MlDenoise {
    /// Denoise with the ONNX model at `weights`.
    pub fn new(weights: impl Into<PathBuf>) -> Self {
        Self {
            onnx: TiledOnnxConfig::new(weights),
        }
    }

    /// Tile stride in px; overlap is `WINDOW − stride`.
    pub fn stride(mut self, stride: usize) -> Self {
        self.onnx.stride = stride;
        self
    }

    pub fn apply(&self, image: &mut LinearImage) -> Result<(), MlError> {
        let denoised = self.onnx.run(image)?;
        *image = denoised;
        Ok(())
    }
}
