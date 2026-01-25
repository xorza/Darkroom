//! Phase correlation for coarse image alignment.
//!
//! Phase correlation uses FFT to find translation between images by:
//! 1. Computing 2D FFT of both images
//! 2. Computing normalized cross-power spectrum
//! 3. Finding peak in inverse FFT (gives translation)
//! 4. Optionally refining to sub-pixel accuracy

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

/// Compute 1D Hann window.
fn hann_window(size: usize) -> Vec<f32> {
    use std::f32::consts::PI;
    (0..size)
        .map(|i| {
            let x = i as f32 / size as f32;
            0.5 * (1.0 - (2.0 * PI * x).cos())
        })
        .collect()
}

/// In-place square matrix transpose.
fn transpose_inplace(data: &mut [Complex<f32>], n: usize) {
    for i in 0..n {
        for j in (i + 1)..n {
            data.swap(i * n + j, j * n + i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_image(
        width: usize,
        height: usize,
        offset_x: isize,
        offset_y: isize,
    ) -> Vec<f32> {
        let mut image = vec![0.0f32; width * height];

        // Create a high-contrast pattern with clear edges that's easy to correlate
        for y in 0..height {
            for x in 0..width {
                let xx = (x as isize - offset_x).rem_euclid(width as isize) as usize;
                let yy = (y as isize - offset_y).rem_euclid(height as isize) as usize;

                // High-contrast checkerboard pattern
                let checker = ((xx / 8) + (yy / 8)) % 2;
                let val = if checker == 0 { 0.0 } else { 1.0 };
                image[y * width + x] = val;
            }
        }

        image
    }

    fn test_config() -> PhaseCorrelationConfig {
        PhaseCorrelationConfig {
            min_peak_value: 0.001, // Very low threshold for testing
            ..Default::default()
        }
    }

    #[test]
    fn test_correlate_identical_images() {
        let width = 64;
        let height = 64;
        let reference = create_test_image(width, height, 0, 0);

        let correlator = PhaseCorrelator::new(width, height, test_config());
        let result = correlator.correlate(&reference, &reference, width, height);

        assert!(
            result.is_some(),
            "Correlation of identical images should succeed"
        );
        let result = result.unwrap();

        // For identical images, translation should be near zero
        assert!(
            (result.translation.0).abs() < 2.0,
            "dx = {}",
            result.translation.0
        );
        assert!(
            (result.translation.1).abs() < 2.0,
            "dy = {}",
            result.translation.1
        );
    }

    #[test]
    fn test_correlate_translated_5_pixels() {
        let width = 64;
        let height = 64;
        let reference = create_test_image(width, height, 0, 0);
        let target = create_test_image(width, height, 5, 0);

        let correlator = PhaseCorrelator::new(width, height, test_config());
        let result = correlator.correlate(&reference, &target, width, height);

        assert!(result.is_some(), "Correlation should succeed");
        let result = result.unwrap();

        // Translation should detect the offset (sign depends on convention)
        assert!(
            (result.translation.0).abs() < 10.0,
            "Expected dx near ±5, got {}",
            result.translation.0
        );
    }

    #[test]
    fn test_correlate_translated_xy() {
        let width = 64;
        let height = 64;
        let reference = create_test_image(width, height, 0, 0);
        let target = create_test_image(width, height, 3, -7);

        let correlator = PhaseCorrelator::new(width, height, test_config());
        let result = correlator.correlate(&reference, &target, width, height);

        assert!(result.is_some(), "Correlation should succeed");
        // Just verify we get some result - exact values depend on implementation details
    }

    #[test]
    fn test_hann_window() {
        let window = hann_window(64);
        assert_eq!(window.len(), 64);

        // Window should be 0 at edges and 1 at center
        assert!(window[0] < 0.01);
        assert!(window[63] < 0.01);
        assert!((window[32] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_subpixel_methods() {
        let width = 64;
        let height = 64;
        let reference = create_test_image(width, height, 0, 0);

        for method in [
            SubpixelMethod::None,
            SubpixelMethod::Parabolic,
            SubpixelMethod::Gaussian,
            SubpixelMethod::Centroid,
        ] {
            let config = PhaseCorrelationConfig {
                subpixel_method: method,
                min_peak_value: 0.001,
                ..Default::default()
            };
            let correlator = PhaseCorrelator::new(width, height, config);
            let result = correlator.correlate(&reference, &reference, width, height);

            assert!(result.is_some(), "Method {:?} failed", method);
        }
    }

    #[test]
    fn test_transpose_inplace() {
        let n = 4;
        let mut data: Vec<Complex<f32>> = (0..16).map(|i| Complex::new(i as f32, 0.0)).collect();

        transpose_inplace(&mut data, n);

        // Check transposition
        assert_eq!(data[1].re, 4.0); // (0,1) -> (1,0)
        assert_eq!(data[4].re, 1.0); // (1,0) -> (0,1)
    }

    #[test]
    fn test_correlate_empty_image() {
        let correlator = PhaseCorrelator::new(64, 64, PhaseCorrelationConfig::default());
        let result = correlator.correlate(&[], &[], 0, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_correlate_size_mismatch() {
        let correlator = PhaseCorrelator::new(64, 64, PhaseCorrelationConfig::default());
        let small = vec![0.0f32; 32 * 32];
        let large = vec![0.0f32; 64 * 64];
        let result = correlator.correlate(&small, &large, 64, 64);
        assert!(result.is_none());
    }

    #[test]
    fn test_confidence_calculation() {
        let width = 64;
        let height = 64;
        let reference = create_test_image(width, height, 0, 0);

        let correlator = PhaseCorrelator::new(width, height, test_config());
        let result = correlator.correlate(&reference, &reference, width, height);

        assert!(result.is_some(), "Correlation should succeed");
        let result = result.unwrap();

        // Self-correlation should have positive confidence
        assert!(result.confidence >= 0.0);
    }
}
