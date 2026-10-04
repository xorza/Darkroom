//! One triangle and the scale-invariant descriptor it is matched by.
//!
//! A triangle is characterized by its two side ratios, which survive translation, rotation and
//! scale, plus the orientation that distinguishes it from its mirror image. A triangle too flat
//! for its orientation to outlast the noise is rejected at construction.

use glam::DVec2;

/// Orientation of a triangle (clockwise or counter-clockwise).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Orientation {
    Clockwise,
    CounterClockwise,
}

/// A triangle formed from three points.
#[derive(Debug, Clone)]
pub(super) struct Triangle {
    /// Indices of the three points in the original list.
    pub(super) indices: [usize; 3],
    /// Invariant ratios: `(sides[0]/sides[2], sides[1]/sides[2])`.
    pub(super) ratios: (f64, f64),
    /// Orientation of the triangle.
    pub(super) orientation: Orientation,
}

impl Triangle {
    /// Create a triangle from three positions; `None` when it stands lower than `min_height` over
    /// its longest side, where noise of that scale can turn it over.
    ///
    /// The ratios move by about `σ/longest` under positional noise σ whatever the shape, so a
    /// short side costs them nothing; Groth's (1986) limit on the side ratio protects invariants
    /// built on the shortest side, which these are not. What a flat triangle loses is its
    /// orientation, which a height within the noise does not fix.
    pub(super) fn from_positions(
        indices: [usize; 3],
        positions: [DVec2; 3],
        min_height: f64,
    ) -> Option<Self> {
        let p0 = positions[0];
        let p1 = positions[1];
        let p2 = positions[2];

        let d01 = (p1 - p0).length();
        let d12 = (p2 - p1).length();
        let d20 = (p0 - p2).length();

        // Sort sides and track which vertices are at each position.
        // Tiebreak equal sides by original vertex index for deterministic ordering.
        let mut side_vertex_pairs = [(d01, 2), (d12, 0), (d20, 1)];
        side_vertex_pairs.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap()
                .then(indices[a.1].cmp(&indices[b.1]))
        });

        let sides = [
            side_vertex_pairs[0].0,
            side_vertex_pairs[1].0,
            side_vertex_pairs[2].0,
        ];

        // Twice the area over the longest side is the height onto it; a coincident pair leaves
        // both at zero, and the test rejects it too.
        let longest = sides[2];
        let twice_area = (p1 - p0).perp_dot(p2 - p0).abs();
        if longest == 0.0 || twice_area < min_height * longest {
            return None;
        }
        let ratios = (sides[0] / longest, sides[1] / longest);

        // Reorder indices by geometric role:
        // indices[0] = vertex opposite shortest side
        // indices[1] = vertex opposite middle side
        // indices[2] = vertex opposite longest side
        let reordered = [
            indices[side_vertex_pairs[0].1],
            indices[side_vertex_pairs[1].1],
            indices[side_vertex_pairs[2].1],
        ];

        // Compute orientation from reordered vertex positions
        let rp0 = positions[side_vertex_pairs[0].1];
        let rp1 = positions[side_vertex_pairs[1].1];
        let rp2 = positions[side_vertex_pairs[2].1];
        let orientation = if (rp1 - rp0).perp_dot(rp2 - rp0) > 0.0 {
            Orientation::CounterClockwise
        } else {
            Orientation::Clockwise
        };

        Some(Self {
            indices: reordered,
            ratios,
            orientation,
        })
    }

    /// Check if two triangles are similar within tolerance.
    pub(super) fn is_similar(&self, other: &Triangle, tolerance: f64) -> bool {
        let dr0 = (self.ratios.0 - other.ratios.0).abs();
        let dr1 = (self.ratios.1 - other.ratios.1).abs();
        dr0 < tolerance && dr1 < tolerance
    }
}
