//! [`FlatGain`]: the factor a flat multiplied each pixel's value and noise by, per channel.

pub(crate) mod gain_bins;

use std::ops::Range;

use arrayvec::ArrayVec;
use glam::DVec2;
use imaginarium::Buffer2;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::image::cfa::cfa_type::CfaType;
use crate::image::channel_layout::ChannelLayout;
use crate::image::error::InvalidFlatGain;
use crate::image::flat_gain::gain_bins::GainBins;
use crate::image::image_dimensions::ImageDimensions;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// Pixels between the grid's nodes on each axis.
///
/// A flat's gain changes over vignetting and dust shadows tens of pixels across, which a node
/// every 4 pixels follows; what it leaves out is the photosite-to-photosite response, about 1% in
/// gain, which a dithered stack averages over its frames. The grid is 1/16 of a plane per channel.
pub(crate) const STEP: usize = 4;

/// The gain `g = 1/f` a normalized flat `f` multiplied an image's pixels by, and so their noise,
/// per channel — a color of the mosaic, or the channel of the frame demosaiced from it.
///
/// Held on a grid of nodes 4 pixels apart, node `(i, j)` at pixel `(4i, 4j)` and the last on each
/// axis at or past the image's last pixel, and read bilinearly between them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(into = "FlatGainRecord", try_from = "FlatGainRecord")]
pub struct FlatGain {
    /// The image the grid covers.
    size: Size2us,
    /// The node grid over [`Self::size`].
    grid: GainGrid,
    nodes: ArrayVec<Buffer2<f32>, 3>,
    /// Each channel's bins of equal count over the sensor's gain, which a frame's noise is
    /// measured over before any warp moves it.
    bins: ArrayVec<GainBins, 3>,
}

/// A [`FlatGain`] as a kept frame's record holds it: the image it covers, each channel's node grid
/// row-major, and each channel's bins, which a warp carries over from the sensor rather than
/// measuring again.
#[derive(Debug, Serialize, Deserialize)]
struct FlatGainRecord {
    width: usize,
    height: usize,
    nodes: Vec<Vec<f32>>,
    bins: Vec<GainBins>,
}

impl From<FlatGain> for FlatGainRecord {
    fn from(gain: FlatGain) -> Self {
        Self {
            width: gain.size.width,
            height: gain.size.height,
            nodes: gain.nodes.into_iter().map(Buffer2::into_vec).collect(),
            bins: gain.bins.into_iter().collect(),
        }
    }
}

impl TryFrom<FlatGainRecord> for FlatGain {
    type Error = InvalidFlatGain;

    fn try_from(record: FlatGainRecord) -> Result<Self, InvalidFlatGain> {
        let invalid = InvalidFlatGain {
            channels: record.nodes.len(),
            bins: record.bins.len(),
        };
        // The size first: a zero side has no grid, and an overflowing one no pixel count.
        let size = Size2us::new(record.width, record.height);
        if ImageDimensions::checked(size, ChannelLayout::Mono).is_err() {
            return Err(invalid);
        }
        let grid = GainGrid::of(size);
        let shaped = (1..=3).contains(&record.nodes.len())
            && record.bins.len() == record.nodes.len()
            && record.nodes.iter().all(|plane| {
                plane.len() == grid.columns * grid.rows
                    && plane.iter().all(|gain| gain.is_finite() && *gain > 0.0)
            });
        if !shaped {
            return Err(invalid);
        }
        Ok(Self {
            size,
            grid,
            nodes: record
                .nodes
                .into_iter()
                .map(|plane| Buffer2::new(grid.columns, grid.rows, plane))
                .collect(),
            bins: record.bins.into_iter().collect(),
        })
    }
}

impl FlatGain {
    /// The gain of `divisor`, a normalized flat of `cfa_type` over its own sensor: each node of
    /// color `c` the mean of `1/f` over the color's photosites within half a step of it, those
    /// `excluded` names left out where any other remains — a photosite the flat did not measure,
    /// or one its floor raised, would pull every node around it. A window with none of the color
    /// widens by a step at a time until it holds one.
    pub(crate) fn of_divisor(
        divisor: &Buffer2<f32>,
        cfa_type: &CfaType,
        excluded: impl Fn(usize) -> bool + Sync,
    ) -> Self {
        let size = Size2us::from(divisor);
        let columns = nodes_along(size.width);
        let rows = nodes_along(size.height);
        let flat = NormalizedFlat {
            divisor,
            cfa_type,
            excluded,
        };
        let nodes = (0..cfa_type.num_colors())
            .map(|color| {
                let mut plane = Buffer2::new_default(columns, rows);
                plane
                    .pixels_mut()
                    .par_chunks_mut(columns)
                    .enumerate()
                    .for_each(|(j, row)| {
                        for (i, node) in row.iter_mut().enumerate() {
                            *node = flat.node_gain(color, Vec2us::new(STEP * i, STEP * j));
                        }
                    });
                plane
            })
            .collect();
        Self::from_nodes(size, nodes)
    }

