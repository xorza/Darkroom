//! The frame under distribution, in the shape the kernels read it.

use std::ops::Range;

use arrayvec::ArrayVec;
use glam::DVec2;
use imaginarium::Buffer2;

use crate::drizzle::accumulator::MAX_CHANNELS;
use crate::drizzle::deposit::Deposit;
use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::cfa::CfaType;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::registration::transform::inverse_warp::InverseWarp;
use crate::registration::transform::{Transform, TransformType, WarpTransform};

/// Area magnification below which a drop is discarded: the transform has collapsed the input pixel
/// to nothing, so there is no output area to spread its flux over.
const JACOBIAN_MIN: f64 = 1e-30;

/// Output pixels between boundary samples when a SIP band's outline is taken back to the input.
///
/// A SIP edge bends away from the chord between two samples by at most `stride²/8·|∂²W|`, and a
/// fitted field's second derivative is far under 1e-2 per pixel, so at this stride the bend is
/// under 0.1 input row — inside the extra row [`FrameSource::input_rows`] adds for it.
const SIP_BOUNDARY_STRIDE: usize = 8;

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

/// One input pixel's drop as the quadrilateral it maps to, for the kernel that clips exactly.
#[derive(Debug, Clone, Copy)]
pub(super) struct DropQuad {
    /// The shrunken drop's corners in output coordinates, wound counterclockwise: BL, BR, TR, TL.
    pub(super) corners: [DVec2; 4],
    /// The pixel weight ÷ |signed area of `corners`|: the square kernel spreads it over the area
    /// the drop maps to, so it deposits the pixel weight in total.
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

/// Input pixels onto the output grid, and output points back onto the input.
#[derive(Debug)]
enum InputMap {
    /// A warp without SIP: one transform each way. A model below a homography magnifies area by
    /// one factor everywhere, which is held once.
    Transform {
        to_output: Transform,
        to_input: Transform,
        uniform_magnification: Option<f64>,
    },
    /// A warp with SIP — boxed, since it carries two copies of the polynomial and the source is
    /// built once per frame.
    Sip(Box<SipMap>),
}

/// [`InputMap::Sip`]: forward by the warp itself, backward by its Newton inverse, each with the
/// output grid's own map.
#[derive(Debug)]
struct SipMap {
    warp: WarpTransform,
    inverse: InverseWarp,
    grid: Transform,
}

impl InputMap {
    /// `grid ∘ warp⁻¹`, the map from input pixels to output pixels.
    fn new(warp: &WarpTransform, grid: Transform) -> Self {
        if warp.has_sip() {
            return Self::Sip(Box::new(SipMap {
                warp: warp.clone(),
                inverse: warp.inverse(),
                grid,
            }));
        }
        let to_output = grid.compose(&warp.transform.inverse());
        let uniform_magnification = (to_output.transform_type() != TransformType::Homography)
            .then(|| to_output.jacobian(DVec2::ZERO).determinant().abs());
        Self::Transform {
            to_output,
            to_input: to_output.inverse(),
            uniform_magnification,
        }
    }

    /// Where input point `p` lands on the output grid, or `None` when the SIP inverse does not
    /// converge there.
    #[inline]
    fn position(&self, p: DVec2) -> Option<DVec2> {
        match self {
            Self::Transform { to_output, .. } => Some(to_output.apply(p)),
            Self::Sip(sip) => sip
                .inverse
                .position(p)
                .map(|position| sip.grid.apply(position)),
        }
    }

