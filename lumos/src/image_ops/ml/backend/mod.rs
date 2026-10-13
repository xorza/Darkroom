//! Shared `ort` (ONNX Runtime) backend for the display-domain ML filters. Loads a caller-supplied
//! `512×512×3` NHWC model and runs it over an image in overlapping, feather-blended 512×512 tiles.
//! Used by [`star_removal`](crate::image_ops::ml::remove_stars) and
//! [`denoise`](crate::image_ops::ml::ml_denoise).
//!
//! The tile loop is **sequential** and lets ONNX Runtime use its default (all-core) intra-op
//! threading. These nets are memory-bandwidth-bound (~125 MB of weights streamed per tile), so
//! running tiles concurrently (one `Session` per worker) was measured slower *and* exhausted RAM.
//! ~60 s for a full 24 MP frame on a 10-core machine.

use std::path::PathBuf;

use ort::session::Session;
use ort::value::TensorRef;

use imaginarium::Buffer2;

use crate::config_value::ConfigValue;
use crate::error::InvalidConfigField;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use crate::image_ops::error::MlError;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// The fixed model processing window — these nets take a `[1, 512, 512, 3]` (NHWC) input.
pub(crate) const WINDOW: usize = 512;
/// Feather ramp (px): tiles fade in over this border width so overlaps blend without seams.
const FEATHER_RAMP: usize = 64;
const FEATHER_MIN: f32 = 0.02;
/// The widest stride at which every pixel lies at full feather weight in some tile: a tile's
/// full-weight core spans `WINDOW − 2·FEATHER_RAMP` pixels, and the next one must start before it
/// ends. Wider, a band between cores takes its value only from two tiles' ramps, within
/// `FEATHER_RAMP` of their window edges, where the model sees least context.
const MAX_STRIDE: usize = WINDOW - 2 * FEATHER_RAMP;

/// Where the ONNX model is and how finely to tile. lumos ships **no model** — the caller supplies a
/// legally-obtained `.onnx`.
#[derive(Debug, Clone)]
pub struct TiledOnnxConfig {
    /// Path to the caller-supplied ONNX model.
    pub weights: PathBuf,
    /// Tile stride in px (overlap = `WINDOW − stride`), in `1..=384`: a wider stride would leave
    /// bands that only the feathered edges of tiles cover. Default 256 (50% overlap).
    pub stride: usize,
}

impl TiledOnnxConfig {
    pub fn new(weights: impl Into<PathBuf>) -> Self {
        Self {
            weights: weights.into(),
            stride: 256,
        }
    }

    /// A stride that covers the frame at full weight: at least one pixel, and at most
    /// [`MAX_STRIDE`].
    fn validate(&self) -> Result<(), InvalidConfigField> {
        ConfigValue::count("ML tile stride", self.stride)
            .must_be("between 1 and 384", (1..=MAX_STRIDE).contains(&self.stride))
    }
}

impl TiledOnnxConfig {
    /// Run the model over `image` in 512² tiles (NHWC `[0,1]` in, NHWC out), feather-blending the
    /// overlaps. Returns a new image with the same channel count as the input (grayscale is
    /// replicated to RGB for the model, then averaged back). Expects a display-domain `[0, 1]`
    /// master.
    ///
    /// Unlike the in-place ops this returns a fresh image rather than mutating: the model's output
    /// is a separate buffer from its input, and the tile loop reads the input long after it has
    /// begun writing the output.
    pub(crate) fn run(&self, image: &LinearImage) -> Result<LinearImage, MlError> {
        self.validate()?;
        let size = Size2us::new(image.width(), image.height());
        if size.width < WINDOW || size.height < WINDOW {
            return Err(MlError::TooSmall(size));
        }
        let mut session = Session::builder()?.commit_from_file(&self.weights)?;

        let mut blend = FeatherBlend::new(image.dimensions());
        // Reused across every tile: `TensorRef::from_array_view` borrows this buffer for the
        // duration of `session.run` instead of taking ownership, so one allocation serves the
        // whole tile loop instead of one per tile (up to ~345 for a full 24 MP frame).
        let mut input = vec![0.0f32; WINDOW * WINDOW * 3];

        let xs = tile_starts(size.width, self.stride);
        let ys = tile_starts(size.height, self.stride);
        for &ty in &ys {
            for &tx in &xs {
                fill_tile_input(image, Vec2us::new(tx, ty), &mut input);
                let tensor =
                    TensorRef::from_array_view(([1usize, WINDOW, WINDOW, 3], input.as_slice()))?;
                let outputs = session.run(ort::inputs![tensor])?;
                let (_shape, tile) = outputs[0].try_extract_tensor::<f32>()?;
                if tile.len() != WINDOW * WINDOW * 3 {
                    return Err(MlError::OutputLength { len: tile.len() });
                }
                blend.add(tile, Vec2us::new(tx, ty));
            }
        }
        Ok(blend.into_image())
    }
}

