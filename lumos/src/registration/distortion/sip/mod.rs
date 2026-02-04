//! SIP (Simple Imaging Polynomial) distortion correction.
//!
//! The SIP convention is the standard in astronomy for representing non-linear
//! geometric distortion in FITS image headers. It is used by Spitzer, HST,
//! Astrometry.net, Siril, and ASTAP.
//!
//! # Model
//!
//! Pixel coordinates (u, v) relative to a reference point are corrected by a 2D
//! polynomial before the linear (CD matrix / homography) transform:
//!
//! ```text
//! u' = u + Σ A_pq * u^p * v^q    (for 2 ≤ p+q ≤ order)
//! v' = v + Σ B_pq * u^p * v^q    (for 2 ≤ p+q ≤ order)
//! ```
//!
//! Linear terms (p+q < 2) are excluded because they are already captured by
//! the homography / CD matrix.
//!
//! # Coefficient counts by order
//!
//! | Order | Terms per axis | Description |
//! |-------|---------------|-------------|
//! | 2     | 3             | Barrel/pincushion (u², uv, v²) |
//! | 3     | 7             | + mustache distortion |
//! | 4     | 12            | + higher-order |
//! | 5     | 18            | Full SIP (HST-level) |
//!
//! # Usage
//!
//! ```ignore
//! use glam::DVec2;
//! use lumos::registration::distortion::{SipPolynomial, SipConfig};
//!
//! // After RANSAC finds homography + inliers, collect residual vectors
//! let ref_points: Vec<DVec2> = /* inlier reference positions */;
//! let residuals: Vec<DVec2> = /* transform(ref) - target for each inlier */;
//!
//! let config = SipConfig { order: 3, ..Default::default() };
//! let sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();
//!
//! // Apply correction to a point before the homography
//! let corrected = sip.correct(DVec2::new(100.0, 200.0));
//! ```

use glam::DVec2;

/// Configuration for SIP polynomial fitting.
#[derive(Debug, Clone)]
pub struct SipConfig {
    /// Polynomial order (2-5). Order 2 handles barrel/pincushion,
    /// order 3 handles mustache distortion.
    pub order: usize,
    /// Reference point for the polynomial (typically image center).
    /// Coordinates are relative to this point before polynomial evaluation.
    /// If None, the centroid of the input points is used.
    pub reference_point: Option<DVec2>,
}

impl Default for SipConfig {
    fn default() -> Self {
        Self {
            order: 3,
            reference_point: None,
        }
    }
}

impl SipConfig {
    /// Validate configuration.
    pub fn validate(&self) {
        assert!(
            (2..=5).contains(&self.order),
            "SIP order must be 2-5, got {}",
            self.order
        );
    }
}

/// Number of polynomial terms for a given order (excluding linear and constant).
/// Terms satisfy: 2 ≤ p+q ≤ order, p ≥ 0, q ≥ 0
fn num_terms(order: usize) -> usize {
    // Total terms with 0 ≤ p+q ≤ order is (order+1)(order+2)/2
    // Minus the 3 terms with p+q < 2 (constant, u, v)
    (order + 1) * (order + 2) / 2 - 3
}

/// Generate the list of (p, q) exponent pairs for a given order.
/// Only includes terms where 2 ≤ p+q ≤ order.
fn term_exponents(order: usize) -> Vec<(usize, usize)> {
    let mut terms = Vec::with_capacity(num_terms(order));
    for total in 2..=order {
        for p in (0..=total).rev() {
            let q = total - p;
            terms.push((p, q));
        }
    }
    debug_assert_eq!(terms.len(), num_terms(order));
    terms
}

/// Evaluate a monomial u^p * v^q.
#[inline]
fn monomial(u: f64, v: f64, p: usize, q: usize) -> f64 {
    u.powi(p as i32) * v.powi(q as i32)
}

/// SIP polynomial distortion correction.
///
/// Stores the forward correction polynomials: given pixel coordinates (u, v)
/// relative to the reference point, computes the distortion correction
/// (du, dv) to apply before the linear transform.
///
/// Internally, coordinates are normalized by a scale factor for numerical
/// stability. The coefficients are stored in normalized space.
#[derive(Debug, Clone)]
pub struct SipPolynomial {
    /// Polynomial order (2-5).
    order: usize,
    /// Reference point (coordinates are relative to this).
    reference_point: DVec2,
    /// Normalization scale: normalized = (pixel - ref_pt) / norm_scale.
    /// Chosen so that the average distance from ref_pt is ~1.0 in normalized space.
    norm_scale: f64,
    /// Coefficients for the u-correction polynomial (A_pq) in normalized space.
    /// The output correction is also in normalized space and must be scaled back.
    coeffs_u: Vec<f64>,
    /// Coefficients for the v-correction polynomial (B_pq) in normalized space.
    coeffs_v: Vec<f64>,
}

