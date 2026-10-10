//! [`UnverifiedConditions`]: the capture conditions a dark match could not compare.

use serde::{Deserialize, Serialize};

/// The capture conditions not compared when a master holding dark signal was taken from a frame:
/// the frame or the master did not state them. A dark of another exposure or temperature leaves a
/// residual of dark signal that nothing reports, so the record says where the match was not
/// checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UnverifiedConditions {
    /// The exposures were not compared.
    pub exposure: bool,
    /// No temperature was compared: the two did not both state the sensor's, nor the camera
    /// body's. Most DSLRs state neither, Canon only the body's.
    pub temperature: bool,
}

impl UnverifiedConditions {
    /// Every condition compared, or no dark taken.
    pub const NONE: Self = Self {
        exposure: false,
        temperature: false,
    };

    /// The conditions either left uncompared.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            exposure: self.exposure || other.exposure,
            temperature: self.temperature || other.temperature,
        }
    }
}
