//! Transformation matrix for image registration.

use glam::DVec2;

use crate::math::dmat3::DMat3;
use crate::stacking::registration::distortion::sip::SipPolynomial;

/// A concrete transformation model, in increasing degrees of freedom.
///
/// Ordered by complexity, which [`Transform::compose`] uses to pick the more general of two.
/// Every variant here is something RANSAC can estimate and a [`Transform`] can hold; asking for a
/// model to be *chosen* is [`TransformModel::Auto`], which is a different question and a
/// different type.
/// No `Default`: nothing asks for "the" model, and picking one here would be an answer invented
/// to satisfy the derive. A caller with nothing to go on wants [`TransformModel::Auto`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransformType {
    /// Translation only (2 DOF: dx, dy)
    Translation = 0,
    /// Translation + Rotation (3 DOF: dx, dy, angle)
    Euclidean = 1,
    /// Translation + Rotation + Uniform Scale (4 DOF)
    Similarity = 2,
    /// Full affine (6 DOF: handles differential scaling and shear)
    Affine = 3,
    /// Projective/Homography (8 DOF: handles perspective)
    Homography = 4,
}

impl TransformType {
    /// Minimum number of point correspondences required to estimate this transform.
    pub fn min_points(&self) -> usize {
        match self {
            TransformType::Translation => 1,
            TransformType::Euclidean => 2,
            TransformType::Similarity => 2,
            TransformType::Affine => 3,
            TransformType::Homography => 4,
        }
    }
}

/// Which model a registration should fit: a specific one, or a request to choose.
///
/// Kept apart from [`TransformType`] so that "pick a model for me" cannot reach the places that
/// can only act on a chosen one — RANSAC, transform estimation, and [`Transform`] itself, each of
/// which used to carry its own arm rejecting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransformModel {
    /// Fit exactly this model.
    Fixed(TransformType),
    /// Ladder Euclidean → Similarity → Affine → Homography, accepting the first fit within
    /// [`AUTO_UPGRADE_THRESHOLD`](crate::stacking::registration::tuning::AUTO_UPGRADE_THRESHOLD).
    #[default]
    Auto,
}

impl TransformModel {
    /// The most general model this could resolve to — `Auto`'s ceiling, or the fixed choice.
    ///
    /// What the star-count and match-count gates size against, since a run that may climb to
    /// homography has to arrive with enough points to fit one.
    pub fn most_general(self) -> TransformType {
        match self {
            Self::Fixed(transform_type) => transform_type,
            Self::Auto => TransformType::Homography,
        }
    }
}

/// 3x3 homogeneous transformation matrix.
///
/// Coefficients are exposed in row-major order:
/// ```text
/// | a  b  tx |   | m[0] m[1] m[2] |
/// | c  d  ty | = | m[3] m[4] m[5] |
/// | g  h  1  |   | m[6] m[7] m[8] |
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Transform {
    matrix: DMat3,
    transform_type: TransformType,
}

impl Default for Transform {
    fn default() -> Self {
        Self::identity()
    }
}

impl std::fmt::Display for Transform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let t = self.translation_components();
        let rotation_deg = self.rotation_angle().to_degrees();
        let scale = self.scale_factor();

        // Each model shows only the components it actually constrains: translation has no angle,
        // Euclidean no scale. The three above it print the same four, differing only in the name.
        match self.transform_type {
            TransformType::Translation => {
                write!(f, "Translation(dx={:.2}, dy={:.2})", t.x, t.y)
            }
            TransformType::Euclidean => {
                write!(
                    f,
                    "Euclidean(dx={:.2}, dy={:.2}, rot={:.3}°)",
                    t.x, t.y, rotation_deg
                )
            }
            model => write!(
                f,
                "{model:?}(dx={:.2}, dy={:.2}, rot={:.3}°, scale={:.4})",
                t.x, t.y, rotation_deg, scale
            ),
        }
    }
}

impl Transform {
    /// Create identity transform.
    pub fn identity() -> Self {
        Self {
            matrix: DMat3::identity(),
            transform_type: TransformType::Translation,
        }
    }

    /// Create translation transform.
    pub fn translation(t: DVec2) -> Self {
        Self {
            matrix: DMat3::from_array([1.0, 0.0, t.x, 0.0, 1.0, t.y, 0.0, 0.0, 1.0]),
            transform_type: TransformType::Translation,
        }
    }

