//! The frame under distribution, in the shape the kernels read it.

use std::ops::Range;

use arrayvec::ArrayVec;
use glam::DVec2;
use imaginarium::Buffer2;

use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::stacking::drizzle::accumulator::MAX_CHANNELS;
use crate::stacking::drizzle::geometry::AreaMagnification;
use crate::stacking::registration::transform::Transform;

/// Area magnification below which a drop is discarded: the transform has collapsed the input pixel
/// to nothing, so there is no output area to spread its flux over.
const JACOBIAN_MIN: f64 = 1e-30;

/// One input pixel's samples, one per channel.
pub(super) type Fluxes = ArrayVec<f32, MAX_CHANNELS>;

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
    /// Frame weight × pixel weight ÷ area magnification — the Jacobian divides out the local
    /// magnification so a stretched drop deposits the flux it carried, not the area it covers.
    pub(super) weight: f64,
}

/// One input pixel's drop as the quadrilateral it maps to, for the kernel that clips exactly.
#[derive(Debug, Clone, Copy)]
pub(super) struct DropQuad {
    /// The shrunken drop's corners in output coordinates, wound counterclockwise: BL, BR, TR, TL.
    pub(super) corners: [DVec2; 4],
    /// Frame weight × pixel weight ÷ |signed area of `corners`|.
    pub(super) weight: f64,
}

/// The frame being distributed, as every band sees it: identical for all of them, and read-only.
#[derive(Debug)]
pub(super) struct FrameSource<'a> {
    /// The channel planes, borrowed once for the frame. `LinearImage::channel` is an enum match and
    /// a release assert, which a per-deposit read would pay for every output pixel a drop touches.
    planes: ArrayVec<&'a [f32], MAX_CHANNELS>,
    size: Size2us,
    /// Input pixels to output-grid coordinates, drizzle scale included.
    to_output: Transform,
    /// [`Self::to_output`] inverted, for turning a band's output rows back into input rows.
    to_input: Transform,
    magnification: AreaMagnification,
    weight: f32,
    pixel_weights: Option<&'a [f32]>,
}

impl<'a> FrameSource<'a> {
    pub(super) fn new(
        image: &'a LinearImage,
        to_output: Transform,
        weight: f32,
        pixel_weights: Option<&'a Buffer2<f32>>,
    ) -> Self {
        Self {
            planes: (0..image.channels())
                .map(|channel| image.channel(channel).pixels())
                .collect(),
            size: Size2us::new(image.width(), image.height()),
            to_input: to_output.inverse(),
            magnification: AreaMagnification::new(&to_output),
            to_output,
            weight,
            pixel_weights: pixel_weights.map(Buffer2::pixels),
        }
    }

    pub(super) fn width(&self) -> usize {
        self.size.width
    }

    /// The transform's y-extent of a drop `half_drop` input pixels across, in output rows.
    ///
    /// Exact for every linear model: the drop is a box, so its mapped extent is the sum of the
    /// absolute second-row coefficients. A homography stretches by more than its linear part in
    /// places, which is why [`Self::input_rows`] does not rely on this alone.
    pub(super) fn quad_row_extent(&self, half_drop: f64) -> f64 {
        let m = self.to_output.matrix();
        half_drop * (m[3].abs() + m[4].abs())
    }

    #[inline]
    pub(super) fn fluxes(&self, pixel: InputPixel) -> Fluxes {
        self.planes.iter().map(|plane| plane[pixel.index]).collect()
    }

    /// The drop at `pixel`, or `None` when it deposits nothing.
    #[inline]
    pub(super) fn droplet(&self, pixel: InputPixel) -> Option<Droplet> {
        let weight = self.deposit_weight(pixel)?;
        let centre = self
            .to_output
            .apply(DVec2::new(pixel.position.x as f64, pixel.position.y as f64));
        let magnification = self.magnification.at(centre, pixel.position);
        (magnification >= JACOBIAN_MIN).then(|| Droplet {
            centre,
            weight: weight / magnification,
        })
    }

