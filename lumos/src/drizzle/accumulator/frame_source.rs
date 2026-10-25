//! The frame under distribution, in the shape the kernels read it.

use std::ops::Range;

use arrayvec::ArrayVec;
use common::Buffer2;
use glam::DVec2;

use crate::drizzle::accumulator::MAX_CHANNELS;
use crate::drizzle::accumulator::band_scan::BandScan;
use crate::drizzle::accumulator::drop_quad::DropQuad;
use crate::drizzle::accumulator::input_map::{InputMap, OutputRect};
use crate::drizzle::accumulator::kernel_plan::DropReach;
use crate::drizzle::deposit::Deposit;
use crate::drizzle::drizzle_frame::DrizzleFrame;
use crate::drizzle::output_grid::OutputGrid;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::image::stackable_image::StackableImage;
use common::{Size2us, Vec2us};

/// Area below which a square drop is discarded: the transform has collapsed the input pixel to
/// nothing, so there is no output area to spread its flux over.
const JACOBIAN_MIN: f64 = 1e-30;

/// One input pixel's samples, one per channel, and the flags it carries to every output pixel its
/// drop reaches.
#[derive(Debug)]
pub(super) struct Fluxes {
    pub(super) values: ArrayVec<f32, MAX_CHANNELS>,
    /// The output channel a mosaic photosite's one sample reaches alone, its color; `None` for a
    /// pixel whose every channel reaches its own.
    pub(super) color: Option<usize>,
    /// The pixel's [`QualityFlags::RESAMPLE_CARRIED`] flags, as a byte.
    pub(super) carried: u8,
}

/// One input pixel, as both the coordinate the transform takes and the flat index its samples live
/// at.
///
/// The two travel together because the scan computes the index once per pixel and everything
/// downstream — samples, pixel weight — is indexed by it; deriving one from the other again is the
/// multiply the scan exists to hoist.
#[derive(Debug, Clone, Copy)]
pub(super) struct InputPixel {
    pub(super) pos: Vec2us,
    pub(super) idx: usize,
}

/// One input pixel's drop reduced to what a kernel shapes: where it lands on the output grid, and
/// the weight it deposits in total.
#[derive(Debug, Clone, Copy)]
pub(super) struct Droplet {
    /// Where the input pixel's centre lands on the output grid.
    pub(super) centre: DVec2,
    /// The pixel weight, whatever the warp magnifies by, as STScI's fixed-footprint kernels
    /// deposit: a drop's footprint does not follow the magnification, so a magnified frame's drops
    /// lie further apart and give each output pixel less already. Dividing by the magnification as
    /// well would count it twice, and weigh a frame of another plate scale unlike the square kernel
    /// does.
    pub(super) weight: f64,
}

/// What became of one input pixel's drop.
#[derive(Debug, Clone, Copy)]
pub(super) enum Drop<T> {
    /// The drop, to deposit.
    Landed(T),
    /// Nothing to deposit: no weight, or an area the transform collapsed.
    Empty,
    /// The pixel's position could not be inverted through the warp's SIP correction. It deposits
    /// nothing, and the drizzle counts it.
    Unconverged,
}

/// A square drop's corners about its centre, in units of its half side, wound counterclockwise:
/// BL, BR, TR, TL.
pub(super) const QUAD_CORNERS: [DVec2; 4] = [
    DVec2::new(-1.0, -1.0),
    DVec2::new(1.0, -1.0),
    DVec2::new(1.0, 1.0),
    DVec2::new(-1.0, 1.0),
];

/// The frame being distributed, as every band sees it: identical for all of them, and read-only.
#[derive(Debug)]
pub(super) struct FrameSource<'a> {
    /// The channel planes, borrowed once for the frame. `LinearImage::channel` is an enum match and
    /// a release assert, which a per-deposit read would pay for every output pixel a drop touches.
    planes: ArrayVec<&'a [f32], MAX_CHANNELS>,
    /// The mosaic whose photosites each reach the channel of their color alone.
    mosaic: Option<Mosaic>,
    size: Size2us,
    map: InputMap,
    pixel_weights: Option<&'a [f32]>,
    /// The frame's flags, when it carries any: those of
    /// [`QualityFlags::RESAMPLE_EXCLUDED`] keep a pixel's drop out, as the warp leaves such a pixel
    /// out of every sample, and those of [`QualityFlags::RESAMPLE_CARRIED`] travel with its drop.
    flags: Option<&'a PixelFlags>,
}

