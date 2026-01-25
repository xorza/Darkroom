//! Phase correlation for coarse image alignment.
//!
//! Phase correlation uses FFT to find translation between images by:
//! 1. Computing 2D FFT of both images
//! 2. Computing normalized cross-power spectrum
//! 3. Finding peak in inverse FFT (gives translation)
//! 4. Optionally refining to sub-pixel accuracy

#[cfg(test)]
mod tests;

#[cfg(feature = "bench")]
pub mod bench;

use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::sync::Arc;

/// Configuration for phase correlation.
#[derive(Debug, Clone)]
pub struct PhaseCorrelationConfig {
    /// Apply Hann window to reduce edge effects.
    pub use_windowing: bool,
    /// Sub-pixel interpolation method.
    pub subpixel_method: SubpixelMethod,
    /// Minimum correlation peak value to accept.
    pub min_peak_value: f32,
}

impl Default for PhaseCorrelationConfig {
    fn default() -> Self {
        Self {
            use_windowing: true,
            subpixel_method: SubpixelMethod::Parabolic,
            min_peak_value: 0.1,
        }
    }
}

/// Sub-pixel interpolation method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubpixelMethod {
    /// No sub-pixel refinement.
    None,
    /// Parabolic fit (fast, ~0.1 pixel accuracy).
    #[default]
    Parabolic,
    /// Gaussian fit (slower, ~0.05 pixel accuracy).
    Gaussian,
    /// Centroid (robust to noise).
    Centroid,
}

/// Result of phase correlation.
#[derive(Debug, Clone)]
pub struct PhaseCorrelationResult {
    /// Estimated translation (dx, dy) in pixels.
    pub translation: (f64, f64),
    /// Peak correlation value (0.0 - 1.0).
    pub peak_value: f64,
    /// Confidence based on peak sharpness.
    pub confidence: f64,
}

/// Phase correlator for translation estimation.
pub struct PhaseCorrelator {
    config: PhaseCorrelationConfig,
    /// FFT size (must be power of 2 for efficiency)
    fft_size: usize,
    /// Cached forward FFT
    forward_fft: Arc<dyn Fft<f32>>,
    /// Cached inverse FFT
    inverse_fft: Arc<dyn Fft<f32>>,
    /// Pre-computed Hann window
    window: Vec<f32>,
}

impl PhaseCorrelator {
    /// Create a new phase correlator for images of given size.
    pub fn new(width: usize, height: usize, config: PhaseCorrelationConfig) -> Self {
        // Use the larger dimension, rounded up to power of 2
        let max_dim = width.max(height);
        let fft_size = max_dim.next_power_of_two();

        let mut planner = FftPlanner::new();
        let forward_fft = planner.plan_fft_forward(fft_size);
        let inverse_fft = planner.plan_fft_inverse(fft_size);

        // Pre-compute 1D Hann window (will be applied separably)
        let window = if config.use_windowing {
            hann_window(fft_size)
        } else {
            vec![1.0; fft_size]
        };

        Self {
            config,
            fft_size,
            forward_fft,
            inverse_fft,
            window,
        }
    }

