//! The frame under distribution, in the shape the kernels read it.

use std::ops::Range;

use arrayvec::ArrayVec;
use glam::DVec2;
use imaginarium::Buffer2;

use crate::drizzle::accumulator::MAX_CHANNELS;
use crate::drizzle::accumulator::drop_quad::DropQuad;
use crate::drizzle::accumulator::input_map::InputMap;
use crate::drizzle::deposit::Deposit;
use crate::frame::stackable_image::StackableImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::registration::transform::Transform;
use crate::registration::transform::warp_transform::WarpTransform;

/// Area below which a square drop is discarded: the transform has collapsed the input pixel to
/// nothing, so there is no output area to spread its flux over.
const JACOBIAN_MIN: f64 = 1e-30;

/// One input pixel's samples, one per channel, and the flags it carries to every output pixel its
/// drop reaches.
#[derive(Debug)]
pub(super) struct Fluxes {
    pub(super) values: ArrayVec<f32, MAX_CHANNELS>,
    /// The output channel a mosaic photosite's one sample reaches alone, its colour; `None` for a
    /// pixel whose every channel reaches its own.
    pub(super) colour: Option<usize>,
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
    pub(super) position: Vec2us,
    pub(super) index: usize,
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

/// Input rows of slack on the strip an outline is cut by: far above the rounding of an outline's
/// vertices — coordinates of 10⁵ pixels carry 10⁻¹¹ — and far below a pixel, so it scans no
/// column more, while a pixel centred exactly on a vertex is not lost to that rounding.
const STRIP_GUARD: f64 = 1e-6;

/// A band's widened output rectangle taken back onto the input as a closed outline, and how far
/// from it, in input pixels, a pixel whose drop reaches inside can lie.
#[derive(Debug, Clone)]
pub(super) struct Outline {
    pub(super) vertices: Vec<DVec2>,
    pub(super) reach: f64,
    /// Whether its columns bound a scan: a closed-form outline's do, see
    /// [`FrameSource::band_scan`].
    pub(super) bounds_columns: bool,
}

impl Outline {
    /// The least and greatest input row the outline spans, widened by its reach.
    fn rows(&self) -> [f64; 2] {
        let [first, last] = self.vertices.iter().fold(
            [f64::INFINITY, f64::NEG_INFINITY],
            |[first, last], vertex| [first.min(vertex.y), last.max(vertex.y)],
        );
        [first - self.reach, last + self.reach]
    }

    /// The input columns of row `row` within reach of the outline, on a frame `width` wide.
    ///
    /// The outline bounds a region, so the region's widest point within the strip of rows a drop
    /// reaches from `row` lies on the outline inside the strip, or where the outline crosses its
    /// edges: the edges cut to the strip give the extent, for any outline, convex or not.
    #[expect(
        clippy::cast_sign_loss,
        reason = "each bound is held at 0 or above first, and the cast saturates at the top"
    )]
    fn columns(&self, row: usize, width: usize) -> Range<usize> {
        let low = row as f64 - self.reach - STRIP_GUARD;
        let high = row as f64 + self.reach + STRIP_GUARD;
        let mut extent = [f64::INFINITY, f64::NEG_INFINITY];
        let edges = self
            .vertices
            .iter()
            .zip(self.vertices.iter().cycle().skip(1));
        for (&a, &b) in edges {
            if a.y.max(b.y) < low || a.y.min(b.y) > high {
                continue;
            }
            let [enter, leave] = if a.y == b.y {
                [0.0, 1.0]
            } else {
                let [to_low, to_high] = [low, high].map(|y| (y - a.y) / (b.y - a.y));
                [to_low.min(to_high).max(0.0), to_low.max(to_high).min(1.0)]
            };
            for t in [enter, leave] {
                let x = a.x + t * (b.x - a.x);
                extent = [extent[0].min(x), extent[1].max(x)];
            }
        }
        if extent[0] > extent[1] {
            return 0..0;
        }
        let start = ((extent[0] - self.reach).floor().max(0.0) as usize).min(width);
        let end = ((extent[1] + self.reach).ceil() + 1.0).max(0.0) as usize;
        start..end.min(width).max(start)
    }
}

/// The input pixels whose drops can reach one band of the output: a run of rows, and in each row
/// the columns its outline spans.
#[derive(Debug, Clone)]
pub(super) struct BandScan {
    rows: Range<usize>,
    /// `None` when nothing bounds the band's input, and every column of a row is scanned.
    outline: Option<Outline>,
    width: usize,
}

