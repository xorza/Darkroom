//! Pixel distribution and accumulation for drizzle reconstruction.

use arrayvec::ArrayVec;
use glam::{DVec2, Vec2};
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::math::rect::Rect;
use crate::stacking::drizzle::config::{DrizzleConfig, DrizzleKernel};
use crate::stacking::drizzle::error::DrizzleError;
use crate::stacking::drizzle::geometry::{boxer, lanczos_kernel, local_jacobian};
use crate::stacking::registration::transform::Transform;
use crate::stacking::stack_product::StackProduct;
use crate::stacking::stack_product::coverage::Coverage;
use crate::stacking::stack_product::quality_map::QualityMap;

const MAX_CHANNELS: usize = 3;
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
    ox: usize,
    oy: usize,
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
    /// Configuration.
    config: DrizzleConfig,
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
        let dimensions = frame.source.dimensions();
        if dimensions != self.input_dims {
            return Err(DrizzleError::DimensionMismatch {
                index,
                expected: self.input_dims,
                actual: dimensions,
            });
        }
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
        self.frames_added += 1;
        Ok(())
    }

    fn accumulate_image(
        &mut self,
        image: LinearImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
    ) {
        let n_channels = self.channels();
        assert_eq!(
            image.channels(),
            n_channels,
            "Channel count mismatch: expected {}, got {}",
            n_channels,
            image.channels()
        );

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
        // Drop size in output pixels: pixfrac is the fraction of input pixel size,
        // and each input pixel maps to `scale` output pixels, so drop = pixfrac * scale.
        // (STScI: pfo = pixel_fraction / pscale_ratio / 2, where pscale_ratio = 1/scale)
        let drop_size = pixfrac * scale;

        if self.config.kernel == DrizzleKernel::Lanczos
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

        match self.config.kernel {
            DrizzleKernel::Square => {
                self.add_image_square(&image, transform, weight, pixel_weights, scale);
            }
            DrizzleKernel::Turbo => {
                self.add_image_turbo(&image, transform, weight, pixel_weights, scale, drop_size);
            }
            DrizzleKernel::Point => {
                self.add_image_point(&image, transform, weight, pixel_weights, scale);
            }
            DrizzleKernel::Gaussian => {
                // Per STScI: Gaussian FWHM = drop_size in output pixels.
                // sigma = FWHM / (2 * sqrt(2 * ln(2))) = FWHM / 2.3548
                let sigma = drop_size / 2.3548;
                let radius = (3.0 * sigma).ceil() as isize;
                let inv_2sigma_sq = 1.0 / (2.0 * sigma * sigma);
                self.add_image_radial(
                    &image,
                    transform,
                    weight,
                    pixel_weights,
                    scale,
                    radius,
                    |dx, dy| {
                        let dist_sq = dx * dx + dy * dy;
                        (-dist_sq * inv_2sigma_sq).exp()
                    },
                );
            }
            DrizzleKernel::Lanczos => {
                // Lanczos-3: support radius 3, kernel defined on [-3, 3].
                let a = 3.0f32;
                self.add_image_radial(
                    &image,
                    transform,
                    weight,
                    pixel_weights,
                    scale,
                    a as isize,
                    |dx, dy| lanczos_kernel(dx, a) * lanczos_kernel(dy, a),
                );
            }
        }
    }

    /// Add image using turbo kernel (axis-aligned rectangular drop).
    fn add_image_turbo(
        &mut self,
        image: &LinearImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
        scale: f32,
        drop_size: f32,
    ) {
        let half_drop = drop_size / 2.0;
        let inv_area = 1.0 / (drop_size * drop_size);
        let output_width = self.width();
        let output_height = self.height();
        let input_width = image.width();
        let input_height = image.height();

        for iy in 0..input_height {
            for ix in 0..input_width {
                let pw = pixel_weights.map_or(1.0, |w| w[(ix, iy)]);
                if pw == 0.0 {
                    continue;
                }

                // Integer-center throughout: input pixel `i` is at coordinate `i` (matching
                // star centroids / `register` / `warp`), and output pixel `o` is the cell
                // `[o - 0.5, o + 0.5)`. The drop center needs no coordinate adjustment.
                let t = transform.apply(DVec2::new(ix as f64, iy as f64));
                let ox_center = t.x as f32 * scale;
                let oy_center = t.y as f32 * scale;

                let jaco = local_jacobian(transform, t, ix, iy, scale as f64) as f32;
                if jaco < JACOBIAN_MIN as f32 {
                    continue;
                }

                // Output pixel `o` is the cell `[o - 0.5, o + 0.5)`, so the drop touches the
                // pixels `round(min) ..= round(max)` (the `overlap > 0.0` test below drops any
                // boundary cell that doesn't actually touch).
                let ox_min = (ox_center - half_drop).round().max(0.0) as usize;
                let oy_min = (oy_center - half_drop).round().max(0.0) as usize;
                let ox_max =
                    ((ox_center + half_drop).round() + 1.0).min(output_width as f32) as usize;
                let oy_max =
                    ((oy_center + half_drop).round() + 1.0).min(output_height as f32) as usize;
                let drop =
                    Rect::from_center_half_extent(Vec2::new(ox_center, oy_center), half_drop);

                let effective_weight = weight * pw / jaco;
                for oy in oy_min..oy_max {
                    for ox in ox_min..ox_max {
                        let pixel =
                            Rect::from_center_half_extent(Vec2::new(ox as f32, oy as f32), 0.5);
                        let overlap = drop.overlap_area(pixel);

                        if overlap > 0.0 {
                            let pixel_weight = effective_weight * overlap * inv_area;
                            self.accumulate(image, ix, iy, ox, oy, pixel_weight);
                        }
                    }
                }
            }
        }
    }

    /// Add image using square kernel (true polygon clipping).
    ///
    /// For each input pixel, transforms all 4 corners of the (pixfrac-shrunken) drop
    /// to output coordinates, computes the Jacobian (signed area of the output
    /// quadrilateral), then iterates output pixels in the bounding box and computes
    /// exact overlap via `boxer()`.
    ///
    /// Reference: STScI cdrizzlebox.c `do_kernel_square`.
    fn add_image_square(
        &mut self,
        image: &LinearImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
        scale: f32,
    ) {
        let pixfrac = self.config.pixfrac;
        let dh = 0.5 * pixfrac as f64;
        let scale_f64 = scale as f64;
        let output_width = self.width();
        let output_height = self.height();
        let input_width = image.width();
        let input_height = image.height();

        for iy in 0..input_height {
            for ix in 0..input_width {
                let pw = pixel_weights.map_or(1.0, |w| w[(ix, iy)]);
                if pw == 0.0 {
                    continue;
                }

                // Compute 4 corners of the shrunken drop in input space. Input pixel (ix, iy)
                // is integer-center (center at (ix, iy), matching centroids / warp).
                // Winding order: BL, BR, TR, TL (counterclockwise).
                let cx = ix as f64;
                let cy = iy as f64;
                let corners_in = [
                    DVec2::new(cx - dh, cy - dh),
                    DVec2::new(cx + dh, cy - dh),
                    DVec2::new(cx + dh, cy + dh),
                    DVec2::new(cx - dh, cy + dh),
                ];

                // Transform the 4 corners to integer-center output coordinates (output pixel
                // `o` is centered at `o`); `boxer` is given each cell as `[o - 0.5, o + 0.5)`.
                let mut xout = [0.0f64; 4];
                let mut yout = [0.0f64; 4];
                for (k, corner) in corners_in.iter().enumerate() {
                    let t = transform.apply(*corner);
                    xout[k] = t.x * scale_f64;
                    yout[k] = t.y * scale_f64;
                }

                // Jacobian: signed area of the output quadrilateral via diagonal cross product
                let jaco = 0.5
                    * ((xout[1] - xout[3]) * (yout[0] - yout[2])
                        - (xout[0] - xout[2]) * (yout[1] - yout[3]));
                let abs_jaco = jaco.abs();
                if abs_jaco < JACOBIAN_MIN {
                    continue; // Degenerate quadrilateral
                }

                // Bounding box of the output quadrilateral
                let xmin = xout.iter().copied().fold(f64::INFINITY, f64::min);
                let xmax = xout.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let ymin = yout.iter().copied().fold(f64::INFINITY, f64::min);
                let ymax = yout.iter().copied().fold(f64::NEG_INFINITY, f64::max);

                // Output pixel `o` is the cell `[o - 0.5, o + 0.5)`, so the quad bbox touches
                // pixels `round(min) ..= round(max)`.
                let ox_min = xmin.round().max(0.0) as usize;
                let oy_min = ymin.round().max(0.0) as usize;
                let ox_max = (xmax.round() + 1.0).min(output_width as f64) as usize;
                let oy_max = (ymax.round() + 1.0).min(output_height as f64) as usize;

                let effective_weight = weight as f64 * pw as f64;
                let w_over_jaco = effective_weight / abs_jaco;

                for oy in oy_min..oy_max {
                    for ox in ox_min..ox_max {
                        let overlap = boxer(ox as f64 - 0.5, oy as f64 - 0.5, &xout, &yout);
                        if overlap > 0.0 {
                            let pixel_weight = (overlap * w_over_jaco) as f32;
                            self.accumulate(image, ix, iy, ox, oy, pixel_weight);
                        }
                    }
                }
            }
        }
    }

    /// Add image using point kernel (fastest, needs good dithering).
    fn add_image_point(
        &mut self,
        image: &LinearImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
        scale: f32,
    ) {
        let output_width = self.width();
        let output_height = self.height();
        let input_width = image.width();
        let input_height = image.height();

        for iy in 0..input_height {
            for ix in 0..input_width {
                let pw = pixel_weights.map_or(1.0, |w| w[(ix, iy)]);
                if pw == 0.0 {
                    continue;
                }

                // Integer-center input; flux lands in the nearest output pixel.
                let t = transform.apply(DVec2::new(ix as f64, iy as f64));
                let ox = (t.x as f32 * scale).round() as isize;
                let oy = (t.y as f32 * scale).round() as isize;

                if ox >= 0 && ox < output_width as isize && oy >= 0 && oy < output_height as isize {
                    let jaco = local_jacobian(transform, t, ix, iy, scale as f64) as f32;
                    if jaco < JACOBIAN_MIN as f32 {
                        continue;
                    }
                    self.accumulate(image, ix, iy, ox as usize, oy as usize, weight * pw / jaco);
                }
            }
        }
    }

    /// Add image using a radial kernel with two-pass normalization.
    ///
    /// Shared implementation for Gaussian and Lanczos kernels. Both iterate output
    /// pixels within `radius` of the transformed center, compute a per-pixel weight
    /// via `kernel_fn(dx, dy)`, normalize so weights sum to 1, then accumulate.
    #[allow(clippy::too_many_arguments)]
    fn add_image_radial(
        &mut self,
        image: &LinearImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
        scale: f32,
        radius: isize,
        kernel_fn: impl Fn(f32, f32) -> f32,
    ) {
        let output_width = self.width() as isize;
        let output_height = self.height() as isize;
        let input_width = image.width();
        let input_height = image.height();
        // `scale` is unbounded by config, so the neighbourhood is sized at run time rather than on
        // the stack. One allocation for the whole frame.
        let side = (2 * radius + 1) as usize;
        let mut taps: Vec<KernelTap> = Vec::with_capacity(side * side);

        for iy in 0..input_height {
            for ix in 0..input_width {
                let pw = pixel_weights.map_or(1.0, |w| w[(ix, iy)]);
                if pw == 0.0 {
                    continue;
                }

                // Integer-center: output pixel `o` is centred at `o`, so the kernel distance
                // is `o - ox_center` with no offset.
                let t = transform.apply(DVec2::new(ix as f64, iy as f64));
                let ox_center = t.x as f32 * scale;
                let oy_center = t.y as f32 * scale;

                let jaco = local_jacobian(transform, t, ix, iy, scale as f64) as f32;
                if jaco < JACOBIAN_MIN as f32 {
                    continue;
                }

                let fluxes = Self::fluxes_at(image, ix, iy);
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
                            ox: ox as usize,
                            oy: oy as usize,
                            value: tap,
                        });
                    }
                }

                if total_weight.abs() < KERNEL_WEIGHT_MIN {
                    continue;
                }

                // Distribute flux with normalized weights. Per-pixel weight scales the effective
                // frame weight; the Jacobian divides out the local area magnification.
                let inv_total = (weight * pw) / (total_weight * jaco);
                for tap in &taps {
                    self.accumulate_samples(&fluxes, tap.ox, tap.oy, tap.value * inv_total);
                }
            }
        }
    }

    /// One input pixel's samples, one per channel.
    ///
    /// Only the radial path pre-reads these. Its drop covers `(2r+1)²` output pixels, so reading
    /// per output pixel repeats the same lookup a hundred times over; the compact kernels cover a
    /// handful and measured *slower* when made to build this first — the loads they repeat are
    /// already hoisted.
    #[inline]
    fn fluxes_at(image: &LinearImage, ix: usize, iy: usize) -> ArrayVec<f32, MAX_CHANNELS> {
        (0..image.channels())
            .map(|c| image.channel(c)[(ix, iy)])
            .collect()
    }

    /// Accumulate weighted flux from input pixel (ix, iy) into output pixel (ox, oy).
    #[inline]
    fn accumulate(
        &mut self,
        image: &LinearImage,
        ix: usize,
        iy: usize,
        ox: usize,
        oy: usize,
        pixel_weight: f32,
    ) {
        for (c, d) in self.data.iter_mut().enumerate() {
            let flux = image.channel(c)[(ix, iy)];
            *d.get_mut(ox, oy) += flux * pixel_weight;
        }
        // Weight is channel-independent, so accumulate it and its square once per output pixel.
        *self.weight.get_mut(ox, oy) += pixel_weight;
        if let Some(weight_sq) = &mut self.weight_sq {
            *weight_sq.get_mut(ox, oy) += pixel_weight * pixel_weight;
        }
    }

    /// Accumulate already-read `fluxes` into output pixel (ox, oy).
    ///
    /// The two weight lines are repeated from [`Self::accumulate`] rather than shared through a
    /// third method: factoring them out measured ~4% slower on the compact kernels, whose hot loop
    /// is this function.
    #[inline]
    fn accumulate_samples(&mut self, fluxes: &[f32], ox: usize, oy: usize, pixel_weight: f32) {
        for (d, &flux) in self.data.iter_mut().zip(fluxes) {
            *d.get_mut(ox, oy) += flux * pixel_weight;
        }
        *self.weight.get_mut(ox, oy) += pixel_weight;
        if let Some(weight_sq) = &mut self.weight_sq {
            *weight_sq.get_mut(ox, oy) += pixel_weight * pixel_weight;
        }
    }

    /// Finalize the drizzle result: normalize flux by weight and emit whichever quality planes
    /// [`DrizzleConfig::quality`] asked for. The weight `Σwᵢ` is channel-independent, so one map
    /// normalizes every channel and seeds the quality outputs.
    ///
    /// The weight map is built whatever the request — the image is `Σfluxᵢwᵢ / Σwᵢ`, and
    /// `min_coverage` gates fill against its maximum — so declining `weight` only declines handing
    /// it out, while declining `coverage` or `variance` skips an output-grid allocation each.
    pub fn finalize(self) -> StackProduct {
        let width = self.width();
        let height = self.height();
        let n_channels = self.channels();
        let needs_clamping = self.config.kernel == DrizzleKernel::Lanczos;
        let min_coverage = self.config.min_coverage;
        let fill_value = self.config.fill_value;

        let planes = self.config.quality;
        let weight_pixels = self.weight.pixels();

        // Find max weight for normalizing the coverage / min_coverage threshold.
        let max_weight = weight_pixels
            .par_iter()
            .copied()
            .reduce(|| 0.0f32, f32::max);
        let weight_threshold = if max_weight > 0.0 {
            min_coverage * max_weight
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

        // Normalized coverage [0, 1] for masking.
        let coverage = planes.coverage.then(|| {
            let mut coverage = Buffer2::new_default(width, height);
            if max_weight > 0.0 {
                let inv_max = 1.0 / max_weight;
                coverage
                    .pixels_mut()
                    .par_iter_mut()
                    .zip(weight_pixels.par_iter())
                    .for_each(|(c, &w)| *c = w * inv_max);
            }
            Coverage::PerPixel(coverage)
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

    pub(crate) fn accumulated_flux_sum(accumulator: &DrizzleAccumulator, channel: usize) -> f32 {
        accumulator.data[channel].pixels().iter().sum()
    }
}
