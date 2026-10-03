//! Anything a calibration bundle can carry.

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use crate::calibration_masters::master_role::MasterRole;
/// Anything a [`CalibrationMasters`](crate::calibration_masters::CalibrationMasters)
/// bundle can carry: one of the master frames, or the defect map derived from them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationComponent {
    /// One of the four stacked master frames.
    Master(MasterRole),
    /// Defect map derived from a dark, flat, or both.
    Defects,
}

impl CalibrationComponent {
    /// The component's `EXTNAME` in a saved bundle.
    pub(crate) const fn extname(self) -> &'static str {
        match self {
            Self::Master(role) => role.extname(),
            Self::Defects => "DEFECT_MAP",
        }
    }

    /// The component an `EXTNAME` names, or `None` for an extension this format does not define.
    pub(crate) fn from_extname(extname: &str) -> Option<Self> {
        MasterRole::ALL
            .into_iter()
            .map(Self::Master)
            .chain([Self::Defects])
            .find(|component| component.extname() == extname)
    }
}

impl From<MasterRole> for CalibrationComponent {
    fn from(role: MasterRole) -> Self {
        Self::Master(role)
    }
}

impl Display for CalibrationComponent {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Master(role) => role.fmt(f),
            Self::Defects => f.write_str("defects"),
        }
    }
}
