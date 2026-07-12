//! Star removal via a StarNet-style ONNX model (caller-supplied weights). See `ml/README.md`.
//!
//! Runs the model through the shared `ort` [`backend`](super::backend) (overlapping 512² tiles,
//! feather-blended) and recovers the stars layer by **unscreen**. A **display-domain** operation:
//! StarNet expects stretched data in `[0, 1]`, so run it after the stretch.

use crate::image_ops::PIXELS_PER_BLOCK;
use crate::image_ops::ml::backend::{MlError, TiledOnnxConfig, run_tiled};
use imaginarium::Image;
use rayon::prelude::*;

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
/// the layer that screens back over the starless image to reconstruct the original. The formula
/// is per-sample and channel-agnostic, so it runs directly (parallel, chunked) over each image's
/// interleaved f32 samples — no deinterleave-to-planes/reinterleave round trip needed.
fn build_stars(image: &Image, starless: &Image) -> Image {
    let orig: &[f32] = bytemuck::cast_slice(image.bytes());
    let sless: &[f32] = bytemuck::cast_slice(starless.bytes());
    let mut stars = vec![0.0f32; orig.len()];
    stars
        .par_chunks_mut(PIXELS_PER_BLOCK)
        .zip(orig.par_chunks(PIXELS_PER_BLOCK))
        .zip(sless.par_chunks(PIXELS_PER_BLOCK))
        .for_each(|((out, o), s)| {
            for (out, (&o, &s)) in out.iter_mut().zip(o.iter().zip(s)) {
                *out = (1.0 - (1.0 - o) / (1.0 - s).max(1e-4)).clamp(0.0, 1.0);
            }
        });
    Image::new_with_data(image.desc, bytemuck::cast_slice(&stars).to_vec())
        .expect("stars image matches the source descriptor")
}

#[cfg(test)]
mod tests {
    use super::*;
    use imaginarium::{ColorFormat, ImageDesc};

    fn l_f32(width: usize, height: usize, samples: Vec<f32>) -> Image {
        Image::new_with_data(
            ImageDesc::new(width, height, ColorFormat::L_F32),
            bytemuck::cast_slice(&samples).to_vec(),
        )
        .unwrap()
    }

    fn rgb_f32(width: usize, height: usize, samples: Vec<f32>) -> Image {
        Image::new_with_data(
            ImageDesc::new(width, height, ColorFormat::RGB_F32),
            bytemuck::cast_slice(&samples).to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn build_stars_unscreens_per_sample() {
        // unscreen(o, s) = 1 - (1-o)/(1-s):
        // (0.75, 0.5) -> 1 - 0.25/0.5 = 0.5
        // (1.0, 0.0)  -> 1 - 0.0/1.0  = 1.0
        // (0.5, 1.0)  -> denominator floors at 1e-4 -> huge negative -> clamps to 0.0
        let orig = l_f32(3, 1, vec![0.75, 1.0, 0.5]);
        let starless = l_f32(3, 1, vec![0.5, 0.0, 1.0]);
        let stars = build_stars(&orig, &starless);
        let out: &[f32] = bytemuck::cast_slice(stars.bytes());
        assert_eq!(out, &[0.5, 1.0, 0.0]);
        assert_eq!(stars.desc, orig.desc);
    }

    #[test]
    fn build_stars_is_channel_agnostic_over_rgb() {
        // Same (o, s) pairs as above, packed as one RGB pixel instead of three L pixels —
        // pins that treating the interleaved buffer as flat samples (no per-channel
        // deinterleave) gives the same per-channel result.
        let orig = rgb_f32(1, 1, vec![0.75, 1.0, 0.5]);
        let starless = rgb_f32(1, 1, vec![0.5, 0.0, 1.0]);
        let stars = build_stars(&orig, &starless);
        let out: &[f32] = bytemuck::cast_slice(stars.bytes());
        assert_eq!(out, &[0.5, 1.0, 0.0]);
    }

    #[test]
    fn build_stars_handles_a_block_boundary() {
        // Exercise the `PIXELS_PER_BLOCK`-chunked parallel path across more than one chunk
        // (2.5 blocks of L samples) with a uniform (o, s) pair everywhere.
        let n = PIXELS_PER_BLOCK * 2 + PIXELS_PER_BLOCK / 2;
        let orig = l_f32(n, 1, vec![0.75; n]);
        let starless = l_f32(n, 1, vec![0.5; n]);
        let stars = build_stars(&orig, &starless);
        let out: &[f32] = bytemuck::cast_slice(stars.bytes());
        assert!(out.iter().all(|&v| (v - 0.5).abs() < 1e-6));
    }
}
