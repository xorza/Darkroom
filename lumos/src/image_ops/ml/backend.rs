//! Shared `ort` (ONNX Runtime) backend for the display-domain ML filters. Loads a caller-supplied
//! `512×512×3` NHWC model and runs it over an image in overlapping, feather-blended 512×512 tiles.
//! Used by [`star_removal`](crate::image_ops::ml::star_removal) and
//! [`denoise`](crate::image_ops::ml::denoise).
//!
//! The tile loop is **sequential** and lets ONNX Runtime use its default (all-core) intra-op
//! threading. These nets are memory-bandwidth-bound (~125 MB of weights streamed per tile), so
//! running tiles concurrently (one `Session` per worker) was measured slower *and* exhausted RAM —
//! see `ml/README.md`. ~60 s for a full 24 MP frame on a 10-core machine.

use std::path::PathBuf;

use ort::session::Session;
use ort::value::TensorRef;

use imaginarium::Buffer2;

use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// The fixed model processing window — these nets take a `[1, 512, 512, 3]` (NHWC) input.
const WINDOW: usize = 512;
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
    #[error("image must be at least {WINDOW}×{WINDOW}, got {}×{}", .0.width, .0.height)]
    TooSmall(Size2us),
    #[error("ONNX model error: {0}")]
    Model(String),
}

fn model_err(e: ort::Error) -> MlError {
    MlError::Model(e.to_string())
}