impl<'a> FrameSource<'a> {
    /// `frame`'s image under its warp — reference to input, as registration produces it — onto
    /// `grid`, its samples reaching the output as `deposit` says.
    pub(super) fn new(
        frame: &'a DrizzleFrame<impl StackableImage>,
        deposit: Deposit,
        grid: OutputGrid,
    ) -> Self {
        let image = &frame.source;
        let dimensions = image.dimensions();
        Self {
            planes: (0..dimensions.channels())
                .map(|channel| image.channel(channel).pixels())
                .collect(),
            mosaic: deposit.mosaic(),
            size: dimensions.size(),
            map: InputMap::new(&frame.warp, grid.reference_map()),
            pixel_weights: frame.pixel_weight_map.as_ref().map(Buffer2::pixels),
            flags: image.flags(),
        }
    }

    pub(super) const fn width(&self) -> usize {
        self.size.width
    }

    #[inline]
    pub(super) fn fluxes(&self, pixel: InputPixel) -> Fluxes {
        Fluxes {
            values: self.planes.iter().map(|plane| plane[pixel.idx]).collect(),
            color: self.mosaic.map(|mosaic| mosaic.slot_at(pixel.pos)),
            carried: self.flags.map_or(0, |flags| {
                flags.at(pixel.idx).byte() & QualityFlags::RESAMPLE_CARRIED.byte()
            }),
        }
    }

    /// The drop at `pixel`: where its centre lands, the one thing a fixed-footprint kernel reads
    /// of the map. Its weight is the pixel's whatever the map magnifies by, so no Jacobian is
    /// computed — none of a homography's per pixel, nor the one a SIP inverse would add to its
    /// Newton steps — as STScI's point, turbo, Gaussian and Lanczos kernels compute none.
    #[inline]
    pub(super) fn droplet(&self, pixel: InputPixel) -> Drop<Droplet> {
        let Some(weight) = self.deposit_weight(pixel) else {
            return Drop::Empty;
        };
        let pos = DVec2::new(pixel.pos.x as f64, pixel.pos.y as f64);
        match self.map.pos(pos) {
            Some(centre) => Drop::Landed(Droplet { centre, weight }),
            None => Drop::Unconverged,
        }
    }

    /// Where input point `p` lands on the output grid, or `None` when the SIP inverse does not
    /// converge there.
    #[inline]
    pub(super) fn pos(&self, p: DVec2) -> Option<DVec2> {
        self.map.pos(p)
    }

    /// The drop at `pixel` as the quadrilateral its corners map to, shrunk by the pixel fraction to
    /// `half_drop` either side of its centre.
    #[inline]
    pub(super) fn quad(&self, pixel: InputPixel, half_drop: f64) -> Drop<DropQuad> {
        let centre = DVec2::new(pixel.pos.x as f64, pixel.pos.y as f64);
        self.quad_of(pixel, |corner| {
            self.map.pos(centre + QUAD_CORNERS[corner] * half_drop)
        })
    }

    /// The drop at `pixel` as the quadrilateral of its mapped corners, `corner(i)` the `i`-th of
    /// [`QUAD_CORNERS`], asked for only when the pixel deposits.
    #[inline]
    pub(super) fn quad_of(
        &self,
        pixel: InputPixel,
        mut corner: impl FnMut(usize) -> Option<DVec2>,
    ) -> Drop<DropQuad> {
        let Some(weight) = self.deposit_weight(pixel) else {
            return Drop::Empty;
        };
        let mut corners = [DVec2::ZERO; 4];
        for (idx, mapped) in corners.iter_mut().enumerate() {
            let Some(pos) = corner(idx) else {
                return Drop::Unconverged;
            };
            *mapped = pos;
        }

        // The magnification is the quadrilateral's own signed area, from the cross product of its
        // diagonals — measured rather than modelled, so it holds for any transform.
        let area = 0.5 * (corners[1] - corners[3]).perp_dot(corners[0] - corners[2]);
        if area.abs() < JACOBIAN_MIN {
            return Drop::Empty;
        }
        Drop::Landed(DropQuad {
            corners,
            weight: weight / area.abs(),
        })
    }

