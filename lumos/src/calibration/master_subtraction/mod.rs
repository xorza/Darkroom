//! [`MasterSubtraction`]: the additive masters taken from every frame of a calibration stack.

use arrayvec::ArrayVec;

use crate::calibration::calibration_pass::Saturation;
use crate::calibration::error::SubtractionError;
use crate::calibration::master_role::MasterRole;
use crate::calibration::subtraction::{self, Subtraction};
use crate::calibration::subtractor::Subtractor;
use crate::combine::frame_cache::frame_step::FrameStep;
use crate::image::calibration_state::CalibrationState;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::pixel_flags::PixelFlags;

/// The additive masters taken from each frame of a calibration stack, in order — a flat's bias,
/// then the dark signal of its flat-dark matched to the flat's own exposure and temperature, so the
/// multiplicative normalization scales the flat's own signal — each checked against the role of
/// the frames it is taken from, with the parts it removes.
///
/// A light's [`calibrate`](crate::CalibrationMasters::calibrate) removes the same two parts through
/// the same [`Subtraction`]: a bias as it stands, and a bias-removed dark scaled to the light.
#[derive(Debug, Clone)]
pub(crate) struct MasterSubtraction<'a> {
    subtractors: ArrayVec<Subtractor<'a>, 2>,
    /// `NO_DATA` where a subtractor holds no measurement, found once for every frame.
    imposed: Option<PixelFlags>,
}

impl<'a> MasterSubtraction<'a> {
    /// The subtraction of `subtractors`, in order, from every frame of a `target` stack, or `None`
    /// when there are none.
    ///
    /// # Errors
    ///
    /// A subtractor lost everything its role holds, or more than its role may lose, or it holds
    /// more than a `target` frame may lose before it is combined, as a flat holds its response,
    /// or it would remove a part an earlier one removes.
    pub(crate) fn new(
        target: MasterRole,
        subtractors: &[Subtractor<'a>],
    ) -> Result<Option<Self>, SubtractionError> {
        let mut removed = CalibrationState::NONE;
        for &subtractor in subtractors {
            let lost = subtractor.master.metadata.derived.calibration();
            let removes = subtractor.removes();
            if removes.is_none() || !subtractor.role.may_have_lost().contains(lost) {
                return Err(SubtractionError::OverCalibratedSubtractor {
                    subtractor: subtractor.role,
                });
            }
            if !target.may_have_lost().contains(removes) || removed.overlaps(removes) {
                return Err(SubtractionError::SubtractorForRole {
                    target,
                    subtractor: subtractor.role,
                });
            }
            removed = removed.union(removes);
        }
        if subtractors.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            subtractors: subtractors.iter().copied().collect(),
            imposed: subtraction::unmeasured(subtractors),
        }))
    }
}

impl FrameStep<CfaImage> for MasterSubtraction<'_> {
    type Error = SubtractionError;

    /// Check every subtractor against frame `idx`, then remove them all in one pass.
    fn apply(&self, idx: usize, frame: &mut CfaImage) -> Result<(), SubtractionError> {
        for subtractor in &self.subtractors {
            let master = subtractor.master;
            if master.cfa_type() != frame.cfa_type() {
                return Err(SubtractionError::SubtractorPattern {
                    idx,
                    frame: frame.cfa_type(),
                    subtractor: master.cfa_type(),
                });
            }
            if master.size() != frame.size() {
                return Err(SubtractionError::SubtractorShape {
                    idx,
                    frame: frame.size(),
                    subtractor: master.size(),
                });
            }
            if frame
                .metadata
                .derived
                .calibration()
                .overlaps(subtractor.removes())
            {
                return Err(SubtractionError::SubtractedTwice {
                    idx,
                    subtractor: subtractor.role,
                });
            }
        }
        Subtraction::resolve(frame, self.subtractors.iter().copied())
            .map_err(|fault| SubtractionError::of_frame(idx, fault))?
            .apply(frame, self.imposed.as_ref(), Saturation::Detect);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