    /// Create Euclidean transform (translation + rotation).
    pub fn euclidean(t: DVec2, angle: f64) -> Self {
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        Self {
            matrix: DMat3::from_array([cos_a, -sin_a, t.x, sin_a, cos_a, t.y, 0.0, 0.0, 1.0]),
            transform_type: TransformType::Euclidean,
        }
    }

    /// Create similarity transform (translation + rotation + uniform scale).
    pub fn similarity(t: DVec2, angle: f64, scale: f64) -> Self {
        let cos_a = angle.cos() * scale;
        let sin_a = angle.sin() * scale;
        Self {
            matrix: DMat3::from_array([cos_a, -sin_a, t.x, sin_a, cos_a, t.y, 0.0, 0.0, 1.0]),
            transform_type: TransformType::Similarity,
        }
    }

    /// Create affine transform from 6 parameters [a, b, tx, c, d, ty].
    pub fn affine(params: [f64; 6]) -> Self {
        Self {
            matrix: DMat3::from_array([
                params[0], params[1], params[2], params[3], params[4], params[5], 0.0, 0.0, 1.0,
            ]),
            transform_type: TransformType::Affine,
        }
    }

    /// Create homography from 8 parameters (9th element is 1.0).
    pub fn homography(params: [f64; 8]) -> Self {
        Self {
            matrix: DMat3::from_array([
                params[0], params[1], params[2], params[3], params[4], params[5], params[6],
                params[7], 1.0,
            ]),
            transform_type: TransformType::Homography,
        }
    }

    /// Create scale transform.
    pub fn scale(s: DVec2) -> Self {
        Self {
            matrix: DMat3::from_array([s.x, 0.0, 0.0, 0.0, s.y, 0.0, 0.0, 0.0, 1.0]),
            transform_type: TransformType::Affine,
        }
    }

    /// Create rotation transform around a specified center point.
    pub fn rotation_around(center: DVec2, angle: f64) -> Self {
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        // T(-cx,-cy) * R(angle) * T(cx,cy)
        let tx = center.x - cos_a * center.x + sin_a * center.y;
        let ty = center.y - sin_a * center.x - cos_a * center.y;
        Self {
            matrix: DMat3::from_array([cos_a, -sin_a, tx, sin_a, cos_a, ty, 0.0, 0.0, 1.0]),
            transform_type: TransformType::Euclidean,
        }
    }

    fn from_matrix(mut matrix: DMat3, transform_type: TransformType) -> Self {
        if transform_type != TransformType::Homography {
            assert!(
                matrix[6].abs() <= 1e-12
                    && matrix[7].abs() <= 1e-12
                    && (matrix[8] - 1.0).abs() <= 1e-12,
                "affine-or-simpler transforms require homogeneous bottom row [0, 0, 1]"
            );
            matrix[6] = 0.0;
            matrix[7] = 0.0;
            matrix[8] = 1.0;
        }
        Self {
            matrix,
            transform_type,
        }
    }

    /// Preserve the arbitrary homogeneous scale produced by the DLT solver.
    pub(crate) fn from_homography_matrix(matrix: DMat3) -> Self {
        Self::from_matrix(matrix, TransformType::Homography)
    }

    /// Row-major homogeneous matrix coefficients.
    pub fn matrix(&self) -> &[f64; 9] {
        self.matrix.as_array()
    }

    /// The concrete model represented by this transform.
    pub fn transform_type(&self) -> TransformType {
        self.transform_type
    }

    /// Apply transform to map a point from REFERENCE coordinates to TARGET coordinates.
    ///
    /// Given a transform T estimated from `register_stars(ref_stars, target_stars)`:
    /// - `T.apply(ref_point)` gives the corresponding target point
    /// - `T.apply_inverse(target_point)` gives the corresponding reference point
    ///
    /// # Image Warping
    ///
    /// To align a target image to the reference frame (so it overlays correctly
    /// with the reference), you need to sample the target image at positions
    /// mapped from reference coordinates. This means using `apply()` to find
    /// where each reference pixel maps to in the target, then sampling there.
    ///
    /// The [`crate::stacking::registration::resample::warp`] function handles this automatically.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let result = registrator.register_stars(&ref_stars, &target_stars)?;
    /// let transform = result.transform();
    ///
    /// // Map a reference point to its corresponding target location
    /// let target_pos = transform.apply(ref_pos);
    ///
    /// // Map a target point back to reference coordinates
    /// let ref_pos = transform.apply_inverse(target_pos);
    /// ```
    pub fn apply(&self, p: DVec2) -> DVec2 {
        self.matrix.transform_point(p)
    }

