//! [`FitScratch`]: the buffers one worker's fits reuse from job to job.

use crate::math::statistics::radix_median::RadixMedian;

/// The buffers one worker's fits reuse from one (frame, slot) job to the next: the median's
/// histogram, the frame's and the reference's samples where they are gathered, and the copy a
/// side's statistics, the seed's ratios and each fit's residuals take in turn.
#[derive(Debug, Default)]
pub(super) struct FitScratch {
    pub(super) median: RadixMedian,
    pub(super) frame: Vec<f32>,
    pub(super) reference: Vec<f32>,
    pub(super) work: Vec<f32>,
}
