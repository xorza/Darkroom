//! [`Moments`]: the weighted moments of matched positions that the Procrustes and affine fits are
//! solved from.

use glam::DVec2;

use crate::registration::estimate::{Pair, Pairs};
use crate::registration::transform::Transform;

/// Weighted first and second moments of the pairs about their weighted centroids.
#[derive(Debug, Clone, Copy)]
pub(super) struct Moments {
    reference_centroid: DVec2,
    target_centroid: DVec2,
    /// `Σ wᵢ·r̃ᵢₐ·t̃ᵢᵦ` for the centred positions, `[xx, xy, yx, yy]`.
    cross: [f64; 4],
    /// `Σ wᵢ·r̃ᵢₐ·r̃ᵢᵦ`, `[xx, xy, yy]`.
    covariance: [f64; 3],
    /// `Σ wᵢ·|rᵢ|²`, about the origin: what the centred spread is compared with.
    raw_spread: f64,
}

/// Whether a Procrustes fit holds the scale at 1 or fits it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Scale {
    Unit,
    Fitted,
}

impl Moments {
    /// The moments of `pairs`, in two passes over them: the centroids, then the centred sums.
    pub(super) fn of(pairs: Pairs<'_>) -> Self {
        let mut total = 0.0;
        let mut reference_sum = DVec2::ZERO;
        let mut target_sum = DVec2::ZERO;
        let mut raw_spread = 0.0;
        for Pair {
            reference,
            target,
            weight,
        } in pairs.iter()
        {
            total += weight;
            reference_sum += weight * reference;
            target_sum += weight * target;
            raw_spread += weight * reference.length_squared();
        }
        let reference_centroid = reference_sum / total;
        let target_centroid = target_sum / total;
        let mut cross = [0.0; 4];
        let mut covariance = [0.0; 3];
        for Pair {
            reference,
            target,
            weight,
        } in pairs.iter()
        {
            let (r, t) = (reference - reference_centroid, target - target_centroid);
            cross[0] += weight * r.x * t.x;
            cross[1] += weight * r.x * t.y;
            cross[2] += weight * r.y * t.x;
            cross[3] += weight * r.y * t.y;
            covariance[0] += weight * r.x * r.x;
            covariance[1] += weight * r.x * r.y;
            covariance[2] += weight * r.y * r.y;
        }
        Self {
            reference_centroid,
            target_centroid,
            cross,
            covariance,
            raw_spread,
        }
    }

    /// Weighted Procrustes: the rotation from the weighted cross-covariance, the scale (when
    /// fitted) from it over the weighted reference spread, and the translation that carries the
    /// weighted centroids onto each other. Exact for isotropic per-pair weights.
    ///
    /// Degenerate when the reference points' spread about their centroid is at most `ε` times
    /// their spread about the origin: their offsets from the centroid then hold fewer than half
    /// the digits of their positions, and the rotation is set by rounding. A fitted scale must be
    /// positive: a negative one is a half-turn the angle already holds.
    pub(super) fn procrustes(&self, scale: Scale) -> Option<Transform> {
        let [xx, xy, yx, yy] = self.cross;
        let spread = self.covariance[0] + self.covariance[2];
        if spread <= f64::EPSILON * self.raw_spread {
            return None;
        }
        let angle = (xy - yx).atan2(xx + yy);
        let (sin, cos) = angle.sin_cos();
        let factor = match scale {
            Scale::Unit => 1.0,
            Scale::Fitted => {
                let factor = ((xx + yy) * cos + (xy - yx) * sin) / spread;
                if factor <= 0.0 {
                    return None;
                }
                factor
            }
        };
        let centroid = self.reference_centroid;
        let rotated = DVec2::new(
            cos * centroid.x - sin * centroid.y,
            sin * centroid.x + cos * centroid.y,
        );
        let translation = self.target_centroid - factor * rotated;
        Some(match scale {
            Scale::Unit => Transform::euclidean(translation, angle),
            Scale::Fitted => Transform::similarity(translation, angle, factor),
        })
    }

    /// The weighted affine map `t̃ = A·r̃` of the centred positions, `A = S·C⁻¹` from the
    /// cross-covariance `S` and the reference covariance `C`, and the translation that carries the
    /// weighted centroids onto each other: the weighted least-squares solution, exactly.
    ///
    /// Degenerate when `det C ≤ √ε·(tr C)²`. For the 2 × 2 covariance `det C / (tr C)²` is
    /// `κ/(1 + κ)²`, about `1/κ`, so the rule refuses a condition number `κ` past `1/√ε`, where `A`
    /// keeps fewer than half the digits of `f64`: points nearly on a line.
    pub(super) fn affine(&self) -> Option<Transform> {
        let [cxx, cxy, cyy] = self.covariance;
        let det = cxx * cyy - cxy * cxy;
        let trace = cxx + cyy;
        if det <= f64::EPSILON.sqrt() * trace * trace {
            return None;
        }
        // `S = [xx yx; xy yy]`: its rows are the target's x and y against the reference's.
        let [xx, xy, yx, yy] = self.cross;
        let a = (xx * cyy - yx * cxy) / det;
        let b = (yx * cxx - xx * cxy) / det;
        let d = (xy * cyy - yy * cxy) / det;
        let e = (yy * cxx - xy * cxy) / det;
        let (r, t) = (self.reference_centroid, self.target_centroid);
        let transform = Transform::affine([
            a,
            b,
            t.x - (a * r.x + b * r.y),
            d,
            e,
            t.y - (d * r.x + e * r.y),
        ]);
        transform.is_valid().then_some(transform)
    }
}
