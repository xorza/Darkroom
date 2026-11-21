//! The errors of a frame set: a frame taken under other conditions than the set's, or a master
//! than the frame it calibrates.

use crate::frame::capture_condition::CaptureCondition;

/// Frame `idx` of a set was taken under another exposure or sensor temperature than an earlier
/// frame of it, where the set has to share one: a dark master's thermal signal is one exposure's
/// at one temperature.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error(
    "frame {idx} has {condition} {value}, outside the tolerance of frame {reference_idx}'s \
     {reference}"
)]
pub struct ConditionMismatch {
    pub condition: CaptureCondition,
    pub idx: usize,
    pub value: f64,
    pub reference_idx: usize,
    pub reference: f64,
}

/// A master was taken at another sensor setting than the frame it calibrates: a bias or a dark at
/// another gain, ISO, offset or binning holds another offset, read noise or dark current.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("the frame has {condition} {frame}, the master {master}")]
pub struct SettingMismatch {
    pub condition: CaptureCondition,
    pub frame: f64,
    pub master: f64,
}
