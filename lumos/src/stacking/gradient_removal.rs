//! Gradient removal for astrophotography images.
//!
//! This module provides background gradient extraction and removal algorithms
//! commonly used in astrophotography post-processing. Gradients are caused by:
//! - Light pollution (ground-based sources)
//! - Moon glow
//! - Twilight
//! - Vignetting
//!
//! # Algorithms
//!
//! Two methods are supported:
//!
//! 1. **Polynomial fitting**: Fits a polynomial surface to background samples.
//!    Degrees 1-4 are supported. Lower degrees (1-2) handle simple linear gradients,
//!    higher degrees (3-4) can model more complex patterns but risk overcorrection.
//!
//! 2. **RBF (Radial Basis Function)**: Uses thin-plate spline interpolation for
//!    smooth, non-parametric background modeling. Better for complex, non-uniform
//!    gradients but requires more samples.
//!
//! # Workflow
//!
//! 1. **Sample placement**: Automatic grid-based sampling that avoids bright objects
//!    (stars, nebulae) by rejecting samples above a brightness threshold.
//!
//! 2. **Background model fitting**: Fit polynomial or RBF model to samples.
//!
//! 3. **Correction**: Either subtract (additive gradients like light pollution)
//!    or divide (multiplicative effects like vignetting).
//!
//! # Example
//!
//! ```ignore
//! use lumos::stacking::gradient_removal::{GradientRemovalConfig, remove_gradient};
//!
//! let config = GradientRemovalConfig::polynomial(2); // Quadratic fit
//! let corrected = remove_gradient(&image_pixels, width, height, &config)?;
//! ```

use crate::AstroImage;
use rayon::prelude::*;

/// Configuration for gradient removal.
#[derive(Debug, Clone)]
pub struct GradientRemovalConfig {
    /// The gradient model to use.
    pub model: GradientModel,
    /// Correction method (subtraction or division).
    pub correction: CorrectionMethod,
    /// Number of samples per line for automatic sample placement.
    /// Higher values = more samples = better accuracy but slower.
    /// Default: 16
    pub samples_per_line: usize,
    /// Brightness tolerance for sample rejection.
    /// Samples brighter than median + tolerance × sigma are rejected.
    /// Default: 1.0
    pub brightness_tolerance: f32,
    /// Minimum number of valid samples required.
    /// Default: 16
    pub min_samples: usize,
}

impl Default for GradientRemovalConfig {
    fn default() -> Self {
        Self {
            model: GradientModel::Polynomial(2),
            correction: CorrectionMethod::Subtract,
            samples_per_line: 16,
            brightness_tolerance: 1.0,
            min_samples: 16,
        }
    }
}

impl GradientRemovalConfig {
    /// Create a polynomial gradient removal configuration.
    ///
    /// # Arguments
    /// * `degree` - Polynomial degree (1-4). Degree 1 is linear, 2 is quadratic.
    ///
    /// # Panics
    /// Panics if degree is 0 or greater than 4.
    pub fn polynomial(degree: u8) -> Self {
        assert!((1..=4).contains(&degree), "Polynomial degree must be 1-4");
        Self {
            model: GradientModel::Polynomial(degree),
            ..Default::default()
        }
    }

    /// Create an RBF (thin-plate spline) gradient removal configuration.
    ///
    /// # Arguments
    /// * `smoothing` - Smoothing parameter (0.0-1.0). Higher values produce
    ///   smoother gradients but may miss localized variations. Default: 0.5
    ///
    /// # Panics
    /// Panics if smoothing is outside 0.0-1.0.
    pub fn rbf(smoothing: f32) -> Self {
        assert!(
            (0.0..=1.0).contains(&smoothing),
            "Smoothing must be 0.0-1.0"
        );
        Self {
            model: GradientModel::Rbf(smoothing),
            ..Default::default()
        }
    }

    /// Set the correction method.
    pub fn with_correction(mut self, correction: CorrectionMethod) -> Self {
        self.correction = correction;
        self
    }

    /// Set the number of samples per line.
    pub fn with_samples_per_line(mut self, samples: usize) -> Self {
        assert!(samples >= 4, "Samples per line must be at least 4");
        self.samples_per_line = samples;
        self
    }

    /// Set the brightness tolerance for sample rejection.
    pub fn with_brightness_tolerance(mut self, tolerance: f32) -> Self {
        assert!(tolerance > 0.0, "Brightness tolerance must be positive");
        self.brightness_tolerance = tolerance;
        self
    }