/// Run the model over `image` in 512² tiles (NHWC `[0,1]` in, NHWC out), feather-blending the
/// overlaps. Returns a new image with the same channel count as the input (grayscale is replicated
/// to RGB for the model, then averaged back). Expects a display-domain `[0, 1]` master.
///
/// Unlike the in-place ops this returns a fresh image rather than mutating: the model's output is a
/// separate buffer from its input, and the tile loop reads the input long after it has begun
/// writing the output.
pub(crate) fn run_tiled(
    image: &LinearImage,
    config: &TiledOnnxConfig,
) -> Result<LinearImage, MlError> {
    let planar = image;
    let size = Size2us::new(image.width(), image.height());
    if size.width < WINDOW || size.height < WINDOW {
        return Err(MlError::TooSmall(size));
    }
    assert!(config.stride > 0, "TiledOnnxConfig.stride must be > 0");
    let mut session = Session::builder()
        .map_err(model_err)?
        .commit_from_file(&config.weights)
        .map_err(model_err)?;

    let pixels = size.pixel_count();
    let mut acc = [vec![0.0f32; pixels], vec![0.0; pixels], vec![0.0; pixels]];
    let mut weight = vec![0.0f32; pixels];
    // Reused across every tile: `TensorRef::from_array_view` borrows this buffer for the
    // duration of `session.run` instead of taking ownership, so one allocation serves the
    // whole tile loop instead of one per tile (up to ~345 for a full 24 MP frame).
    let mut input = vec![0.0f32; WINDOW * WINDOW * 3];

    let xs = tile_starts(size.width, config.stride);
    let ys = tile_starts(size.height, config.stride);
    for &ty in &ys {
        for &tx in &xs {
            fill_tile_input(planar, Vec2us::new(tx, ty), &mut input);
            let tensor =
                TensorRef::from_array_view(([1usize, WINDOW, WINDOW, 3], input.as_slice()))
                    .map_err(model_err)?;
            let outputs = session.run(ort::inputs![tensor]).map_err(model_err)?;
            let (_shape, tile) = outputs[0].try_extract_tensor::<f32>().map_err(model_err)?;
            let expected = WINDOW * WINDOW * 3;
            if tile.len() != expected {
                return Err(MlError::Model(format!(
                    "model output has {} values, expected {expected} (NHWC [1,{WINDOW},{WINDOW},3])",
                    tile.len()
                )));
            }
            accumulate(tile, Vec2us::new(tx, ty), size.width, &mut acc, &mut weight);
        }
    }
    Ok(build_output(planar.is_rgb(), &acc, &weight, size))
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

/// Fill the reused NHWC `[1,512,512,3]` `input` buffer from the tile at `(tx, ty)`, clamped to
/// `[0,1]`. Grayscale replicates its single channel to R=G=B. `input` is the caller's
/// tile-loop-lifetime scratch buffer (see `run_tiled`), overwritten in full every call.
fn fill_tile_input(planar: &LinearImage, tile: Vec2us, input: &mut [f32]) {
    let w = planar.width();
    let chans: [&[f32]; 3] = if planar.is_rgb() {
        [
            planar.channel(0).pixels(),
            planar.channel(1).pixels(),
            planar.channel(2).pixels(),
        ]
    } else {
        let c = planar.channel(0).pixels();
        [c, c, c]
    };
    for hh in 0..WINDOW {
        let row = (tile.y + hh) * w + tile.x;
        for ww in 0..WINDOW {
            let src = row + ww;
            let dst = (hh * WINDOW + ww) * 3;
            for c in 0..3 {
                input[dst + c] = chans[c][src].clamp(0.0, 1.0);
            }
        }
    }
}

/// Per-axis feather weight in `[FEATHER_MIN, 1]` for each of the `WINDOW` tile positions,
/// precomputed once at compile time — it only depends on distance from the tile edge, not on
/// which tile, so recomputing the ramp/clamp per pixel of every tile (as `accumulate` does,
/// once per axis) was pure waste.
const FEATHER_LUT: [f32; WINDOW] = {
    let mut lut = [0.0f32; WINDOW];
    let mut i = 0;
    while i < WINDOW {
        let d = if i < WINDOW - 1 - i {
            i
        } else {
            WINDOW - 1 - i
        } as f32;
        lut[i] = (d / FEATHER_RAMP).clamp(FEATHER_MIN, 1.0);
        i += 1;
    }
    lut
};

#[inline]
const fn feather(i: usize) -> f32 {
    FEATHER_LUT[i]
}

fn accumulate(out: &[f32], tile: Vec2us, w: usize, acc: &mut [Vec<f32>; 3], weight: &mut [f32]) {
    for hh in 0..WINDOW {
        let fy = feather(hh);
        let row = (tile.y + hh) * w + tile.x;
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

/// Normalize the feather-weighted accumulation into an image matching the input's channels.
fn build_output(rgb: bool, acc: &[Vec<f32>; 3], weight: &[f32], size: Size2us) -> LinearImage {
    if rgb {
        LinearImage::from(std::array::from_fn::<_, 3, _>(|c| {
            let px = acc[c]
                .iter()
                .zip(weight)
                .map(|(&a, &wt)| (a / wt).clamp(0.0, 1.0))
                .collect();
            Buffer2::new(size.width, size.height, px)
        }))
    } else {
        // Average the three model output channels back to grayscale.
        let gray: Vec<f32> = (0..size.pixel_count())
            .map(|i| ((acc[0][i] + acc[1][i] + acc[2][i]) / (3.0 * weight[i])).clamp(0.0, 1.0))
            .collect();
        LinearImage::from(Buffer2::new(size.width, size.height, gray))
    }
}

#[cfg(test)]
mod tests {
    use crate::image_ops::ml::backend::*;

    /// Plane of `width × height` whose value at `(x, y)` is `base + x + y * 1000`, so a wrong tile
    /// origin or a swapped channel produces a distinctly wrong number rather than a near miss.
    fn ramp(width: usize, height: usize, base: f32) -> Buffer2<f32> {
        let pixels = (0..width * height)
            .map(|i| base + (i % width) as f32 + (i / width) as f32 * 1000.0)
            .collect();
        Buffer2::new(width, height, pixels)
    }

    #[test]
    fn a_tile_reads_from_its_own_origin_with_each_channel_in_its_own_model_slot() {
        // 640² so the tile origin is not forced to (0,0) and an off-by-one in the row stride shows.
        let (side, tile) = (640usize, Vec2us::new(128, 96));
        let planar = LinearImage::from(std::array::from_fn::<_, 3, _>(|c| {
            ramp(side, side, c as f32 * 0.5)
        }));
        let mut input = vec![0.0f32; WINDOW * WINDOW * 3];
        fill_tile_input(&planar, tile, &mut input);

        // Model pixel (ww, hh) must be master pixel (tile.x + ww, tile.y + hh). Every value here
        // exceeds 1.0 and so arrives clamped — which is the contract, the model wants [0,1].
        for (ww, hh) in [(0usize, 0usize), (1, 0), (0, 1), (WINDOW - 1, WINDOW - 1)] {
            let dst = (hh * WINDOW + ww) * 3;
            let src = (tile.y + hh) * side + tile.x + ww;
            for c in 0..3 {
                let expected = planar.channel(c).pixels()[src].clamp(0.0, 1.0);
                assert_eq!(
                    input[dst + c],
                    expected,
                    "channel {c} at model ({ww}, {hh})"
                );
            }
        }
    }

    #[test]
    fn a_mono_master_replicates_into_all_three_model_channels() {
        // The net is RGB-only, so a grayscale master must arrive as R=G=B rather than leaving two
        // channels at zero — and the clamp is what keeps a sub-background or star-core sample in
        // the [0,1] domain the model was trained on.
        let side = WINDOW;
        let plane = Buffer2::new(
            side,
            side,
            (0..side * side)
                .map(|i| match i {
                    0 => -0.25,
                    1 => 1.5,
                    _ => 0.5,
                })
                .collect(),
        );
        let planar = LinearImage::from(plane);
        let mut input = vec![0.0f32; WINDOW * WINDOW * 3];
        fill_tile_input(&planar, Vec2us::new(0, 0), &mut input);

        for (pixel, expected) in [(0usize, 0.0f32), (1, 1.0), (2, 0.5), (side * side - 1, 0.5)] {
            assert_eq!(
                &input[pixel * 3..pixel * 3 + 3],
                &[expected; 3],
                "pixel {pixel}"
            );
        }
    }
}