    /// Estimate translation between two images.
    ///
    /// Returns (dx, dy) where target = reference shifted by (dx, dy).
    pub fn correlate(
        &self,
        reference: &[f32],
        target: &[f32],
        width: usize,
        height: usize,
    ) -> Option<PhaseCorrelationResult> {
        if reference.len() != width * height || target.len() != width * height {
            return None;
        }

        // Pad and window the images
        let ref_padded = self.prepare_image(reference, width, height);
        let tar_padded = self.prepare_image(target, width, height);

        // Compute 2D FFT of both images
        let ref_fft = self.fft_2d(&ref_padded);
        let tar_fft = self.fft_2d(&tar_padded);

        // Compute normalized cross-power spectrum
        let cross_power = self.cross_power_spectrum(&ref_fft, &tar_fft);

        // Inverse FFT to get correlation surface
        let correlation = self.ifft_2d(&cross_power);

        // Find peak
        let (peak_x, peak_y, peak_val) = self.find_peak(&correlation);

        if peak_val < self.config.min_peak_value as f64 {
            return None;
        }

        // Convert to translation (handle wraparound)
        let dx = if peak_x > self.fft_size / 2 {
            peak_x as f64 - self.fft_size as f64
        } else {
            peak_x as f64
        };

        let dy = if peak_y > self.fft_size / 2 {
            peak_y as f64 - self.fft_size as f64
        } else {
            peak_y as f64
        };

        // Sub-pixel refinement
        let (sub_dx, sub_dy) = match self.config.subpixel_method {
            SubpixelMethod::None => (dx, dy),
            SubpixelMethod::Parabolic => {
                self.subpixel_parabolic(&correlation, peak_x, peak_y, dx, dy)
            }
            SubpixelMethod::Gaussian => {
                self.subpixel_gaussian(&correlation, peak_x, peak_y, dx, dy)
            }
            SubpixelMethod::Centroid => {
                self.subpixel_centroid(&correlation, peak_x, peak_y, dx, dy)
            }
        };

        // Compute confidence based on peak sharpness
        let confidence = self.compute_confidence(&correlation, peak_x, peak_y, peak_val);

        Some(PhaseCorrelationResult {
            translation: (sub_dx, sub_dy),
            peak_value: peak_val,
            confidence,
        })
    }

    /// Prepare image: pad to FFT size and apply window.
    fn prepare_image(&self, image: &[f32], width: usize, height: usize) -> Vec<f32> {
        let n = self.fft_size;
        let mut padded = vec![0.0f32; n * n];

        // Copy with centering and windowing
        let offset_x = (n - width) / 2;
        let offset_y = (n - height) / 2;

        for y in 0..height {
            for x in 0..width {
                let src_idx = y * width + x;
                let dst_idx = (y + offset_y) * n + (x + offset_x);

                // Apply separable window
                let wx = self.window[(x + offset_x) % n];
                let wy = self.window[(y + offset_y) % n];

                padded[dst_idx] = image[src_idx] * wx * wy;
            }
        }

        padded
    }

    /// Compute 2D FFT using row-column decomposition.
    fn fft_2d(&self, image: &[f32]) -> Vec<Complex<f32>> {
        let n = self.fft_size;
        let mut data: Vec<Complex<f32>> = image.iter().map(|&v| Complex::new(v, 0.0)).collect();

        // FFT on rows
        for row in 0..n {
            let start = row * n;
            let end = start + n;
            self.forward_fft.process(&mut data[start..end]);
        }

        // Transpose
        transpose_inplace(&mut data, n);

        // FFT on columns (now rows after transpose)
        for row in 0..n {
            let start = row * n;
            let end = start + n;
            self.forward_fft.process(&mut data[start..end]);
        }

        // Transpose back
        transpose_inplace(&mut data, n);

        data
    }

    /// Compute inverse 2D FFT.
    fn ifft_2d(&self, data: &[Complex<f32>]) -> Vec<f32> {
        let n = self.fft_size;
        let mut work = data.to_vec();

        // IFFT on rows
        for row in 0..n {
            let start = row * n;
            let end = start + n;
            self.inverse_fft.process(&mut work[start..end]);
        }

        // Transpose
        transpose_inplace(&mut work, n);

        // IFFT on columns
        for row in 0..n {
            let start = row * n;
            let end = start + n;
            self.inverse_fft.process(&mut work[start..end]);
        }

        // Transpose back
        transpose_inplace(&mut work, n);

        // Normalize and take real part
        let norm = 1.0 / (n * n) as f32;
        work.iter().map(|c| c.re * norm).collect()
    }

    /// Compute normalized cross-power spectrum.
    fn cross_power_spectrum(
        &self,
        fft1: &[Complex<f32>],
        fft2: &[Complex<f32>],
    ) -> Vec<Complex<f32>> {
        fft1.iter()
            .zip(fft2.iter())
            .map(|(&a, &b)| {
                // Cross-power: F1 * conj(F2) / |F1 * conj(F2)|
                let product = a * b.conj();
                let magnitude = product.norm();
                if magnitude > 1e-10 {
                    product / magnitude
                } else {
                    Complex::new(0.0, 0.0)
                }
            })
            .collect()
    }

