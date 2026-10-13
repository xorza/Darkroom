//! [`MosaicSampling`]: the photosites of a mosaic a noise statistic samples.

use arrayvec::ArrayVec;

use crate::image::cfa::cfa_type::CfaType;
use crate::image::flat_gain::FlatGain;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// The farthest a same-colour neighbour can be in a row: the X-Trans period.
const MAX_REACH: usize = 6;

/// The photosites of a `size` mosaic of `cfa_type` a statistic samples: those of the rows
/// [`CfaType::sampled_rows`] takes for about `per_colour` samples a colour, less the pixels
/// `excluded` names.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MosaicSampling<'a, E> {
    pub(crate) size: Size2us,
    pub(crate) cfa_type: &'a CfaType,
    pub(crate) excluded: E,
    pub(crate) per_colour: usize,
}

/// A photosite and the nearest same-colour photosite to its right.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pair {
    x: usize,
    neighbour: usize,
    y: usize,
    pub(crate) colour: usize,
}

impl Pair {
    /// The photosite's sample less its neighbour's, in the row-major `mosaic` sampled.
    pub(crate) fn difference(self, mosaic: &[f32], size: Size2us) -> f32 {
        let row = &mosaic[self.y * size.width..(self.y + 1) * size.width];
        row[self.x] - row[self.neighbour]
    }

    /// The flat `gain` of the pair: the mean of its two photosites' gains.
    pub(crate) fn gain(self, gain: &FlatGain) -> f32 {
        let y = self.y as f32;
        f32::midpoint(
            gain.at(self.colour, self.x as f32, y),
            gain.at(self.colour, self.neighbour as f32, y),
        )
    }
}

impl<E: Fn(usize) -> bool> MosaicSampling<'_, E> {
    /// The index and colour of every sampled photosite.
    pub(crate) fn for_each_photosite(&self, mut visit: impl FnMut(usize, usize)) {
        let Size2us { width, .. } = self.size;
        for y in self.cfa_type.sampled_rows(self.size, self.per_colour) {
            for x in 0..width {
                let index = y * width + x;
                if !(self.excluded)(index) {
                    visit(index, self.cfa_type.color_at(Vec2us::new(x, y)));
                }
            }
        }
    }

    /// How many photosites of each colour [`Self::for_each_photosite`] visits.
    pub(crate) fn photosite_counts(&self) -> [usize; 3] {
        let mut counts = [0; 3];
        self.for_each_photosite(|_, colour| counts[colour] += 1);
        counts
    }

    /// Every pair of a sampled photosite and the nearest same-colour photosite to its right in its
    /// row; a pair touching an excluded pixel is skipped.
    pub(crate) fn for_each_pair(&self, mut visit: impl FnMut(Pair)) {
        let Size2us { width, .. } = self.size;
        for y in self.cfa_type.sampled_rows(self.size, self.per_colour) {
            for x in 0..width {
                let colour = self.cfa_type.color_at(Vec2us::new(x, y));
                let Some(neighbour) = (x + 1..(x + 1 + MAX_REACH).min(width))
                    .find(|&nx| self.cfa_type.color_at(Vec2us::new(nx, y)) == colour)
                else {
                    continue;
                };
                if (self.excluded)(y * width + x) || (self.excluded)(y * width + neighbour) {
                    continue;
                }
                visit(Pair {
                    x,
                    neighbour,
                    y,
                    colour,
                });
            }
        }
    }

    /// How many pairs of each colour [`Self::for_each_pair`] visits.
    pub(crate) fn pair_counts(&self) -> ArrayVec<usize, 3> {
        let mut counts: ArrayVec<usize, 3> = (0..self.cfa_type.num_colors()).map(|_| 0).collect();
        self.for_each_pair(|pair| counts[pair.colour] += 1);
        counts
    }
}
