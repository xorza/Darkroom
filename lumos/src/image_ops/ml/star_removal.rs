//! Star removal via a StarNet-style ONNX model (caller-supplied weights). See `ml/README.md`.
//!
//! Runs the model through the shared `ort` [`backend`](crate::image_ops::ml::backend) (overlapping 512² tiles,
//! feather-blended) and recovers the stars layer by **unscreen**. A **display-domain** operation:
//! StarNet expects stretched data in `[0, 1]`, so run it after the stretch.

use std::path::PathBuf;

use crate::image_ops::SAMPLES_PER_BLOCK;
use crate::image_ops::ml::backend::{MlError, TiledOnnxConfig};
use crate::io::image::linear::LinearImage;
use rayon::prelude::*;

/// The two layers from a star-removal pass.
#[derive(Debug)]
pub struct StarRemovalResult {
    /// The image with stars removed.
    pub starless: LinearImage,
    /// The recovered stars layer — `unscreen(original, starless)`.
    pub stars: LinearImage,
}

/// Remove stars from a *stretched* (display-domain, `[0, 1]`) image using a caller-supplied
/// StarNet-style ONNX model.
#[derive(Debug, Clone)]
pub struct RemoveStars {
    /// Backend settings: which model to run and at what tile stride.
    pub onnx: TiledOnnxConfig,
}

impl RemoveStars {
    /// Remove stars with the ONNX model at `weights`.
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

    /// Replace `image` with the starless layer, discarding the stars.
    ///
    /// The ONNX inference dominates either way, but this skips the whole-image unscreen pass
    /// [`split`](Self::split) needs to derive the stars layer.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), MlError> {
        let starless = self.onnx.run(image)?;
        *image = starless;
        Ok(())
    }

    /// Both layers: the starless image and the stars unscreened out of it.
    ///
    /// Takes `image` by value: once `starless` is computed, `image`'s original pixels are no
    /// longer needed, so `build_stars` overwrites `image`'s own buffer in place to become `stars`
    /// instead of allocating a fresh one — one fewer full-image allocation than building `stars`
    /// as a new `Image`. Callers who still need the original pixels afterward should `.clone()`
    /// before calling.
    pub fn split(&self, mut image: LinearImage) -> Result<StarRemovalResult, MlError> {
        let starless = self.onnx.run(&image)?;
        build_stars(&mut image, &starless);
        Ok(StarRemovalResult {
            starless,
            stars: image,
        })
    }
}

/// `stars = unscreen(original, starless)` — the screen-blend inverse `1 − (1−orig)/(1−starless)`,
/// the layer that screens back over the starless image to reconstruct the original. In place:
/// overwrites `image`'s own buffer with the result (each output sample only ever depends on the
/// *same-index* input samples, so reading then immediately overwriting is safe). The formula is
/// per-sample and channel-agnostic, so it runs plane by plane with no regard to which channel is
/// which.
fn build_stars(image: &mut LinearImage, starless: &LinearImage) {
    for (channel, plane) in image.planes_mut().enumerate() {
        let sless = starless.channel(channel).pixels();
        plane
            .pixels_mut()
            .par_chunks_mut(SAMPLES_PER_BLOCK)
            .zip(sless.par_chunks(SAMPLES_PER_BLOCK))
            .for_each(|(out, s)| {
                for (out, &s) in out.iter_mut().zip(s) {
                    let o = *out;
                    *out = (1.0 - (1.0 - o) / (1.0 - s).max(1e-4)).clamp(0.0, 1.0);
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use crate::image_ops::internals::{gray_image, rgb_image};
    use crate::image_ops::ml::star_removal::*;
    use crate::math::size2us::Size2us;

    #[test]
    fn build_stars_unscreens_per_sample_in_place() {
        // unscreen(o, s) = 1 - (1-o)/(1-s):
        // (0.75, 0.5) -> 1 - 0.25/0.5 = 0.5
        // (1.0, 0.0)  -> 1 - 0.0/1.0  = 1.0
        // (0.5, 1.0)  -> denominator floors at 1e-4 -> huge negative -> clamps to 0.0
        let mut orig = gray_image(Size2us::new(3, 1), vec![0.75, 1.0, 0.5]);
        let dimensions = orig.dimensions();
        let starless = gray_image(Size2us::new(3, 1), vec![0.5, 0.0, 1.0]);
        build_stars(&mut orig, &starless);
        assert_eq!(orig.channel(0).pixels(), &[0.5, 1.0, 0.0]);
        assert_eq!(orig.dimensions(), dimensions);
    }

    #[test]
    fn build_stars_is_channel_agnostic_over_rgb() {
        // Same (o, s) pairs as above, spread one per channel of a single RGB pixel instead of
        // three mono pixels — pins that the unscreen is per-sample, with no channel meaning.
        let mut orig = rgb_image(Size2us::new(1, 1), vec![0.75], vec![1.0], vec![0.5]);
        let starless = rgb_image(Size2us::new(1, 1), vec![0.5], vec![0.0], vec![1.0]);
        build_stars(&mut orig, &starless);
        assert_eq!(orig.channel(0).pixels(), &[0.5]);
        assert_eq!(orig.channel(1).pixels(), &[1.0]);
        assert_eq!(orig.channel(2).pixels(), &[0.0]);
    }

    #[test]
    fn build_stars_handles_a_block_boundary() {
        // Exercise the `SAMPLES_PER_BLOCK`-chunked parallel path across more than one chunk
        // (2.5 blocks of L samples) with a uniform (o, s) pair everywhere.
        let n = SAMPLES_PER_BLOCK * 2 + SAMPLES_PER_BLOCK / 2;
        let mut orig = gray_image(Size2us::new(n, 1), vec![0.75; n]);
        let starless = gray_image(Size2us::new(n, 1), vec![0.5; n]);
        build_stars(&mut orig, &starless);
        assert!(
            orig.channel(0)
                .pixels()
                .iter()
                .all(|&v| (v - 0.5).abs() < 1e-6)
        );
    }
}