impl SipPolynomial {
    /// Fit SIP polynomial to residual vectors after a linear transform.
    ///
    /// Given matched inlier positions in the reference frame and their
    /// residual errors after applying a homography, fits a polynomial
    /// correction to minimize those residuals.
    ///
    /// # Arguments
    /// * `ref_points` - Inlier reference positions (in pixel coordinates)
    /// * `residuals` - Residual vectors: `homography(ref) - target` for each inlier
    /// * `config` - SIP configuration (order, reference point)
    ///
    /// # Returns
    /// Fitted SIP polynomial, or None if the system is underdetermined or singular.
    pub fn fit_residuals(
        ref_points: &[DVec2],
        residuals: &[DVec2],
        config: &SipConfig,
    ) -> Option<Self> {
        config.validate();
        let n = ref_points.len();
        assert_eq!(
            n,
            residuals.len(),
            "ref_points and residuals must have the same length"
        );

        let n_terms = num_terms(config.order);

        // Need at least as many points as terms
        if n < n_terms {
            return None;
        }

        let terms = term_exponents(config.order);

        // Compute reference point
        let ref_pt = config.reference_point.unwrap_or_else(|| {
            let sum: DVec2 = ref_points.iter().sum();
            sum / n as f64
        });

        // Compute normalization scale: average distance from reference point.
        // This keeps the polynomial basis values near O(1) for numerical stability.
        let avg_dist = ref_points
            .iter()
            .map(|p| (*p - ref_pt).length())
            .sum::<f64>()
            / n as f64;
        let norm_scale = if avg_dist > 1e-10 { avg_dist } else { 1.0 };

        // Build the normal equations A^T*A * x = A^T*b in normalized coordinates.
        // We normalize both the input coordinates and the residuals by norm_scale.
        let mut ata = vec![0.0; n_terms * n_terms];
        let mut atb_u = vec![0.0; n_terms];
        let mut atb_v = vec![0.0; n_terms];

        for i in 0..n {
            let u = (ref_points[i].x - ref_pt.x) / norm_scale;
            let v = (ref_points[i].y - ref_pt.y) / norm_scale;

            // Residuals are also normalized so coefficients stay well-conditioned
            let res_u = -residuals[i].x / norm_scale;
            let res_v = -residuals[i].y / norm_scale;

            let basis: Vec<f64> = terms.iter().map(|&(p, q)| monomial(u, v, p, q)).collect();

            for j in 0..n_terms {
                for k in j..n_terms {
                    let val = basis[j] * basis[k];
                    ata[j * n_terms + k] += val;
                    if k != j {
                        ata[k * n_terms + j] += val;
                    }
                }

                atb_u[j] += basis[j] * res_u;
                atb_v[j] += basis[j] * res_v;
            }
        }

        // Solve A^T*A * x = A^T*b using Cholesky decomposition
        let coeffs_u = solve_symmetric_positive(&ata, &atb_u, n_terms)?;
        let coeffs_v = solve_symmetric_positive(&ata, &atb_v, n_terms)?;

        Some(Self {
            order: config.order,
            reference_point: ref_pt,
            norm_scale,
            coeffs_u,
            coeffs_v,
        })
    }

    /// Fit SIP polynomial directly from matched point pairs and a transform.
    ///
    /// Convenience method that computes residuals internally.
    ///
    /// # Arguments
    /// * `ref_points` - Reference positions
    /// * `target_points` - Target positions
    /// * `transform` - The linear transform (homography) mapping ref → target
    /// * `config` - SIP configuration
    pub fn fit_from_transform(
        ref_points: &[DVec2],
        target_points: &[DVec2],
        transform: &crate::registration::transform::Transform,
        config: &SipConfig,
    ) -> Option<Self> {
        assert_eq!(
            ref_points.len(),
            target_points.len(),
            "ref_points and target_points must have the same length"
        );

        let residuals: Vec<DVec2> = ref_points
            .iter()
            .zip(target_points.iter())
            .map(|(&r, &t)| transform.apply(r) - t)
            .collect();

        Self::fit_residuals(ref_points, &residuals, config)
    }

    /// Apply the SIP correction to a point.
    ///
    /// Returns the corrected coordinates: `p + (du, dv)` where `(du, dv)`
    /// is the polynomial correction evaluated in normalized coordinates,
    /// then scaled back to pixel space.
    pub fn correct(&self, p: DVec2) -> DVec2 {
        p + self.correction_at(p)
    }

    /// Apply the SIP correction to multiple points.
    pub fn correct_points(&self, points: &[DVec2]) -> Vec<DVec2> {
        points.iter().map(|&p| self.correct(p)).collect()
    }

    /// Evaluate just the correction vector (du, dv) at a point in pixel space.
    ///
    /// Internally normalizes coordinates, evaluates the polynomial, and
    /// scales the result back to pixel space.
    pub fn correction_at(&self, p: DVec2) -> DVec2 {
        // Normalize to the same space used during fitting
        let u = (p.x - self.reference_point.x) / self.norm_scale;
        let v = (p.y - self.reference_point.y) / self.norm_scale;

        let terms = term_exponents(self.order);

        let mut du_norm = 0.0;
        let mut dv_norm = 0.0;
        for (i, &(exp_p, exp_q)) in terms.iter().enumerate() {
            let basis = monomial(u, v, exp_p, exp_q);
            du_norm += self.coeffs_u[i] * basis;
            dv_norm += self.coeffs_v[i] * basis;
        }

        // Scale correction back to pixel space
        DVec2::new(du_norm * self.norm_scale, dv_norm * self.norm_scale)
    }

