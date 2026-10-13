//! [`Subtractor`]: a master in its role, and how subtracting it from a frame is done and recorded.

use crate::calibration::dark_match::DarkMatch;
use crate::calibration::error::CalibrationError;
use crate::calibration::master_role::MasterRole;
use crate::frame::unverified_conditions::UnverifiedConditions;
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
    /// The map that expresses the master's samples in `frame`, the domain a frame states.
    ///
    /// The identity when either declares no domain: a synthesized frame has none, and there is
    /// nothing to convert between. An error when the two cannot be related exactly — a different
    /// stated unit, a span the decoder had to assume, or a known pedestal against an unknown one —
    /// because subtracting across such a gap is silently wrong.
    pub(crate) fn map_into(
        self,
        frame: Option<&SampleDomain>,
    ) -> Result<DomainMap, CalibrationError> {
        let (Some(frame), Some(master)) = (frame, &self.master.metadata.domain) else {
            return Ok(DomainMap::IDENTITY);
        };
        master
            .conversion_to(frame)
            .ok_or_else(|| CalibrationError::SampleDomainMismatch {
                component: self.role,
                frame: frame.clone(),
                master: master.clone(),
            })
    }

    /// [`Self::remove_from`] through the map from the master's domain to `frame`'s current one.
    pub(crate) fn subtract_from(
        self,
        frame: &mut CfaImage,
        matched: Option<DarkMatch>,
    ) -> Result<(), CalibrationError> {
        let map = self.map_into(frame.metadata.domain.as_ref())?;
        self.remove_from(frame, map, matched);
        Ok(())
    }

    /// Subtract the master from `frame` through `map`, its dark signal scaled as `matched` says,
    /// and [record](Self::record_removal) it.
    pub(crate) fn remove_from(
        self,
        frame: &mut CfaImage,
        map: DomainMap,
        matched: Option<DarkMatch>,
    ) {
        frame.subtract_scaled(self.master, map, matched.map_or(1.0, DarkMatch::factor));
        self.record_removal(frame, matched);
    }

    /// Record in `frame` what the master held and what its match did not compare, once its
    /// samples are subtracted. Every removal of a master records through here, so neither record
    /// can be left behind.
    pub(crate) fn record_removal(self, frame: &mut CfaImage, matched: Option<DarkMatch>) {
        let role = self.role;
        let removes = role.signal().without(self.master.metadata.calibration);
        debug_assert!(
            !frame.metadata.calibration.overlaps(removes),
            "{role} would remove {removes:?} from a frame that lost {:?}",
            frame.metadata.calibration
        );
        debug_assert_eq!(
            matched.is_some(),
            removes.thermal,
            "{role}: a master is matched to the frame exactly when it holds dark signal"
        );
        frame.metadata.calibration = frame.metadata.calibration.union(removes);
        if let Some(matched) = matched {
            // A temperature that differed is no more known to match than one never compared.
            let differed = UnverifiedConditions {
                exposure: false,
                temperature: matched.temperature_difference.is_some(),
            };
            frame.metadata.unverified_dark = frame
                .metadata
                .unverified_dark
                .union(matched.unverified)
                .union(differed);
            if let Some(difference) = matched.temperature_difference {
                tracing::warn!(%role, %difference, "a dark of another temperature was subtracted");
            }
        }
    }
}
