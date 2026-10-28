//! Star removal via a StarNet-style ONNX model (caller-supplied weights).
//!
//! Runs the model through the shared `ort` [`backend`](crate::image_ops::ml::backend) (overlapping
//! 512² tiles, feather-blended) and recovers the stars layer by **unscreen**. A **display-domain**
//! operation: StarNet expects stretched data in `[0, 1]`, so run it after the stretch.

use common::{CancelToken, par};
use imaginarium::PlanarImage;
use rayon::prelude::*;

use crate::image::SAMPLES_PER_BLOCK;
use crate::image::linear_image::LinearImage;
use crate::image_ops::error::MlError;
use crate::image_ops::ml::backend::OnnxModel;

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
#[derive(Debug)]
pub struct RemoveStars {
    /// The model the filter runs.
    pub model: OnnxModel,
    /// Tile stride in px (overlap = 512 − stride), in `1..=384`: a wider stride would leave bands
    /// that only the feathered edges of tiles cover.
    pub stride: usize,
}

impl RemoveStars {
    /// Remove stars with `model`, at [`OnnxModel::DEFAULT_STRIDE`].
    pub const fn new(model: OnnxModel) -> Self {
        Self {
            model,
            stride: OnnxModel::DEFAULT_STRIDE,
        }
    }

    /// Replace `image` with the starless layer, discarding the stars.
    ///
    /// The ONNX inference dominates either way, but this skips the whole-image unscreen pass
    /// [`split`](Self::split) needs to derive the stars layer. A run cancelled through `cancel`
    /// returns [`MlError::Cancelled`] and leaves `image` partly starless.
    pub fn apply(&mut self, image: &mut LinearImage, cancel: &CancelToken) -> Result<(), MlError> {
        self.model.run(image, self.stride, cancel)
    }

    /// Both layers: the starless image and the stars unscreened out of it.
    ///
    /// Takes `image` by value: the model reads it into new starless planes, and once those are
    /// done its original pixels are no longer needed, so `build_stars` overwrites `image`'s own
    /// buffer in place to become `stars` — a copy of the input and a fresh stars image both
    /// spared. Callers who still need the original pixels afterward should `.clone()` before
    /// calling. Both layers keep `image`'s metadata, flags and flat gain. A run cancelled through
    /// `cancel` returns [`MlError::Cancelled`].
    pub fn split(
        &mut self,
        mut image: LinearImage,
        cancel: &CancelToken,
    ) -> Result<StarRemovalResult, MlError> {
        let desc = image.desc();
        let mut starless = LinearImage::new(
            PlanarImage::from_planar_channels(
                desc,
                (0..desc.channel_count()).map(|_| vec![0.0; desc.pixel_count()]),
            ),
            image.flags().cloned(),
            image.metadata.clone(),
        );
        starless.set_flat_gain(image.flat_gain().cloned());
        self.model
            .run_into(&image, &mut starless, self.stride, cancel)?;
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
        par::chunks_mut(plane.pixels_mut(), SAMPLES_PER_BLOCK)
            .zip(par::chunks(sless, SAMPLES_PER_BLOCK))
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
    use crate::image_ops::ml::remove_stars::*;
    use crate::internals::images::{mono_image, rgb_image};
    use common::USize2;

    #[test]
    fn build_stars_unscreens_per_sample_in_place() {
        // unscreen(o, s) = 1 - (1-o)/(1-s):
        // (0.75, 0.5) -> 1 - 0.25/0.5 = 0.5
        // (1.0, 0.0)  -> 1 - 0.0/1.0  = 1.0
        // (0.5, 1.0)  -> denominator floors at 1e-4 -> huge negative -> clamps to 0.0
        let mut orig = mono_image(USize2::new(3, 1), vec![0.75, 1.0, 0.5]);
        let desc = orig.desc();
        let starless = mono_image(USize2::new(3, 1), vec![0.5, 0.0, 1.0]);
        build_stars(&mut orig, &starless);
        assert_eq!(orig.channel(0).pixels(), &[0.5, 1.0, 0.0]);
        assert_eq!(orig.desc(), desc);
    }

    #[test]
    fn build_stars_is_channel_agnostic_over_rgb() {
        // Same (o, s) pairs as above, spread one per channel of a single RGB pixel instead of
        // three mono pixels — pins that the unscreen is per-sample, with no channel meaning.
        let mut orig = rgb_image(USize2::new(1, 1), [vec![0.75], vec![1.0], vec![0.5]]);
        let starless = rgb_image(USize2::new(1, 1), [vec![0.5], vec![0.0], vec![1.0]]);
        build_stars(&mut orig, &starless);
        assert_eq!(orig.channel(0).pixels(), &[0.5]);
        assert_eq!(orig.channel(1).pixels(), &[1.0]);
        assert_eq!(orig.channel(2).pixels(), &[0.0]);
    }

    /// Across 2.5 `SAMPLES_PER_BLOCK` chunks every sample unscreens its own pair. With
    /// `s = (i mod 7)/8` and `o = 1 − (1 − s)·t`, `t = (i mod 5)/8`, the result is `1 − t` exactly
    /// — all dyadic — while a chunk of `orig` read against another chunk of `starless` would pair
    /// an `o` with the wrong `s`.
    #[test]
    fn build_stars_handles_a_block_boundary() {
        let n = SAMPLES_PER_BLOCK * 2 + SAMPLES_PER_BLOCK / 2;
        let s = |i: usize| (i % 7) as f32 / 8.0;
        let t = |i: usize| (i % 5) as f32 / 8.0;
        let mut orig = mono_image(
            USize2::new(n, 1),
            (0..n).map(|i| 1.0 - (1.0 - s(i)) * t(i)).collect(),
        );
        let starless = mono_image(USize2::new(n, 1), (0..n).map(s).collect());
        build_stars(&mut orig, &starless);
        for (i, &v) in orig.channel(0).pixels().iter().enumerate() {
            assert_eq!(v, 1.0 - t(i), "sample {i}");
        }
    }
}
