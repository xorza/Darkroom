//! Transformation matrix for image registration.

pub(crate) mod linear_model;
pub(crate) mod local_geometry;

use std::fmt;
use std::fmt::{Display, Formatter};

use glam::{DMat2, DVec2};

use crate::math::dmat3::DMat3;
use crate::registration::transform::local_geometry::LocalGeometry;

/// The unit roundoff of f64: half the gap between 1 and the next float.
const UNIT_ROUNDOFF: f64 = f64::EPSILON / 2.0;
/// The largest coordinate magnitude a transform is asked to map, in pixels: 2²⁰, far past any
/// sensor and its dither or mosaic offset.
const COORDINATE_RANGE: f64 = 1_048_576.0;
/// How far normalizing a matrix may move a mapped point, in pixels: well under the registration's
/// sub-pixel residuals.
const NORMALIZATION_TOLERANCE_PX: f64 = 1e-3;
/// The smallest `|m[8]|`, relative to the largest entry, that a matrix is normalized from.
///
/// `m[8]` comes out of the same arithmetic as the entries beside it, so its absolute rounding
/// error is of order `u·M` for the largest entry `M`, and its relative error `u·M/|m[8]|`. Dividing
/// by it scales entries 0–7 by that error while `m[8]` itself becomes exactly 1, which moves a
/// mapped point by about `u·(M/|m[8]|)·|T(p)|`. Holding that to [`NORMALIZATION_TOLERANCE_PX`] for
/// `|T(p)|` up to [`COORDINATE_RANGE`] needs `|m[8]|/M ≥ u·COORDINATE_RANGE/tolerance`, about
/// 1.2e-7: a pure translation of over 8 million pixels, or a homography that sends the origin
/// that far.
const MIN_HOMOGENEOUS_SCALE: f64 = UNIT_ROUNDOFF * COORDINATE_RANGE / NORMALIZATION_TOLERANCE_PX;

/// A concrete transformation model, in increasing degrees of freedom.
///
/// Ordered by complexity, which [`Transform::compose`] uses to pick the more general of two. Every
/// variant here is something RANSAC can estimate and a [`Transform`] can hold; asking for a model
/// to be *chosen* is [`TransformModel::Auto`](crate::TransformModel::Auto), which is a different
/// question and a different type. No `Default`: nothing asks for "the" model, and picking one here
/// would be an answer invented to satisfy the derive. A caller with nothing to go on wants
/// [`TransformModel::Auto`](crate::TransformModel::Auto).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransformType {
    /// Translation only (2 DOF: dx, dy)
    Translation,
    /// Translation + Rotation (3 DOF: dx, dy, angle)
    Euclidean,
    /// Translation + Rotation + Uniform Scale (4 DOF)
    Similarity,
    /// Full affine (6 DOF: handles differential scaling and shear)
    Affine,
    /// Projective/Homography (8 DOF: handles perspective)
    Homography,
}

impl TransformType {
    /// The transform's free parameters: 2 for a translation, 3 with a rotation, 4 with a scale, 6
    /// for an affine map and 8 for a homography.
    pub const fn parameter_count(self) -> usize {
        match self {
            Self::Translation => 2,
            Self::Euclidean => 3,
            Self::Similarity => 4,
            Self::Affine => 6,
            Self::Homography => 8,
        }
    }

    /// Minimum number of point correspondences required to estimate this transform: each pair
    /// gives two equations, one per coordinate, so half the parameters, rounded up.
    pub const fn min_points(self) -> usize {
        self.parameter_count().div_ceil(2)
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

impl Display for Transform {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let t = self.translation_components();
        // At the origin, where a homography's own matrix entries act; every other model is the
        // same everywhere.
        let geometry = self.local_geometry(DVec2::ZERO);
        let rotation_deg = geometry.rotation.to_degrees();
        let [major, minor] = geometry.stretch;

        // Each model shows only the components it actually constrains: translation has no angle,
        // Euclidean no scale, a similarity one scale, and the two above it a stretch per axis.
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
            TransformType::Similarity => write!(
                f,
                "Similarity(dx={:.2}, dy={:.2}, rot={:.3}°, scale={:.4})",
                t.x, t.y, rotation_deg, major
            ),
            model => write!(
                f,
                "{model:?}(dx={:.2}, dy={:.2}, rot={:.3}°, stretch={:.4}/{:.4})",
                t.x, t.y, rotation_deg, major, minor
            ),
        }
    }
}