    /// Find peak location and value in correlation surface.
    fn find_peak(&self, correlation: &[f32]) -> (usize, usize, f64) {
        let n = self.fft_size;
        let mut max_val = f32::NEG_INFINITY;
        let mut max_x = 0;
        let mut max_y = 0;

        for y in 0..n {
            for x in 0..n {
                let val = correlation[y * n + x];
                if val > max_val {
                    max_val = val;
                    max_x = x;
                    max_y = y;
                }
            }
        }

        (max_x, max_y, max_val as f64)
    }

    /// Sub-pixel refinement using parabolic fit.
    fn subpixel_parabolic(
        &self,
        correlation: &[f32],
        peak_x: usize,
        peak_y: usize,
        dx: f64,
        dy: f64,
    ) -> (f64, f64) {
        let n = self.fft_size;

        // Get 3x3 neighborhood around peak
        let get_val = |x: isize, y: isize| -> f32 {
            let xx = ((x % n as isize) + n as isize) as usize % n;
            let yy = ((y % n as isize) + n as isize) as usize % n;
            correlation[yy * n + xx]
        };

        let px = peak_x as isize;
        let py = peak_y as isize;

        let c = get_val(px, py);
        let l = get_val(px - 1, py);
        let r = get_val(px + 1, py);
        let t = get_val(px, py - 1);
        let b = get_val(px, py + 1);

        // Parabolic interpolation: offset = (left - right) / (2 * (left + right - 2 * center))
        let denom_x = 2.0 * (l + r - 2.0 * c);
        let denom_y = 2.0 * (t + b - 2.0 * c);

        let sub_x = if denom_x.abs() > 1e-10 {
            dx + (l - r) as f64 / denom_x as f64
        } else {
            dx
        };

        let sub_y = if denom_y.abs() > 1e-10 {
            dy + (t - b) as f64 / denom_y as f64
        } else {
            dy
        };

        (sub_x, sub_y)
    }

    /// Sub-pixel refinement using Gaussian fit.
    fn subpixel_gaussian(
        &self,
        correlation: &[f32],
        peak_x: usize,
        peak_y: usize,
        dx: f64,
        dy: f64,
    ) -> (f64, f64) {
        let n = self.fft_size;

        let get_val = |x: isize, y: isize| -> f32 {
            let xx = ((x % n as isize) + n as isize) as usize % n;
            let yy = ((y % n as isize) + n as isize) as usize % n;
            correlation[yy * n + xx].max(1e-10)
        };

        let px = peak_x as isize;
        let py = peak_y as isize;

        let c = get_val(px, py).ln();
        let l = get_val(px - 1, py).ln();
        let r = get_val(px + 1, py).ln();
        let t = get_val(px, py - 1).ln();
        let b = get_val(px, py + 1).ln();

        // Gaussian fit: offset = (ln(left) - ln(right)) / (2 * (ln(left) + ln(right) - 2 * ln(center)))
        let denom_x = 2.0 * (l + r - 2.0 * c);
        let denom_y = 2.0 * (t + b - 2.0 * c);

        let sub_x = if denom_x.abs() > 1e-10 {
            dx + (l - r) as f64 / denom_x as f64
        } else {
            dx
        };

        let sub_y = if denom_y.abs() > 1e-10 {
            dy + (t - b) as f64 / denom_y as f64
        } else {
            dy
        };

        (sub_x, sub_y)
    }

    /// Sub-pixel refinement using centroid.
    fn subpixel_centroid(
        &self,
        correlation: &[f32],
        peak_x: usize,
        peak_y: usize,
        dx: f64,
        dy: f64,
    ) -> (f64, f64) {
        let n = self.fft_size;
        let radius = 2isize;

        let get_val = |x: isize, y: isize| -> f32 {
            let xx = ((x % n as isize) + n as isize) as usize % n;
            let yy = ((y % n as isize) + n as isize) as usize % n;
            correlation[yy * n + xx].max(0.0)
        };

        let px = peak_x as isize;
        let py = peak_y as isize;

        let mut sum_x = 0.0f64;
        let mut sum_y = 0.0f64;
        let mut sum_w = 0.0f64;

        for dy_off in -radius..=radius {
            for dx_off in -radius..=radius {
                let w = get_val(px + dx_off, py + dy_off) as f64;
                sum_x += dx_off as f64 * w;
                sum_y += dy_off as f64 * w;
                sum_w += w;
            }
        }

        if sum_w > 1e-10 {
            (dx + sum_x / sum_w, dy + sum_y / sum_w)
        } else {
            (dx, dy)
        }
    }

