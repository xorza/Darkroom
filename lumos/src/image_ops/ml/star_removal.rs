//! Star removal via a StarNet-style ONNX model (caller-supplied weights). See `ml/README.md`.
//!
//! Runs the model through the shared `ort` [`backend`](super::backend) (overlapping 512² tiles,
//! feather-blended) and recovers the stars layer by **unscreen**. A **display-domain** operation:
//! StarNet expects stretched data in `[0, 1]`, so run it after the stretch.

use crate::image_ops::ml::backend::{MlError, TiledOnnxConfig, run_tiled};
use crate::image_ops::{deinterleave_f32, interleave_f32};
use imaginarium::{Buffer2, Image};

/// The two layers from a star-removal pass.
#[derive(Debug)]
pub struct StarRemovalResult {
    /// The image with stars removed.
    pub starless: Image,
    /// The recovered stars layer — `unscreen(original, starless)`.
    pub stars: Image,
}

/// Remove stars from a *stretched* (display-domain, `[0, 1]`) image using a caller-supplied
/// StarNet-style ONNX model. Returns the starless image and the stars layer.
pub fn remove_stars(image: &Image, config: &TiledOnnxConfig) -> Result<StarRemovalResult, MlError> {
    let starless = remove_stars_starless_only(image, config)?;
    let stars = build_stars(image, &starless);
    Ok(StarRemovalResult { starless, stars })
}

/// Like [`remove_stars`], but skips the `stars` unscreen derivation entirely.
/// Use when only the starless image is needed — the ONNX inference is the
/// expensive part regardless, but this still saves the whole-image unscreen
/// pass over every pixel.
pub fn remove_stars_starless_only(
    image: &Image,
    config: &TiledOnnxConfig,
) -> Result<Image, MlError> {
    run_tiled(image, config)
}

/// `stars = unscreen(original, starless)` — the screen-blend inverse `1 − (1−orig)/(1−starless)`,
/// the layer that screens back over the starless image to reconstruct the original.
fn build_stars(image: &Image, starless: &Image) -> Image {
    let (w, h) = (image.desc.width, image.desc.height);
    let planes: Vec<Buffer2<f32>> = deinterleave_f32(image)
        .iter()
        .zip(deinterleave_f32(starless).iter())
        .map(|(orig, starless)| {
            let px = orig
                .pixels()
                .iter()
                .zip(starless.pixels())
                .map(|(&o, &s)| (1.0 - (1.0 - o) / (1.0 - s).max(1e-4)).clamp(0.0, 1.0))
                .collect();
            Buffer2::new(w, h, px)
        })
        .collect();
    interleave_f32(planes)
}