    /// Set the minimum number of valid samples.
    pub fn with_min_samples(mut self, min: usize) -> Self {
        assert!(min >= 3, "Minimum samples must be at least 3");
        self.min_samples = min;
        self
    }
}

/// Gradient model type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GradientModel {
    /// Polynomial surface fitting.
    /// The parameter is the polynomial degree (1-4).
    Polynomial(u8),
    /// Radial Basis Function (thin-plate spline) interpolation.
    /// The parameter is the smoothing factor (0.0-1.0).
    Rbf(f32),
}

impl Default for GradientModel {
    fn default() -> Self {
        Self::Polynomial(2)
    }
}

/// Method for correcting the gradient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CorrectionMethod {
    /// Subtract the gradient (for additive effects like light pollution).
    #[default]
    Subtract,
    /// Divide by the gradient (for multiplicative effects like vignetting).
    Divide,
}

/// Result of gradient removal.
#[derive(Debug)]
pub struct GradientRemovalResult {
    /// The corrected image.
    pub corrected: Vec<f32>,
    /// The extracted gradient model.
    pub gradient: Vec<f32>,
    /// Number of samples used for fitting.
    pub sample_count: usize,
    /// Sample positions (x, y).
    pub sample_positions: Vec<(usize, usize)>,
    /// Sample values.
    pub sample_values: Vec<f32>,
}

/// A background sample point.
#[derive(Debug, Clone, Copy)]
struct Sample {
    x: usize,
    y: usize,
    value: f32,
}

/// Remove gradient from an image.
///
/// # Arguments
/// * `pixels` - Image pixel data (single channel, row-major)
/// * `width` - Image width
/// * `height` - Image height
/// * `config` - Gradient removal configuration
///
/// # Returns
/// `GradientRemovalResult` containing corrected image and gradient model.
///
/// # Errors
/// Returns error if:
/// * Not enough valid samples found
/// * Model fitting fails
pub fn remove_gradient(
    pixels: &[f32],
    width: usize,
    height: usize,
    config: &GradientRemovalConfig,
) -> Result<GradientRemovalResult, GradientRemovalError> {
    assert_eq!(
        pixels.len(),
        width * height,
        "Pixel count must match dimensions"
    );

    // Generate sample positions
    let samples = generate_samples(pixels, width, height, config)?;

    if samples.len() < config.min_samples {
        return Err(GradientRemovalError::InsufficientSamples {
            found: samples.len(),
            required: config.min_samples,
        });
    }

    // Fit gradient model
    let gradient = match config.model {
        GradientModel::Polynomial(degree) => {
            fit_polynomial_gradient(&samples, width, height, degree)?
        }
        GradientModel::Rbf(smoothing) => fit_rbf_gradient(&samples, width, height, smoothing)?,
    };

    // Apply correction
    let corrected = apply_correction(pixels, &gradient, config.correction);

    Ok(GradientRemovalResult {
        corrected,
        gradient,
        sample_count: samples.len(),
        sample_positions: samples.iter().map(|s| (s.x, s.y)).collect(),
        sample_values: samples.iter().map(|s| s.value).collect(),
    })
}

/// Remove gradient from an image, returning only the corrected pixels.
///
/// This is a convenience function that discards the gradient model.
pub fn remove_gradient_simple(
    pixels: &[f32],
    width: usize,
    height: usize,
    config: &GradientRemovalConfig,
) -> Result<Vec<f32>, GradientRemovalError> {
    remove_gradient(pixels, width, height, config).map(|r| r.corrected)
}

/// Remove gradient from an AstroImage.
///
/// This is a convenience wrapper around [`remove_gradient`] that takes an
/// `AstroImage` instead of raw pixel data. For RGB images, gradient removal
/// is applied to each channel independently.
///
/// # Arguments
/// * `image` - Astronomical image (grayscale or RGB)
/// * `config` - Gradient removal configuration
///
/// # Returns
/// A new `AstroImage` with the gradient removed.
///
/// # Errors
/// Returns error if gradient fitting fails (insufficient samples, singular matrix).
///
/// # Example
/// ```rust,ignore
/// use lumos::{AstroImage, remove_gradient_image, GradientRemovalConfig};
///
/// let image = AstroImage::from_file("stacked.fits")?;
/// let config = GradientRemovalConfig::polynomial(2);
/// let corrected = remove_gradient_image(&image, &config)?;
/// ```
pub fn remove_gradient_image(
    image: &AstroImage,
    config: &GradientRemovalConfig,
) -> Result<AstroImage, GradientRemovalError> {
    let width = image.width();
    let height = image.height();
    let channels = image.channels();

    if channels == 1 {
        // Grayscale: apply directly
        let corrected = remove_gradient_simple(image.pixels(), width, height, config)?;
        Ok(AstroImage::from_pixels(width, height, channels, corrected))
    } else {
        // RGB: process each channel independently
        let pixels = image.pixels();
        let channel_size = width * height;
        let mut corrected = vec![0.0f32; channel_size * channels];

        for c in 0..channels {
            let channel_data: Vec<f32> = (0..channel_size)
                .map(|i| pixels[i * channels + c])
                .collect();

            let channel_corrected = remove_gradient_simple(&channel_data, width, height, config)?;

            for (i, &val) in channel_corrected.iter().enumerate() {
                corrected[i * channels + c] = val;
            }
        }

        Ok(AstroImage::from_pixels(width, height, channels, corrected))
    }
}

