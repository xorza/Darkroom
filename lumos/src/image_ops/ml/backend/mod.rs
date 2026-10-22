//! Shared `ort` (ONNX Runtime) backend for the display-domain ML filters: a caller-loaded
//! `512×512×3` NHWC [`OnnxModel`], run over an image in overlapping, feather-blended 512×512 tiles.
//! Used by [`remove_stars`](crate::image_ops::ml::remove_stars) and
//! [`ml_denoise`](crate::image_ops::ml::ml_denoise).
//!
//! The tile loop is **sequential** and lets ONNX Runtime use its default (all-core) intra-op
//! threading. These nets are memory-bandwidth-bound (~125 MB of weights streamed per tile), so
//! running tiles concurrently (one `Session` per worker) was measured slower *and* exhausted RAM.
//! ~60 s for a full 24 MP frame on a 10-core machine.

use std::path::Path;
use std::sync::Mutex;

use common::fs;
use ort::environment::Environment;
use ort::session::Session;
use ort::value::TensorRef;

use crate::config_field::ConfigField;
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

/// A loaded ONNX model of the shape these filters run: one NHWC `[1, 512, 512, 3]` tile in `[0, 1]`
/// in, one out. lumos ships **no model**: the caller loads a legally-obtained `.onnx`, once, at its
/// own edge, and runs it over as many images as it likes.
#[derive(Debug)]
pub struct OnnxModel {
    session: Session,
}

impl OnnxModel {
    /// The tile stride a filter runs at unless told otherwise: 50% overlap.
    pub const DEFAULT_STRIDE: usize = 256;

    /// The model at `path`, its weights read now: about 125 MB for the nets these filters run.
    ///
    /// # Errors
    /// [`MlError::Read`] when the file cannot be read; [`MlError::Model`] when ONNX Runtime cannot
    /// start or parse the model.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, MlError> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|source| MlError::Read {
            path: path.to_owned(),
            source,
        })?;
        let session = Session::builder(&environment()?)?.commit_from_memory(&bytes)?;
        Ok(Self { session })
    }

    /// The model over `image` in 512² tiles at `stride`, feather-blended, written into `image`'s
    /// planes: see [`run_tiles`].
    pub(crate) fn run(&mut self, image: &mut LinearImage, stride: usize) -> Result<(), MlError> {
        run_tiles(image, stride, |input, output| self.infer(input, output))
    }

    /// The model's output tile for the `input` tile, written into `output`.
    fn infer(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), MlError> {
        let tensor = TensorRef::from_array_view(([1usize, WINDOW, WINDOW, 3], input))?;
        let outputs = self.session.run(ort::inputs![tensor])?;
        let (_shape, tile) = outputs[0].try_extract_tensor::<f32>()?;
        if tile.len() != output.len() {
            return Err(MlError::OutputLen { len: tile.len() });
        }
        output.copy_from_slice(tile);
        Ok(())
    }
}

/// Run `infer` over `image` in 512² tiles at `stride` (NHWC `[0,1]` in, NHWC out), feather-blending
/// the overlaps, and write the blend into `image`'s planes: its metadata and flags stay, as every
/// other op keeps them. Grayscale is replicated to RGB for the model, then averaged back. Expects a
/// display-domain `[0, 1]` master.
///
/// The tile loop reads the input until its last tile, so the blend gathers in buffers of its own
/// and reaches the planes once every tile is in. One input and one output buffer serve every tile.
///
/// # Errors
/// [`MlError::Config`] for a stride that does not cover the frame at full weight, refused before
/// any tile runs; [`MlError::TooSmall`] for an image smaller than a tile; and what `infer` returns.
fn run_tiles(
    image: &mut LinearImage,
    stride: usize,
    mut infer: impl FnMut(&[f32], &mut [f32]) -> Result<(), MlError>,
) -> Result<(), MlError> {
    validate_stride(stride)?;
    let size = Size2us::new(image.width(), image.height());
    if size.width < WINDOW || size.height < WINDOW {
        return Err(MlError::TooSmall(size));
    }
    let mut blend = FeatherBlend::new(image.dimensions());
    let mut input = vec![0.0f32; WINDOW * WINDOW * 3];
    let mut output = vec![0.0f32; WINDOW * WINDOW * 3];
    let xs = tile_starts(size.width, stride);
    let ys = tile_starts(size.height, stride);
    for &ty in &ys {
        for &tx in &xs {
            let tile = Vec2us::new(tx, ty);
            fill_tile_input(image, tile, &mut input);
            infer(&input, &mut output)?;
            blend.add(&output, tile);
        }
    }
    blend.write_into(image);
    Ok(())
}

/// A stride that covers the frame at full weight: at least one pixel, and at most [`MAX_STRIDE`].
fn validate_stride(stride: usize) -> Result<(), InvalidConfigField> {
    ConfigField::count("ML tile stride", stride)
        .must_be("between 1 and 384", (1..=MAX_STRIDE).contains(&stride))
}

/// The process's ONNX Runtime environment, built on first use. ONNX Runtime allows one environment
/// per process, created once even after a release, so it is never dropped.
fn environment() -> Result<Environment, ort::Error> {
    static ENVIRONMENT: Mutex<Option<Environment>> = Mutex::new(None);
    let mut environment = ENVIRONMENT.lock().unwrap();
    if let Some(environment) = environment.as_ref() {
        return Ok(environment.clone());
    }
    let built = ort::init().build()?;
    *environment = Some(built.clone());
    Ok(built)
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
/// tile-loop-lifetime scratch buffer (see [`run_tiles`]), overwritten in full every call.
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
/// channel and the sum of the weights, normalized into the frame's planes once every tile is in.
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

    /// Write the normalized blend over `image`'s planes, of the frame it was made for: a
    /// grayscale frame takes the mean of the model's three channels.
    fn write_into(self, image: &mut LinearImage) {
        let Self {
            dimensions,
            channels,
            weight,
        } = self;
        debug_assert_eq!(image.dimensions(), dimensions);
        if dimensions.is_rgb() {
            for (plane, channel) in image.planes_mut().zip(&channels) {
                for ((out, &sum), &wt) in plane.pixels_mut().iter_mut().zip(channel).zip(&weight) {
                    *out = (sum / wt).clamp(0.0, 1.0);
                }
            }
        } else {
            let plane = image.channel_mut(0).pixels_mut();
            for (i, out) in plane.iter_mut().enumerate() {
                *out = ((channels[0][i] + channels[1][i] + channels[2][i]) / (3.0 * weight[i]))
                    .clamp(0.0, 1.0);
            }
        }
    }
}

#[cfg(test)]
mod tests;
