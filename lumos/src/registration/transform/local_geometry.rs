//! [`LocalGeometry`]: what a transform does to a small neighbourhood of a point.

use glam::DMat2;

/// The polar decomposition of a transform's Jacobian at a point, `J = R(θ)·S`: the rotation `θ`
/// and the stretch along the two principal axes of `S`, its singular values.
///
/// For `J = [[a, b], [c, d]]` they have a closed form: with `E = |(a + d, c − b)|` and
/// `F = |(a − d, c + b)|`, the stretches are `(E + F)/2` and `(E − F)/2` and `θ = atan2(c − b, a + d)`.
/// Their product is `det J`, so the second stretch is negative exactly when the map mirrors.
/// A similarity has two equal stretches, its scale, and its own angle; only an affine or a
/// homography stretches the two axes apart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalGeometry {
    /// The rotation, in radians, in `(−π, π]`.
    pub rotation: f64,
    /// The stretch along each principal axis, the larger first; the second is negative where the
    /// map mirrors.
    pub stretch: [f64; 2],
}

impl LocalGeometry {
    /// The decomposition of `jacobian`.
    pub(crate) fn of(jacobian: DMat2) -> Self {
        let [a, c] = jacobian.x_axis.to_array();
        let [b, d] = jacobian.y_axis.to_array();
        let conformal = (a + d).hypot(c - b);
        let anticonformal = (a - d).hypot(c + b);
        Self {
            rotation: (c - b).atan2(a + d),
            stretch: [
                f64::midpoint(conformal, anticonformal),
                f64::midpoint(conformal, -anticonformal),
            ],
        }
    }

    /// Whether the map mirrors the neighbourhood: a negative determinant.
    pub const fn mirrors(&self) -> bool {
        self.stretch[1] < 0.0
    }
}

#[cfg(test)]
mod tests {
    use glam::{DMat2, DVec2};

    use crate::registration::transform::Transform;
    use crate::registration::transform::local_geometry::LocalGeometry;

    /// A similarity's rotation and its scale on both axes at every scale and angle, a few ulps
    /// each: the scale enters the angle's `atan2` as a common factor, and the angle the stretches
    /// as `cos² + sin²`.
    #[test]
    fn a_similarity_reads_its_own_angle_and_scale() {
        for scale in [0.5, 1.0, 2.5] {
            for angle in [-2.0, -0.3, 0.0, 0.5, 1.2, 2.5] {
                let t = Transform::similarity(DVec2::new(3.0, -1.0), angle, scale);
                let geometry = t.local_geometry(DVec2::ZERO);
                let tolerance = 4.0 * f64::EPSILON;
                assert!(
                    (geometry.rotation - angle).abs() <= tolerance * angle.abs().max(1.0),
                    "θ {angle} at s {scale}: {geometry:?}"
                );
                for stretch in geometry.stretch {
                    assert!((stretch - scale).abs() <= tolerance * scale, "{geometry:?}");
                }
                assert!(!geometry.mirrors());
            }
        }
    }

    /// The axes the first column alone cannot see: a y-stretch of 3, a shear, and a mirror.
    /// `[[1, 0], [0, 3]]` has E = 4 and F = 2: stretches 3 and 1, no rotation. The shear
    /// `[[1, 1], [0, 1]]` has E = √5 and F = 1: stretches (√5 ± 1)/2, the golden ratio and its
    /// inverse, and θ = atan2(−1, 2). The mirror `[[1, 0], [0, −1]]` has E = 0 and F = 2:
    /// stretches 1 and −1.
    #[test]
    fn stretches_and_mirrors_off_the_first_column_are_seen() {
        let geometry = |a, b, c, d| LocalGeometry::of(DMat2::from_cols_array(&[a, c, b, d]));
        assert_eq!(
            geometry(1.0, 0.0, 0.0, 3.0),
            LocalGeometry {
                rotation: 0.0,
                stretch: [3.0, 1.0],
            }
        );
        let golden = f64::midpoint(1.0, 5f64.sqrt());
        let shear = geometry(1.0, 1.0, 0.0, 1.0);
        assert_eq!(shear.rotation, (-1f64).atan2(2.0));
        assert!((shear.stretch[0] - golden).abs() <= 2.0 * f64::EPSILON * golden);
        assert!((shear.stretch[1] - 1.0 / golden).abs() <= 2.0 * f64::EPSILON);
        let mirror = geometry(1.0, 0.0, 0.0, -1.0);
        assert_eq!(mirror.stretch, [1.0, -1.0]);
        assert!(mirror.mirrors());
    }
}