    /// Compute confidence based on peak sharpness.
    fn compute_confidence(
        &self,
        correlation: &[f32],
        peak_x: usize,
        peak_y: usize,
        peak_val: f64,
    ) -> f64 {
        let n = self.fft_size;

        // Find second-highest peak (at least some distance away)
        let min_dist = n / 8;
        let mut second_peak = 0.0f32;

        for y in 0..n {
            for x in 0..n {
                let dx = (x as isize - peak_x as isize).unsigned_abs();
                let dy = (y as isize - peak_y as isize).unsigned_abs();
                let dx = dx.min(n - dx);
                let dy = dy.min(n - dy);

                if dx >= min_dist || dy >= min_dist {
                    second_peak = second_peak.max(correlation[y * n + x]);
                }
            }
        }

        // Confidence is ratio of primary to secondary peak
        if second_peak > 1e-10 {
            ((peak_val as f32 / second_peak).min(10.0) / 10.0) as f64
        } else {
            1.0
        }
    }
}

/// Estimate translation for potentially large offsets using multi-scale approach.
///
/// Standard phase correlation has limited range due to FFT wraparound (typically
/// up to image_size/4). This method handles larger offsets by downsampling both
/// images and running phase correlation at reduced resolution, which extends
/// the detectable offset range by the downsampling factor.
///
/// # Arguments
/// * `reference` - Reference image
/// * `target` - Target image
/// * `width` - Image width
/// * `height` - Image height
/// * `config` - Phase correlation configuration
///
/// # Returns
/// Phase correlation result with extended range, or None if estimation failed.
///
/// # Note
/// The accuracy is reduced by the downsampling factor (typically 4 pixels instead
/// of sub-pixel). For large offsets where standard phase correlation fails, this
/// provides a coarse estimate that can be refined by other means (e.g., star matching).
pub fn correlate_large_offset(
    reference: &[f32],
    target: &[f32],
    width: usize,
    height: usize,
    config: &PhaseCorrelationConfig,
) -> Option<PhaseCorrelationResult> {
    // Downsampling factor - 4x allows detecting offsets up to ~image_size (vs image_size/4)
    const DOWNSAMPLE_FACTOR: usize = 4;

    // Minimum size for downsampled image to maintain correlation quality
    const MIN_DOWNSAMPLE_SIZE: usize = 64;

    let ds_width = width / DOWNSAMPLE_FACTOR;
    let ds_height = height / DOWNSAMPLE_FACTOR;

    // If image is too small for downsampling, use standard correlation
    if ds_width < MIN_DOWNSAMPLE_SIZE || ds_height < MIN_DOWNSAMPLE_SIZE {
        let correlator = PhaseCorrelator::new(width, height, config.clone());
        return correlator.correlate(reference, target, width, height);
    }

    // Downsample both images using box filter (averaging)
    let ref_ds = downsample_image(reference, width, height, DOWNSAMPLE_FACTOR);
    let tar_ds = downsample_image(target, width, height, DOWNSAMPLE_FACTOR);

    // Run phase correlation on downsampled images
    let correlator = PhaseCorrelator::new(ds_width, ds_height, config.clone());
    let result = correlator.correlate(&ref_ds, &tar_ds, ds_width, ds_height)?;

    // Scale up the translation to full resolution
    let dx = result.translation.0 * DOWNSAMPLE_FACTOR as f64;
    let dy = result.translation.1 * DOWNSAMPLE_FACTOR as f64;

    Some(PhaseCorrelationResult {
        translation: (dx, dy),
        peak_value: result.peak_value,
        // Slightly lower confidence due to reduced resolution
        confidence: result.confidence * 0.9,
    })
}

/// Downsample an image using box filter (averaging).
fn downsample_image(image: &[f32], width: usize, height: usize, factor: usize) -> Vec<f32> {
    let new_width = width / factor;
    let new_height = height / factor;
    let mut result = vec![0.0f32; new_width * new_height];

    let factor_sq = (factor * factor) as f32;

    for ny in 0..new_height {
        for nx in 0..new_width {
            let mut sum = 0.0f32;
            for dy in 0..factor {
                for dx in 0..factor {
                    let x = nx * factor + dx;
                    let y = ny * factor + dy;
                    sum += image[y * width + x];
                }
            }
            result[ny * new_width + nx] = sum / factor_sq;
        }
    }

    result
}

