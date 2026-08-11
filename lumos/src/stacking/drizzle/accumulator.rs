//! Pixel distribution and accumulation for drizzle reconstruction.

use std::ops::Range;

use arrayvec::ArrayVec;
use glam::{DVec2, Vec2};
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::error::FrameDimensionMismatch;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::math::lanczos;
use crate::math::rect::Rect;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::stacking::drizzle::config::{DrizzleConfig, DrizzleKernel};
use crate::stacking::drizzle::error::DrizzleError;
use crate::stacking::drizzle::geometry::{AreaMagnification, boxer};
use crate::stacking::registration::transform::Transform;
use crate::stacking::stack_product::StackProduct;
use crate::stacking::stack_product::coverage::Coverage;
use crate::stacking::stack_product::quality_map::QualityMap;

const MAX_CHANNELS: usize = 3;
/// Output bands per rayon worker — enough for work-stealing to even out bands that different amounts
/// of the input reach.
const BANDS_PER_WORKER: usize = 4;
/// Output rows of slack added to every band's input-row estimate, covering the rounding between a
/// drop's centre and the pixels it touches.
const BAND_ROW_SLACK: f64 = 2.0;
const JACOBIAN_MIN: f64 = 1e-30;
const KERNEL_WEIGHT_MIN: f32 = 1e-10;

/// One drizzle input and all metadata that must remain aligned with it.
#[derive(Debug, Clone)]
pub struct DrizzleFrame<T> {
    /// Image or path to load.
    pub source: T,
    /// Registration transform from input coordinates to the common reference grid.
    pub transform: Transform,
    /// Non-negative per-frame quality weight.
    pub weight: f32,
    /// Optional non-negative per-pixel quality weights with the same dimensions as the image.
    pub pixel_weight_map: Option<Buffer2<f32>>,
}

impl<T> DrizzleFrame<T> {
    /// Create an equally weighted frame without a per-pixel weight map.
    pub fn new(source: T, transform: Transform) -> Self {
        Self {
            source,
            transform,
            weight: 1.0,
            pixel_weight_map: None,
        }
    }
}

/// One output pixel a radial drop touches, with the kernel value there.
#[derive(Debug, Clone, Copy)]
struct KernelTap {
    output: Vec2us,
    value: f32,
}

/// Drizzle accumulator for building the output image.
#[derive(Debug)]
pub struct DrizzleAccumulator {
    input_dims: ImageDimensions,
    frames_added: usize,
    /// Accumulated weighted flux values (`Σ fluxᵢ·wᵢ`), one Buffer2 per channel.
    data: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    /// Accumulated drizzle weight `Σ wᵢ` per output pixel. Channel-independent (the per-pixel
    /// `wᵢ` is purely geometric × frame weight), so a single map serves all channels.
    weight: Buffer2<f32>,
    /// Accumulated squared weight `Σwᵢ²` per output pixel — drives the linear-variance factor
    /// (`Var = Σwᵢ²/(Σwᵢ)²` per unit input variance), which the correlation-suppressed image RMS
    /// understates.
    ///
    /// `None` when the config declines the variance plane, which is the one quality output with a
    /// cost beyond its own allocation: this is an output-grid plane resident for the whole run, and
    /// two more arithmetic operations at every deposit.
    weight_sq: Option<Buffer2<f32>>,
    /// How many frames deposited any flux at each output pixel, when the config asks for coverage.
    ///
    /// A frame reaches an output pixel through however many of its input pixels land on it, so this
    /// cannot simply count deposits — [`CoverageBand`] carries the "already counted this frame" marker
    /// that makes it one per frame.
    frame_counts: Option<Buffer2<f32>>,
    /// Configuration.
    config: DrizzleConfig,
}

/// One horizontal slice of the output grid: the rows it owns, and every accumulator restricted to
/// them.
///
/// The scatter cannot be parallelized over the input — neighbouring input pixels write overlapping
/// output pixels — so it is parallelized over the *output*. A band owns its rows exclusively, so the
/// deposits need no synchronisation; and because each output pixel belongs to exactly one band, and a
/// band walks its inputs in the same order the serial loop did, every output pixel accumulates its
/// contributions in the serial order. Bit-identical output whatever the band count is what makes this
/// safe for a science product, and `band_count_does_not_change_the_result` pins it.
#[derive(Debug)]
struct OutputBand<'a> {
    /// Absolute output rows this band owns.
    rows: Range<usize>,
    width: usize,
    /// Weighted flux per channel, this band's rows of each plane.
    data: ArrayVec<&'a mut [f32], MAX_CHANNELS>,
    weight: &'a mut [f32],
    weight_sq: Option<&'a mut [f32]>,
    coverage: Option<CoverageBand<'a>>,
}

