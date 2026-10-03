//! The four master frames a calibration bundle can carry.

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use crate::stacking::combine::config::StackConfig;

/// One of the four master frames a bundle can carry.
///
/// Split from [`CalibrationComponent`](crate::stacking::calibration_masters::calibration_component::CalibrationComponent) so that everything indexed by role — every
/// [`CalibrationSet`](crate::stacking::calibration_masters::calibration_set::CalibrationSet) accessor — is total. The defect map is a component of a bundle but not a
/// master, and folding it in here made each of those return an `Option` for a case that could
/// never arise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MasterRole {
    /// Master dark frame.
    Dark,
    /// Master flat frame.
    Flat,
    /// Master bias frame.
    Bias,
    /// Master dark frame taken at the flat exposure time.
    FlatDark,
}

impl MasterRole {
    /// The four roles, in calibration order. Adding a fifth is a compile error in
    /// [`CalibrationSet`](crate::stacking::calibration_masters::calibration_set::CalibrationSet) rather than a silent omission wherever roles are walked.
    pub const ALL: [Self; 4] = [Self::Dark, Self::Flat, Self::Bias, Self::FlatDark];

    /// The role's `EXTNAME` in a saved bundle — the name a writer stamps on its HDU and a reader
    /// recognizes it by, so the two cannot disagree about where a role lives.
    pub(crate) fn extname(self) -> &'static str {
        match self {
            Self::Dark => "MASTER_DARK",
            Self::Flat => "MASTER_FLAT",
            Self::Bias => "MASTER_BIAS",
            Self::FlatDark => "MASTER_FLAT_DARK",
        }
    }

    /// Whether this role is stored already prepared — bias/flat-dark subtracted, per-colour
    /// normalized and clamped. Only the flat is; the others are stored as stacked.
    pub(crate) fn prepared(self) -> bool {
        matches!(self, Self::Flat)
    }

    /// The preset this role's frames stack under — the one role → preset table. Darks, biases
    /// and flat-darks (a flat-dark is a dark taken at the flat's exposure time) are a Winsorized
    /// mean at any frame count; flats a σ-clipped mean that falls back to the median below 8
    /// frames. Each preset carries its own small-frame fallback (`StackConfig::small_n`).
    pub fn stack_config(self) -> StackConfig {
        match self {
            Self::Dark | Self::FlatDark => StackConfig::dark(),
            Self::Flat => StackConfig::flat(),
            Self::Bias => StackConfig::bias(),
        }
    }
}

impl Display for MasterRole {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Dark => "dark",
            Self::Flat => "flat",
            Self::Bias => "bias",
            Self::FlatDark => "flat-dark",
        })
    }
}