    /// Compute residuals after applying SIP correction.
    ///
    /// For each point, computes `|transform(sip_correct(ref)) - target|`.
    pub fn compute_corrected_residuals(
        &self,
        ref_points: &[DVec2],
        target_points: &[DVec2],
        transform: &crate::registration::transform::Transform,
    ) -> Vec<f64> {
        ref_points
            .iter()
            .zip(target_points.iter())
            .map(|(&r, &t)| {
                let corrected = self.correct(r);
                let mapped = transform.apply(corrected);
                (mapped - t).length()
            })
            .collect()
    }

    /// Get the polynomial order.
    pub fn order(&self) -> usize {
        self.order
    }

    /// Get the reference point.
    pub fn reference_point(&self) -> DVec2 {
        self.reference_point
    }

    /// Get the u-correction coefficients (A_pq).
    pub fn coeffs_u(&self) -> &[f64] {
        &self.coeffs_u
    }

    /// Get the v-correction coefficients (B_pq).
    pub fn coeffs_v(&self) -> &[f64] {
        &self.coeffs_v
    }

    /// Get the maximum correction magnitude across a grid of points.
    ///
    /// Useful for assessing the magnitude of the distortion correction.
    pub fn max_correction(&self, width: usize, height: usize, grid_spacing: f64) -> f64 {
        let mut max_mag = 0.0f64;
        let mut y = 0.0;
        while y <= height as f64 {
            let mut x = 0.0;
            while x <= width as f64 {
                let c = self.correction_at(DVec2::new(x, y));
                max_mag = max_mag.max(c.length());
                x += grid_spacing;
            }
            y += grid_spacing;
        }
        max_mag
    }
}

/// Solve a symmetric positive definite system Ax = b using Cholesky decomposition.
///
/// `a` is stored as a flat row-major n×n matrix.
#[allow(clippy::needless_range_loop)]
fn solve_symmetric_positive(a: &[f64], b: &[f64], n: usize) -> Option<Vec<f64>> {
    debug_assert_eq!(a.len(), n * n);
    debug_assert_eq!(b.len(), n);

    // Cholesky decomposition: A = L * L^T
    let mut l = vec![0.0; n * n];

    for i in 0..n {
        for j in 0..=i {
            let mut sum = 0.0;
            for k in 0..j {
                sum += l[i * n + k] * l[j * n + k];
            }

            if i == j {
                let diag = a[i * n + i] - sum;
                if diag <= 0.0 {
                    // Not positive definite — fall back to LU
                    return solve_lu(a, b, n);
                }
                l[i * n + j] = diag.sqrt();
            } else {
                l[i * n + j] = (a[i * n + j] - sum) / l[j * n + j];
            }
        }
    }

    // Forward substitution: L * y = b
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut sum = 0.0;
        for j in 0..i {
            sum += l[i * n + j] * y[j];
        }
        y[i] = (b[i] - sum) / l[i * n + i];
    }

    // Back substitution: L^T * x = y
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut sum = 0.0;
        for j in (i + 1)..n {
            sum += l[j * n + i] * x[j];
        }
        x[i] = (y[i] - sum) / l[i * n + i];
    }

    Some(x)
}

/// Fallback LU decomposition solver with partial pivoting.
#[allow(clippy::needless_range_loop)]
fn solve_lu(a: &[f64], b: &[f64], n: usize) -> Option<Vec<f64>> {
    // Create augmented matrix
    let mut aug = vec![0.0; n * (n + 1)];
    for i in 0..n {
        for j in 0..n {
            aug[i * (n + 1) + j] = a[i * n + j];
        }
        aug[i * (n + 1) + n] = b[i];
    }

    // Forward elimination with partial pivoting
    for col in 0..n {
        // Find pivot
        let mut max_row = col;
        let mut max_val = aug[col * (n + 1) + col].abs();
        for row in (col + 1)..n {
            let val = aug[row * (n + 1) + col].abs();
            if val > max_val {
                max_val = val;
                max_row = row;
            }
        }

        if max_val < 1e-12 {
            return None; // Singular
        }

        // Swap rows
        if max_row != col {
            for j in 0..=n {
                let idx_a = col * (n + 1) + j;
                let idx_b = max_row * (n + 1) + j;
                aug.swap(idx_a, idx_b);
            }
        }

        // Eliminate
        for row in (col + 1)..n {
            let factor = aug[row * (n + 1) + col] / aug[col * (n + 1) + col];
            for j in col..=n {
                aug[row * (n + 1) + j] -= factor * aug[col * (n + 1) + j];
            }
        }
    }

    // Back substitution
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        x[i] = aug[i * (n + 1) + n];
        for j in (i + 1)..n {
            x[i] -= aug[i * (n + 1) + j] * x[j];
        }
        x[i] /= aug[i * (n + 1) + i];
    }

    Some(x)
}

#[cfg(test)]
mod tests;