    /// The gain of an image of `size` whose node grids, one per channel, are `nodes`: a grid a
    /// file stored. Each must be [`GainGrid::of`] `size`.
    pub(crate) fn from_nodes(size: Size2us, nodes: ArrayVec<Buffer2<f32>, 3>) -> Self {
        let grid = GainGrid::of(size);
        debug_assert!(
            nodes
                .iter()
                .all(|plane| plane.width() == grid.columns && plane.height() == grid.rows)
        );
        let bins = nodes
            .iter()
            .map(|plane| GainBins::of(&mut plane.pixels().to_vec()))
            .collect();
        Self {
            size,
            grid,
            nodes,
            bins,
        }
    }

    /// This gain as an output frame of `size` sees it through `to_source`, its output-to-source
    /// map: each node the source's gain where the map samples it.
    pub(crate) fn warped(&self, to_source: impl Fn(DVec2) -> DVec2 + Sync, size: Size2us) -> Self {
        let grid = GainGrid::of(size);
        let to_source = &to_source;
        let source_grid = self.grid;
        // Where each output node samples the source, located among its nodes once for every
        // channel.
        let points: Vec<GainPoint> = (0..grid.rows)
            .into_par_iter()
            .flat_map_iter(|j| {
                (0..grid.columns).map(move |i| {
                    let source = to_source(DVec2::new((STEP * i) as f64, (STEP * j) as f64));
                    source_grid.point(source.x as f32, source.y as f32)
                })
            })
            .collect();
        let nodes = self
            .nodes
            .iter()
            .map(|source| {
                let mut plane = Buffer2::new_default(grid.columns, grid.rows);
                plane
                    .pixels_mut()
                    .par_iter_mut()
                    .zip(&points)
                    .for_each(|(node, point)| {
                        *node = point.read(source.pixels(), source_grid.columns, 0);
                    });
                plane
            })
            .collect();
        Self {
            size,
            grid,
            nodes,
            bins: self.bins.clone(),
        }
    }

    /// The image the grid covers.
    pub(crate) const fn size(&self) -> Size2us {
        self.size
    }

    /// `channel`'s bins of equal count over the sensor's gain.
    pub(crate) fn bins(&self, channel: usize) -> &GainBins {
        &self.bins[channel]
    }

    /// Each channel's nodes, row-major over the [`GainGrid::of`] [`Self::size`].
    pub(crate) fn planes(&self) -> impl Iterator<Item = &[f32]> {
        self.nodes.iter().map(Buffer2::pixels)
    }

    /// The gain of `channel` at pixel `(x, y)`, between the nodes around it; held at the outermost
    /// nodes past the grid.
    pub(crate) fn at(&self, channel: usize, x: f32, y: f32) -> f32 {
        self.grid
            .point(x, y)
            .read(self.nodes[channel].pixels(), self.grid.columns, 0)
    }

    /// The colors or channels the gain covers.
    pub(crate) const fn channels(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the gain covers an image of `size` and `channels` channels, or of `channels`
    /// colors for a mosaic.
    pub(crate) fn covers(&self, size: Size2us, channels: usize) -> bool {
        self.size() == size && self.channels() == channels
    }
}

/// The node grid over an image: its columns and rows of nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GainGrid {
    pub(crate) columns: usize,
    pub(crate) rows: usize,
}

impl GainGrid {
    /// The grid over an image of `size`.
    pub(crate) const fn of(size: Size2us) -> Self {
        Self {
            columns: nodes_along(size.width),
            rows: nodes_along(size.height),
        }
    }

    /// The node rows the pixel rows `rows` read: from the low node of the first to the high node of
    /// the last.
    pub(crate) fn rows_for(self, rows: Range<usize>) -> Range<usize> {
        debug_assert!(!rows.is_empty());
        let first = AxisPoint::locate(rows.start as f32, self.rows);
        let last = AxisPoint::locate((rows.end - 1) as f32, self.rows);
        first.low..last.high + 1
    }

    /// Where pixel `(x, y)` falls among the nodes: the same for every grid over an image of this
    /// size, so located once for all of them.
    pub(crate) fn point(self, x: f32, y: f32) -> GainPoint {
        GainPoint {
            column: AxisPoint::locate(x, self.columns),
            row: AxisPoint::locate(y, self.rows),
        }
    }
}

