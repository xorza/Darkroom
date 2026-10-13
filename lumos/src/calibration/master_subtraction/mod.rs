//! [`MasterSubtraction`]: a master taken from every frame of a calibration stack.

use crate::calibration::dark_match::{DarkBias, DarkMatch};
use crate::calibration::error::SubtractionError;
use crate::calibration::master_role::MasterRole;
use crate::calibration::subtractor::Subtractor;
use crate::frame::capture_conditions::CaptureConditions;
use crate::image::calibration_state::CalibrationState;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::sample_domain::DomainMap;
use crate::ingest::frame_step::FrameStep;

/// A [`Subtractor`] taken from each frame of a calibration stack — a flat's flat-dark or bias, so
/// the multiplicative normalization scales the flat's own signal, or a dark's bias — checked
/// against the role of the frames it is taken from, with the parts it removes from each.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MasterSubtraction<'a> {
    subtractor: Subtractor<'a>,
    removes: CalibrationState,
}

impl<'a> MasterSubtraction<'a> {
    /// The subtraction of `subtractor` from every frame of a `target` stack.
    ///
    /// # Errors
    ///
    /// The subtractor lost everything its role holds, or more than its role may lose, or it holds
    /// more than a `target` frame may lose before it is combined, as a flat holds its response.
    pub(crate) const fn new(
        target: MasterRole,
        subtractor: Subtractor<'a>,
    ) -> Result<Self, SubtractionError> {
        let lost = subtractor.master.metadata.calibration;
        let removes = subtractor.role.signal().without(lost);
        if removes.is_none() || !subtractor.role.may_have_lost().contains(lost) {
            return Err(SubtractionError::OverCalibratedSubtractor {
                subtractor: subtractor.role,
            });
        }
        if !target.may_have_lost().contains(removes) {
            return Err(SubtractionError::SubtractorForRole {
                target,
                subtractor: subtractor.role,
            });
        }
        Ok(Self {
            subtractor,
            removes,
        })
    }
}

impl FrameStep<CfaImage> for MasterSubtraction<'_> {
    type Error = SubtractionError;

    fn apply(&self, index: usize, frame: &mut CfaImage) -> Result<(), SubtractionError> {
        let master = self.subtractor.master;
        if master.cfa_type != frame.cfa_type || master.size() != frame.size() {
            return Err(SubtractionError::SubtractorShape {
                index,
                frame: frame.size(),
                subtractor: master.size(),
            });
        }
        if frame.metadata.calibration.overlaps(self.removes) {
            return Err(SubtractionError::SubtractedTwice {
                index,
                subtractor: self.subtractor.role,
            });
        }
        let matched = self
            .removes
            .thermal
            .then(|| {
                DarkMatch::new(
                    CaptureConditions::of(&frame.metadata),
                    CaptureConditions::of(&master.metadata),
                    DarkBias::held_in(self.removes),
                )
            })
            .transpose()
            .map_err(|source| SubtractionError::SubtractorConditions { index, source })?;
        let map = match (&frame.metadata.domain, &master.metadata.domain) {
            (Some(frame_domain), Some(master_domain)) => master_domain
                .conversion_to(frame_domain)
                .ok_or_else(|| SubtractionError::SubtractorDomain {
                    index,
                    frame: Box::new(frame_domain.clone()),
                    subtractor: Box::new(master_domain.clone()),
                })?,
            _ => DomainMap::IDENTITY,
        };
        self.subtractor.remove_from(frame, map, matched);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
