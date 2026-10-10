//! One horizontal slice of the output grid, and the kernels that scatter flux into it.

use std::ops::Range;

use glam::DVec2;

use crate::concurrency::job_scratch_pool::JobScratchPool;
use crate::drizzle::accumulator::PlaneSpan;
use crate::drizzle::accumulator::frame_source::{
    BandScan, Drop, DropQuad, Fluxes, FrameSource, InputPixel,
};
use crate::drizzle::accumulator::kernel_plan::{KernelPlan, LANCZOS_ORDER};
use crate::drizzle::geometry::boxer;
use crate::math::lanczos::lanczos_lut::LANCZOS_LUT_RESOLUTION;
use crate::math::vec2us::Vec2us;

/// The lattice points of [`QUAD_CORNERS`](crate::drizzle::accumulator::frame_source::QUAD_CORNERS)
/// about pixel `(x, y)`, as `(x + dx, y + dy)`: lattice
/// point `(j, k)` is input point `(j − ½, k − ½)`.
const LATTICE_CORNERS: [[usize; 2]; 4] = [[0, 0], [1, 0], [1, 1], [0, 1]];

/// A drop whose kernel taps sum to less than this has no normalizer worth dividing by: the radial
/// kernels' taps sum to about the drop's area in output pixels, so this is far below any drop.
const KERNEL_WEIGHT_MIN: f32 = 1e-10;

/// One radial drop's kernel along each axis, one value per output column and per output row of
/// its neighbourhood. Both kernels are separable — the Gaussian is a product of one-axis
/// Gaussians and the Lanczos kernel is defined as one — so a `(2r + 1)²` neighbourhood needs
/// `2·(2r + 1)` evaluations rather than one per tap, and the same values exactly.
#[derive(Debug, Default)]
pub(super) struct RadialScratch {
    columns: Vec<f32>,
    rows: Vec<f32>,
}

/// The square kernel's corner lattice at pixfrac 1, where neighbouring drops share their corners:
/// lattice point `(j, k)` is input point `(j − ½, k − ½)`, the corner the up to four drops around
/// it meet at, mapped once for all of them instead of once for each — a fourth of the transforms,
/// and of a SIP map's Newton inversions. The points are the ones each drop would map itself, so
/// the deposits are too.
///
/// Two lattice rows are held, the two an input row's drops span. Each point carries the lattice
/// row it was mapped for, so a row is reused without being cleared, by the next input row and by
/// the next band on the same worker alike.
#[derive(Debug, Default)]
pub(super) struct CornerLattice {
    rows: [LatticeRow; 2],
}

/// One lattice row of a [`CornerLattice`].
#[derive(Debug, Default)]
struct LatticeRow {
    /// `k + 1` for a point mapped for lattice row `k`, 0 for one not mapped this frame.
    stamps: Vec<usize>,
    points: Vec<Option<DVec2>>,
}

impl CornerLattice {
    /// Forget every point: a lattice row has a point more than a frame `width` pixels wide has
    /// pixels, and a new frame maps every one of them differently.
    fn reset(&mut self, width: usize) {
        for row in &mut self.rows {
            row.stamps.clear();
            row.stamps.resize(width + 1, 0);
            row.points.clear();
            row.points.resize(width + 1, None);
        }
    }

    /// Lattice point `(j, k)` on the output grid, mapped through `source` the first time it is
    /// asked for in lattice row `k`.
    #[inline]
    fn corner(&mut self, source: &FrameSource<'_>, j: usize, k: usize) -> Option<DVec2> {
        let row = &mut self.rows[k % 2];
        if row.stamps[j] != k + 1 {
            row.points[j] = source.position(DVec2::new(j as f64 - 0.5, k as f64 - 0.5));
            row.stamps[j] = k + 1;
        }
        row.points[j]
    }
}

