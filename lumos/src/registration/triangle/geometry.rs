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
    /// Invariant ratios: `(sides[0]/sides[2], sides[1]/sides[2])`, the point a triangle is
    /// matched at.
    pub(super) ratios: DVec2,
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

        // Equal sides tie-break by vertex index, so the order is deterministic.
        let mut side_vertex_pairs = [(d01, 2), (d12, 0), (d20, 1)];
        side_vertex_pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(indices[a.1].cmp(&indices[b.1])));

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
        let ratios = DVec2::new(sides[0] / longest, sides[1] / longest);

        // Ordered by role: the vertex opposite the shortest, the middle and the longest side.
        let reordered = [
            indices[side_vertex_pairs[0].1],
            indices[side_vertex_pairs[1].1],
            indices[side_vertex_pairs[2].1],
        ];

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

    /// Whether both ratios are within `tolerance` of `other`'s.
    pub(super) fn is_similar(&self, other: &Self, tolerance: f64) -> bool {
        let dr0 = (self.ratios.x - other.ratios.x).abs();
        let dr1 = (self.ratios.y - other.ratios.y).abs();
        dr0 < tolerance && dr1 < tolerance
    }

    /// Whether sides `i` and `j` are equal within `tolerance` of the longest, so noise can trade
    /// their vertices' roles.
    pub(super) fn ties(&self, i: usize, j: usize, tolerance: f64) -> bool {
        (self.ratio(i) - self.ratio(j)).abs() < tolerance
    }

    /// Side `side`, by length from the shortest, over the longest.
    const fn ratio(&self, side: usize) -> f64 {
        match side {
            0 => self.ratios.x,
            1 => self.ratios.y,
            _ => 1.0,
        }
    }
}
