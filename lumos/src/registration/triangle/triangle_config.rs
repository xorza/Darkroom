//! [`TriangleConfig`]: how triangle matching pairs the stars of two frames.

use crate::error::InvalidConfigField;

/// Configuration for triangle matching.
#[derive(Debug, Clone)]
pub struct TriangleConfig {
    pub ratio_tolerance: f64,
    pub min_votes: usize,
    pub check_orientation: bool,
}

impl TriangleConfig {
    /// Validate the matching invariants this config owns.
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite(
            "ratio_tolerance",
            "finite and in (0, 1)",
            self.ratio_tolerance,
            |value| value > 0.0 && value < 1.0,
        )?;
        InvalidConfigField::check(
            self.min_votes >= 1,
            "min_votes",
            "at least 1",
            self.min_votes as f64,
        )
    }
}

impl Default for TriangleConfig {
    fn default() -> Self {
        Self {
            ratio_tolerance: 0.01,
            min_votes: 3,
            check_orientation: true,
        }
    }
}
