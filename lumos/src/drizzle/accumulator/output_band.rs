//! One horizontal slice of the output grid, and the kernels that scatter flux into it.

use std::ops::Range;

use glam::DVec2;

use crate::concurrency::JobScratchPool;
use crate::drizzle::accumulator::PlaneSpan;
use crate::drizzle::accumulator::frame_source::{Drop, Fluxes, FrameSource, InputPixel};
use crate::drizzle::config::{DrizzleConfig, DrizzleKernel};
use crate::drizzle::geometry::boxer;
use crate::math::fwhm::FWHM_PER_SIGMA;
use crate::math::lanczos;
use crate::math::vec2us::Vec2us;

/// Output rows of slack on every kernel's input-row estimate. [`OutputBand::deposit_rows`] rounds a
/// drop's extent to the nearest row, so a drop stopping half a row short of the band still reaches
/// it; nothing else separates a drop's centre from the rows it touches.
const ROW_ROUNDING_SLACK: f64 = 0.5;
/// A drop whose kernel taps sum to less than this has no normalizer worth dividing by: the radial
/// kernels' taps sum to about the drop's area in output pixels, so this is far below any drop.
const KERNEL_WEIGHT_MIN: f32 = 1e-10;
/// Where the Gaussian is truncated, in σ — it has fallen to ~1% of its peak by there.
const GAUSSIAN_RADIUS_SIGMAS: f64 = 3.0;
/// The drizzle Lanczos kernel is `STScI`'s Lanczos-3: support radius 3, defined on [-3, 3].
const LANCZOS_A: f32 = 3.0;

/// One radial drop's kernel along each axis, one value per output column and per output row of
/// its neighbourhood. Both kernels are separable — the Gaussian is a product of one-axis
/// Gaussians and the Lanczos kernel is defined as one — so a `(2r + 1)²` neighbourhood needs
/// `2·(2r + 1)` evaluations rather than one per tap, and the same values exactly.
#[derive(Debug, Default)]
pub(super) struct RadialScratch {
    columns: Vec<f32>,
    rows: Vec<f32>,
}

/// Everything a kernel needs beyond the frame, resolved once per run.
///
/// Every field is a function of [`DrizzleConfig`] alone, so resolving it here is what keeps a band
/// from recomputing an `exp`'s σ, or a drop's area, on each of the thousands of frames × bands the
/// scatter is split into.
#[derive(Debug, Clone, Copy)]
pub(super) enum KernelPlan {
    /// Half the drop's side, in *input* pixels: the square kernel shrinks the input pixel and then
    /// maps its corners.
    Square {
        half_drop: f64,
    },
    /// Half the drop's side and the reciprocal of its area, in *output* pixels.
    Turbo {
        half_drop: f64,
        inv_area: f64,
    },
    Point,
    Gaussian {
        radius: isize,
        inv_2sigma_sq: f32,
    },
    Lanczos {
        radius: isize,
    },
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
    /// One bit per pixel of the band, indexed identically to its planes — band-local, so it needs
    /// no padding and no coordinates, just the index the deposit already computed. Cleared per
    /// frame, which is what "this frame has not been counted here yet" means. Empty unless the
    /// config asks for coverage.
    touched: &'a mut [u64],
    /// Which band this is, and the input rows every band scans for this frame — this one's, and the
    /// earlier ones', which decide whether a pixel that failed to invert is this band's to count.
    index: usize,
    scans: &'a [Range<usize>],
}
impl KernelPlan {
    pub(super) fn new(config: &DrizzleConfig) -> Self {
        // Drop size in output pixels: pixfrac is the fraction of input pixel size, and each input
        // pixel maps to `scale` output pixels, so drop = pixfrac · scale. (STScI: pfo =
        // pixel_fraction / pscale_ratio / 2, where pscale_ratio = 1/scale.)
        let drop_size = f64::from(config.pixfrac) * f64::from(config.scale);
        match config.kernel {
            DrizzleKernel::Square => Self::Square {
                half_drop: 0.5 * f64::from(config.pixfrac),
            },
            DrizzleKernel::Turbo => Self::Turbo {
                half_drop: 0.5 * drop_size,
                inv_area: 1.0 / (drop_size * drop_size),
            },
            DrizzleKernel::Point => Self::Point,
            DrizzleKernel::Gaussian => {
                // Per `STScI` the Gaussian's FWHM is the drop size.
                let sigma = drop_size / FWHM_PER_SIGMA;
                Self::Gaussian {
                    radius: (GAUSSIAN_RADIUS_SIGMAS * sigma).ceil() as isize,
                    inv_2sigma_sq: (1.0 / (2.0 * sigma * sigma)) as f32,
                }
            }
            DrizzleKernel::Lanczos => Self::Lanczos {
                radius: LANCZOS_A as isize,
            },
        }
    }
}

