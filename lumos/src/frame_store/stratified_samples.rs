//! [`StratifiedSamples`]: the pixels normalization pairs between frames that each cover every
//! pixel.

use crate::io::image::cfa::CfaType;
use crate::io::image::cfa::colour_raster::ColourRaster;
use crate::math::size2us::Size2us;

/// Samples a slot's gain fit runs on, at most: a stratified 65 536 of its measured pixels.
pub(crate) const SAMPLE_LIMIT: usize = 65_536;

/// One slot's stratified sample over every pixel of its frames: up to [`SAMPLE_LIMIT`] pixel
/// indices, ascending and evenly spread by rank over the slot's pixels — a channel's every pixel,
/// or the photosites of one colour of a mosaic: the `k`-th of `m` is the one of rank `⌊k·n/m⌋`
/// among the `n`. Drawn by rank within a colour, a sample cannot alias with the mosaic.
///
/// The same for every frame of a set, so a frame written to disk gathers its samples while it is
/// in memory, and normalization reads them without another pass over its planes.
#[derive(Debug, Clone)]
pub(crate) struct StratifiedSamples {
    /// A colour's photosites, or `None` for every pixel.
    colour: Option<ColourRaster>,
    pixels: usize,
    retained: usize,
}

impl StratifiedSamples {
    /// The sample of slot `slot` of an image of `size`: a colour of `mosaic`, or a channel
    /// without one.
    pub(crate) fn new(size: Size2us, mosaic: Option<&CfaType>, slot: usize) -> Self {
        let colour = mosaic.map(|cfa_type| ColourRaster::new(cfa_type, size, slot as u8));
        let pixels = colour
            .as_ref()
            .map_or(size.pixel_count(), ColourRaster::count);
        Self {
            colour,
            pixels,
            retained: pixels.min(SAMPLE_LIMIT),
        }
    }

    /// How many pixels the sample holds.
    pub(crate) const fn len(&self) -> usize {
        self.retained
    }

    /// The sampled pixel indices, ascending.
    pub(crate) fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.retained).map(|k| {
            let rank = k * self.pixels / self.retained;
            self.colour
                .as_ref()
                .map_or(rank, |colour| colour.index(rank))
        })
    }

    /// The values of `plane`, a whole channel, at the sampled pixels.
    pub(crate) fn gather(&self, plane: &[f32]) -> Vec<f32> {
        self.indices().map(|index| plane[index]).collect()
    }
}