/// One horizontal slice of the output grid: the rows it owns, and every accumulator restricted to
/// them.
///
/// The scatter cannot be parallelized over the input — neighbouring input pixels write overlapping
/// output pixels — so it is parallelized over the *output*. A band owns its rows exclusively, so
/// the deposits need no synchronisation; and because each output pixel belongs to exactly one band,
/// and a band walks its inputs in the same order the serial loop did, every output pixel
/// accumulates its contributions in the serial order. Bit-identical output whatever the band count
/// is what makes this safe for a science product, and `band_count_does_not_change_the_result` pins
/// it.
#[derive(Debug)]
pub(super) struct OutputBand<'a> {
    /// Absolute output rows this band owns.
    rows: Range<usize>,
    /// Output pixels per row. Every band spans the full output width.
    width: usize,
    planes: PlaneSpan<'a>,
    /// Which band this is, and the input pixels every band scans for this frame — this one's, and
    /// the earlier ones', which decide whether a pixel that failed to invert is this band's to
    /// count.
    index: usize,
    scans: &'a [BandScan],
}
impl<'a> OutputBand<'a> {
    pub(super) const fn new(
        rows: Range<usize>,
        width: usize,
        planes: PlaneSpan<'a>,
        index: usize,
        scans: &'a [BandScan],
    ) -> Self {
        Self {
            rows,
            width,
            planes,
            index,
            scans,
        }
    }

    /// Scatter one frame into this band, returning how many of the input pixels it is the first to
    /// scan could not be inverted through the warp's SIP correction.
    pub(super) fn distribute(
        &mut self,
        source: &FrameSource<'_>,
        plan: KernelPlan,
        radial: &JobScratchPool<RadialScratch>,
        lattice: &JobScratchPool<CornerLattice>,
    ) -> usize {
        match plan {
            // Pixfrac 1: every drop's corners are lattice points its neighbours share.
            KernelPlan::Square { half_drop: 0.5 } => {
                let mut lattice = lattice.acquire();
                lattice.reset(source.width());
                self.distribute_square(source, |pixel| {
                    let Vec2us { x, y } = pixel.position;
                    source.quad_of(pixel, |corner| {
                        let [dx, dy] = LATTICE_CORNERS[corner];
                        lattice.corner(source, x + dx, y + dy)
                    })
                })
            }
            KernelPlan::Square { half_drop } => {
                self.distribute_square(source, |pixel| source.quad(pixel, half_drop))
            }
            KernelPlan::Turbo {
                half_drop,
                inv_area,
            } => self.distribute_turbo(source, half_drop, inv_area),
            KernelPlan::Point => self.distribute_point(source),
            KernelPlan::Gaussian {
                radius,
                inv_2sigma_sq,
            } => self.distribute_radial(source, radius, &mut radial.acquire(), |d| {
                (-d * d * inv_2sigma_sq).exp()
            }),
            KernelPlan::Lanczos { radius } => {
                // The warp's table, within 1.1e-7 of the kernel: two `sin`s and two divisions a
                // tap, fourteen taps a drop, become two reads and a fused multiply-add.
                let lut = LANCZOS_ORDER.lut();
                self.distribute_radial(source, radius, &mut radial.acquire(), |d| {
                    lut.at(d.abs() * LANCZOS_LUT_RESOLUTION as f32)
                })
            }
        }
    }

    /// Walk every input pixel whose drop can reach this band, and return how many of them this band
    /// is the first to scan and could not invert.
    ///
    /// The one place the input is scanned: the kernels differ in the shape they give a drop, not in
    /// how they find the pixels that make one. A pixel whose position does not invert is visited by
    /// every band whose scan reaches its row; it is counted by the first of them, so each is
    /// counted once however the scans overlap.
    #[inline]
    fn scan<T>(
        &mut self,
        source: &FrameSource<'_>,
        mut drop: impl FnMut(InputPixel) -> Drop<T>,
        mut deposit: impl FnMut(&mut Self, InputPixel, T),
    ) -> usize {
        let width = source.width();
        let mut unconverged = 0;
        let scan = &self.scans[self.index];
        for iy in scan.rows() {
            let row = iy * width;
            for ix in scan.columns(iy) {
                let pixel = InputPixel {
                    position: Vec2us::new(ix, iy),
                    index: row + ix,
                };
                match drop(pixel) {
                    Drop::Landed(landed) => deposit(self, pixel, landed),
                    Drop::Empty => {}
                    Drop::Unconverged => {
                        unconverged += usize::from(
                            !self.scans[..self.index]
                                .iter()
                                .any(|scan| scan.contains(pixel.position)),
                        );
                    }
                }
            }
        }
        unconverged
    }