/// Generate background samples, avoiding bright regions.
fn generate_samples(
    pixels: &[f32],
    width: usize,
    height: usize,
    config: &GradientRemovalConfig,
) -> Result<Vec<Sample>, GradientRemovalError> {
    // Compute image statistics for brightness rejection
    let (median, sigma) = compute_robust_statistics(pixels);
    let threshold = median + config.brightness_tolerance * sigma;

    // Calculate sample spacing
    let spacing_x = width / config.samples_per_line;
    let spacing_y = height / config.samples_per_line;

    if spacing_x == 0 || spacing_y == 0 {
        return Err(GradientRemovalError::ImageTooSmall);
    }

    // Sample size for local median computation (5x5 box)
    let sample_radius = 2;

    let mut samples = Vec::new();

    for gy in 0..config.samples_per_line {
        for gx in 0..config.samples_per_line {
            let cx = (gx * spacing_x + spacing_x / 2).min(width - 1);
            let cy = (gy * spacing_y + spacing_y / 2).min(height - 1);

            // Compute local median in a small box
            let local_median = compute_local_median(pixels, width, height, cx, cy, sample_radius);

            // Reject if too bright
            if local_median <= threshold {
                samples.push(Sample {
                    x: cx,
                    y: cy,
                    value: local_median,
                });
            }
        }
    }

    Ok(samples)
}

/// Compute robust statistics (median and MAD-based sigma).
fn compute_robust_statistics(pixels: &[f32]) -> (f32, f32) {
    let mut sorted: Vec<f32> = pixels.iter().filter(|&&v| v.is_finite()).copied().collect();
    if sorted.is_empty() {
        return (0.0, 1.0);
    }

    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];

    // MAD (Median Absolute Deviation)
    let mut deviations: Vec<f32> = sorted.iter().map(|&v| (v - median).abs()).collect();
    deviations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mad = deviations[deviations.len() / 2];

    // Convert MAD to sigma equivalent (for Gaussian distribution)
    let sigma = mad * 1.4826;

    (median, sigma.max(0.001))
}

/// Compute local median in a box around (cx, cy).
fn compute_local_median(
    pixels: &[f32],
    width: usize,
    height: usize,
    cx: usize,
    cy: usize,
    radius: usize,
) -> f32 {
    let x0 = cx.saturating_sub(radius);
    let y0 = cy.saturating_sub(radius);
    let x1 = (cx + radius + 1).min(width);
    let y1 = (cy + radius + 1).min(height);

    let mut values = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            let v = pixels[y * width + x];
            if v.is_finite() {
                values.push(v);
            }
        }
    }

    if values.is_empty() {
        return 0.0;
    }

    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    values[values.len() / 2]
}

/// Fit a polynomial gradient model.
fn fit_polynomial_gradient(
    samples: &[Sample],
    width: usize,
    height: usize,
    degree: u8,
) -> Result<Vec<f32>, GradientRemovalError> {
    // Normalize coordinates to [-1, 1] for numerical stability
    let w = width as f64;
    let h = height as f64;

    // Build the design matrix
    let num_terms = polynomial_terms(degree);
    let n = samples.len();

    if n < num_terms {
        return Err(GradientRemovalError::InsufficientSamples {
            found: n,
            required: num_terms,
        });
    }

    // Design matrix A (n × num_terms)
    let mut a = vec![vec![0.0; num_terms]; n];
    let mut b = vec![0.0; n];

    for (i, sample) in samples.iter().enumerate() {
        // Normalize to [-1, 1]
        let x = 2.0 * sample.x as f64 / w - 1.0;
        let y = 2.0 * sample.y as f64 / h - 1.0;

        // Build polynomial terms
        a[i] = build_polynomial_terms(x, y, degree);
        b[i] = sample.value as f64;
    }

    // Solve using normal equations: (A^T A) c = A^T b
    let coeffs = solve_least_squares(&a, &b)?;

    // Generate gradient image
    let mut gradient = vec![0.0f32; width * height];

    gradient
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            let yn = 2.0 * y as f64 / h - 1.0;
            for (x, pixel) in row.iter_mut().enumerate() {
                let xn = 2.0 * x as f64 / w - 1.0;
                let terms = build_polynomial_terms(xn, yn, degree);
                let value: f64 = terms.iter().zip(&coeffs).map(|(t, c)| t * c).sum();
                *pixel = value as f32;
            }
        });

    Ok(gradient)
}