/// The frame tally for one band's rows, and which of them this frame has been counted at.
#[derive(Debug)]
struct CoverageBand<'a> {
    counts: &'a mut [f32],
    /// One bit per pixel of `counts`, indexed identically — band-local, so it needs no padding and
    /// no coordinates, just the index the deposit already computed. Fresh per band per frame, which
    /// is what "this frame has not been counted here yet" means.
    touched: Vec<u64>,
}

impl CoverageBand<'_> {
    /// Count the frame in progress at `index` unless it is counted there already.
    #[inline]
    fn touch(&mut self, index: usize) {
        let mask = 1u64 << (index % u64::BITS as usize);
        let word = &mut self.touched[index / u64::BITS as usize];
        if *word & mask == 0 {
            *word |= mask;
            self.counts[index] += 1.0;
        }
    }
}

/// The frame being distributed, as every band sees it: identical for all of them, and read-only.
#[derive(Debug)]
struct FrameSource<'a> {
    image: &'a LinearImage,
    /// Input pixels to output-grid coordinates, drizzle scale included.
    to_output: Transform,
    /// [`Self::to_output`] inverted, for turning a band's output rows back into input rows.
    to_input: Transform,
    magnification: AreaMagnification,
    weight: f32,
    pixel_weights: Option<&'a Buffer2<f32>>,
    output: Size2us,
}

impl FrameSource<'_> {
    #[inline]
    fn pixel_weight(&self, input: Vec2us) -> f32 {
        self.pixel_weights
            .map_or(1.0, |weights| weights[(input.x, input.y)])
    }

    /// Where `input`'s centre lands on the output grid.
    #[inline]
    fn project(&self, input: Vec2us) -> DVec2 {
        self.to_output
            .apply(DVec2::new(input.x as f64, input.y as f64))
    }

    /// The input rows whose drops can reach `band`, given the drop's half-extent in output rows.
    ///
    /// The band's output rows, widened by `margin` and taken back through the inverse transform: a
    /// straight-line map takes the rectangle to a quadrilateral, so its corners bound the input rows
    /// involved. Deliberately generous — over-scanning costs one transform and a rejected row test
    /// per pixel, measured at ~0.9 ns against ~100 ns for a deposit, while under-scanning would drop
    /// flux.
    fn input_rows(&self, band: &OutputBand, margin: f64) -> Range<usize> {
        let low = band.rows.start as f64 - margin;
        let high = band.rows.end as f64 - 1.0 + margin;
        let right = self.output.width as f64 - 1.0;
        let mut first = f64::INFINITY;
        let mut last = f64::NEG_INFINITY;
        for corner in [
            DVec2::new(0.0, low),
            DVec2::new(right, low),
            DVec2::new(0.0, high),
            DVec2::new(right, high),
        ] {
            let input = self.to_input.apply(corner);
            first = first.min(input.y);
            last = last.max(input.y);
        }

        // Saturating float casts, so a degenerate inverse (non-finite corners) yields an empty range
        // rather than a wild one.
        let height = self.image.height();
        let start = (first.floor().max(0.0) as usize).min(height);
        let end = ((last.ceil() + 1.0).max(0.0) as usize).min(height);
        start..end.max(start)
    }
}