/// Shift an image by a fractional offset using bilinear interpolation.
#[cfg(test)]
fn shift_image(image: &[f32], width: usize, height: usize, dx: f64, dy: f64) -> Vec<f32> {
    let mut result = vec![0.0f32; width * height];

    for y in 0..height {
        for x in 0..width {
            // Sample from source position (inverse of shift)
            let sx = x as f64 - dx;
            let sy = y as f64 - dy;
            result[y * width + x] = bilinear_sample(image, width, height, sx, sy);
        }
    }

    result
}

/// Compute 1D Hann window.
pub fn hann_window(size: usize) -> Vec<f32> {
    use std::f32::consts::PI;
    (0..size)
        .map(|i| {
            let x = i as f32 / size as f32;
            0.5 * (1.0 - (2.0 * PI * x).cos())
        })
        .collect()
}

/// In-place square matrix transpose.
pub fn transpose_inplace(data: &mut [Complex<f32>], n: usize) {
    for i in 0..n {
        for j in (i + 1)..n {
            data.swap(i * n + j, j * n + i);
        }
    }
}

/// Result of log-polar phase correlation.
#[derive(Debug, Clone)]
pub struct LogPolarResult {
    /// Estimated rotation angle in radians.
    pub rotation: f64,
    /// Estimated scale factor (1.0 = no scaling).
    pub scale: f64,
    /// Peak correlation value.
    pub peak_value: f64,
    /// Confidence in the estimate.
    pub confidence: f64,
}

/// Log-polar phase correlation for rotation and scale estimation.
///
/// This technique works by:
/// 1. Computing the magnitude spectrum of both images (shift-invariant)
/// 2. Converting magnitude spectra to log-polar coordinates
/// 3. Using phase correlation to find rotation (angular shift) and scale (radial shift)
///
/// The key insight is that in log-polar coordinates:
/// - Rotation becomes a vertical (angular) translation
/// - Scale change becomes a horizontal (log-radius) translation
///
/// This makes rotation/scale estimation a simple translation problem.
pub struct LogPolarCorrelator {
    /// Size of the log-polar image
    size: usize,
    /// Logarithmic base for radius mapping
    log_base: f64,
    /// Minimum radius (avoids DC component)
    min_radius: f64,
    /// Maximum radius
    max_radius: f64,
    /// Phase correlator for translation in log-polar space
    correlator: PhaseCorrelator,
}

impl LogPolarCorrelator {
    /// Create a new log-polar correlator.
    ///
    /// # Arguments
    /// * `size` - Size of the log-polar image (typically 256 or 512)
    /// * `max_radius` - Maximum radius to consider (typically half the image size)
    pub fn new(size: usize, max_radius: f64) -> Self {
        let config = PhaseCorrelationConfig {
            use_windowing: true,
            subpixel_method: SubpixelMethod::Parabolic,
            min_peak_value: 0.05,
        };
        Self::with_config(size, max_radius, config)
    }

    /// Create a new log-polar correlator with custom config.
    pub fn with_config(size: usize, max_radius: f64, config: PhaseCorrelationConfig) -> Self {
        let min_radius = 4.0; // Avoid DC component artifacts
        let log_base = (max_radius / min_radius).ln();

        Self {
            size,
            log_base,
            min_radius,
            max_radius,
            correlator: PhaseCorrelator::new(size, size, config),
        }
    }