/// A pixel's place among a grid's nodes on both axes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GainPoint {
    column: AxisPoint,
    row: AxisPoint,
}

impl GainPoint {
    /// The gain at this point, bilinear between the four nodes around it, of `plane`: a grid's
    /// node rows `first..`, `columns` nodes each, row-major.
    fn read(self, plane: &[f32], columns: usize, first: usize) -> f32 {
        let Self { column, row } = self;
        let node = |i: usize, j: usize| plane[(j - first) * columns + i];
        let top = node(column.low, row.low)
            + (node(column.high, row.low) - node(column.low, row.low)) * column.fraction;
        let bottom = node(column.low, row.high)
            + (node(column.high, row.high) - node(column.low, row.high)) * column.fraction;
        top + (bottom - top) * row.fraction
    }
}

/// The node rows `first..` of a grid, each channel's, as the combine reads them for a band of
/// pixel rows.
#[derive(Debug, Clone)]
pub(crate) struct GainRows<'a> {
    grid: GainGrid,
    /// The grid row the planes start at.
    first: usize,
    planes: ArrayVec<&'a [f32], 3>,
}

impl<'a> GainRows<'a> {
    /// Each channel's node rows `first..` of `grid`, row-major.
    pub(crate) const fn new(grid: GainGrid, first: usize, planes: ArrayVec<&'a [f32], 3>) -> Self {
        Self {
            grid,
            first,
            planes,
        }
    }

    /// The gain of `channel` at `point`, which these rows must reach.
    pub(crate) fn at(&self, channel: usize, point: GainPoint) -> f32 {
        point.read(self.planes[channel], self.grid.columns, self.first)
    }
}

/// The two nodes around a coordinate on one axis, and its fraction of the way from the low one to
/// the high one.
#[derive(Debug, Clone, Copy)]
struct AxisPoint {
    low: usize,
    high: usize,
    fraction: f32,
}

impl AxisPoint {
    /// Pixel coordinate `position` on an axis of `nodes` nodes, held at the outermost nodes past
    /// it.
    #[expect(
        clippy::cast_sign_loss,
        reason = "the node coordinate is clamped to be non-negative before the conversion"
    )]
    fn locate(position: f32, nodes: usize) -> Self {
        let last = nodes - 1;
        let coordinate = (position / STEP as f32).clamp(0.0, last as f32);
        let low = (coordinate as usize).min(last.saturating_sub(1));
        Self {
            low,
            high: (low + 1).min(last),
            fraction: coordinate - low as f32,
        }
    }
}

/// Nodes on an axis of `extent` pixels: one every [`STEP`], the last at or past the last pixel.
const fn nodes_along(extent: usize) -> usize {
    (extent - 1).div_ceil(STEP) + 1
}

/// A normalized flat over its own sensor, the photosites `excluded` names left out of its nodes
/// where any other remains.
#[derive(Debug)]
struct NormalizedFlat<'a, E> {
    divisor: &'a Buffer2<f32>,
    cfa_type: &'a CfaType,
    excluded: E,
}

impl<E: Fn(usize) -> bool> NormalizedFlat<'_, E> {
    /// The mean of `1/f` over the photosites of `color` within half a step of `node`; 1, no gain
    /// known, for a color the sensor has no photosite of.
    fn node_gain(&self, color: usize, node: Vec2us) -> f32 {
        let divisor = self.divisor;
        let size = Size2us::from(divisor);
        let mut half = STEP / 2;
        loop {
            let window = |extent: usize, centre: usize| {
                centre.saturating_sub(half).min(extent - 1)..=(centre + half).min(extent - 1)
            };
            let mut measured = (0.0f64, 0usize);
            let mut all = (0.0f64, 0usize);
            for y in window(size.height, node.y) {
                for x in window(size.width, node.x) {
                    let position = Vec2us::new(x, y);
                    if self.cfa_type.slot_at(position) != color {
                        continue;
                    }
                    let index = y * size.width + x;
                    let gain = 1.0 / f64::from(divisor.pixels()[index]);
                    all = (all.0 + gain, all.1 + 1);
                    if !(self.excluded)(index) {
                        measured = (measured.0 + gain, measured.1 + 1);
                    }
                }
            }
            let (sum, count) = if measured.1 > 0 { measured } else { all };
            if count > 0 {
                return (sum / count as f64) as f32;
            }
            if half >= size.width.max(size.height) {
                return 1.0;
            }
            half += STEP;
        }
    }
}

#[cfg(test)]
mod tests;