impl OutputBand<'_> {
    /// Turbo kernel: an axis-aligned rectangular drop.
    fn distribute_turbo(&mut self, source: &FrameSource, drop_size: f32) {
        let half_drop = drop_size / 2.0;
        let inv_area = 1.0 / (drop_size * drop_size);
        let input_width = source.image.width();

        for iy in source.input_rows(self, f64::from(half_drop) + BAND_ROW_SLACK) {
            for ix in 0..input_width {
                let input = Vec2us::new(ix, iy);
                let pw = source.pixel_weight(input);
                if pw == 0.0 {
                    continue;
                }

                // Integer-center throughout: input pixel `i` is at coordinate `i` (matching
                // star centroids / `register` / `warp`), and output pixel `o` is the cell
                // `[o - 0.5, o + 0.5)`. The drop center needs no coordinate adjustment.
                let t = source.project(input);
                let ox_center = t.x as f32;
                let oy_center = t.y as f32;

                let jaco = source.magnification.at(t, input) as f32;
                if jaco < JACOBIAN_MIN as f32 {
                    continue;
                }

                // Output pixel `o` is the cell `[o - 0.5, o + 0.5)`, so the drop touches the
                // pixels `round(min) ..= round(max)` (the `overlap > 0.0` test below drops any
                // boundary cell that doesn't actually touch).
                let ox_min = (ox_center - half_drop).round().max(0.0) as usize;
                let ox_max = ((ox_center + half_drop).round() + 1.0).min(source.output.width as f32)
                    as usize;
                let Some(rows) = self.deposit_rows(oy_center - half_drop, oy_center + half_drop)
                else {
                    continue;
                };
                let drop =
                    Rect::from_center_half_extent(Vec2::new(ox_center, oy_center), half_drop);

                let effective_weight = source.weight * pw / jaco;
                for oy in rows {
                    for ox in ox_min..ox_max {
                        let cell =
                            Rect::from_center_half_extent(Vec2::new(ox as f32, oy as f32), 0.5);
                        let overlap = drop.overlap_area(cell);

                        if overlap > 0.0 {
                            let pixel_weight = effective_weight * overlap * inv_area;
                            self.accumulate(source.image, input, Vec2us::new(ox, oy), pixel_weight);
                        }
                    }
                }
            }
        }
    }

    /// Square kernel: true polygon clipping.
    ///
    /// For each input pixel, transforms all 4 corners of the (pixfrac-shrunken) drop
    /// to output coordinates, computes the Jacobian (signed area of the output
    /// quadrilateral), then iterates output pixels in the bounding box and computes
    /// exact overlap via `boxer()`.
    ///
    /// Reference: STScI cdrizzlebox.c `do_kernel_square`.
    fn distribute_square(&mut self, source: &FrameSource, pixfrac: f32) {
        let dh = 0.5 * pixfrac as f64;
        let input_width = source.image.width();
        // The drop's reach in output rows: a pixel-sized box mapped out spans at most `dh` of each
        // linear column, and the doubling covers a projective transform's local stretch.
        let m = source.to_output.matrix();
        let margin = 2.0 * dh * (m[3].abs() + m[4].abs()) + BAND_ROW_SLACK;

        for iy in source.input_rows(self, margin) {
            for ix in 0..input_width {
                let input = Vec2us::new(ix, iy);
                let pw = source.pixel_weight(input);
                if pw == 0.0 {
                    continue;
                }

                // Compute 4 corners of the shrunken drop in input space. Input pixel (ix, iy)
                // is integer-center (center at (ix, iy), matching centroids / warp).
                // Winding order: BL, BR, TR, TL (counterclockwise).
                let center = DVec2::new(ix as f64, iy as f64);
                let corners_in = [
                    center + DVec2::new(-dh, -dh),
                    center + DVec2::new(dh, -dh),
                    center + DVec2::new(dh, dh),
                    center + DVec2::new(-dh, dh),
                ];

                // Transform the 4 corners to integer-center output coordinates (output pixel
                // `o` is centered at `o`); `boxer` is given each cell as `[o - 0.5, o + 0.5)`.
                let quad = corners_in.map(|corner| source.to_output.apply(corner));

                // Jacobian: signed area of the output quadrilateral, from the cross product of its
                // diagonals.
                let jaco = 0.5 * (quad[1] - quad[3]).perp_dot(quad[0] - quad[2]);
                let abs_jaco = jaco.abs();
                if abs_jaco < JACOBIAN_MIN {
                    continue; // Degenerate quadrilateral
                }

                // Bounding box of the output quadrilateral
                let min = quad
                    .iter()
                    .copied()
                    .fold(DVec2::splat(f64::INFINITY), DVec2::min);
                let max = quad
                    .iter()
                    .copied()
                    .fold(DVec2::splat(f64::NEG_INFINITY), DVec2::max);

                // Output pixel `o` is the cell `[o - 0.5, o + 0.5)`, so the quad bbox touches
                // pixels `round(min) ..= round(max)`.
                let ox_min = min.x.round().max(0.0) as usize;
                let ox_max = (max.x.round() + 1.0).min(source.output.width as f64) as usize;
                let Some(rows) = self.deposit_rows(min.y as f32, max.y as f32) else {
                    continue;
                };

                let effective_weight = source.weight as f64 * pw as f64;
                let w_over_jaco = effective_weight / abs_jaco;

                for oy in rows {
                    for ox in ox_min..ox_max {
                        let corner = DVec2::new(ox as f64 - 0.5, oy as f64 - 0.5);
                        let overlap = boxer(corner, &quad);
                        if overlap > 0.0 {
                            let pixel_weight = (overlap * w_over_jaco) as f32;
                            self.accumulate(source.image, input, Vec2us::new(ox, oy), pixel_weight);
                        }
                    }
                }
            }
        }
    }

    /// Point kernel: fastest, needs good dithering.
    fn distribute_point(&mut self, source: &FrameSource) {
        let input_width = source.image.width();

        for iy in source.input_rows(self, BAND_ROW_SLACK) {
            for ix in 0..input_width {
                let input = Vec2us::new(ix, iy);
                let pw = source.pixel_weight(input);
                if pw == 0.0 {
                    continue;
                }

                // Integer-center input; flux lands in the nearest output pixel.
                let t = source.project(input);
                let ox = t.x.round() as isize;
                let oy = t.y.round() as isize;

                if ox < 0 || ox >= source.output.width as isize || !self.owns_row(oy) {
                    continue;
                }
                let jaco = source.magnification.at(t, input) as f32;
                if jaco < JACOBIAN_MIN as f32 {
                    continue;
                }
                let output = Vec2us::new(ox as usize, oy as usize);
                self.accumulate(source.image, input, output, source.weight * pw / jaco);
            }
        }
    }

    /// A radial kernel with two-pass normalization, shared by Gaussian and Lanczos.
    ///
    /// Both iterate output pixels within `radius` of the transformed center, compute a per-pixel
    /// weight via `kernel_fn(dx, dy)`, normalize so the weights sum to 1, then accumulate.
    ///
    /// The tap sum runs over the drop's **whole** neighbourhood, not just the part inside this band:
    /// the normalizer has to be the same number every band, or a drop straddling a boundary would be
    /// scaled differently on each side of it.
    fn distribute_radial(
        &mut self,
        source: &FrameSource,
        radius: isize,
        kernel_fn: impl Fn(f32, f32) -> f32,
    ) {
        let output_width = source.output.width as isize;
        let output_height = source.output.height as isize;
        let input_width = source.image.width();
        // `scale` is unbounded by config, so the neighbourhood is sized at run time rather than on
        // the stack. One allocation for the whole band.
        let side = (2 * radius + 1) as usize;
        let mut taps: Vec<KernelTap> = Vec::with_capacity(side * side);

        for iy in source.input_rows(self, radius as f64 + BAND_ROW_SLACK) {
            for ix in 0..input_width {
                let input = Vec2us::new(ix, iy);
                let pw = source.pixel_weight(input);
                if pw == 0.0 {
                    continue;
                }

                // Integer-center: output pixel `o` is centred at `o`, so the kernel distance
                // is `o - ox_center` with no offset.
                let t = source.project(input);
                let ox_center = t.x as f32;
                let oy_center = t.y as f32;

                let jaco = source.magnification.at(t, input) as f32;
                if jaco < JACOBIAN_MIN as f32 {
                    continue;
                }

                let fluxes = Self::fluxes_at(source.image, input);
                let ox_int = ox_center.round() as isize;
                let oy_int = oy_center.round() as isize;

                // The kernel must be summed before it can be normalised, so the neighbourhood is
                // visited twice. The taps are kept from the first visit rather than recomputed on
                // the second: `kernel_fn` is an `exp` for Gaussian and two `sinc`s for Lanczos, and
                // that evaluation is the dominant cost here. `taps` is allocated once per frame and
                // refilled in place.
                taps.clear();
                let mut total_weight = 0.0f32;
                for dy in -radius..=radius {
                    let oy = oy_int + dy;
                    if oy < 0 || oy >= output_height {
                        continue;
                    }
                    for dx in -radius..=radius {
                        let ox = ox_int + dx;
                        if ox < 0 || ox >= output_width {
                            continue;
                        }
                        let dist_x = ox as f32 - ox_center;
                        let dist_y = oy as f32 - oy_center;
                        let tap = kernel_fn(dist_x, dist_y);
                        total_weight += tap;
                        taps.push(KernelTap {
                            output: Vec2us::new(ox as usize, oy as usize),
                            value: tap,
                        });
                    }
                }

                if total_weight.abs() < KERNEL_WEIGHT_MIN {
                    continue;
                }

                // Distribute flux with normalized weights. Per-pixel weight scales the effective
                // frame weight; the Jacobian divides out the local area magnification.
                let inv_total = (source.weight * pw) / (total_weight * jaco);
                for tap in &taps {
                    if self.rows.contains(&tap.output.y) {
                        self.accumulate_samples(&fluxes, tap.output, tap.value * inv_total);
                    }
                }
            }
        }
    }

    /// The rows of a drop spanning `[first, last]` output rows that belong to this band, or `None`
    /// when none do — the early skip that keeps a band's cost proportional to what reaches it.
    #[inline]
    fn deposit_rows(&self, first: f32, last: f32) -> Option<Range<usize>> {
        let start = (first.round().max(0.0) as usize).max(self.rows.start);
        let end = ((last.round() + 1.0).max(0.0) as usize).min(self.rows.end);
        (start < end).then_some(start..end)
    }

    #[inline]
    fn owns_row(&self, oy: isize) -> bool {
        oy >= 0 && self.rows.contains(&(oy as usize))
    }

    /// One input pixel's samples, one per channel.
    ///
    /// Only the radial path pre-reads these. Its drop covers `(2r+1)²` output pixels, so reading
    /// per output pixel repeats the same lookup a hundred times over; the compact kernels cover a
    /// handful and measured *slower* when made to build this first — the loads they repeat are
    /// already hoisted.
    #[inline]
    fn fluxes_at(image: &LinearImage, pixel: Vec2us) -> ArrayVec<f32, MAX_CHANNELS> {
        (0..image.channels())
            .map(|c| image.channel(c)[(pixel.x, pixel.y)])
            .collect()
    }

    /// Accumulate weighted flux from `input` into the `output` pixel.
    ///
    /// Both coordinates travel as one value each: four bare indices in a row let an input/output
    /// mix-up compile.
    #[inline]
    fn accumulate(
        &mut self,
        image: &LinearImage,
        input: Vec2us,
        output: Vec2us,
        pixel_weight: f32,
    ) {
        let local = self.local_index(output);
        for (c, plane) in self.data.iter_mut().enumerate() {
            plane[local] += image.channel(c)[(input.x, input.y)] * pixel_weight;
        }
        // Weight is channel-independent, so accumulate it and its square once per output pixel.
        self.weight[local] += pixel_weight;
        if let Some(weight_sq) = &mut self.weight_sq {
            weight_sq[local] += pixel_weight * pixel_weight;
        }
        self.count_frame(local);
    }

    /// Accumulate already-read `fluxes` into the `output` pixel.
    ///
    /// The two weight lines are repeated from [`Self::accumulate`] rather than shared through a
    /// third method: factoring them out measured ~4% slower on the compact kernels, whose hot loop
    /// is this function.
    #[inline]
    fn accumulate_samples(&mut self, fluxes: &[f32], output: Vec2us, pixel_weight: f32) {
        let local = self.local_index(output);
        for (plane, &flux) in self.data.iter_mut().zip(fluxes) {
            plane[local] += flux * pixel_weight;
        }
        self.weight[local] += pixel_weight;
        if let Some(weight_sq) = &mut self.weight_sq {
            weight_sq[local] += pixel_weight * pixel_weight;
        }
        self.count_frame(local);
    }

    /// Index of an absolute `output` pixel within this band's slices.
    #[inline]
    fn local_index(&self, output: Vec2us) -> usize {
        debug_assert!(self.rows.contains(&output.y), "deposit outside the band");
        (output.y - self.rows.start) * self.width + output.x
    }

    #[inline]
    fn count_frame(&mut self, local: usize) {
        if let Some(coverage) = &mut self.coverage {
            coverage.touch(local);
        }
    }
}

