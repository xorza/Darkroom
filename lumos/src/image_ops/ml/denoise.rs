//! Image denoising via a caller-supplied ONNX model (e.g. DeepSNR). See `ml/README.md`.
//!
//! Runs the model through the shared `ort` [`backend`](crate::image_ops::ml::backend) (overlapping 512² tiles,
//! feather-blended). A **display-domain** operation: these CNN denoisers are trained on stretched
//! data, so feed the stretched `[0,1]` image — mirroring how NoiseXTerminator / GraXpert AI denoise
//! are applied (after the stretch / channel combination).

use crate::image_ops::ml::backend::{MlError, TiledOnnxConfig, run_tiled};
use imaginarium::Image;

/// Denoise a *stretched* (display-domain, `[0, 1]`) image with a caller-supplied ONNX denoiser.
pub fn ml_denoise(image: &Image, config: &TiledOnnxConfig) -> Result<Image, MlError> {
    run_tiled(image, config)
}
