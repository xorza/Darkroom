//! Shared `ort` (ONNX Runtime) backend for the display-domain ML filters. Loads a caller-supplied
//! `512×512×3` NHWC model and runs it over an image in overlapping, feather-blended 512×512 tiles.
//! Used by [`star_removal`](super::star_removal) and [`denoise`](super::denoise).

use std::path::PathBuf;

use common::Vec2us;
use ort::session::Session;
use ort::value::Tensor;

use crate::io::astro_image::{AstroImage, ImageDimensions};

/// The fixed model processing window — these nets take a `[1, 512, 512, 3]` (NHWC) input.
pub(crate) const WINDOW: usize = 512;
/// Feather ramp (px): tiles fade in over this border width so overlaps blend without seams.
const FEATHER_RAMP: f32 = 64.0;
const FEATHER_MIN: f32 = 0.02;

/// Where the ONNX model is and how finely to tile. lumos ships **no model** — the caller supplies a
/// legally-obtained `.onnx` (see `ml/README.md`).
#[derive(Debug, Clone)]
pub struct TiledOnnxConfig {
    /// Path to the caller-supplied ONNX model.
    pub weights: PathBuf,
    /// Tile stride in px (overlap = `WINDOW − stride`). Default 256 (50% overlap).
    pub stride: usize,
}

impl TiledOnnxConfig {
    pub fn new(weights: impl Into<PathBuf>) -> Self {
        Self {
            weights: weights.into(),
            stride: 256,
        }
    }
}

/// Why an ML filter failed.
#[derive(Debug, thiserror::Error)]
pub enum MlError {
    #[error("image must be at least {WINDOW}×{WINDOW}, got {0}×{1}")]
    TooSmall(usize, usize),
    #[error("ONNX model error: {0}")]
    Model(String),
}

fn model_err(e: ort::Error) -> MlError {
    MlError::Model(e.to_string())
}

/// Run the model over `image` in 512² tiles (NHWC `[0,1]` in, NHWC out), feather-blending the
/// overlaps. Returns the output as an `AstroImage` with the same channel count as the input
/// (grayscale is replicated to RGB for the model, then averaged back). Expects display-domain data.
pub(crate) fn run_tiled(
    image: &AstroImage,
    config: &TiledOnnxConfig,
) -> Result<AstroImage, MlError> {
    let (w, h) = (image.width(), image.height());
    if w < WINDOW || h < WINDOW {
        return Err(MlError::TooSmall(w, h));
    }
    let mut session = Session::builder()
        .map_err(model_err)?
        .commit_from_file(&config.weights)
        .map_err(model_err)?;

    let mut acc = [vec![0.0f32; w * h], vec![0.0; w * h], vec![0.0; w * h]];
    let mut weight = vec![0.0f32; w * h];

    let xs = tile_starts(w, config.stride);
    let ys = tile_starts(h, config.stride);
    let mut input = vec![0.0f32; WINDOW * WINDOW * 3];
    for &ty in &ys {
        for &tx in &xs {
            fill_tile_input(image, tx, ty, &mut input);
            let tensor = Tensor::from_array(([1usize, WINDOW, WINDOW, 3], input.clone()))
                .map_err(model_err)?;
            let outputs = session.run(ort::inputs![tensor]).map_err(model_err)?;
            let (_shape, tile) = outputs[0].try_extract_tensor::<f32>().map_err(model_err)?;
            accumulate(tile, tx, ty, w, &mut acc, &mut weight);
        }
    }
    Ok(build_output(image, &acc, &weight))
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

/// Normalize the feather-weighted accumulation into an `AstroImage` matching the input's channels.
fn build_output(image: &AstroImage, acc: &[Vec<f32>; 3], weight: &[f32]) -> AstroImage {
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
