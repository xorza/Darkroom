//! Star removal via a StarNet-style ONNX model (caller-supplied weights). See `ml/README.md`.
//!
//! Runs a pre-trained star-removal CNN through `tract` (pure-Rust, CPU) over the image in
//! overlapping 512×512 tiles, feather-blends the outputs, and returns a **starless** image plus the
//! recovered **stars** layer. A **display-domain** operation: StarNet expects stretched data in
//! `[0, 1]`, so run it after the stretch.

use std::path::PathBuf;

use common::Vec2us;
use tract_onnx::prelude::*;

use crate::io::astro_image::{AstroImage, ImageDimensions};

/// StarNet's fixed processing window — the model input is `[1, 512, 512, 3]` (NHWC).
const WINDOW: usize = 512;
/// Feather ramp (px): tiles fade in over this border width so overlaps blend without seams.
const FEATHER_RAMP: f32 = 64.0;
const FEATHER_MIN: f32 = 0.02;

/// Map a tract error into the public error type.
fn model_err(e: TractError) -> StarRemovalError {
    StarRemovalError::Model(e.to_string())
}

/// Parameters for [`remove_stars`].
#[derive(Debug, Clone)]
pub struct StarRemovalConfig {
    /// Path to the caller-supplied ONNX model (e.g. `StarNet2_weights.onnx`). **lumos ships none.**
    pub weights: PathBuf,
    /// Tile stride in px (overlap = `WINDOW − stride`). Default 256 (50% overlap), like StarNet.
    pub stride: usize,
}

impl StarRemovalConfig {
    pub fn new(weights: impl Into<PathBuf>) -> Self {
        Self {
            weights: weights.into(),
            stride: 256,
        }
    }
}

/// Why star removal failed.
#[derive(Debug, thiserror::Error)]
pub enum StarRemovalError {
    #[error("image must be at least {WINDOW}×{WINDOW} for StarNet, got {0}×{1}")]
    TooSmall(usize, usize),
    #[error("ONNX model error: {0}")]
    Model(String),
}

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
    config: &StarRemovalConfig,
) -> Result<StarRemovalResult, StarRemovalError> {
    let (w, h) = (image.width(), image.height());
    if w < WINDOW || h < WINDOW {
        return Err(StarRemovalError::TooSmall(w, h));
    }
    // Pin the batch dim to 1 (H/W/C are already fixed at 512/512/3) so all shapes are static and
    // tract can const-fold the dynamic Resize-size machinery, leaving a plain conv U-Net.
    let plan = tract_onnx::onnx()
        .model_for_path(&config.weights)
        .map_err(model_err)?
        .with_input_fact(0, f32::fact([1, WINDOW, WINDOW, 3]).into())
        .map_err(model_err)?
        .into_optimized()
        .map_err(model_err)?
        .into_runnable()
        .map_err(model_err)?;

    // Feather-weighted accumulation of the per-tile starless estimates (always 3 model channels).
    let mut acc = [vec![0.0f32; w * h], vec![0.0; w * h], vec![0.0; w * h]];
    let mut weight = vec![0.0f32; w * h];

    let xs = tile_starts(w, config.stride);
    let ys = tile_starts(h, config.stride);
    let mut input = vec![0.0f32; WINDOW * WINDOW * 3];
    for &ty in &ys {
        for &tx in &xs {
            fill_tile_input(image, tx, ty, &mut input);
            let tensor = Tensor::from_shape(&[1, WINDOW, WINDOW, 3], &input).map_err(model_err)?;
            let out = plan.run(tvec!(tensor.into())).map_err(model_err)?;
            let view = out[0].to_plain_array_view::<f32>().map_err(model_err)?;
            let tile = view.as_slice().expect("model output is contiguous NHWC");
            accumulate(tile, tx, ty, w, &mut acc, &mut weight);
        }
    }

    let starless = build_starless(image, &acc, &weight);
    let stars = build_stars(image, &starless);
    Ok(StarRemovalResult { starless, stars })
}

/// Tile origins covering `dim` with 512-px windows at `stride`, the last one flush to the edge.
fn tile_starts(dim: usize, stride: usize) -> Vec<usize> {
    let mut v = Vec::new();
    let mut x = 0;
    loop {
        v.push(x.min(dim - WINDOW));
        if x + WINDOW >= dim {
            break;
        }
        x += stride;
    }
    v.dedup();
    v
}

/// Fill the NHWC `[1,512,512,3]` model input from the tile at `(tx, ty)`, clamped to `[0,1]`.
/// Grayscale replicates its single channel to R=G=B.
fn fill_tile_input(image: &AstroImage, tx: usize, ty: usize, input: &mut [f32]) {
    let w = image.width();
    let chans: [&[f32]; 3] = if image.is_rgb() {
        [
            image.channel(0).pixels(),
            image.channel(1).pixels(),
            image.channel(2).pixels(),
        ]
    } else {
        let c = image.channel(0).pixels();
        [c, c, c]
    };
    for hh in 0..WINDOW {
        let row = (ty + hh) * w + tx;
        for ww in 0..WINDOW {
            let src = row + ww;
            let dst = (hh * WINDOW + ww) * 3;
            for c in 0..3 {
                input[dst + c] = chans[c][src].clamp(0.0, 1.0);
            }
        }
    }
}

/// Feather weight in `[FEATHER_MIN, 1]` — ramps up over `FEATHER_RAMP` px from each tile edge.
#[inline]
fn feather(i: usize) -> f32 {
    let d = i.min(WINDOW - 1 - i) as f32;
    (d / FEATHER_RAMP).clamp(FEATHER_MIN, 1.0)
}

fn accumulate(
    out: &[f32],
    tx: usize,
    ty: usize,
    w: usize,
    acc: &mut [Vec<f32>; 3],
    weight: &mut [f32],
) {
    for hh in 0..WINDOW {
        let fy = feather(hh);
        let row = (ty + hh) * w + tx;
        for ww in 0..WINDOW {
            let fw = feather(ww) * fy;
            let idx = row + ww;
            let s = (hh * WINDOW + ww) * 3;
            for c in 0..3 {
                acc[c][idx] += out[s + c] * fw;
            }
            weight[idx] += fw;
        }
    }
}

fn build_starless(image: &AstroImage, acc: &[Vec<f32>; 3], weight: &[f32]) -> AstroImage {
    let (w, h) = (image.width(), image.height());
    if image.is_rgb() {
        let channels: Vec<Vec<f32>> = (0..3)
            .map(|c| {
                acc[c]
                    .iter()
                    .zip(weight)
                    .map(|(&a, &wt)| (a / wt).clamp(0.0, 1.0))
                    .collect()
            })
            .collect();
        AstroImage::from_planar_channels(ImageDimensions::new(Vec2us::new(w, h), 3), channels)
    } else {
        // Average the three model output channels back to grayscale.
        let gray: Vec<f32> = (0..w * h)
            .map(|i| ((acc[0][i] + acc[1][i] + acc[2][i]) / (3.0 * weight[i])).clamp(0.0, 1.0))
            .collect();
        AstroImage::from_planar_channels(ImageDimensions::new(Vec2us::new(w, h), 1), [gray])
    }
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
