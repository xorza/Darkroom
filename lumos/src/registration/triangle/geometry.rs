use glam::DVec2;

/// Minimum side length for valid triangles.
pub(crate) const MIN_TRIANGLE_SIDE: f64 = 1e-10;

/// Minimum area squared for valid triangles (Heron's formula).
/// Prevents very flat/degenerate triangles.
pub(crate) const MIN_TRIANGLE_AREA_SQ: f64 = 1e-6;

/// Orientation of a triangle (clockwise or counter-clockwise).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Orientation {
    Clockwise,
    CounterClockwise,
}

/// A triangle formed from three stars.
#[derive(Debug, Clone)]
pub(crate) struct Triangle {
    /// Indices of the three stars in the original list.
    pub star_indices: [usize; 3],
    /// Side lengths sorted: sides[0] <= sides[1] <= sides[2].
    #[allow(dead_code)]
    pub sides: [f64; 3],
    /// Invariant ratios: (sides[0]/sides[2], sides[1]/sides[2]).
    pub ratios: (f64, f64),
    /// Orientation of the triangle.
    pub orientation: Orientation,
}

impl Triangle {
    /// Create a triangle from three star positions.
    ///
    /// Returns None if the triangle is degenerate (collinear points).
    pub fn from_positions(indices: [usize; 3], positions: [DVec2; 3]) -> Option<Self> {
        let p0 = positions[0];
        let p1 = positions[1];
        let p2 = positions[2];

        // Compute side lengths
        let d01 = (p1 - p0).length();
        let d12 = (p2 - p1).length();
        let d20 = (p0 - p2).length();

        // Check for degenerate triangle (sides too short)
        if d01 < MIN_TRIANGLE_SIDE || d12 < MIN_TRIANGLE_SIDE || d20 < MIN_TRIANGLE_SIDE {
            return None;
        }

        // Sort sides and track which vertices are at each position
        let mut side_vertex_pairs = [(d01, 2), (d12, 0), (d20, 1)];
        side_vertex_pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        let sides = [
            side_vertex_pairs[0].0,
            side_vertex_pairs[1].0,
            side_vertex_pairs[2].0,
        ];

        // Compute invariant ratios
        let longest = sides[2];
        if longest < MIN_TRIANGLE_SIDE {
            return None;
        }

        let ratios = (sides[0] / longest, sides[1] / longest);

        // Check for very flat triangles using Heron's formula for area
        // area² = s(s-a)(s-b)(s-c) where s = (a+b+c)/2
        let s = (sides[0] + sides[1] + sides[2]) / 2.0;
        let area_sq = s * (s - sides[0]) * (s - sides[1]) * (s - sides[2]);
        if area_sq < MIN_TRIANGLE_AREA_SQ {
            return None; // Too flat / nearly collinear
        }

        // Compute orientation using cross product
        let v01 = p1 - p0;
        let v02 = p2 - p0;
        let cross = v01.x * v02.y - v01.y * v02.x;
        if cross.abs() < 1e-10 * longest * longest {
            return None; // Degenerate
        }

        let orientation = if cross > 0.0 {
            Orientation::CounterClockwise
        } else {
            Orientation::Clockwise
        };

        Some(Self {
            star_indices: indices,
            sides,
            ratios,
            orientation,
        })
    }

    /// Check if two triangles are similar within tolerance.
    pub fn is_similar(&self, other: &Triangle, tolerance: f64) -> bool {
        let dr0 = (self.ratios.0 - other.ratios.0).abs();
        let dr1 = (self.ratios.1 - other.ratios.1).abs();
        dr0 < tolerance && dr1 < tolerance
    }

    /// Compute hash key for geometric hashing.
    /// Returns (bin_x, bin_y) where each is in [0, bins).
    pub fn hash_key(&self, bins: usize) -> (usize, usize) {
        // Ratios are in (0, 1], map to bins
        let bin_x = ((self.ratios.0 * bins as f64) as usize).min(bins - 1);
        let bin_y = ((self.ratios.1 * bins as f64) as usize).min(bins - 1);
        (bin_x, bin_y)
    }
}
