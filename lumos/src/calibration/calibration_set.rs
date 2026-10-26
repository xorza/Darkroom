//! Four calibration roles carrying values of one common type.

use crate::calibration::master_role::MasterRole;

/// Four calibration roles carrying values of one common type.
///
/// Named fields prevent swapping roles when `T` is the same for all four. Raw inputs use path
/// slices, while prebuilt inputs use optional CFA images.
#[derive(Debug, Default, Clone, Copy)]
pub struct CalibrationSet<T> {
    /// Thermal-noise calibration data.
    pub dark: T,
    /// Vignetting and dust correction data.
    pub flat: T,
    /// Read-noise calibration data.
    pub bias: T,
    /// Dark calibration data taken at the flat exposure time.
    pub flat_dark: T,
}

impl<T> CalibrationSet<T> {
    /// The value for `role`. Total, because a set holds one of everything [`MasterRole`] names.
    pub const fn get(&self, role: MasterRole) -> &T {
        match role {
            MasterRole::Dark => &self.dark,
            MasterRole::Flat => &self.flat,
            MasterRole::Bias => &self.bias,
            MasterRole::FlatDark => &self.flat_dark,
        }
    }

    /// [`Self::get`] by unique reference, for filling a set one role at a time.
    pub(crate) const fn get_mut(&mut self, role: MasterRole) -> &mut T {
        match role {
            MasterRole::Dark => &mut self.dark,
            MasterRole::Flat => &mut self.flat,
            MasterRole::Bias => &mut self.bias,
            MasterRole::FlatDark => &mut self.flat_dark,
        }
    }

    /// The four roles in bundle order, each with the component that names it. The single
    /// place that decides what "all the roles" means — a caller that iterates cannot miss one,
    /// and adding a fifth is a compile error here rather than a silent omission elsewhere.
    pub fn iter(&self) -> impl Iterator<Item = (MasterRole, &T)> {
        MasterRole::ALL
            .into_iter()
            .map(|role| (role, self.get(role)))
    }
}
