//! Four calibration roles carrying values of one common type.

use crate::io::image::cfa::{CfaImage, CfaType};
use crate::math::size2us::Size2us;
use crate::stacking::calibration_masters::error::CalibrationError;

use crate::stacking::calibration_masters::master_role::MasterRole;
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

    /// The four roles in calibration order, each with the component that names it. The single
    /// place that decides what "all the roles" means — a caller that iterates cannot miss one,
    /// and adding a fifth is a compile error here rather than a silent omission elsewhere.
    pub fn iter(&self) -> impl Iterator<Item = (MasterRole, &T)> {
        MasterRole::ALL
            .into_iter()
            .map(|role| (role, self.get(role)))
    }

    /// The four roles by value, in [`MasterRole::ALL`] order.
    ///
    /// An array rather than an iterator because the concurrent half of
    /// [`CalibrationMasters::from_files`] hands it straight to rayon, which parallelizes `[T; N]`
    /// but not an array iterator. [`Self::from_roles`] is its inverse; the two are the only place
    /// the field-to-role correspondence is written, and `roles_round_trip_in_master_order` pins it.
    pub(crate) fn into_roles(self) -> [(MasterRole, T); 4] {
        [
            (MasterRole::Dark, self.dark),
            (MasterRole::Flat, self.flat),
            (MasterRole::Bias, self.bias),
            (MasterRole::FlatDark, self.flat_dark),
        ]
    }

    /// Convert every role, in calibration order, stopping at the first failure.
    pub(crate) fn try_map<U, E>(
        self,
        mut convert: impl FnMut(MasterRole, T) -> Result<U, E>,
    ) -> Result<CalibrationSet<U>, E> {
        let [dark, flat, bias, flat_dark] = self.into_roles();
        Ok(CalibrationSet {
            dark: convert(dark.0, dark.1)?,
            flat: convert(flat.0, flat.1)?,
            bias: convert(bias.0, bias.1)?,
            flat_dark: convert(flat_dark.0, flat_dark.1)?,
        })
    }
}

impl CalibrationSet<Option<CfaImage>> {
    /// The sensor extent every present master shares, or `None` when the set is empty; they must
    /// share one CFA pattern too.
    ///
    /// The masters are combined pixel-for-pixel by flat index — dark subtracted from flat,
    /// defects detected on one and corrected on another — so a set that spans two sensors has no
    /// coherent interpretation. Reported as an error rather than left to the individual
    /// operations, which each assert on only the pair they touch and cover the set unevenly: a
    /// bias in a set with no flat is never anyone's operand.
    pub(crate) fn common_dimensions(&self) -> Result<Option<Size2us>, CalibrationError> {
        let mut expected: Option<(Size2us, CfaType)> = None;
        for (role, master) in self
            .iter()
            .filter_map(|(role, master)| master.as_ref().map(|master| (role, master)))
        {
            let size = Size2us::new(master.data.width(), master.data.height());
            let Some((expected_size, expected_pattern)) = expected else {
                expected = Some((size, master.cfa_type));
                continue;
            };
            if size != expected_size {
                return Err(CalibrationError::DimensionMismatch {
                    component: role.into(),
                    expected: expected_size,
                    master: size,
                });
            }
            if master.cfa_type != expected_pattern {
                return Err(CalibrationError::CfaPatternMismatch {
                    component: role,
                    expected: expected_pattern,
                    master: master.cfa_type,
                });
            }
        }
        Ok(expected.map(|(size, _)| size))
    }
}