/// Get the number of terms for a polynomial of given degree.
fn polynomial_terms(degree: u8) -> usize {
    match degree {
        1 => 3,  // 1, x, y
        2 => 6,  // 1, x, y, x², xy, y²
        3 => 10, // 1, x, y, x², xy, y², x³, x²y, xy², y³
        4 => 15, // + x⁴, x³y, x²y², xy³, y⁴
        _ => 6,  // Default to quadratic
    }
}

/// Build polynomial terms for a point (x, y).
fn build_polynomial_terms(x: f64, y: f64, degree: u8) -> Vec<f64> {
    let mut terms = vec![1.0, x, y]; // Degree 1

    if degree >= 2 {
        terms.extend_from_slice(&[x * x, x * y, y * y]);
    }
    if degree >= 3 {
        terms.extend_from_slice(&[x * x * x, x * x * y, x * y * y, y * y * y]);
    }
    if degree >= 4 {
        terms.extend_from_slice(&[
            x * x * x * x,
            x * x * x * y,
            x * x * y * y,
            x * y * y * y,
            y * y * y * y,
        ]);
    }

    terms
}

/// Solve least squares using normal equations.
fn solve_least_squares(a: &[Vec<f64>], b: &[f64]) -> Result<Vec<f64>, GradientRemovalError> {
    let n = a.len();
    let m = a[0].len();

    // Compute A^T A
    let mut ata = vec![vec![0.0; m]; m];
    for i in 0..m {
        for j in 0..m {
            for row in a.iter().take(n) {
                ata[i][j] += row[i] * row[j];
            }
        }
    }

    // Compute A^T b
    let mut atb = vec![0.0; m];
    for i in 0..m {
        for (k, row) in a.iter().enumerate().take(n) {
            atb[i] += row[i] * b[k];
        }
    }

    // Solve using Gaussian elimination with partial pivoting
    solve_linear_system(&ata, &atb)
}

/// Solve a linear system using Gaussian elimination with partial pivoting.
#[allow(clippy::needless_range_loop)]
fn solve_linear_system(a: &[Vec<f64>], b: &[f64]) -> Result<Vec<f64>, GradientRemovalError> {
    let n = b.len();

    // Create augmented matrix
    let mut aug: Vec<Vec<f64>> = a
        .iter()
        .zip(b.iter())
        .map(|(row, &bi)| {
            let mut new_row = row.clone();
            new_row.push(bi);
            new_row
        })
        .collect();

    // Forward elimination with partial pivoting
    for col in 0..n {
        // Find pivot
        let mut max_row = col;
        let mut max_val = aug[col][col].abs();
        for row in (col + 1)..n {
            let val = aug[row][col].abs();
            if val > max_val {
                max_val = val;
                max_row = row;
            }
        }

        if max_val < 1e-12 {
            return Err(GradientRemovalError::SingularMatrix);
        }

        // Swap rows
        if max_row != col {
            aug.swap(col, max_row);
        }

        // Eliminate
        for row in (col + 1)..n {
            let factor = aug[row][col] / aug[col][col];
            for j in col..=n {
                aug[row][j] -= factor * aug[col][j];
            }
        }
    }

    // Back substitution
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        x[i] = aug[i][n];
        for j in (i + 1)..n {
            x[i] -= aug[i][j] * x[j];
        }
        x[i] /= aug[i][i];
    }

    Ok(x)
}

/// Fit an RBF (thin-plate spline) gradient model.
fn fit_rbf_gradient(
    samples: &[Sample],
    width: usize,
    height: usize,
    smoothing: f32,
) -> Result<Vec<f32>, GradientRemovalError> {
    // Build a custom scalar TPS (simpler than using the 2D one)
    let gradient = fit_scalar_tps(samples, width, height, smoothing)?;

    Ok(gradient)
}