impl DrizzleAccumulator {
    /// Create a new drizzle accumulator for the given input dimensions.
    ///
    /// # Errors
    ///
    /// Returns an error when `config` is invalid.
    pub fn new(input_dims: ImageDimensions, config: DrizzleConfig) -> Result<Self, DrizzleError> {
        config.validate()?;
        let output_width = (input_dims.width() as f32 * config.scale).ceil() as usize;
        let output_height = (input_dims.height() as f32 * config.scale).ceil() as usize;

        let mut data = ArrayVec::new();
        for _ in 0..input_dims.channels() {
            data.push(Buffer2::new_default(output_width, output_height));
        }

        Ok(Self {
            input_dims,
            frames_added: 0,
            data,
            weight: Buffer2::new_default(output_width, output_height),
            weight_sq: config
                .quality
                .variance
                .then(|| Buffer2::new_default(output_width, output_height)),
            frame_counts: config
                .quality
                .coverage
                .then(|| Buffer2::new_default(output_width, output_height)),
            config,
        })
    }

    fn width(&self) -> usize {
        self.data[0].width()
    }

    fn height(&self) -> usize {
        self.data[0].height()
    }

    fn channels(&self) -> usize {
        self.data.len()
    }

    /// Output dimensions.
    pub fn dimensions(&self) -> ImageDimensions {
        ImageDimensions::new((self.width(), self.height()), self.channels())
    }