impl BandScan {
    /// The input rows the band scans.
    pub(super) fn rows(&self) -> Range<usize> {
        self.rows.clone()
    }

    /// The input columns the band scans in `row`.
    pub(super) fn columns(&self, row: usize) -> Range<usize> {
        match &self.outline {
            Some(outline) => outline.columns(row, self.width),
            None => 0..self.width,
        }
    }

    /// Whether the band scans input pixel `pixel`.
    pub(super) fn contains(&self, pixel: Vec2us) -> bool {
        self.rows.contains(&pixel.y) && self.columns(pixel.y).contains(&pixel.x)
    }
}

/// The frame being distributed, as every band sees it: identical for all of them, and read-only.
#[derive(Debug)]
pub(super) struct FrameSource<'a> {
    /// The channel planes, borrowed once for the frame. `LinearImage::channel` is an enum match and
    /// a release assert, which a per-deposit read would pay for every output pixel a drop touches.
    planes: ArrayVec<&'a [f32], MAX_CHANNELS>,
    /// The mosaic whose photosites each reach the channel of their colour alone.
    mosaic: Option<CfaType>,
    size: Size2us,
    map: InputMap,
    pixel_weights: Option<&'a [f32]>,
    /// The frame's flags, when it carries any: those of
    /// [`QualityFlags::RESAMPLE_EXCLUDED`] keep a pixel's drop out, as the warp leaves such a pixel
    /// out of every sample, and those of [`QualityFlags::RESAMPLE_CARRIED`] travel with its drop.
    flags: Option<&'a PixelFlags>,
}

impl<'a> FrameSource<'a> {
    /// `image` under `warp` — reference to input, as registration produces it — onto an output grid
    /// `scale` times finer, its samples reaching the output as `deposit` says.
    pub(super) fn new(
        image: &'a impl StackableImage,
        deposit: Deposit,
        warp: &WarpTransform,
        scale: f64,
        pixel_weights: Option<&'a Buffer2<f32>>,
    ) -> Self {
        let dimensions = image.dimensions();
        Self {
            planes: (0..dimensions.channels())
                .map(|channel| image.channel(channel))
                .collect(),
            mosaic: deposit.mosaic(),
            size: dimensions.size(),
            map: InputMap::new(warp, output_grid(scale)),
            pixel_weights: pixel_weights.map(Buffer2::pixels),
            flags: image.flags(),
        }
    }

    pub(super) const fn width(&self) -> usize {
        self.size.width
    }

