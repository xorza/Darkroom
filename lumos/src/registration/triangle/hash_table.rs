use super::geometry::Triangle;

/// Hash table for fast triangle lookup using geometric hashing.
#[derive(Debug)]
pub(crate) struct TriangleHashTable {
    /// 2D grid of bins, each containing triangle indices.
    table: Vec<Vec<usize>>,
    /// Number of bins per dimension.
    bins: usize,
}

impl TriangleHashTable {
    /// Build a hash table from a list of triangles.
    pub fn build(triangles: &[Triangle], bins: usize) -> Self {
        let mut table = vec![Vec::new(); bins * bins];

        for (idx, triangle) in triangles.iter().enumerate() {
            let (bx, by) = triangle.hash_key(bins);
            table[by * bins + bx].push(idx);
        }

        Self { table, bins }
    }

    /// Find candidate triangles that might match the query.
    /// Returns indices into the original triangle array.
    #[cfg(test)]
    pub fn find_candidates(&self, query: &Triangle, tolerance: f64) -> Vec<usize> {
        let mut candidates = Vec::new();
        self.find_candidates_into(query, tolerance, &mut candidates);
        candidates
    }

    /// Find candidate triangles into a pre-allocated buffer.
    ///
    /// The buffer is cleared before use. This avoids allocations when
    /// called repeatedly in a loop.
    pub fn find_candidates_into(
        &self,
        query: &Triangle,
        tolerance: f64,
        candidates: &mut Vec<usize>,
    ) {
        candidates.clear();

        let (bx, by) = query.hash_key(self.bins);

        // Search in neighboring bins based on tolerance
        let bin_tolerance = ((tolerance * self.bins as f64).ceil() as usize).max(1);

        let x_min = bx.saturating_sub(bin_tolerance);
        let x_max = (bx + bin_tolerance + 1).min(self.bins);
        let y_min = by.saturating_sub(bin_tolerance);
        let y_max = (by + bin_tolerance + 1).min(self.bins);

        for y in y_min..y_max {
            for x in x_min..x_max {
                candidates.extend_from_slice(&self.table[y * self.bins + x]);
            }
        }
    }

    /// Get the number of triangles in the table.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.table.iter().map(|v| v.len()).sum()
    }

    /// Check if the table is empty.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
