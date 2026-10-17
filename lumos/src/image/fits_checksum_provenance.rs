//! [`FitsChecksumProvenance`]: what a FITS HDU's `DATASUM` and `CHECKSUM` said when it was read.

use serde::{Deserialize, Serialize};

/// What an HDU's two checksum keywords said when it was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FitsChecksumProvenance {
    pub datasum: FitsChecksumState,
    pub checksum: FitsChecksumState,
}

impl FitsChecksumProvenance {
    /// Neither keyword checked: what a load under [`FitsChecksumPolicy::Ignore`] records.
    ///
    /// [`FitsChecksumPolicy::Ignore`]: crate::FitsChecksumPolicy::Ignore
    pub(crate) const NOT_CHECKED: Self = Self {
        datasum: FitsChecksumState::NotChecked,
        checksum: FitsChecksumState::NotChecked,
    };
}

/// What one checksum keyword said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FitsChecksumState {
    NotChecked,
    Absent,
    Unknown,
    Valid,
}
