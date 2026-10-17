//! [`Subtractor`]: a master in its role, and how its removal from a frame is recorded.

use crate::calibration::dark_match::DarkMatch;
use crate::calibration::error::SubtractionFault;
use crate::calibration::master_role::MasterRole;
use crate::image::calibration_state::CalibrationState;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::sample_domain::{DomainMap, SampleDomain};

/// A master in the role it is subtracted in: a light's bias or dark, a flat's flat-dark or bias, a
/// dark's bias.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Subtractor<'a> {
    pub(crate) role: MasterRole,
    pub(crate) master: &'a CfaImage,
}

impl Subtractor<'_> {
    /// What the master removes from a frame: its role's signal, less what it lost itself.
    pub(crate) const fn removes(self) -> CalibrationState {
        self.role
            .signal()
            .without(self.master.metadata.derived.calibration())
    }

    /// The map that expresses the master's samples in `frame`, the domain a frame states.
    ///
    /// The identity when either declares no domain: a synthesized frame has none, and there is
    /// nothing to convert between. Refused when the two cannot be related exactly — a different
    /// stated unit, a span the decoder had to assume, or a known pedestal against an unknown one —
    /// because subtracting across such a gap is silently wrong.
    pub(crate) fn map_into(
        self,
        frame: Option<&SampleDomain>,
    ) -> Result<DomainMap, SubtractionFault> {
        let (Some(frame), Some(master)) = (frame, self.master.metadata.derived.domain()) else {
            return Ok(DomainMap::IDENTITY);
        };
        master
            .conversion_to(frame)
            .ok_or_else(|| SubtractionFault::Domain {
                role: self.role,
                frame: frame.clone(),
                master: master.clone(),
            })
    }

    /// Record in `frame` what the master held and what its match did not compare, once its
    /// samples are subtracted. Every removal of a master records through here, so neither record
    /// can be left behind.
    pub(crate) fn record_removal(self, frame: &mut CfaImage, matched: Option<DarkMatch>) {
        let role = self.role;
        let removes = self.removes();
        debug_assert!(
            !frame.metadata.derived.calibration().overlaps(removes),
            "{role} would remove {removes:?} from a frame that lost {:?}",
            frame.metadata.derived.calibration()
        );
        debug_assert_eq!(
            matched.is_some(),
            removes.thermal,
            "{role}: a master is matched to the frame exactly when it holds dark signal"
        );
        frame.metadata.derived.record_removal(removes);
        if let Some(matched) = matched {
            let comparison = matched.comparison;
            frame.metadata.derived.record_dark_comparison(comparison);
            if let Some(difference) = comparison.temperature_difference {
                tracing::warn!(%role, %difference, "a dark of another temperature was subtracted");
            }
        }
    }
}