/// Tile origins covering `dim` with 512-px windows at `stride`, the last one flush to the edge. The
/// loop ends at the first window that reaches the edge, so only that one is clamped and no origin
/// repeats.
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
    v
}

/// Fill the reused NHWC `[1,512,512,3]` `input` buffer from the tile at `(tx, ty)`, clamped to
/// `[0,1]`. Grayscale replicates its single channel to R=G=B. `input` is the caller's
/// tile-loop-lifetime scratch buffer (see [`TiledOnnxConfig::run`]), overwritten in full every
/// call.
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
/// precomputed at compile time: it depends only on the distance from the tile edge, not on the
/// tile, so [`FeatherBlend::add`] reads it per axis rather than ramping and clamping per pixel.
const FEATHER_LUT: [f32; WINDOW] = {
    let mut lut = [0.0f32; WINDOW];
    let mut i = 0;
    while i < WINDOW {
        let d = if i < WINDOW - 1 - i {
            i
        } else {
            WINDOW - 1 - i
        } as f32;
        lut[i] = (d / FEATHER_RAMP as f32).clamp(FEATHER_MIN, 1.0);
        i += 1;
    }
    lut
};

#[inline]
const fn feather(i: usize) -> f32 {
    FEATHER_LUT[i]
}

/// The model's tiles over a frame, each feathered by its weight: the weighted sum of each output
/// channel and the sum of the weights, normalized into an image once every tile is in.
#[derive(Debug)]
struct FeatherBlend {
    dimensions: ImageDimensions,
    channels: [Vec<f32>; 3],
    weight: Vec<f32>,
}

impl FeatherBlend {
    /// Nothing yet over a frame of `dimensions`, whose channel count the image it makes takes.
    fn new(dimensions: ImageDimensions) -> Self {
        let pixels = dimensions.pixel_count();
        Self {
            dimensions,
            channels: [vec![0.0f32; pixels], vec![0.0; pixels], vec![0.0; pixels]],
            weight: vec![0.0f32; pixels],
        }
    }

    /// Add `out`, the model's NHWC output for the tile at `tile`.
    fn add(&mut self, out: &[f32], tile: Vec2us) {
        let width = self.dimensions.width();
        for hh in 0..WINDOW {
            let fy = feather(hh);
            let row = (tile.y + hh) * width + tile.x;
            for ww in 0..WINDOW {
                let fw = feather(ww) * fy;
                let idx = row + ww;
                let s = (hh * WINDOW + ww) * 3;
                for c in 0..3 {
                    self.channels[c][idx] += out[s + c] * fw;
                }
                self.weight[idx] += fw;
            }
        }
    }

    /// The normalized blend, with the frame's channel count: a grayscale frame takes the mean of
    /// the model's three channels.
    fn into_image(self) -> LinearImage {
        let Self {
            dimensions,
            channels,
            weight,
        } = self;
        let size = dimensions.size();
        if dimensions.is_rgb() {
            LinearImage::from(channels.map(|channel| {
                let px = channel
                    .iter()
                    .zip(&weight)
                    .map(|(&a, &wt)| (a / wt).clamp(0.0, 1.0))
                    .collect();
                Buffer2::new(size.width, size.height, px)
            }))
        } else {
            let gray: Vec<f32> = (0..size.pixel_count())
                .map(|i| {
                    ((channels[0][i] + channels[1][i] + channels[2][i]) / (3.0 * weight[i]))
                        .clamp(0.0, 1.0)
                })
                .collect();
            LinearImage::from(Buffer2::new(size.width, size.height, gray))
        }
    }
}

#[cfg(test)]
mod tests;
