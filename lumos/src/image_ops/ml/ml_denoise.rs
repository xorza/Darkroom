//! Image denoising via a caller-supplied ONNX model (e.g. DeepSNR).
//!
//! Runs the model through the shared `ort` [`backend`](crate::image_ops::ml::backend) (overlapping
//! 512² tiles, feather-blended). A **display-domain** operation: these CNN denoisers are trained on
//! stretched data, so feed the stretched `[0,1]` image — mirroring how `NoiseXTerminator` /
//! GraXpert AI denoise are applied (after the stretch / channel combination).

use common::CancelToken;

use crate::image::linear_image::LinearImage;
use crate::image_ops::error::MlError;
use crate::image_ops::ml::backend::OnnxModel;

/// Denoise a *stretched* (display-domain, `[0, 1]`) image with a caller-supplied ONNX denoiser.
///
/// The learned counterpart to the wavelet [`Denoise`](crate::image_ops::denoise::Denoise).
#[derive(Debug)]
pub struct MlDenoise {
    /// The model the filter runs.
    pub model: OnnxModel,
    /// Tile stride in px (overlap = 512 − stride), in `1..=384`: a wider stride would leave bands
    /// that only the feathered edges of tiles cover.
    pub stride: usize,
}

impl MlDenoise {
    /// Denoise with `model`, at [`OnnxModel::DEFAULT_STRIDE`].
    pub const fn new(model: OnnxModel) -> Self {
        Self {
            model,
            stride: OnnxModel::DEFAULT_STRIDE,
        }
    }

    /// Denoise `image` in place, until `cancel` is set: a cancelled run returns
    /// [`MlError::Cancelled`] and leaves `image` partly denoised.
    pub fn apply(&mut self, image: &mut LinearImage, cancel: &CancelToken) -> Result<(), MlError> {
        self.model.run(image, self.stride, cancel)
    }
}