    /// Turbo kernel: an axis-aligned rectangular drop.
    fn distribute_turbo(
        &mut self,
        source: &FrameSource<'_>,
        half_drop: f64,
        inv_area: f64,
    ) -> usize {
        self.scan(
            source,
            |pixel| source.droplet(pixel),
            |band, pixel, drop| {
                // Integer-center throughout: input pixel `i` is at coordinate `i` (matching star
                // centroids / `register` / `warp`), and output pixel `o` is the cell `[o - 0.5, o +
                // 0.5)`. The drop centre needs no coordinate adjustment, and the pixels it touches
                // are `round(min) ..= round(max)`.
                let (top, bottom) = (drop.centre.y - half_drop, drop.centre.y + half_drop);
                let (left, right) = (drop.centre.x - half_drop, drop.centre.x + half_drop);
                let Some(rows) = band.deposit_rows(top, bottom) else {
                    return;
                };
                let cols = band.deposit_cols(left, right);
                if cols.is_empty() {
                    return;
                }

                let fluxes = source.fluxes(pixel);
                let weight = drop.weight * inv_area;
                for oy in rows {
                    // The drop is axis-aligned, so its vertical overlap is the same in every
                    // column.
                    let overlap_y = bottom.min(oy as f64 + 0.5) - top.max(oy as f64 - 0.5);
                    if overlap_y <= 0.0 {
                        continue;
                    }
                    let base = band.row_base(oy);
                    for ox in cols.clone() {
                        let overlap_x = right.min(ox as f64 + 0.5) - left.max(ox as f64 - 0.5);
                        if overlap_x > 0.0 {
                            band.accumulate(
                                &fluxes,
                                base + ox,
                                (weight * overlap_x * overlap_y) as f32,
                            );
                        }
                    }
                }
            },
        )
    }

    /// Square kernel: true polygon clipping.
    ///
    /// For each input pixel, transforms all 4 corners of the (pixfrac-shrunken) drop to output
    /// coordinates, then iterates the output pixels in the bounding box and computes the exact
    /// overlap via `boxer()`.
    ///
    /// Reference: `STScI` cdrizzlebox.c `do_kernel_square`.
    fn distribute_square(
        &mut self,
        source: &FrameSource<'_>,
        quad: impl FnMut(InputPixel) -> Drop<DropQuad>,
    ) -> usize {
        self.scan(source, quad, |band, pixel, drop| {
            let min = drop
                .corners
                .iter()
                .copied()
                .fold(DVec2::splat(f64::INFINITY), DVec2::min);
            let max = drop
                .corners
                .iter()
                .copied()
                .fold(DVec2::splat(f64::NEG_INFINITY), DVec2::max);
            let Some(rows) = band.deposit_rows(min.y, max.y) else {
                return;
            };
            let cols = band.deposit_cols(min.x, max.x);
            if cols.is_empty() {
                return;
            }

            let fluxes = source.fluxes(pixel);
            for oy in rows {
                let base = band.row_base(oy);
                for ox in cols.clone() {
                    // `boxer` clips against the unit square, so it takes the cell's lower-left
                    // corner rather than its centre.
                    let corner = DVec2::new(ox as f64 - 0.5, oy as f64 - 0.5);
                    let overlap = boxer(corner, &drop.corners);
                    if overlap > 0.0 {
                        band.accumulate(&fluxes, base + ox, (overlap * drop.weight) as f32);
                    }
                }
            }
        })
    }

    /// Point kernel: fastest, needs good dithering.
    fn distribute_point(&mut self, source: &FrameSource<'_>) -> usize {
        self.scan(
            source,
            |pixel| source.droplet(pixel),
            |band, pixel, drop| {
                // All the flux lands in the pixel nearest the drop's centre, which is the single
                // row and column a zero-extent drop spans.
                let Some(rows) = band.deposit_rows(drop.centre.y, drop.centre.y) else {
                    return;
                };
                let cols = band.deposit_cols(drop.centre.x, drop.centre.x);
                if cols.is_empty() {
                    return;
                }

                let fluxes = source.fluxes(pixel);
                let index = band.row_base(rows.start) + cols.start;
                band.accumulate(&fluxes, index, drop.weight as f32);
            },
        )
    }

