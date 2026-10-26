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

use common::fs;
use imaginarium::PlanarDesc;
use ort::session::Session;
use ort::value::TensorRef;

use crate::config_field::ConfigField;
use crate::error::InvalidConfigField;
use crate::image::linear_image::LinearImage;
use crate::image_ops::error::MlError;
use common::Vec2us;

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
        let session = Session::builder()?.commit_from_memory(&bytes)?;
        Ok(Self { session })
    }

    /// The model over `image` in 512² tiles at `stride`, feather-blended, written into `image`'s
    /// planes: see [`run_tiles`].
    pub(crate) fn run(&mut self, image: &mut LinearImage, stride: usize) -> Result<(), MlError> {
        run_tiles(Planes::InPlace(image), stride, |input, output| {
            self.infer(input, output)
        })
    }

    /// [`Self::run`] over `input`, written into `output`'s planes, of `input`'s dimensions.
    pub(crate) fn run_into(
        &mut self,
        input: &LinearImage,
        output: &mut LinearImage,
        stride: usize,
    ) -> Result<(), MlError> {
        debug_assert_eq!(output.desc(), input.desc());
        run_tiles(Planes::Apart { input, output }, stride, |input, output| {
            self.infer(input, output)
        })
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

/// Where a run reads its tiles and writes its blend: one image's planes, read and then
/// overwritten, or another image's planes, of the input's dimensions.
#[derive(Debug)]
enum Planes<'a> {
    InPlace(&'a mut LinearImage),
    Apart {
        input: &'a LinearImage,
        output: &'a mut LinearImage,
    },
}

impl Planes<'_> {
    const fn input(&self) -> &LinearImage {
        match self {
            Self::InPlace(image) => image,
            Self::Apart { input, .. } => input,
        }
    }

    const fn output(&mut self) -> &mut LinearImage {
        match self {
            Self::InPlace(image) => image,
            Self::Apart { output, .. } => output,
        }
    }
}

/// Run `infer` over `planes`' input in 512² tiles at `stride` (NHWC `[0,1]` in, NHWC out),
/// feather-blending the overlaps, and write the blend into its output's planes: their metadata
/// and flags stay, as every other op keeps them. Mono is replicated to RGB for the model,
/// then averaged back. Expects a display-domain `[0, 1]` master.
///
/// The tiles run a row of them at a time, and every row above the next tile row's start is then
/// final: no later tile adds to it or reads it. So the blend gathers in a band of [`WINDOW`] rows
/// and writes each final row as the tile row ends, which an image read in place allows too. One
/// input and one output buffer serve every tile.
///
/// # Errors
/// [`MlError::Config`] for a stride that does not cover the frame at full weight, refused before
/// any tile runs; [`MlError::TooSmall`] for an image smaller than a tile; and what `infer` returns.
fn run_tiles(
    mut planes: Planes<'_>,
    stride: usize,
    mut infer: impl FnMut(&[f32], &mut [f32]) -> Result<(), MlError>,
) -> Result<(), MlError> {
    validate_stride(stride)?;
    let desc = planes.input().desc();
    let size = desc.size();
    if size.width < WINDOW || size.height < WINDOW {
        return Err(MlError::TooSmall(size));
    }
    let mut blend = FeatherBlend::new(desc);
    let mut input = vec![0.0f32; WINDOW * WINDOW * 3];
    let mut output = vec![0.0f32; WINDOW * WINDOW * 3];
    let xs = tile_starts(size.width, stride);
    let ys = tile_starts(size.height, stride);
    for (row, &ty) in ys.iter().enumerate() {
        for &tx in &xs {
            let tile = Vec2us::new(tx, ty);
            fill_tile_input(planes.input(), tile, &mut input);
            infer(&input, &mut output)?;
            blend.add(&output, tile);
        }
        let final_rows = ys.get(row + 1).copied().unwrap_or(size.height);
        blend.write_rows(planes.output(), final_rows);
    }
    Ok(())
}

/// A stride that covers the frame at full weight: at least one pixel, and at most [`MAX_STRIDE`].
fn validate_stride(stride: usize) -> Result<(), InvalidConfigField> {
    ConfigField::count("ML tile stride", stride)
        .must_be("between 1 and 384", (1..=MAX_STRIDE).contains(&stride))
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
/// `[0,1]`. Mono replicates its single channel to R=G=B. `input` is the caller's
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

/// The model's tiles over a band of [`WINDOW`] rows of a frame, each feathered by its weight: the
/// weighted sum of each output channel and the sum of the weights, row `y` of the frame in band
/// row `y mod WINDOW`. The rows a tile row spans are `WINDOW` consecutive ones, so none of them
/// shares a band row, and a row is normalized into the frame's planes and cleared once final.
#[derive(Debug)]
struct FeatherBlend {
    desc: PlanarDesc,
    channels: [Vec<f32>; 3],
    weight: Vec<f32>,
    /// The frame rows written so far: every row above this one.
    written: usize,
}

impl FeatherBlend {
    /// Nothing yet over a frame of `desc`, whose channel count the image it makes takes.
    fn new(desc: PlanarDesc) -> Self {
        let band = WINDOW * desc.width();
        Self {
            desc,
            channels: [vec![0.0f32; band], vec![0.0; band], vec![0.0; band]],
            weight: vec![0.0f32; band],
            written: 0,
        }
    }

    /// Add `out`, the model's NHWC output for the tile at `tile`.
    fn add(&mut self, out: &[f32], tile: Vec2us) {
        let width = self.desc.width();
        for hh in 0..WINDOW {
            let fy = feather(hh);
            let row = (tile.y + hh) % WINDOW * width + tile.x;
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

    /// Write the normalized blend of every row above `end` not written yet over `image`'s rows,
    /// of the frame it was made for, and clear their band rows: a mono frame takes the mean
    /// of the model's three channels.
    fn write_rows(&mut self, image: &mut LinearImage, end: usize) {
        let Self {
            desc,
            channels,
            weight,
            written,
        } = self;
        debug_assert_eq!(image.desc(), *desc);
        let width = desc.width();
        for y in *written..end {
            let band = y % WINDOW * width..(y % WINDOW + 1) * width;
            let weight = &mut weight[band.clone()];
            if desc.is_rgb() {
                for (c, channel) in channels.iter_mut().enumerate() {
                    let sums = &mut channel[band.clone()];
                    let row = image.channel_mut(c).row_mut(y);
                    for ((out, &sum), &wt) in row.iter_mut().zip(&*sums).zip(&*weight) {
                        *out = (sum / wt).clamp(0.0, 1.0);
                    }
                    sums.fill(0.0);
                }
            } else {
                let [red, green, blue] = channels
                    .each_mut()
                    .map(|channel| &mut channel[band.clone()]);
                let row = image.channel_mut(0).row_mut(y);
                for (i, out) in row.iter_mut().enumerate() {
                    *out = ((red[i] + green[i] + blue[i]) / (3.0 * weight[i])).clamp(0.0, 1.0);
                }
                for sums in [red, green, blue] {
                    sums.fill(0.0);
                }
            }
            weight.fill(0.0);
        }
        *written = end;
    }
}

#[cfg(test)]
mod tests;