    /// Validate and add one coherent frame to the accumulator.
    ///
    /// # Errors
    ///
    /// Returns an error when image dimensions differ from the accumulator, or when frame or pixel
    /// weights are negative or non-finite. The accumulator is unchanged on error.
    pub fn add_frame(&mut self, frame: DrizzleFrame<LinearImage>) -> Result<(), DrizzleError> {
        let index = self.frames_added;
        FrameDimensionMismatch::check(index, self.input_dims, frame.source.dimensions())?;
        if !frame.weight.is_finite() || frame.weight < 0.0 {
            return Err(DrizzleError::InvalidFrameWeight {
                index,
                value: frame.weight,
            });
        }
        if let Some(pixel_weights) = &frame.pixel_weight_map {
            if (pixel_weights.width(), pixel_weights.height())
                != (self.input_dims.width(), self.input_dims.height())
            {
                return Err(DrizzleError::PixelWeightDimensionMismatch {
                    index,
                    expected_width: self.input_dims.width(),
                    expected_height: self.input_dims.height(),
                    actual_width: pixel_weights.width(),
                    actual_height: pixel_weights.height(),
                });
            }
            if let Some((pixel_index, &value)) = pixel_weights
                .pixels()
                .iter()
                .enumerate()
                .find(|(_, value)| !value.is_finite() || **value < 0.0)
            {
                return Err(DrizzleError::InvalidPixelWeight {
                    frame_index: index,
                    pixel_index,
                    value,
                });
            }
        }

        self.accumulate_image(
            frame.source,
            &frame.transform,
            frame.weight,
            frame.pixel_weight_map.as_ref(),
        );
        Ok(())
    }