    /// A separable radial kernel, shared by Gaussian and Lanczos: `kernel(dx)·kernel(dy)` over the
    /// output pixels within `radius` of the drop's rounded centre, normalized so the weights sum
    /// to 1.
    ///
    /// The normalizer runs over the drop's **whole** neighbourhood, past the edge of the output
    /// grid included: it has to be the same number in every band, and flux that falls off the grid
    /// has to be lost rather than redistributed inward, which is what the compact kernels do and
    /// what leaves an edge pixel's weight recording how little of the drop actually landed.
    /// Separable, it is the product of the two axes' sums.
    fn distribute_radial(
        &mut self,
        source: &FrameSource<'_>,
        radius: isize,
        scratch: &mut RadialScratch,
        kernel: impl Fn(f32) -> f32,
    ) -> usize {
        let reach = radius as f64;
        self.scan(
            source,
            |pixel| source.droplet(pixel),
            |band, pixel, drop| {
                // Integer-center: output pixel `o` is centred at `o`, so the neighbourhood is the
                // `radius` pixels around the drop's rounded centre and the kernel distance is
                // `o - centre` with no offset.
                let nearest_row = drop.centre.y.round();
                let nearest_col = drop.centre.x.round();

                // Both tests come before a single tap is evaluated: a drop landing outside this
                // band — which every band's over-scan produces at its boundaries — must not build
                // one. They also bound the neighbourhood's centre to the grid, so the offsets below
                // cannot overflow.
                let Some(rows) = band.deposit_rows(nearest_row - reach, nearest_row + reach) else {
                    return;
                };
                if band
                    .deposit_cols(nearest_col - reach, nearest_col + reach)
                    .is_empty()
                {
                    return;
                }

                let centre_row = nearest_row as isize;
                let centre_col = nearest_col as isize;
                let offsets = -radius..=radius;
                scratch.columns.clear();
                scratch.columns.extend(
                    offsets
                        .clone()
                        .map(|dx| kernel(((centre_col + dx) as f64 - drop.centre.x) as f32)),
                );
                scratch.rows.clear();
                scratch.rows.extend(
                    offsets
                        .clone()
                        .map(|dy| kernel(((centre_row + dy) as f64 - drop.centre.y) as f32)),
                );
                let total = scratch.columns.iter().sum::<f32>() * scratch.rows.iter().sum::<f32>();
                if total.abs() < KERNEL_WEIGHT_MIN {
                    return;
                }
                let fluxes = source.fluxes(pixel);
                let normalizer = (drop.weight / f64::from(total)) as f32;
                for (oy, &row_value) in offsets.clone().map(|dy| centre_row + dy).zip(&scratch.rows)
                {
                    let Ok(oy) = usize::try_from(oy) else {
                        continue;
                    };
                    if !rows.contains(&oy) {
                        continue;
                    }
                    let base = band.row_base(oy);
                    for (ox, &column_value) in offsets
                        .clone()
                        .map(|dx| centre_col + dx)
                        .zip(&scratch.columns)
                    {
                        if let Ok(ox) = usize::try_from(ox)
                            && ox < band.width
                        {
                            band.accumulate(
                                &fluxes,
                                base + ox,
                                column_value * row_value * normalizer,
                            );
                        }
                    }
                }
            },
        )
    }