    /// [`Self::position`], with the area the map magnifies by there.
    #[inline]
    fn landing(&self, p: DVec2) -> Option<Landing> {
        match self {
            Self::Transform {
                to_output,
                uniform_magnification,
                ..
            } => Some(Landing {
                position: to_output.apply(p),
                magnification: uniform_magnification
                    .unwrap_or_else(|| to_output.jacobian(p).determinant().abs()),
            }),
            Self::Sip(sip) => sip.inverse.apply(p).map(|mapped| Landing {
                position: sip.grid.apply(mapped.position),
                magnification: (sip.grid.jacobian(mapped.position) * mapped.jacobian)
                    .determinant()
                    .abs(),
            }),
        }
    }
}

/// Where an input point lands on the output grid, and the area the map magnifies by there.
#[derive(Debug, Clone, Copy)]
struct Landing {
    position: DVec2,
    magnification: f64,
}

/// The input rows a region of the output maps onto, before any margin.
#[derive(Debug, Clone, Copy)]
struct RowExtent {
    first: f64,
    last: f64,
}

impl RowExtent {
    /// The least and greatest of `rows`.
    fn of(rows: impl IntoIterator<Item = f64>) -> Self {
        rows.into_iter().fold(
            Self {
                first: f64::INFINITY,
                last: f64::NEG_INFINITY,
            },
            |extent, row| Self {
                first: extent.first.min(row),
                last: extent.last.max(row),
            },
        )
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
                .map(|cfa_type| usize::from(cfa_type.color_at(pixel.position))),
            carried: self.flags.map_or(0, |flags| {
                flags.at(pixel.index).byte() & QualityFlags::RESAMPLE_CARRIED.byte()
            }),
        }
    }

    /// The drop at `pixel`.
    #[inline]
    pub(super) fn droplet(&self, pixel: InputPixel) -> Drop<Droplet> {
        let Some(weight) = self.deposit_weight(pixel) else {
            return Drop::Empty;
        };
        let position = DVec2::new(pixel.position.x as f64, pixel.position.y as f64);
        let Some(landing) = self.map.landing(position) else {
            return Drop::Unconverged;
        };
        if landing.magnification < JACOBIAN_MIN {
            return Drop::Empty;
        }
        Drop::Landed(Droplet {
            centre: landing.position,
            weight,
        })
    }

    /// The drop at `pixel` as the quadrilateral its corners map to, shrunk by the pixel fraction.
    #[inline]
    pub(super) fn quad(&self, pixel: InputPixel, half_drop: f64) -> Drop<DropQuad> {
        let Some(weight) = self.deposit_weight(pixel) else {
            return Drop::Empty;
        };
        let centre = DVec2::new(pixel.position.x as f64, pixel.position.y as f64);
        let mut corners = [DVec2::ZERO; 4];
        for (corner, offset) in corners.iter_mut().zip([
            DVec2::new(-half_drop, -half_drop),
            DVec2::new(half_drop, -half_drop),
            DVec2::new(half_drop, half_drop),
            DVec2::new(-half_drop, half_drop),
        ]) {
            let Some(position) = self.map.position(centre + offset) else {
                return Drop::Unconverged;
            };
            *corner = position;
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

    /// The input rows whose drops can reach output `rows`: a drop reaching `output_margin` output
    /// rows, or `input_margin` input rows, from its pixel's centre.
    ///
    /// The output rows widened by `output_margin` — and the columns too, since a rotated drop
    /// reaches the band from beside the grid as well as from above and below it — taken back to the
    /// input, and the input rows they span widened by `input_margin`. Exact rather than estimated:
    /// a drop that reaches the band has a point inside it, and that point's input row lies within
    /// `input_margin` of its pixel's, or its output row within `output_margin` of the band.
    /// Deliberately generous at the row level — over-scanning costs one transform and a rejected
    /// row test per pixel, measured at ~0.9 ns against ~100 ns for a deposit, while under-scanning
    /// would drop flux.
    #[expect(
        clippy::cast_sign_loss,
        reason = "each bound is held at 0 or above first, and the cast saturates at the top"
    )]
    pub(super) fn input_rows(
        &self,
        rows: &Range<usize>,
        output_width: usize,
        output_margin: f64,
        input_margin: f64,
    ) -> Range<usize> {
        let low = rows.start as f64 - output_margin;
        let high = rows.end as f64 - 1.0 + output_margin;
        let left = -output_margin;
        let right = output_width as f64 - 1.0 + output_margin;
        let Some(extent) = self.input_row_extent(low, high, left, right) else {
            return 0..self.size.height;
        };

        // Saturating float casts, so a degenerate inverse (non-finite corners) yields an empty
        // range rather than a wild one.
        let height = self.size.height;
        let start = ((extent.first - input_margin).floor().max(0.0) as usize).min(height);
        let end = ((extent.last + input_margin).ceil() + 1.0).max(0.0) as usize;
        start..end.min(height).max(start)
    }