impl Transform {
    /// Create identity transform.
    pub const fn identity() -> Self {
        Self {
            matrix: DMat3::identity(),
            transform_type: TransformType::Translation,
        }
    }

    /// Create translation transform.
    pub const fn translation(t: DVec2) -> Self {
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
    pub const fn affine(params: [f64; 6]) -> Self {
        Self {
            matrix: DMat3::from_array([
                params[0], params[1], params[2], params[3], params[4], params[5], 0.0, 0.0, 1.0,
            ]),
            transform_type: TransformType::Affine,
        }
    }

    /// Create homography from 8 parameters (9th element is 1.0).
    pub const fn homography(params: [f64; 8]) -> Self {
        Self {
            matrix: DMat3::from_array([
                params[0], params[1], params[2], params[3], params[4], params[5], params[6],
                params[7], 1.0,
            ]),
            transform_type: TransformType::Homography,
        }
    }

    /// Create scale transform.
    pub const fn scale(s: DVec2) -> Self {
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

    /// Bring `matrix` to the representation every reader assumes, `m[8] = 1`; `None` when that
    /// division would cost more precision than [`MIN_HOMOGENEOUS_SCALE`] allows, or an entry is
    /// not finite.
    ///
    /// A model below [`TransformType::Homography`] carries no perspective row: every one of them
    /// is built with `m[6] = m[7] = 0`, and products and inverses of them keep those exact zeros.
    fn from_matrix(matrix: DMat3, transform_type: TransformType) -> Option<Self> {
        let entries = matrix.as_array();
        if !entries.iter().all(|value| value.is_finite()) {
            return None;
        }
        let largest = entries
            .iter()
            .fold(0.0f64, |acc, value| acc.max(value.abs()));
        let scale = entries[8];
        if scale == 0.0 || scale.abs() < MIN_HOMOGENEOUS_SCALE * largest {
            return None;
        }
        let matrix = DMat3::from_array(entries.map(|value| value / scale));
        debug_assert!(
            transform_type == TransformType::Homography || (matrix[6] == 0.0 && matrix[7] == 0.0),
            "a {transform_type:?} transform has no perspective row, got [{}, {}]",
            matrix[6],
            matrix[7]
        );
        Some(Self {
            matrix,
            transform_type,
        })
    }

    /// A homography from a matrix of arbitrary homogeneous scale, such as a DLT solve's null
    /// vector; `None` when [`Self::from_matrix`] cannot normalize it.
    pub(super) fn from_homography_matrix(matrix: DMat3) -> Option<Self> {
        Self::from_matrix(matrix, TransformType::Homography)
    }

    /// Row-major homogeneous matrix coefficients.
    pub const fn matrix(&self) -> &[f64; 9] {
        self.matrix.as_array()
    }

    /// The concrete model represented by this transform.
    pub const fn transform_type(&self) -> TransformType {
        self.transform_type
    }

    /// Apply transform to map a point from REFERENCE coordinates to TARGET coordinates.
    ///
    /// Given a transform T a registration of `ref_stars` to `target_stars` found:
    /// - `T.apply(ref_point)` gives the corresponding target point
    /// - `T.inverse().apply(target_point)` gives the corresponding reference point
    ///
    /// # Image Warping
    ///
    /// To align a target image to the reference frame (so it overlays correctly
    /// with the reference), you need to sample the target image at positions
    /// mapped from reference coordinates. This means using `apply()` to find
    /// where each reference pixel maps to in the target, then sampling there.
    ///
    /// [`WarpParams::warp`](crate::WarpParams::warp) handles this automatically.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use glam::DVec2;
    /// use lumos::{RegistrationConfig, Star};
    ///
    /// # fn example(ref_stars: &[Star], target_stars: &[Star], ref_pos: DVec2)
    /// # -> Result<(), Box<dyn std::error::Error>> {
    /// let result = RegistrationConfig::default().registrar()?.register(ref_stars, target_stars)?;
    /// let transform = result.transform();
    ///
    /// // Map a reference point to its corresponding target location
    /// let target_pos = transform.apply(ref_pos);
    ///
    /// // And back again
    /// let round_tripped = transform.inverse().apply(target_pos);
    /// # Ok(())
    /// # }
    /// ```
    pub const fn apply(&self, p: DVec2) -> DVec2 {
        self.matrix.transform_point(p)
    }

    /// The inverse transform, or `None` when the matrix is singular or, for a homography, its
    /// inverse sends the origin so far out that it cannot be normalized — see
    /// [`Self::is_valid`], which holds exactly when this is `Some`.
    #[must_use]
    pub fn try_inverse(&self) -> Option<Self> {
        Self::from_matrix(self.matrix.inverse()?, self.transform_type)
    }

    /// The inverse transform.
    ///
    /// # Panics
    /// When [`Self::try_inverse`] is `None`: the matrix is singular, or the inverse of a
    /// homography cannot be normalized.
    #[must_use]
    pub fn inverse(&self) -> Self {
        self.try_inverse()
            .expect("the transform is singular, or its inverse cannot be normalized")
    }

    /// Compose two transforms: `self · other`, which applies `other` first. The result is the more
    /// general of the two models.
    ///
    /// # Panics
    /// When the product is a homography that sends the origin too far out to be normalized.
    #[must_use]
    pub fn compose(&self, other: &Self) -> Self {
        self.try_compose(other)
            .expect("the product of two transforms cannot be normalized")
    }

    /// [`Self::compose`], or `None` when the product cannot be normalized — a fit from a
    /// near-degenerate sample can carry entries so large that its homogeneous scale vanishes
    /// beside them.
    pub fn try_compose(&self, other: &Self) -> Option<Self> {
        let transform_type = self.transform_type.max(other.transform_type);
        Self::from_matrix(self.matrix.mul_mat(&other.matrix), transform_type)
    }

    /// The Jacobian of [`Self::apply`] at `p`: how a small step at `p` moves its image.
    ///
    /// For `T(p) = (u, v) / w` with `u = a·x + b·y + c`, `v = d·x + e·y + f` and
    /// `w = g·x + h·y + 1`, it is `[[a − X·g, b − X·h], [d − Y·g, e − Y·h]] / w` at the image
    /// `(X, Y) = T(p)`; for every model below a homography `g = h = 0` and `w = 1`, so it is the
    /// constant linear part. Columns are the images of the x and y steps.
    pub fn jacobian(&self, p: DVec2) -> DMat2 {
        let m = self.matrix.as_array();
        let w = m[6] * p.x + m[7] * p.y + m[8];
        let image = self.apply(p);
        DMat2::from_cols(
            DVec2::new(m[0] - image.x * m[6], m[3] - image.y * m[6]) / w,
            DVec2::new(m[1] - image.x * m[7], m[4] - image.y * m[7]) / w,
        )
    }

    /// Extract translation components as `DVec2`.
    pub fn translation_components(&self) -> DVec2 {
        DVec2::new(self.matrix[2], self.matrix[5])
    }

    /// What the transform does to a small neighbourhood of `p`: the rotation and the stretch of
    /// its Jacobian there, one value everywhere for every model below a homography.
    pub fn local_geometry(&self, p: DVec2) -> LocalGeometry {
        LocalGeometry::of(self.jacobian(p))
    }

    /// Whether `p` maps to a real image: on the side of a homography's horizon the origin is on
    /// ([`DMat3::maps_forward`]). Every model below a homography maps every point.
    pub(crate) const fn maps_forward(&self, p: DVec2) -> bool {
        DMat3::maps_forward(self.matrix.weight_at(p))
    }

    /// Whether this is a usable transformation: every entry finite, and an inverse that
    /// [`Self::try_inverse`] can represent.
    pub fn is_valid(&self) -> bool {
        self.try_inverse().is_some()
    }
}

#[cfg(test)]
mod tests;