    /// The rows of a drop spanning `[first, last]` output rows that this band owns, or `None` when
    /// it owns none — the early skip that keeps a band's cost proportional to what reaches it.
    #[inline]
    #[expect(
        clippy::cast_sign_loss,
        reason = "each bound is held at 0 or above first, and the cast saturates at the top"
    )]
    fn deposit_rows(&self, first: f64, last: f64) -> Option<Range<usize>> {
        let start = (first.round().max(0.0) as usize).max(self.rows.start);
        let end = ((last.round() + 1.0).max(0.0) as usize).min(self.rows.end);
        (start < end).then_some(start..end)
    }

    /// The columns of a drop spanning `[first, last]` output columns. A band spans the full output
    /// width, so clamping to the band is clamping to the grid.
    #[inline]
    #[expect(
        clippy::cast_sign_loss,
        reason = "each bound is held at 0 or above first, and the cast saturates at the top"
    )]
    fn deposit_cols(&self, first: f64, last: f64) -> Range<usize> {
        let start = (first.round().max(0.0) as usize).min(self.width);
        let end = ((last.round() + 1.0).max(0.0) as usize).min(self.width);
        start..end
    }

    /// Index of the first pixel of absolute output row `oy` within this band's slices.
    #[inline]
    fn row_base(&self, oy: usize) -> usize {
        debug_assert!(self.rows.contains(&oy), "deposit outside the band");
        (oy - self.rows.start) * self.width
    }

    /// Accumulate one input pixel's `fluxes` into the band pixel at `index`.
    ///
    /// The flux values travel rather than the coordinate they came from: with one index in the
    /// signature there is no input/output pair to mix up, and the samples are read once per drop
    /// instead of once per output pixel it covers.
    ///
    /// The flags stay tested per deposit although a frame fixes whether it has any: the test is on
    /// a value the loop cannot change, so it hoists.
    #[inline]
    fn accumulate(&mut self, fluxes: &Fluxes, index: usize, weight: f32) {
        // A pixel whose channels all deposit shares one weight plane; a photosite deposits its one
        // sample, and its weight, in its colour's.
        let weights = match fluxes.colour {
            None => {
                for (plane, &flux) in self.planes.data.iter_mut().zip(&fluxes.values) {
                    plane[index] += flux * weight;
                }
                0
            }
            Some(colour) => {
                self.planes.data[colour][index] += fluxes.values[0] * weight;
                colour
            }
        };
        self.planes.weight[weights][index] += weight;
        self.planes.weight_sq[weights][index] += weight * weight;
        // A tap of zero weight, where a Lanczos lobe crosses zero, deposits nothing and carries
        // nothing.
        if weight != 0.0
            && let Some(flags) = &mut self.planes.flags
        {
            flags[index] |= fluxes.carried;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drizzle::deposit::Deposit;
    use crate::internals::prelude::*;
    use crate::registration::distortion::sip::SipPolynomial;
    use crate::registration::transform::{Transform, WarpTransform};

    /// The corner lattice gives every drop at pixfrac 1 the quadrilateral it would map itself, bit
    /// for bit — corners, weight, and the failures of a SIP inverse alike — whatever order its
    /// drops are asked for in: rows forward, as the scan walks them, and a column at a time, which
    /// leaves every lattice row half stale. Over a homography, whose corners each divide by their
    /// own denominator, and a SIP warp whose field `−0.05·d²` folds 10 pixels right of the centre,
    /// so its inverse fails at the input pixels past the fold.
    #[test]
    fn the_corner_lattice_maps_the_corners_each_drop_would() {
        let size = Size2us::new(24, 16);
        let image = gray_image(size, vec![1.0; size.pixel_count()]);
        let centre = DVec2::new(12.0, 8.0);
        let reference: Vec<DVec2> = (0..size.height)
            .flat_map(|y| (0..size.width).map(move |x| DVec2::new(x as f64, y as f64)))
            .collect();
        let bend = |r: DVec2| {
            let d = r - centre;
            r + DVec2::new(-0.05 * d.x * d.x, 0.004 * d.x * d.y)
        };
        let target: Vec<DVec2> = reference.iter().map(|&r| bend(r)).collect();
        let identity = Transform::identity();
        let sip = SipPolynomial::fitted_under(&identity, &reference, &target, 3, centre);
        let warps = [
            (
                "homography",
                WarpTransform::new(Transform::homography([
                    1.01, 0.02, 1.5, -0.01, 0.99, -0.7, 3e-4, -2e-4,
                ])),
            ),
            ("sip", WarpTransform::with_sip(identity, sip)),
        ];
        let same = |a: Drop<DropQuad>, b: Drop<DropQuad>| match (a, b) {
            (Drop::Landed(a), Drop::Landed(b)) => {
                a.corners.map(|c| c.to_array().map(f64::to_bits))
                    == b.corners.map(|c| c.to_array().map(f64::to_bits))
                    && a.weight.to_bits() == b.weight.to_bits()
            }
            (Drop::Empty, Drop::Empty) | (Drop::Unconverged, Drop::Unconverged) => true,
            _ => false,
        };
        for (name, warp) in warps {
            let source = FrameSource::new(&image, Deposit::of(&image), &warp, 2.0, None);
            let pixel = |x: usize, y: usize| InputPixel {
                position: Vec2us::new(x, y),
                index: y * size.width + x,
            };
            let rows_first = (0..size.height).flat_map(|y| (0..size.width).map(move |x| (x, y)));
            let columns_first = (0..size.width).flat_map(|x| (0..size.height).map(move |y| (x, y)));
            let mut unconverged = 0;
            for (order, walk) in [
                ("rows", rows_first.collect::<Vec<_>>()),
                ("columns", columns_first.collect()),
            ] {
                let mut lattice = CornerLattice::default();
                lattice.reset(size.width);
                for (x, y) in walk {
                    let own = source.quad(pixel(x, y), 0.5);
                    unconverged += usize::from(matches!(own, Drop::Unconverged));
                    let shared = source.quad_of(pixel(x, y), |corner| {
                        let [dx, dy] = LATTICE_CORNERS[corner];
                        lattice.corner(&source, x + dx, y + dy)
                    });
                    assert!(same(own, shared), "{name} {order}: pixel ({x}, {y})");
                }
            }
            if name == "sip" {
                assert!(unconverged > 0, "the SIP fixture must fail somewhere");
            }
        }
    }
}