    fn accumulate_image(
        &mut self,
        image: LinearImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
    ) {
        self.accumulate_image_banded(image, transform, weight, pixel_weights, None);
    }

    /// [`Self::accumulate_image`] with the band height overridable, for the band-invariance test.
    fn accumulate_image_banded(
        &mut self,
        image: LinearImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
        band_rows: Option<usize>,
    ) {
        let n_channels = self.channels();
        assert_eq!(
            image.channels(),
            n_channels,
            "Channel count mismatch: expected {}, got {}",
            n_channels,
            image.channels()
        );
        // Here rather than in `add_frame`: this is the one place a frame is accumulated, and the
        // coverage tally would otherwise miss any caller that reaches it directly.
        self.frames_added += 1;

        if let Some(pw) = pixel_weights {
            assert_eq!(
                (pw.width(), pw.height()),
                (image.width(), image.height()),
                "Pixel weight map dimensions ({}x{}) must match image ({}x{})",
                pw.width(),
                pw.height(),
                image.width(),
                image.height()
            );
        }

        let scale = self.config.scale;
        let pixfrac = self.config.pixfrac;
        let kernel = self.config.kernel;
        // The output grid's scale composed into the transform, so the kernels map an input pixel
        // straight to output coordinates instead of transforming and then scaling both components,
        // and the area magnification is the composed determinant with no `scale²` left to apply.
        let to_output = Transform::scale(DVec2::splat(scale as f64)).compose(transform);
        // Drop size in output pixels: pixfrac is the fraction of input pixel size,
        // and each input pixel maps to `scale` output pixels, so drop = pixfrac * scale.
        // (STScI: pfo = pixel_fraction / pscale_ratio / 2, where pscale_ratio = 1/scale)
        let drop_size = pixfrac * scale;

        if kernel == DrizzleKernel::Lanczos
            && ((pixfrac - 1.0).abs() > f32::EPSILON || (scale - 1.0).abs() > f32::EPSILON)
        {
            // Per STScI DrizzlePac: Lanczos "should never be used for pixfrac != 1.0,
            // and is not recommended for scale != 1.0."
            tracing::warn!(
                pixfrac,
                scale,
                "Lanczos kernel should only be used with pixfrac=1.0 and scale=1.0"
            );
        }

        let source = FrameSource {
            image: &image,
            to_output,
            to_input: to_output.inverse(),
            magnification: AreaMagnification::new(&to_output),
            weight,
            pixel_weights,
            output: Size2us::new(self.width(), self.height()),
        };

        let band_rows = band_rows.unwrap_or_else(|| Self::band_rows(self.height()));
        self.distribute(&source, kernel, pixfrac, drop_size, band_rows);
    }

