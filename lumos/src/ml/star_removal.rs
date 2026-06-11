//! Star removal via a StarNet-style ONNX model (caller-supplied weights). See `ml/README.md`.
//!
//! Runs the model through the shared `ort` [`backend`](super::backend) (overlapping 512² tiles,
//! feather-blended) and recovers the stars layer by **unscreen**. A **display-domain** operation:
//! StarNet expects stretched data in `[0, 1]`, so run it after the stretch.

use common::Vec2us;

use crate::io::astro_image::{AstroImage, ImageDimensions};
use crate::ml::backend::{MlError, TiledOnnxConfig, run_tiled};

/// The two layers from a star-removal pass.
#[derive(Debug)]
pub struct StarRemovalResult {
    /// The image with stars removed.
    pub starless: AstroImage,
    /// The recovered stars layer — `unscreen(original, starless)`.
    pub stars: AstroImage,
}

/// Remove stars from a *stretched* (display-domain, `[0, 1]`) image using a caller-supplied
/// StarNet-style ONNX model. Returns the starless image and the stars layer.
pub fn remove_stars(
    image: &AstroImage,
    config: &TiledOnnxConfig,
) -> Result<StarRemovalResult, MlError> {
    let starless = run_tiled(image, config)?;
    let stars = build_stars(image, &starless);
    Ok(StarRemovalResult { starless, stars })
}

/// `stars = unscreen(original, starless)` — the screen-blend inverse `1 − (1−orig)/(1−starless)`,
/// the layer that screens back over the starless image to reconstruct the original.
fn build_stars(image: &AstroImage, starless: &AstroImage) -> AstroImage {
    let (w, h) = (image.width(), image.height());
    let n = image.channels();
    let channels: Vec<Vec<f32>> = (0..n)
        .map(|c| {
            image
                .channel(c)
                .pixels()
                .iter()
                .zip(starless.channel(c).pixels())
                .map(|(&o, &s)| (1.0 - (1.0 - o) / (1.0 - s).max(1e-4)).clamp(0.0, 1.0))
                .collect()
        })
        .collect();
    AstroImage::from_planar_channels(ImageDimensions::new(Vec2us::new(w, h), n), channels)
}