/// Fit a scalar thin-plate spline for gradient modeling.
fn fit_scalar_tps(
    samples: &[Sample],
    width: usize,
    height: usize,
    smoothing: f32,
) -> Result<Vec<f32>, GradientRemovalError> {
    let n = samples.len();
    if n < 3 {
        return Err(GradientRemovalError::InsufficientSamples {
            found: n,
            required: 3,
        });
    }

    // Convert smoothing (0-1) to regularization parameter
    // Higher smoothing -> higher regularization
    let regularization = (smoothing as f64).powi(2) * 1000.0;

    // Build the TPS system matrix
    // [K + λI  P] [w]   [v]
    // [P^T     0] [a] = [0]

    let matrix_size = n + 3;
    let mut matrix = vec![vec![0.0; matrix_size]; matrix_size];

    // Fill K matrix with TPS kernel values
    for i in 0..n {
        for j in 0..n {
            if i == j {
                matrix[i][j] = regularization;
            } else {
                let dx = samples[i].x as f64 - samples[j].x as f64;
                let dy = samples[i].y as f64 - samples[j].y as f64;
                let r = (dx * dx + dy * dy).sqrt();
                matrix[i][j] = tps_kernel(r);
            }
        }
    }

    // Fill P matrix
    for i in 0..n {
        let x = samples[i].x as f64;
        let y = samples[i].y as f64;
        matrix[i][n] = 1.0;
        matrix[i][n + 1] = x;
        matrix[i][n + 2] = y;

        matrix[n][i] = 1.0;
        matrix[n + 1][i] = x;
        matrix[n + 2][i] = y;
    }

    // Right-hand side
    let mut rhs = vec![0.0; matrix_size];
    for i in 0..n {
        rhs[i] = samples[i].value as f64;
    }

    // Solve
    let solution = solve_linear_system(&matrix, &rhs)?;

    // Extract weights and affine coefficients
    let weights: Vec<f64> = solution[..n].to_vec();
    let affine = [solution[n], solution[n + 1], solution[n + 2]];

    // Generate gradient image
    let mut gradient = vec![0.0f32; width * height];

    gradient
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, pixel) in row.iter_mut().enumerate() {
                // Affine component
                let mut value = affine[0] + affine[1] * x as f64 + affine[2] * y as f64;

                // RBF component
                for (i, sample) in samples.iter().enumerate() {
                    let dx = x as f64 - sample.x as f64;
                    let dy = y as f64 - sample.y as f64;
                    let r = (dx * dx + dy * dy).sqrt();
                    value += weights[i] * tps_kernel(r);
                }

                *pixel = value as f32;
            }
        });

    Ok(gradient)
}

/// TPS radial basis function: U(r) = r² log(r)
#[inline]
fn tps_kernel(r: f64) -> f64 {
    if r < 1e-10 { 0.0 } else { r * r * r.ln() }
}

/// Apply correction to the image.
fn apply_correction(pixels: &[f32], gradient: &[f32], method: CorrectionMethod) -> Vec<f32> {
    match method {
        CorrectionMethod::Subtract => {
            // Subtract gradient, preserving the overall mean level
            let gradient_median = compute_median(gradient);
            pixels
                .par_iter()
                .zip(gradient.par_iter())
                .map(|(&p, &g)| p - (g - gradient_median))
                .collect()
        }
        CorrectionMethod::Divide => {
            // Divide by normalized gradient
            let gradient_mean: f32 = gradient.iter().sum::<f32>() / gradient.len() as f32;
            let gradient_mean = gradient_mean.max(0.001);

            pixels
                .par_iter()
                .zip(gradient.par_iter())
                .map(|(&p, &g)| {
                    let norm_g = (g / gradient_mean).max(0.001);
                    p / norm_g
                })
                .collect()
        }
    }
}

/// Compute median of a slice.
fn compute_median(values: &[f32]) -> f32 {
    let mut sorted: Vec<f32> = values.iter().filter(|&&v| v.is_finite()).copied().collect();
    if sorted.is_empty() {
        return 0.0;
    }
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sorted[sorted.len() / 2]
}

/// Errors that can occur during gradient removal.
#[derive(Debug, Clone)]
pub enum GradientRemovalError {
    /// Not enough valid samples found.
    InsufficientSamples { found: usize, required: usize },
    /// The image is too small for the requested sample density.
    ImageTooSmall,
    /// The system matrix is singular (cannot fit model).
    SingularMatrix,
}

impl std::fmt::Display for GradientRemovalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GradientRemovalError::InsufficientSamples { found, required } => {
                write!(
                    f,
                    "Insufficient background samples: found {}, need {}",
                    found, required
                )
            }
            GradientRemovalError::ImageTooSmall => {
                write!(f, "Image is too small for the requested sample density")
            }
            GradientRemovalError::SingularMatrix => {
                write!(f, "Cannot fit gradient model: singular matrix")
            }
        }
    }
}