    /// Distribute one frame across `band_rows`-tall output bands, in parallel.
    ///
    /// The band count is a parameter so a test can compare one band against many: the result must be
    /// bit-identical either way, which is the property that makes parallelizing a float accumulation
    /// acceptable here.
    fn distribute(
        &mut self,
        source: &FrameSource,
        kernel: DrizzleKernel,
        pixfrac: f32,
        drop_size: f32,
        band_rows: usize,
    ) {
        let mut bands = self.bands(band_rows);
        bands.par_iter_mut().for_each(|band| match kernel {
            DrizzleKernel::Square => band.distribute_square(source, pixfrac),
            DrizzleKernel::Turbo => band.distribute_turbo(source, drop_size),
            DrizzleKernel::Point => band.distribute_point(source),
            DrizzleKernel::Gaussian => {
                // Per STScI: Gaussian FWHM = drop_size in output pixels.
                // sigma = FWHM / (2 * sqrt(2 * ln(2))) = FWHM / 2.3548
                let sigma = drop_size / 2.3548;
                let inv_2sigma_sq = 1.0 / (2.0 * sigma * sigma);
                band.distribute_radial(source, (3.0 * sigma).ceil() as isize, |dx, dy| {
                    let dist_sq = dx * dx + dy * dy;
                    (-dist_sq * inv_2sigma_sq).exp()
                });
            }
            DrizzleKernel::Lanczos => {
                // Lanczos-3: support radius 3, kernel defined on [-3, 3].
                let a = 3.0f32;
                band.distribute_radial(source, a as isize, |dx, dy| {
                    lanczos::kernel(dx, a) * lanczos::kernel(dy, a)
                });
            }
        });
    }

    /// How many output rows one band covers.
    ///
    /// Several bands per worker so rayon can steal: a band's cost varies with how much of the input
    /// actually reaches it, which is not uniform once the transform rotates. Not tuned against the
    /// margin — over-scanning at a band boundary is nearly free (see [`FrameSource::input_rows`]), so
    /// there is nothing to trade off against balance.
    fn band_rows(output_height: usize) -> usize {
        let target = rayon::current_num_threads() * BANDS_PER_WORKER;
        output_height.div_ceil(target.max(1)).max(1)
    }

    /// The output planes split into `band_rows`-tall horizontal slices, each owning its rows.
    ///
    /// Every accumulator is output-sized and splits on the same row boundaries, so the bands are
    /// disjoint by construction.
    fn bands(&mut self, band_rows: usize) -> Vec<OutputBand<'_>> {
        debug_assert!(band_rows > 0);
        let width = self.width();
        let height = self.height();
        // Every plane is output-sized, so one chunk length splits all of them the same way.
        let band_len = width * band_rows;

        let mut channels: ArrayVec<_, MAX_CHANNELS> = self
            .data
            .iter_mut()
            .map(|plane| plane.pixels_mut().chunks_mut(band_len))
            .collect();
        let weight = self.weight.pixels_mut().chunks_mut(band_len);
        let mut weight_sq = self
            .weight_sq
            .as_mut()
            .map(|buffer| buffer.pixels_mut().chunks_mut(band_len));
        let mut counts = self
            .frame_counts
            .as_mut()
            .map(|buffer| buffer.pixels_mut().chunks_mut(band_len));