impl KernelPlan {
    /// How far a drop reaches from its pixel's centre: in output rows, and in input rows — the
    /// square kernel's drop is a box in the input, mapped corner by corner. Each includes
    /// [`ROW_ROUNDING_SLACK`] where the deposit rounds an output extent to rows.
    pub(super) fn reach(self) -> DropReach {
        match self {
            Self::Square { half_drop } => DropReach {
                output_rows: ROW_ROUNDING_SLACK,
                input_rows: half_drop,
            },
            Self::Turbo { half_drop, .. } => DropReach {
                output_rows: half_drop + ROW_ROUNDING_SLACK,
                input_rows: 0.0,
            },
            Self::Point => DropReach {
                output_rows: ROW_ROUNDING_SLACK,
                input_rows: 0.0,
            },
            Self::Gaussian { radius, .. } | Self::Lanczos { radius } => DropReach {
                output_rows: radius as f64 + ROW_ROUNDING_SLACK,
                input_rows: 0.0,
            },
        }
    }
}

/// How far a drop reaches from its pixel's centre, in the two grids — see [`KernelPlan::reach`].
#[derive(Debug, Clone, Copy)]
pub(super) struct DropReach {
    pub(super) output_rows: f64,
    pub(super) input_rows: f64,
}

impl<'a> OutputBand<'a> {
    pub(super) const fn new(
        rows: Range<usize>,
        width: usize,
        planes: PlaneSpan<'a>,
        touched: &'a mut [u64],
        index: usize,
        scans: &'a [Range<usize>],
    ) -> Self {
        Self {
            rows,
            width,
            planes,
            touched,
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
    ) -> usize {
        // Here rather than where the band is built: this runs on a worker, so clearing the bitset
        // is spread across the pool instead of paid serially between frames.
        self.touched.fill(0);

        match plan {
            KernelPlan::Square { half_drop } => self.distribute_square(source, half_drop),
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
                self.distribute_radial(source, radius, &mut radial.acquire(), |d| {
                    lanczos::kernel(d, LANCZOS_A)
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
        drop: impl Fn(InputPixel) -> Drop<T>,
        mut deposit: impl FnMut(&mut Self, InputPixel, T),
    ) -> usize {
        let width = source.width();
        let mut unconverged = 0;
        for iy in self.scans[self.index].clone() {
            let row = iy * width;
            for ix in 0..width {
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
                                .any(|scan| scan.contains(&iy)),
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
    fn distribute_square(&mut self, source: &FrameSource<'_>, half_drop: f64) -> usize {
        self.scan(
            source,
            |pixel| source.quad(pixel, half_drop),
            |band, pixel, drop| {
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
            },
        )
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
    /// The two quality planes stay `Option`s tested per deposit although the config fixes them for
    /// the whole run: the test is on a value the loop cannot change, so it hoists, and
    /// monomorphizing the band over the four combinations to prove it would quadruple every kernel
    /// below.
    #[inline]
    fn accumulate(&mut self, fluxes: &Fluxes, index: usize, weight: f32) {
        for (plane, &flux) in self.planes.data.iter_mut().zip(fluxes.iter()) {
            plane[index] += flux * weight;
        }
        // Weight is channel-independent, so accumulate it and its square once per output pixel.
        self.planes.weight[index] += weight;
        if let Some(weight_sq) = &mut self.planes.weight_sq {
            weight_sq[index] += weight * weight;
        }

        // A frame reaches an output pixel through however many of its input pixels land on it, so
        // coverage cannot simply count deposits; the bitmap is what makes it one per frame.
        if let Some(counts) = &mut self.planes.counts {
            let mask = 1u64 << (index % u64::BITS as usize);
            let word = &mut self.touched[index / u64::BITS as usize];
            if *word & mask == 0 {
                *word |= mask;
                counts[index] += 1.0;
            }
        }
    }
}