    /// The input pixels whose drops can reach output `rows` of a grid `output_width` wide: a drop
    /// reaching `reach.output` output rows, or `reach.input` input rows, from its pixel's centre.
    ///
    /// The output rows widened by `reach.output` — and the columns too, since a rotated drop
    /// reaches the band from beside the grid as well as from above and below it — taken back to the
    /// input, and the pixels within `reach.input` of that. Exact rather than estimated: a drop that
    /// reaches the band has a point inside it, and that point lies within `reach.input` of its
    /// pixel's centre, or its output position within `reach.output` of the band. Deliberately
    /// generous by a row and a column, since over-scanning costs one transform and a rejected test
    /// per pixel while under-scanning would drop flux.
    ///
    /// A closed-form map also limits each row to the columns its outline spans there, so a band of
    /// a rotated frame reads the slanted strip of the input that reaches it rather than every
    /// column of every row the strip touches — at a quarter turn, a few columns of each row instead
    /// of all of them. A SIP map scans whole rows: its outline is sampled, a fold it does not see
    /// would lose drops at its columns where whole rows lose none, and every pixel of a row is then
    /// tried, so the pixels its inverse cannot place are counted the same whatever the bands.
    pub(super) fn band_scan(
        &self,
        rows: &Range<usize>,
        output_width: usize,
        reach: DropReach,
    ) -> BandScan {
        let band = OutputRect {
            min: DVec2::new(-reach.output, rows.start as f64 - reach.output),
            max: DVec2::new(
                output_width as f64 - 1.0 + reach.output,
                rows.end as f64 - 1.0 + reach.output,
            ),
        };
        BandScan::new(self.map.outline(band, reach.input), self.size)
    }

    /// The pixel weight at `pixel`, or `None` when it is zero or the pixel's flags exclude it.
    ///
    /// Zero is the one value worth testing for: it deposits nothing anywhere, and letting it
    /// through would have the frame counted as covering every pixel it reached. An excluded pixel
    /// is the same case: the decoder's fill under a null would otherwise pull every output pixel it
    /// reaches toward the frame's median.
    #[inline]
    fn deposit_weight(&self, pixel: InputPixel) -> Option<f64> {
        if self.flags.is_some_and(|flags| {
            flags
                .at(pixel.idx)
                .intersects(QualityFlags::RESAMPLE_EXCLUDED)
        }) {
            return None;
        }
        let weight = self.pixel_weights.map_or(1.0, |weights| weights[pixel.idx]);
        (weight > 0.0).then_some(f64::from(weight))
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use std::ops::Range;

    use crate::drizzle::accumulator::frame_source::FrameSource;
    use crate::drizzle::accumulator::kernel_plan::DropReach;
    use crate::drizzle::deposit::Deposit;
    use crate::drizzle::drizzle_frame::DrizzleFrame;
    use crate::drizzle::output_grid::OutputGrid;
    use crate::image::linear_image::LinearImage;

    /// The input rows the band of output rows 0 to 3, on a grid `output_width` wide at `frame`'s
    /// own scale, would scan for drops that `reach` so far, each with its columns, without running
    /// a drizzle to find out.
    pub(crate) fn band_scan(
        frame: &DrizzleFrame<LinearImage>,
        output_width: usize,
        reach: DropReach,
    ) -> impl Iterator<Item = (usize, Range<usize>)> {
        let grid = OutputGrid::new(1.0, frame.source.dimensions().size());
        let scan = FrameSource::new(frame, Deposit::of(&frame.source), grid).band_scan(
            &(0..4),
            output_width,
            reach,
        );
        scan.rows().map(move |row| (row, scan.cols(row)))
    }
}