    /// Apply inverse transform to map a point from TARGET coordinates to REFERENCE coordinates.
    ///
    /// This is the inverse of `apply()`. Given a point in the target image,
    /// it returns the corresponding point in the reference image.
    ///
    /// See [`apply`](Self::apply) for more details on transform direction.
    pub fn apply_inverse(&self, p: DVec2) -> DVec2 {
        self.inverse().apply(p)
    }

    /// Compute matrix inverse.
    ///
    /// # Panics
    /// Panics if the matrix is singular (determinant near zero).
    pub fn inverse(&self) -> Self {
        let inv = self
            .matrix
            .inverse()
            .expect("Cannot invert singular transform matrix");
        Self::from_matrix(inv, self.transform_type)
    }

    /// Compose two transforms: self * other (apply other first, then self).
    pub fn compose(&self, other: &Self) -> Self {
        // Result type is the more complex of the two
        let transform_type = self.transform_type.max(other.transform_type);

        Self::from_matrix(self.matrix.mul_mat(&other.matrix), transform_type)
    }

    /// Extract translation components as DVec2.
    pub fn translation_components(&self) -> DVec2 {
        DVec2::new(self.matrix[2], self.matrix[5])
    }

    /// Extract rotation angle in radians (valid for Euclidean/Similarity transforms).
    pub fn rotation_angle(&self) -> f64 {
        self.matrix[3].atan2(self.matrix[0])
    }

    /// Extract scale factor (valid for Similarity transforms).
    pub fn scale_factor(&self) -> f64 {
        let a = self.matrix[0];
        let c = self.matrix[3];
        (a * a + c * c).sqrt()
    }

    /// Check if this is a valid (non-degenerate) transformation.
    ///
    /// Requires every matrix element finite (an isolated NaN/inf in a translation
    /// or perspective term would otherwise slip past a finite-determinant check)
    /// and the represented transform non-singular.
    pub fn is_valid(&self) -> bool {
        if !(0..9).all(|i| self.matrix[i].is_finite()) {
            return false;
        }
        let det = if self.transform_type == TransformType::Homography {
            self.matrix.determinant()
        } else {
            self.matrix[0] * self.matrix[4] - self.matrix[1] * self.matrix[3]
        };
        det.is_finite() && det.abs() > 1e-10
    }
}

/// Combined transform + optional SIP distortion correction for warping.
///
/// Bundles a linear `Transform` with an optional `SipPolynomial` so that
/// callers of `warp()` cannot forget to include the SIP correction.
/// For each output pixel `p`, the source coordinate is:
/// `src = transform.apply(sip.correct(p))` when SIP is present,
/// or `src = transform.apply(p)` otherwise.
#[derive(Debug, Clone)]
pub struct WarpTransform {
    pub transform: Transform,
    pub sip: Option<SipPolynomial>,
}

impl WarpTransform {
    /// Create a warp transform with no SIP correction.
    pub fn new(transform: Transform) -> Self {
        Self {
            transform,
            sip: None,
        }
    }

    /// Create a warp transform with SIP distortion correction.
    pub fn with_sip(transform: Transform, sip: SipPolynomial) -> Self {
        Self {
            transform,
            sip: Some(sip),
        }
    }

    /// Compute the source coordinate for a given output pixel position.
    pub fn apply(&self, p: DVec2) -> DVec2 {
        let corrected = match &self.sip {
            Some(sip) => sip.correct(p),
            None => p,
        };
        self.transform.apply(corrected)
    }

    /// Whether this transform has a nonlinear SIP component.
    pub fn has_sip(&self) -> bool {
        self.sip.is_some()
    }

    /// Whether this transform is purely linear (affine or simpler, no SIP).
    /// When true, incremental stepping and SIMD can be used.
    pub fn is_linear(&self) -> bool {
        self.sip.is_none() && self.transform.transform_type() != TransformType::Homography
    }
}

#[cfg(test)]
mod internals {
    use super::*;

    impl Transform {
        /// Frobenius norm of the difference from the identity matrix. Test-only diagnostic.
        pub(crate) fn deviation_from_identity(&self) -> f64 {
            self.matrix.deviation_from_identity()
        }
    }
}

#[cfg(test)]
mod tests;
