//! [`StratifiedSamples`]: the pixels normalization pairs between frames that each cover every
//! pixel.

use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::color_raster::ColorRaster;
use crate::image::cfa::mosaic::Mosaic;
use crate::math::size2us::Size2us;

/// Samples a slot's gain fit runs on, at most: a stratified 65 536 of its measured pixels.
pub(crate) const SAMPLE_LIMIT: usize = 65_536;

/// One slot's stratified sample over every pixel of its frames: up to [`SAMPLE_LIMIT`] pixel
/// indices, ascending and evenly spread by rank over the slot's pixels — a channel's every pixel,
/// or the photosites of one color of a mosaic: the `k`-th of `m` is the one of rank `⌊k·n/m⌋`
/// among the `n`. Drawn by rank within a color, a sample cannot alias with the mosaic.
///
/// The same for every frame of a set, so a frame written to disk gathers its samples while it is
/// in memory, and normalization reads them without another pass over its planes.
#[derive(Debug, Clone)]
pub(crate) struct StratifiedSamples {
    /// A color's photosites, or `None` for every pixel.
    color: Option<ColorRaster>,
    pixels: usize,
    retained: usize,
}

impl StratifiedSamples {
    /// The sample of slot `slot` of an image of `size`: a color of `mosaic`, or a channel
    /// without one.
    pub(crate) fn new(size: Size2us, mosaic: Option<Mosaic>, slot: usize) -> Self {
        let color = mosaic.map(|mosaic| ColorRaster::new(&CfaType::from(mosaic), size, slot));
        let pixels = color
            .as_ref()
            .map_or_else(|| size.pixel_count(), ColorRaster::count);
        Self {
            color,
            pixels,
            retained: pixels.min(SAMPLE_LIMIT),
        }
    }

    /// How many pixels the sample holds.
    pub(crate) const fn len(&self) -> usize {
        self.retained
    }

    /// The sampled pixel indices, ascending.
    pub(crate) fn idxs(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.retained).map(|k| {
            let rank = k * self.pixels / self.retained;
            self.color.as_ref().map_or(rank, |color| color.idx(rank))
        })
    }

    /// The values of `plane`, a whole channel, at the sampled pixels.
    pub(crate) fn gather(&self, plane: &[f32]) -> Vec<f32> {
        self.idxs().map(|idx| plane[idx]).collect()
    }
}