    /// The drop at `pixel` as the quadrilateral its corners map to, shrunk by the pixel fraction,
    /// or `None` when it deposits nothing.
    #[inline]
    pub(super) fn quad(&self, pixel: InputPixel, half_drop: f64) -> Option<DropQuad> {
        let weight = self.deposit_weight(pixel)?;
        let centre = DVec2::new(pixel.position.x as f64, pixel.position.y as f64);
        let corners = [
            centre + DVec2::new(-half_drop, -half_drop),
            centre + DVec2::new(half_drop, -half_drop),
            centre + DVec2::new(half_drop, half_drop),
            centre + DVec2::new(-half_drop, half_drop),
        ]
        .map(|corner| self.to_output.apply(corner));

        // The magnification is the quadrilateral's own signed area, from the cross product of its
        // diagonals — measured rather than modelled, so it holds for a homography too.
        let area = 0.5 * (corners[1] - corners[3]).perp_dot(corners[0] - corners[2]);
        (area.abs() >= JACOBIAN_MIN).then(|| DropQuad {
            corners,
            weight: weight / area.abs(),
        })
    }

    /// The input rows whose drops can reach `rows`, given the drop's half-extent in output rows.
    ///
    /// Those output rows, widened by `margin` and taken back through the inverse transform: a
    /// straight-line map takes the rectangle to a quadrilateral, so its corners bound the input rows
    /// involved. Deliberately generous — over-scanning costs one transform and a rejected row test
    /// per pixel, measured at ~0.9 ns against ~100 ns for a deposit, while under-scanning would drop
    /// flux.
    pub(super) fn input_rows(
        &self,
        rows: &Range<usize>,
        output_width: usize,
        margin: f64,
    ) -> Range<usize> {
        let low = rows.start as f64 - margin;
        let high = rows.end as f64 - 1.0 + margin;
        let right = output_width as f64 - 1.0;
        let corners = [
            DVec2::new(0.0, low),
            DVec2::new(right, low),
            DVec2::new(0.0, high),
            DVec2::new(right, high),
        ];

        // The corner hull bounds the interior only while the inverse's homogeneous divisor keeps one
        // sign across the widened band. That divisor is affine in the output coordinates, so a sign
        // change between corners means the band straddles the transform's vanishing line, where the
        // mapped region is unbounded and four corners bound nothing. Only a homography can do it —
        // every other model divides by a constant 1 — and the answer is to scan the frame, which
        // costs the rejected-pixel test and never loses flux.
        let m = self.to_input.matrix();
        let divisor = |p: DVec2| m[6] * p.x + m[7] * p.y + m[8];
        let reference = divisor(corners[0]);
        if !corners
            .iter()
            .all(|&corner| divisor(corner) * reference > 0.0)
        {
            return 0..self.size.height;
        }

        let mut first = f64::INFINITY;
        let mut last = f64::NEG_INFINITY;
        for corner in corners {
            let input = self.to_input.apply(corner);
            first = first.min(input.y);
            last = last.max(input.y);
        }

        // Saturating float casts, so a degenerate inverse (non-finite corners) yields an empty range
        // rather than a wild one.
        let height = self.size.height;
        let start = (first.floor().max(0.0) as usize).min(height);
        let end = ((last.ceil() + 1.0).max(0.0) as usize).min(height);
        start..end.max(start)
    }

    /// Frame weight × pixel weight at `pixel`, or `None` when the product is zero.
    ///
    /// Zero is the one value worth testing for: it deposits nothing anywhere, and letting it through
    /// would have a frame that carries no weight still counted as covering every pixel it reached.
    #[inline]
    fn deposit_weight(&self, pixel: InputPixel) -> Option<f64> {
        let pixel_weight = self
            .pixel_weights
            .map_or(1.0, |weights| weights[pixel.index]);
        let weight = self.weight * pixel_weight;
        (weight > 0.0).then_some(weight as f64)
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;

    /// The input rows a band covering `rows` would scan, without running a drizzle to find out.
    pub(crate) fn input_rows(
        image: &LinearImage,
        to_output: Transform,
        rows: Range<usize>,
        output_width: usize,
        margin: f64,
    ) -> Range<usize> {
        FrameSource::new(image, to_output, 1.0, None).input_rows(&rows, output_width, margin)
    }
}
