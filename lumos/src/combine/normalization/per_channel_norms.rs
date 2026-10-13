//! [`PerChannelNorms`]: the norms of frames gathered per channel, by whichever estimator is the
//! more precise.

use arrayvec::ArrayVec;

use crate::combine::error::StackError;
use crate::combine::normalization::norm_fit::Estimator;
use crate::combine::normalization::{FrameNorm, SlotNorm};

/// The largest of `errors`; 0 for none.
fn worst(errors: impl Iterator<Item = f64>) -> f64 {
    errors.fold(0.0, f64::max)
}

/// One slot's norm and its gain's relative standard error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SlotEstimate {
    pub(super) norm: SlotNorm,
    pub(super) relative_error: f64,
}

impl SlotEstimate {
    /// The reference's own norm, which no estimate makes.
    pub(super) const EXACT_IDENTITY: Self = Self {
        norm: SlotNorm::IDENTITY,
        relative_error: 0.0,
    };
}

/// The norms of frames gathered per channel, and which estimator gave them.
#[derive(Debug)]
pub(super) struct PerChannelNorms {
    pub(super) norms: Vec<FrameNorm>,
    pub(super) estimator: Estimator,
}

impl PerChannelNorms {
    /// The pairwise norms when every pair shares pixels and each pair's gain is no less precise
    /// than the per-frame estimate of the same frame and slot; the per-frame norms otherwise, for
    /// every frame. The per-frame estimator also carries a bias its error does not state — two
    /// frames measured over different pixels — so a tie goes to the pairs.
    ///
    /// # Errors
    /// The per-frame estimator's, when the pairs do not stand.
    pub(super) fn choose(
        pairwise: Vec<ArrayVec<Option<SlotEstimate>, 3>>,
        per_frame: Result<Vec<ArrayVec<SlotEstimate, 3>>, StackError>,
    ) -> Result<Self, StackError> {
        let shared = pairwise.iter().flatten().all(Option::is_some);
        let pairs_stand = shared
            && per_frame.as_ref().map_or(true, |per_frame| {
                pairwise.iter().zip(per_frame).all(|(pairs, singles)| {
                    pairs.iter().zip(singles).all(|(pair, single)| {
                        pair.is_some_and(|pair| pair.relative_error <= single.relative_error)
                    })
                })
            });
        let pairwise_worst = worst(
            pairwise
                .iter()
                .flatten()
                .flatten()
                .map(|pair| pair.relative_error),
        );
        if pairs_stand {
            tracing::debug!(pairwise_worst, "per-channel norms fitted pair by pair");
            return Ok(Self {
                norms: pairwise
                    .into_iter()
                    .map(|pairs| FrameNorm {
                        slots: pairs.into_iter().flatten().map(|pair| pair.norm).collect(),
                    })
                    .collect(),
                estimator: Estimator::Pairwise,
            });
        }
        let per_frame = per_frame?;
        tracing::debug!(
            shared,
            pairwise_worst,
            per_frame_worst = worst(
                per_frame
                    .iter()
                    .flatten()
                    .map(|single| single.relative_error)
            ),
            "per-channel norms measured frame by frame"
        );
        Ok(Self {
            norms: per_frame
                .into_iter()
                .map(|singles| FrameNorm {
                    slots: singles.into_iter().map(|single| single.norm).collect(),
                })
                .collect(),
            estimator: Estimator::PerFrame,
        })
    }
}