    /// Estimate rotation and scale between two images.
    ///
    /// # Arguments
    /// * `reference` - Reference image
    /// * `target` - Target image
    /// * `width` - Image width
    /// * `height` - Image height
    ///
    /// # Returns
    /// Estimated rotation (radians) and scale, or None if estimation failed.
    pub fn estimate_rotation_scale(
        &self,
        reference: &[f32],
        target: &[f32],
        width: usize,
        height: usize,
    ) -> Option<LogPolarResult> {
        if reference.len() != width * height || target.len() != width * height {
            return None;
        }

        // Step 1: Compute magnitude spectra (shift-invariant)
        let ref_mag = self.compute_magnitude_spectrum(reference, width, height);
        let tar_mag = self.compute_magnitude_spectrum(target, width, height);

        // Step 2: Convert to log-polar coordinates
        let ref_lp = self.to_log_polar(&ref_mag, width.max(height).next_power_of_two());
        let tar_lp = self.to_log_polar(&tar_mag, width.max(height).next_power_of_two());

        // Step 3: Phase correlate in log-polar space
        let result = self
            .correlator
            .correlate(&ref_lp, &tar_lp, self.size, self.size)?;

        // Convert translation to rotation and scale
        let (dx, dy) = result.translation;

        // dy corresponds to rotation (angular shift)
        // dx corresponds to scale (log-radius shift)
        let rotation = -dy * 2.0 * std::f64::consts::PI / self.size as f64;

        // Scale is exp(dx * log_base / size)
        let scale = (dx * self.log_base / self.size as f64).exp();

        // Clamp scale to reasonable range
        let scale = scale.clamp(0.5, 2.0);

        Some(LogPolarResult {
            rotation,
            scale,
            peak_value: result.peak_value,
            confidence: result.confidence,
        })
    }

    /// Compute the magnitude spectrum of an image.
    fn compute_magnitude_spectrum(&self, image: &[f32], width: usize, height: usize) -> Vec<f32> {
        let n = width.max(height).next_power_of_two();

        // Pad image to power of 2
        let mut padded = vec![0.0f32; n * n];
        let offset_x = (n - width) / 2;
        let offset_y = (n - height) / 2;

        for y in 0..height {
            for x in 0..width {
                padded[(y + offset_y) * n + (x + offset_x)] = image[y * width + x];
            }
        }

        // Apply Hann window
        let window = hann_window(n);
        for y in 0..n {
            for x in 0..n {
                padded[y * n + x] *= window[x] * window[y];
            }
        }

        // Compute 2D FFT
        let mut planner = rustfft::FftPlanner::new();
        let fft = planner.plan_fft_forward(n);

        let mut data: Vec<Complex<f32>> = padded.iter().map(|&v| Complex::new(v, 0.0)).collect();

        // FFT on rows
        for row in 0..n {
            let start = row * n;
            fft.process(&mut data[start..start + n]);
        }

        // Transpose
        transpose_inplace(&mut data, n);

        // FFT on columns
        for row in 0..n {
            let start = row * n;
            fft.process(&mut data[start..start + n]);
        }

        // Transpose back
        transpose_inplace(&mut data, n);

        // Compute magnitude and apply log scaling
        // Also shift zero frequency to center
        let mut magnitude = vec![0.0f32; n * n];
        for y in 0..n {
            for x in 0..n {
                // FFT shift: swap quadrants
                let sx = (x + n / 2) % n;
                let sy = (y + n / 2) % n;
                let mag = data[y * n + x].norm();
                // Log scale to compress dynamic range
                magnitude[sy * n + sx] = (1.0 + mag).ln();
            }
        }

        magnitude
    }

    /// Convert magnitude spectrum to log-polar coordinates.
    fn to_log_polar(&self, magnitude: &[f32], fft_size: usize) -> Vec<f32> {
        let mut log_polar = vec![0.0f32; self.size * self.size];

        let cx = fft_size as f64 / 2.0;
        let cy = fft_size as f64 / 2.0;

        for theta_idx in 0..self.size {
            // Angle from 0 to 2*PI
            let theta = theta_idx as f64 * 2.0 * std::f64::consts::PI / self.size as f64;
            let cos_t = theta.cos();
            let sin_t = theta.sin();

            for rho_idx in 0..self.size {
                // Log-spaced radius
                let t = rho_idx as f64 / self.size as f64;
                let radius = self.min_radius * (t * self.log_base).exp();

                if radius <= self.max_radius {
                    // Convert to Cartesian coordinates
                    let x = cx + radius * cos_t;
                    let y = cy + radius * sin_t;

                    // Bilinear interpolation
                    let value = bilinear_sample(magnitude, fft_size, fft_size, x, y);
                    log_polar[theta_idx * self.size + rho_idx] = value;
                }
            }
        }

        log_polar
    }
}