    #[inline]
    pub(super) fn fluxes(&self, pixel: InputPixel) -> Fluxes {
        Fluxes {
            values: self.planes.iter().map(|plane| plane[pixel.index]).collect(),
            colour: self
                .mosaic
                .map(|cfa_type| cfa_type.color_at(pixel.position)),
            carried: self.flags.map_or(0, |flags| {
                flags.at(pixel.index).byte() & QualityFlags::RESAMPLE_CARRIED.byte()
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
        let position = DVec2::new(pixel.position.x as f64, pixel.position.y as f64);
        match self.map.position(position) {
            Some(centre) => Drop::Landed(Droplet { centre, weight }),
            None => Drop::Unconverged,
        }
    }

    /// Where input point `p` lands on the output grid, or `None` when the SIP inverse does not
    /// converge there.
    #[inline]
    pub(super) fn position(&self, p: DVec2) -> Option<DVec2> {
        self.map.position(p)
    }

    /// The drop at `pixel` as the quadrilateral its corners map to, shrunk by the pixel fraction to
    /// `half_drop` either side of its centre.
    #[inline]
    pub(super) fn quad(&self, pixel: InputPixel, half_drop: f64) -> Drop<DropQuad> {
        let centre = DVec2::new(pixel.position.x as f64, pixel.position.y as f64);
        self.quad_of(pixel, |corner| {
            self.map.position(centre + QUAD_CORNERS[corner] * half_drop)
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
        for (index, mapped) in corners.iter_mut().enumerate() {
            let Some(position) = corner(index) else {
                return Drop::Unconverged;
            };
            *mapped = position;
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

    /// The input pixels whose drops can reach output `rows`: a drop reaching `output_margin`
    /// output rows, or `input_margin` input rows, from its pixel's centre.
    ///
    /// The output rows widened by `output_margin` — and the columns too, since a rotated drop
    /// reaches the band from beside the grid as well as from above and below it — taken back to the
    /// input, and the pixels within `input_margin` of that. Exact rather than estimated: a drop that
    /// reaches the band has a point inside it, and that point lies within `input_margin` of its
    /// pixel's centre, or its output position within `output_margin` of the band. Deliberately
    /// generous by a row and a column, since over-scanning costs one transform and a rejected test
    /// per pixel while under-scanning would drop flux.
    ///
    /// A closed-form map also limits each row to the columns its outline spans there, so a band of
    /// a rotated frame reads the slanted strip of the input that reaches it rather than every
    /// column of every row the strip touches — at a quarter turn, a few columns of each row instead
    /// of all of them. A SIP map scans whole rows: its outline is sampled, a fold it does not see
    /// would lose drops at its columns where whole rows lose none, and every pixel of a row is then
    /// tried, so the pixels its inverse cannot place are counted the same whatever the bands.
    #[expect(
        clippy::cast_sign_loss,
        reason = "each bound is held at 0 or above first, and the cast saturates at the top"
    )]
    pub(super) fn band_scan(
        &self,
        rows: &Range<usize>,
        output_width: usize,
        output_margin: f64,
        input_margin: f64,
    ) -> BandScan {
        let low = rows.start as f64 - output_margin;
        let high = rows.end as f64 - 1.0 + output_margin;
        let left = -output_margin;
        let right = output_width as f64 - 1.0 + output_margin;
        let width = self.size.width;
        let height = self.size.height;
        let Some(outline) = self.map.outline(low, high, left, right, input_margin) else {
            return BandScan {
                rows: 0..height,
                outline: None,
                width,
            };
        };

        // Saturating float casts, so a degenerate inverse (non-finite vertices) yields an empty
        // range rather than a wild one.
        let [first, last] = outline.rows();
        let start = (first.floor().max(0.0) as usize).min(height);
        let end = (last.ceil() + 1.0).max(0.0) as usize;
        BandScan {
            rows: start..end.min(height).max(start),
            outline: outline.bounds_columns.then_some(outline),
            width,
        }
    }

    /// The pixel weight at `pixel`, or `None` when it is zero or the pixel's flags exclude it.
    ///
    /// Zero is the one value worth testing for: it deposits nothing anywhere, and letting it
    /// through would have the frame counted as covering every pixel it reached. An excluded pixel is
    /// the same case: the decoder's fill under a null would otherwise pull every output pixel it
    /// reaches toward the frame's median.
    #[inline]
    fn deposit_weight(&self, pixel: InputPixel) -> Option<f64> {
        if self.flags.is_some_and(|flags| {
            flags
                .at(pixel.index)
                .intersects(QualityFlags::RESAMPLE_EXCLUDED)
        }) {
            return None;
        }
        let weight = self
            .pixel_weights
            .map_or(1.0, |weights| weights[pixel.index]);
        (weight > 0.0).then_some(f64::from(weight))
    }
}

/// The map from reference pixels onto the output grid `scale` times finer.
///
/// Integer-centred on both sides: reference pixel `p` spans `[p − ½, p + ½]` and output pixel `o`
/// spans `[o − ½, o + ½]`, so the reference footprint `[−½, w − ½]` has to land on `[−½, s·w − ½]`,
/// which is `o = s·p + (s − 1)/2`. A bare `o = s·p` would put the footprint at `[−s/2, s·w − s/2]`,
/// losing a strip of `(s − 1)/2` output pixels at the top and left of every frame and half-covering
/// the last row and column.
fn output_grid(scale: f64) -> Transform {
    let offset = (scale - 1.0) / 2.0;
    Transform::affine([scale, 0.0, offset, 0.0, scale, offset])
}

#[cfg(test)]
pub(crate) mod internals {
    use std::ops::Range;

    use crate::drizzle::accumulator::frame_source::FrameSource;
    use crate::drizzle::deposit::Deposit;
    use crate::image::linear_image::LinearImage;
    use crate::registration::transform::warp_transform::WarpTransform;

    /// The input rows a band covering `rows` would scan, each with its columns, without running a
    /// drizzle to find out.
    pub(crate) fn band_scan(
        image: &LinearImage,
        warp: &WarpTransform,
        scale: f64,
        rows: Range<usize>,
        output_width: usize,
        output_margin: f64,
        input_margin: f64,
    ) -> impl Iterator<Item = (usize, Range<usize>)> {
        let scan = FrameSource::new(image, Deposit::of(image), warp, scale, None).band_scan(
            &rows,
            output_width,
            output_margin,
            input_margin,
        );
        scan.rows().map(move |row| (row, scan.columns(row)))
    }
}
