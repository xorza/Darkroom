//! The errors of a frame set: a frame taken under other conditions than the set's.

use crate::frame::capture_condition::CaptureCondition;

/// Frame `index` of a set was taken under another exposure or sensor temperature than an earlier
/// frame of it, where the set has to share one: a dark master's thermal signal is one exposure's
/// at one temperature.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error(
    "frame {index} has {condition} {value}, outside the tolerance of frame {reference_index}'s \
     {reference}"
)]
pub struct ConditionMismatch {
    pub condition: CaptureCondition,
    pub index: usize,
    pub value: f64,
    pub reference_index: usize,
    pub reference: f64,
}