/// Bilinear interpolation for f32 images.
fn bilinear_sample(image: &[f32], width: usize, height: usize, x: f64, y: f64) -> f32 {
    let x0 = x.floor() as isize;
    let y0 = y.floor() as isize;
    let x1 = x0 + 1;
    let y1 = y0 + 1;

    let fx = (x - x0 as f64) as f32;
    let fy = (y - y0 as f64) as f32;

    let get_pixel = |px: isize, py: isize| -> f32 {
        if px >= 0 && px < width as isize && py >= 0 && py < height as isize {
            image[py as usize * width + px as usize]
        } else {
            0.0
        }
    };

    let p00 = get_pixel(x0, y0);
    let p10 = get_pixel(x1, y0);
    let p01 = get_pixel(x0, y1);
    let p11 = get_pixel(x1, y1);

    let top = p00 + fx * (p10 - p00);
    let bottom = p01 + fx * (p11 - p01);

    top + fy * (bottom - top)
}

/// Combined rotation, scale, and translation estimator.
///
/// Uses log-polar phase correlation for rotation/scale,
/// then standard phase correlation for translation.
pub struct FullPhaseCorrelator {
    log_polar: LogPolarCorrelator,
    translation: PhaseCorrelator,
}

/// Result of full phase correlation (rotation + scale + translation).
#[derive(Debug, Clone)]
pub struct FullPhaseResult {
    /// Estimated rotation angle in radians.
    pub rotation: f64,
    /// Estimated scale factor.
    pub scale: f64,
    /// Estimated translation (dx, dy).
    pub translation: (f64, f64),
    /// Overall confidence.
    pub confidence: f64,
}

impl FullPhaseCorrelator {
    /// Create a new full phase correlator.
    pub fn new(width: usize, height: usize) -> Self {
        Self::with_config(width, height, PhaseCorrelationConfig::default())
    }

    /// Create a new full phase correlator with custom config.
    pub fn with_config(width: usize, height: usize, config: PhaseCorrelationConfig) -> Self {
        let max_dim = width.max(height);
        let lp_size = 256.min(max_dim.next_power_of_two());

        // Use lower threshold for log-polar correlation
        let lp_config = PhaseCorrelationConfig {
            min_peak_value: config.min_peak_value * 0.5,
            ..config
        };

        Self {
            log_polar: LogPolarCorrelator::with_config(lp_size, max_dim as f64 / 2.0, lp_config),
            translation: PhaseCorrelator::new(width, height, config),
        }
    }

    /// Estimate full transformation (rotation, scale, translation).
    pub fn estimate(
        &self,
        reference: &[f32],
        target: &[f32],
        width: usize,
        height: usize,
    ) -> Option<FullPhaseResult> {
        // Step 1: Estimate rotation and scale
        let rs_result = self
            .log_polar
            .estimate_rotation_scale(reference, target, width, height)?;

        // Step 2: De-rotate and de-scale target, then estimate translation
        let corrected = rotate_and_scale_image(
            target,
            width,
            height,
            -rs_result.rotation,
            1.0 / rs_result.scale,
        );

        let trans_result = self
            .translation
            .correlate(reference, &corrected, width, height)?;

        Some(FullPhaseResult {
            rotation: rs_result.rotation,
            scale: rs_result.scale,
            translation: trans_result.translation,
            confidence: (rs_result.confidence + trans_result.confidence) / 2.0,
        })
    }
}

/// Rotate and scale an image around its center.
fn rotate_and_scale_image(
    image: &[f32],
    width: usize,
    height: usize,
    angle: f64,
    scale: f64,
) -> Vec<f32> {
    let mut result = vec![0.0f32; width * height];

    let cx = width as f64 / 2.0;
    let cy = height as f64 / 2.0;

    let cos_a = angle.cos();
    let sin_a = angle.sin();

    for y in 0..height {
        for x in 0..width {
            // Transform from output to input coordinates
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;

            // Rotate and scale (inverse transform)
            let sx = (dx * cos_a + dy * sin_a) / scale + cx;
            let sy = (-dx * sin_a + dy * cos_a) / scale + cy;

            result[y * width + x] = bilinear_sample(image, width, height, sx, sy);
        }
    }

    result
}
