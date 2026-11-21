//! [`Additive`]: which additive masters a light loses.

use crate::calibration::error::CalibrationError;
use crate::image::cfa::cfa_image::CfaImage;

/// The additive masters a light loses, as one of the shapes a set can hold: a bias beside a dark
/// that still holds it is no case, as the bias would be subtracted twice or kept for nothing.
#[derive(Debug, Default)]
#[expect(
    clippy::large_enum_variant,
    reason = "a set holds one; a box would save nothing and cost a light's every read a hop"
)]
pub(crate) enum Additive {
    #[default]
    None,
    Bias(CfaImage),
    /// A dark that still holds the bias, subtracted as it is at its own exposure.
    DarkWithBias(CfaImage),
    /// A dark that lost its bias, with no bias to take beside it.
    Thermal(CfaImage),
    BiasAndThermal {
        bias: CfaImage,
        thermal: CfaImage,
    },
}

impl Additive {
    /// The shape `bias` and `dark` make, the dark's record saying whether it still holds the bias.
    ///
    /// # Errors
    ///
    /// [`CalibrationError::UnseparatedDark`] for a bias beside a dark that still holds it.
    pub(crate) fn of(
        bias: Option<CfaImage>,
        dark: Option<CfaImage>,
    ) -> Result<Self, CalibrationError> {
        let holds_bias = |dark: &CfaImage| !dark.metadata.derived.calibration().bias;
        Ok(match (bias, dark) {
            (None, None) => Self::None,
            (Some(bias), None) => Self::Bias(bias),
            (None, Some(dark)) if holds_bias(&dark) => Self::DarkWithBias(dark),
            (None, Some(thermal)) => Self::Thermal(thermal),
            (Some(_), Some(dark)) if holds_bias(&dark) => {
                return Err(CalibrationError::UnseparatedDark);
            }
            (Some(bias), Some(thermal)) => Self::BiasAndThermal { bias, thermal },
        })
    }

    /// The bias a light loses.
    pub(crate) const fn bias(&self) -> Option<&CfaImage> {
        match self {
            Self::Bias(bias) | Self::BiasAndThermal { bias, .. } => Some(bias),
            Self::None | Self::DarkWithBias(_) | Self::Thermal(_) => None,
        }
    }

    /// The dark a light loses, with or without the bias.
    pub(crate) const fn dark(&self) -> Option<&CfaImage> {
        match self {
            Self::DarkWithBias(dark)
            | Self::Thermal(dark)
            | Self::BiasAndThermal { thermal: dark, .. } => Some(dark),
            Self::None | Self::Bias(_) => None,
        }
    }

    /// Whether the dark still holds the bias, so it cannot be scaled to another exposure.
    pub(crate) const fn dark_holds_bias(&self) -> bool {
        matches!(self, Self::DarkWithBias(_))
    }
}

#[cfg(test)]
mod tests {
    use common::USize2;

    use crate::calibration::calibration_masters::additive::Additive;
    use crate::calibration::error::CalibrationError;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::image::metadata::calibration_state::CalibrationState;
    use crate::internals::cfa::constant_cfa;

    /// Each pair of a bias and a dark makes its shape, the dark's record saying whether it lost the
    /// bias; a bias beside a dark that still holds it is refused.
    #[test]
    fn a_bias_and_a_dark_make_their_shape() {
        let master = |level| constant_cfa(USize2::new(2, 2), level, CfaType::Mono);
        let thermal = || {
            let mut dark = master(0.25);
            dark.metadata
                .derived
                .set_calibration(CalibrationState::BIAS);
            dark
        };
        let shape = |additive: Result<Additive, CalibrationError>| {
            additive.map(|additive| {
                (
                    additive.bias().map(|bias| bias.data()[0]),
                    additive.dark().map(|dark| dark.data()[0]),
                    additive.dark_holds_bias(),
                )
            })
        };
        assert_eq!(shape(Additive::of(None, None)), Ok((None, None, false)));
        assert_eq!(
            shape(Additive::of(Some(master(0.125)), None)),
            Ok((Some(0.125), None, false))
        );
        assert_eq!(
            shape(Additive::of(None, Some(master(0.5)))),
            Ok((None, Some(0.5), true))
        );
        assert_eq!(
            shape(Additive::of(None, Some(thermal()))),
            Ok((None, Some(0.25), false))
        );
        assert_eq!(
            shape(Additive::of(Some(master(0.125)), Some(thermal()))),
            Ok((Some(0.125), Some(0.25), false))
        );
        assert_eq!(
            shape(Additive::of(Some(master(0.125)), Some(master(0.5)))),
            Err(CalibrationError::UnseparatedDark)
        );
    }
}