        weight
            .enumerate()
            .map(|(band, weight)| {
                let start = band * band_rows;
                OutputBand {
                    rows: start..(start + band_rows).min(height),
                    width,
                    data: channels
                        .iter_mut()
                        .map(|chunks| chunks.next().expect("one chunk per band per channel"))
                        .collect(),
                    weight,
                    weight_sq: weight_sq
                        .as_mut()
                        .map(|chunks| chunks.next().expect("one variance chunk per band")),
                    coverage: counts.as_mut().map(|chunks| {
                        let counts = chunks.next().expect("one coverage chunk per band");
                        CoverageBand {
                            touched: vec![0; counts.len().div_ceil(u64::BITS as usize)],
                            counts,
                        }
                    }),
                }
            })
            .collect()
    }

    /// Finalize the drizzle result: normalize flux by weight and emit whichever quality planes
    /// [`DrizzleConfig::quality`] asked for. The weight `Σwᵢ` is channel-independent, so one map
    /// normalizes every channel and seeds the quality outputs.
    ///
    /// The weight map is built whatever the request — the image is `Σfluxᵢwᵢ / Σwᵢ`, and
    /// `min_weight_fraction` gates fill against its maximum — so declining `weight` only declines
    /// handing it out, while declining `coverage` or `variance` skips an output-grid allocation
    /// each.
    pub fn finalize(self) -> StackProduct {
        let width = self.width();
        let height = self.height();
        let n_channels = self.channels();
        let needs_clamping = self.config.kernel == DrizzleKernel::Lanczos;
        let min_weight_fraction = self.config.min_weight_fraction;
        let fill_value = self.config.fill_value;

        let planes = self.config.quality;
        let weight_pixels = self.weight.pixels();

        // The fill gate is a share of the deepest pixel's weight, so it needs that maximum.
        let max_weight = weight_pixels
            .par_iter()
            .copied()
            .reduce(|| 0.0f32, f32::max);
        let weight_threshold = if max_weight > 0.0 {
            min_weight_fraction * max_weight
        } else {
            0.0
        };

        // Build per-channel output (row-parallel normalization by the shared weight). Bounded by
        // `MAX_CHANNELS` like `self.data`, which `n_channels` is the length of, so the collect
        // cannot overflow the capacity.
        let output_channels: ArrayVec<Vec<f32>, MAX_CHANNELS> = (0..n_channels)
            .map(|c| {
                let data_pixels = self.data[c].pixels();
                // Collected from the parallel iterator rather than pre-filled and overwritten: a
                // `vec![fill_value; width * height]` wrote every pixel once before this pass wrote
                // nearly all of them again, which at 6144x6144 is ~150 MB of dead stores per
                // channel. `fill_value` is now the uncovered branch rather than a survivor.
                (0..width * height)
                    .into_par_iter()
                    .map(|idx| {
                        let w = weight_pixels[idx];
                        if w > 0.0 && w >= weight_threshold {
                            let val = data_pixels[idx] / w;
                            if needs_clamping { val.max(0.0) } else { val }
                        } else {
                            fill_value
                        }
                    })
                    .collect()
            })
            .collect();

        // Linear output-variance factor: Var(O) = Σ(wᵢ²)/(Σwᵢ)². `0` where uncovered.
        let linear_variance = self.weight_sq.as_ref().map(|weight_sq| {
            let weight_sq_pixels = weight_sq.pixels();
            let mut linear_variance = Buffer2::new_default(width, height);
            linear_variance
                .pixels_mut()
                .par_iter_mut()
                .enumerate()
                .for_each(|(idx, v)| {
                    let w = weight_pixels[idx];
                    *v = if w > 0.0 {
                        weight_sq_pixels[idx] / (w * w)
                    } else {
                        0.0
                    };
                });
            QualityMap::Shared(linear_variance)
        });

        // Coverage is the share of frames that reached each pixel — the same quantity the
        // statistical combine reports, so a reader need not know which producer built the product.
        // It is deliberately *not* `weight / max_weight`: that answers how deep a pixel is relative
        // to the best-covered one in this particular run, which is a different question and remains
        // available as `weight` divided by its own maximum.
        let frames_added = self.frames_added;
        let coverage = self.frame_counts.map(|mut counts| {
            if frames_added > 0 {
                let inv_frames = 1.0 / frames_added as f32;
                counts
                    .pixels_mut()
                    .par_iter_mut()
                    .for_each(|count| *count *= inv_frames);
            }
            Coverage::PerPixel(counts)
        });

        let image = LinearImage::from_planar_channels(
            ImageDimensions::new((width, height), n_channels),
            output_channels,
        );
        StackProduct {
            image,
            coverage,
            weight: planes.weight.then_some(QualityMap::Shared(self.weight)),
            linear_variance,
            // Drizzle redistributes flux by geometry rather than combining aligned samples, so
            // there is no per-frame quantization step to propagate.
            quantization_sigma: None,
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::stacking::drizzle::accumulator::*;

    pub(crate) fn add_image(
        accumulator: &mut DrizzleAccumulator,
        image: LinearImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
    ) {
        accumulator.accumulate_image(image, transform, weight, pixel_weights);
    }

    /// [`add_image`] with the output band height pinned, so a test can compare band counts.
    pub(crate) fn add_image_with_band_rows(
        accumulator: &mut DrizzleAccumulator,
        image: LinearImage,
        transform: &Transform,
        band_rows: usize,
    ) {
        accumulator.accumulate_image_banded(image, transform, 1.0, None, Some(band_rows));
    }

    pub(crate) fn accumulated_flux_sum(accumulator: &DrizzleAccumulator, channel: usize) -> f32 {
        accumulator.data[channel].pixels().iter().sum()
    }
}