    /// The lowest and highest input row the output rectangle `[left, right] × [low, high]` maps
    /// onto, or `None` when no bound exists and the whole frame has to be scanned.
    #[expect(
        clippy::cast_sign_loss,
        reason = "an output rectangle's width and height are non-negative, and an empty one saturates to 0"
    )]
    fn input_row_extent(&self, low: f64, high: f64, left: f64, right: f64) -> Option<RowExtent> {
        match &self.map {
            InputMap::Transform { to_input, .. } => {
                let corners = [
                    DVec2::new(left, low),
                    DVec2::new(right, low),
                    DVec2::new(left, high),
                    DVec2::new(right, high),
                ];
                // The corner hull bounds the interior only while the inverse's homogeneous divisor
                // keeps one sign across the rectangle. That divisor is affine in the output
                // coordinates, so a sign change between corners means the rectangle straddles the
                // vanishing line, where the mapped region is unbounded and four corners bound
                // nothing. Only a homography can do it — every other model divides by a constant 1.
                let m = to_input.matrix();
                let divisor = |p: DVec2| m[6] * p.x + m[7] * p.y + m[8];
                let reference = divisor(corners[0]);
                if !corners
                    .iter()
                    .all(|&corner| divisor(corner) * reference > 0.0)
                {
                    return None;
                }
                Some(RowExtent::of(
                    corners.map(|corner| to_input.apply(corner).y),
                ))
            }
            InputMap::Sip(sip) => {
                // A non-linear map bends straight edges, so the outline is sampled rather than its
                // corners taken, and one input row is added for the bend between samples — see
                // `SIP_BOUNDARY_STRIDE`. The image of a region's boundary bounds the image of the
                // region for any map without folds, which a converging inverse guarantees here.
                let from_grid = sip.grid.inverse();
                let back = |p: DVec2| sip.warp.apply(from_grid.apply(p)).y;
                let columns = (0..=(right - left).ceil() as usize)
                    .step_by(SIP_BOUNDARY_STRIDE)
                    .map(|dx| (left + dx as f64).min(right))
                    .chain([right]);
                let rows = (0..=(high - low).ceil() as usize)
                    .step_by(SIP_BOUNDARY_STRIDE)
                    .map(|dy| (low + dy as f64).min(high))
                    .chain([high]);
                let horizontal =
                    columns.flat_map(|x| [back(DVec2::new(x, low)), back(DVec2::new(x, high))]);
                let vertical =
                    rows.flat_map(|y| [back(DVec2::new(left, y)), back(DVec2::new(right, y))]);
                let extent = RowExtent::of(horizontal.chain(vertical));
                Some(RowExtent {
                    first: extent.first - 1.0,
                    last: extent.last + 1.0,
                })
            }
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
    use crate::io::image::linear::LinearImage;
    use crate::registration::transform::WarpTransform;

    /// The input rows a band covering `rows` would scan, without running a drizzle to find out.
    pub(crate) fn input_rows(
        image: &LinearImage,
        warp: &WarpTransform,
        scale: f64,
        rows: Range<usize>,
        output_width: usize,
        output_margin: f64,
        input_margin: f64,
    ) -> Range<usize> {
        FrameSource::new(image, Deposit::of(image), warp, scale, None).input_rows(
            &rows,
            output_width,
            output_margin,
            input_margin,
        )
    }
}