impl std::error::Error for GradientRemovalError {}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== Configuration Tests ==========

    #[test]
    fn test_config_default() {
        let config = GradientRemovalConfig::default();
        assert!(matches!(config.model, GradientModel::Polynomial(2)));
        assert_eq!(config.correction, CorrectionMethod::Subtract);
        assert_eq!(config.samples_per_line, 16);
        assert!((config.brightness_tolerance - 1.0).abs() < f32::EPSILON);
        assert_eq!(config.min_samples, 16);
    }

    #[test]
    fn test_config_polynomial() {
        let config = GradientRemovalConfig::polynomial(3);
        assert!(matches!(config.model, GradientModel::Polynomial(3)));
    }

    #[test]
    #[should_panic(expected = "Polynomial degree must be 1-4")]
    fn test_config_polynomial_zero_panics() {
        GradientRemovalConfig::polynomial(0);
    }

    #[test]
    #[should_panic(expected = "Polynomial degree must be 1-4")]
    fn test_config_polynomial_five_panics() {
        GradientRemovalConfig::polynomial(5);
    }

    #[test]
    fn test_config_rbf() {
        let config = GradientRemovalConfig::rbf(0.5);
        assert!(matches!(config.model, GradientModel::Rbf(s) if (s - 0.5).abs() < f32::EPSILON));
    }

    #[test]
    #[should_panic(expected = "Smoothing must be 0.0-1.0")]
    fn test_config_rbf_negative_panics() {
        GradientRemovalConfig::rbf(-0.1);
    }

    #[test]
    #[should_panic(expected = "Smoothing must be 0.0-1.0")]
    fn test_config_rbf_above_one_panics() {
        GradientRemovalConfig::rbf(1.1);
    }

    #[test]
    fn test_config_with_correction() {
        let config = GradientRemovalConfig::default().with_correction(CorrectionMethod::Divide);
        assert_eq!(config.correction, CorrectionMethod::Divide);
    }

    #[test]
    fn test_config_with_samples_per_line() {
        let config = GradientRemovalConfig::default().with_samples_per_line(32);
        assert_eq!(config.samples_per_line, 32);
    }

    #[test]
    #[should_panic(expected = "Samples per line must be at least 4")]
    fn test_config_samples_per_line_too_low_panics() {
        GradientRemovalConfig::default().with_samples_per_line(3);
    }

    #[test]
    fn test_config_with_brightness_tolerance() {
        let config = GradientRemovalConfig::default().with_brightness_tolerance(2.0);
        assert!((config.brightness_tolerance - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    #[should_panic(expected = "Brightness tolerance must be positive")]
    fn test_config_brightness_tolerance_zero_panics() {
        GradientRemovalConfig::default().with_brightness_tolerance(0.0);
    }

    #[test]
    fn test_config_with_min_samples() {
        let config = GradientRemovalConfig::default().with_min_samples(10);
        assert_eq!(config.min_samples, 10);
    }

    #[test]
    #[should_panic(expected = "Minimum samples must be at least 3")]
    fn test_config_min_samples_too_low_panics() {
        GradientRemovalConfig::default().with_min_samples(2);
    }

    // ========== Gradient Model Tests ==========

    #[test]
    fn test_gradient_model_default() {
        let model = GradientModel::default();
        assert!(matches!(model, GradientModel::Polynomial(2)));
    }

    // ========== Correction Method Tests ==========

    #[test]
    fn test_correction_method_default() {
        let method = CorrectionMethod::default();
        assert_eq!(method, CorrectionMethod::Subtract);
    }

    // ========== Helper Function Tests ==========

    #[test]
    fn test_polynomial_terms() {
        assert_eq!(polynomial_terms(1), 3);
        assert_eq!(polynomial_terms(2), 6);
        assert_eq!(polynomial_terms(3), 10);
        assert_eq!(polynomial_terms(4), 15);
    }

    #[test]
    fn test_build_polynomial_terms_degree_1() {
        let terms = build_polynomial_terms(0.5, -0.3, 1);
        assert_eq!(terms.len(), 3);
        assert!((terms[0] - 1.0).abs() < 1e-10);
        assert!((terms[1] - 0.5).abs() < 1e-10);
        assert!((terms[2] - (-0.3)).abs() < 1e-10);
    }

    #[test]
    fn test_build_polynomial_terms_degree_2() {
        let terms = build_polynomial_terms(0.5, 0.5, 2);
        assert_eq!(terms.len(), 6);
        // 1, x, y, x², xy, y²
        assert!((terms[0] - 1.0).abs() < 1e-10);
        assert!((terms[1] - 0.5).abs() < 1e-10);
        assert!((terms[2] - 0.5).abs() < 1e-10);
        assert!((terms[3] - 0.25).abs() < 1e-10); // x²
        assert!((terms[4] - 0.25).abs() < 1e-10); // xy
        assert!((terms[5] - 0.25).abs() < 1e-10); // y²
    }

    #[test]
    fn test_compute_robust_statistics() {
        let pixels = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let (median, sigma) = compute_robust_statistics(&pixels);
        assert!((median - 3.0).abs() < 1e-5);
        assert!(sigma > 0.0);
    }

    #[test]
    fn test_compute_robust_statistics_uniform() {
        let pixels = vec![5.0; 100];
        let (median, sigma) = compute_robust_statistics(&pixels);
        assert!((median - 5.0).abs() < 1e-5);
        // MAD is 0, but we clamp sigma to minimum
        assert!(sigma >= 0.001);
    }

    #[test]
    fn test_compute_local_median() {
        let width = 10;
        let height = 10;
        let mut pixels = vec![0.0; width * height];
        // Set center pixel to 5
        pixels[5 * width + 5] = 5.0;

        let median = compute_local_median(&pixels, width, height, 5, 5, 2);
        // Most values are 0, so median should be 0
        assert!((median - 0.0).abs() < 1e-5);
    }

    #[test]
    fn test_compute_median() {
        let values = vec![1.0, 5.0, 3.0, 2.0, 4.0];
        let median = compute_median(&values);
        assert!((median - 3.0).abs() < 1e-5);
    }

    #[test]
    fn test_compute_median_empty() {
        let values: Vec<f32> = vec![];
        let median = compute_median(&values);
        assert!((median - 0.0).abs() < 1e-5);
    }

    #[test]
    fn test_tps_kernel_zero() {
        assert!((tps_kernel(0.0) - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_tps_kernel_nonzero() {
        let r: f64 = 2.0;
        let expected = r * r * r.ln();
        assert!((tps_kernel(r) - expected).abs() < 1e-10);
    }

    // ========== Integration Tests ==========

    #[test]
    fn test_remove_gradient_uniform_image() {
        // Uniform image should have minimal gradient removal
        let width = 64;
        let height = 64;
        let pixels = vec![100.0f32; width * height];

        let config = GradientRemovalConfig::polynomial(1)
            .with_samples_per_line(8)
            .with_min_samples(4);

        let result = remove_gradient(&pixels, width, height, &config).unwrap();

        // Corrected should be similar to original
        let avg: f32 = result.corrected.iter().sum::<f32>() / result.corrected.len() as f32;
        assert!(
            (avg - 100.0).abs() < 1.0,
            "Average should be ~100, got {}",
            avg
        );
    }

    #[test]
    fn test_remove_gradient_linear_gradient() {
        // Create linear gradient from left (50) to right (150)
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.0f32; width * height];

        for y in 0..height {
            for x in 0..width {
                let t = x as f32 / (width - 1) as f32;
                pixels[y * width + x] = 50.0 + 100.0 * t;
            }
        }

        let config = GradientRemovalConfig::polynomial(1)
            .with_samples_per_line(8)
            .with_min_samples(4)
            .with_brightness_tolerance(10.0); // High tolerance to accept all samples

        let result = remove_gradient(&pixels, width, height, &config).unwrap();

        // After removal, variance should be much lower
        let corrected_variance = compute_variance(&result.corrected);
        let original_variance = compute_variance(&pixels);

        assert!(
            corrected_variance < original_variance * 0.1,
            "Variance should be reduced by >90%, original: {}, corrected: {}",
            original_variance,
            corrected_variance
        );
    }

    #[test]
    fn test_remove_gradient_quadratic() {
        // Create quadratic gradient (brighter in center)
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.0f32; width * height];

        let cx = width as f32 / 2.0;
        let cy = height as f32 / 2.0;

        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let r2 = dx * dx + dy * dy;
                // Parabolic profile
                pixels[y * width + x] = 100.0 + 0.01 * r2;
            }
        }

        let config = GradientRemovalConfig::polynomial(2)
            .with_samples_per_line(8)
            .with_min_samples(6)
            .with_brightness_tolerance(10.0);

        let result = remove_gradient(&pixels, width, height, &config).unwrap();

        // After removal, variance should be much lower
        let corrected_variance = compute_variance(&result.corrected);
        let original_variance = compute_variance(&pixels);

        assert!(
            corrected_variance < original_variance * 0.5,
            "Quadratic variance should be reduced, original: {}, corrected: {}",
            original_variance,
            corrected_variance
        );
    }

    #[test]
    fn test_remove_gradient_rbf() {
        // Create simple gradient
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.0f32; width * height];

        for y in 0..height {
            for x in 0..width {
                pixels[y * width + x] = 50.0 + (x as f32) * 0.5 + (y as f32) * 0.3;
            }
        }

        let config = GradientRemovalConfig::rbf(0.5)
            .with_samples_per_line(8)
            .with_min_samples(4)
            .with_brightness_tolerance(10.0);

        let result = remove_gradient(&pixels, width, height, &config).unwrap();

        // Should reduce variance
        let corrected_variance = compute_variance(&result.corrected);
        let original_variance = compute_variance(&pixels);

        assert!(
            corrected_variance < original_variance * 0.5,
            "RBF should reduce variance"
        );
    }

    #[test]
    fn test_remove_gradient_divide_correction() {
        // Create multiplicative gradient (vignetting-like)
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.0f32; width * height];

        let cx = width as f32 / 2.0;
        let cy = height as f32 / 2.0;

        for y in 0..height {
            for x in 0..width {
                let dx = (x as f32 - cx) / cx;
                let dy = (y as f32 - cy) / cy;
                let r2 = dx * dx + dy * dy;
                // Vignetting: brighter in center, darker at edges
                let vignette = 1.0 - 0.3 * r2;
                pixels[y * width + x] = 100.0 * vignette;
            }
        }

        let config = GradientRemovalConfig::polynomial(2)
            .with_correction(CorrectionMethod::Divide)
            .with_samples_per_line(8)
            .with_min_samples(6)
            .with_brightness_tolerance(10.0);

        let result = remove_gradient(&pixels, width, height, &config).unwrap();

        // After division correction, variance should be reduced
        let corrected_variance = compute_variance(&result.corrected);
        let original_variance = compute_variance(&pixels);

        assert!(
            corrected_variance < original_variance * 0.5,
            "Division should reduce variance"
        );
    }

    #[test]
    fn test_remove_gradient_insufficient_samples() {
        // Test scenario: require more samples than available from a small grid
        // With 4 samples per line, we get at most 4x4=16 sample positions
        // Request 20 minimum samples - will fail
        let width = 64;
        let height = 64;
        let pixels = vec![100.0f32; width * height];

        let config = GradientRemovalConfig::polynomial(1)
            .with_samples_per_line(4) // 4x4 = 16 sample positions
            .with_min_samples(20); // Need 20 but only have 16

        let result = remove_gradient(&pixels, width, height, &config);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GradientRemovalError::InsufficientSamples { .. }
        ));
    }

    #[test]
    fn test_remove_gradient_simple() {
        let width = 64;
        let height = 64;
        let pixels = vec![100.0f32; width * height];

        let config = GradientRemovalConfig::polynomial(1)
            .with_samples_per_line(8)
            .with_min_samples(4);

        let corrected = remove_gradient_simple(&pixels, width, height, &config).unwrap();

        assert_eq!(corrected.len(), width * height);
    }

    #[test]
    fn test_gradient_removal_result_fields() {
        let width = 64;
        let height = 64;
        let pixels = vec![100.0f32; width * height];

        let config = GradientRemovalConfig::polynomial(1)
            .with_samples_per_line(8)
            .with_min_samples(4);

        let result = remove_gradient(&pixels, width, height, &config).unwrap();

        assert_eq!(result.corrected.len(), width * height);
        assert_eq!(result.gradient.len(), width * height);
        assert!(result.sample_count > 0);
        assert_eq!(result.sample_positions.len(), result.sample_count);
        assert_eq!(result.sample_values.len(), result.sample_count);
    }

    // ========== Error Display Tests ==========

    #[test]
    fn test_gradient_removal_error_display() {
        let err = GradientRemovalError::InsufficientSamples {
            found: 5,
            required: 10,
        };
        assert!(err.to_string().contains("5"));
        assert!(err.to_string().contains("10"));

        let err = GradientRemovalError::ImageTooSmall;
        assert!(err.to_string().contains("too small"));

        let err = GradientRemovalError::SingularMatrix;
        assert!(err.to_string().contains("singular"));
    }

    // Helper function for tests
    fn compute_variance(pixels: &[f32]) -> f32 {
        let n = pixels.len() as f32;
        let mean = pixels.iter().sum::<f32>() / n;
        pixels.iter().map(|&x| (x - mean).powi(2)).sum::<f32>() / n
    }
}
