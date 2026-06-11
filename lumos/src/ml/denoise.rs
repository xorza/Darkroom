//! Image denoising via a caller-supplied ONNX model (e.g. DeepSNR). See `ml/README.md`.
//!
//! Runs the model through the shared `ort` [`backend`](super::backend) (overlapping 512² tiles,
//! feather-blended). A **display-domain** operation: these CNN denoisers are trained on stretched
//! data, so feed the stretched `[0,1]` image — mirroring how NoiseXTerminator / GraXpert AI denoise
//! are applied (after the stretch / channel combination).

use crate::io::astro_image::AstroImage;
use crate::ml::backend::{MlError, TiledOnnxConfig, run_tiled};

/// Denoise a *stretched* (display-domain, `[0, 1]`) image with a caller-supplied ONNX denoiser.
pub fn ml_denoise(image: &AstroImage, config: &TiledOnnxConfig) -> Result<AstroImage, MlError> {
    run_tiled(image, config)
}
