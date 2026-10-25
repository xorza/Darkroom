//! [`SlotPlanes`]: the frames a normalization measures, read by slot, and where a plane of them is
//! measured.

use arrayvec::ArrayVec;
use common::par;
use common::{CancelToken, PerWorker};
use imaginarium::PlanarDesc;
use rayon::prelude::*;

use crate::combine::error::StackError;
use crate::combine::frame_read::slots::Slots;
use crate::combine::normalization::common_domain::CommonDomain;
use crate::combine::normalization::slot_pixels::SlotPixels;
use crate::error::Cancelled;
use crate::frame_store::stored_frame::StoredFrame;
use crate::math::statistics::radix_median::RadixMedian;

/// The frames a normalization measures, read by slot: every one of `desc`, under the run's
/// `cancel`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SlotPlanes<'a> {
    pub(crate) frames: &'a [StoredFrame],
    pub(crate) desc: PlanarDesc,
    pub(crate) slots: Slots,
    pub(crate) cancel: &'a CancelToken,
}

impl<'a> SlotPlanes<'a> {
    /// Every value of `slot`'s plane in frame `frame`.
    pub(super) fn values(&self, frame: usize, slot: usize) -> &'a [f32] {
        self.frames[frame].channels[self.slots.channel(slot)].chunk(0..self.desc.pixel_count())
    }

    /// Every frame's slot medians over the pixels the frames share, one pass per slot.
    pub(super) fn slot_medians(
        &self,
        pixels: &[SlotPixels<'_>],
    ) -> Result<Vec<ArrayVec<f32, 3>>, StackError> {
        let slot_count = pixels.len();
        let radix = PerWorker::new(RadixMedian::default);
        let medians = par::into_iter(0..self.frames.len() * slot_count)
            .map(|pair_idx| {
                let mut slot = radix.get();
                let median = &mut *slot;
                let slot = pair_idx % slot_count;
                let sampling = PlaneSampling {
                    domain: pixels[slot].shared(),
                    idxs: &[],
                };
                let measured = sampling.median(
                    self.values(pair_idx / slot_count, slot),
                    median,
                    self.cancel,
                )?;
                Ok(measured.expect("the frames share only some pixels"))
            })
            .collect::<Result<Vec<_>, StackError>>()?;
        Ok(medians
            .chunks(slot_count)
            .map(|slots| slots.iter().copied().collect())
            .collect())
    }

    /// The noise variance of slot `slot` of frame `frame` at the sampled pixels `idxs`: the
    /// source's white noise σ², scaled by the mean inverse confidence there, since interpolation
    /// that averaged several source pixels left less noise than the source had.
    pub(super) fn noise_variance(
        &self,
        frame: usize,
        slot: usize,
        idxs: &[usize],
    ) -> Result<f64, StackError> {
        let frame = &self.frames[frame];
        let sigma = f64::from(frame.source_stats.slot_noise(slot));
        let Some(confidence) = frame.quality.confidence(self.slots.channel(slot)) else {
            return Ok(sigma * sigma);
        };
        let values = confidence.chunk(0..self.desc.pixel_count());
        let mut inverse_confidence = 0.0;
        for chunk in idxs.chunks(CancelToken::POLL_CHUNK) {
            Cancelled::check(self.cancel)?;
            for &idx in chunk {
                let value = f64::from(values[idx]);
                // `idxs` are common-domain pixels, which clear the coverage floor, and a
                // frame-quality pair has confidence wherever it has support — so this is never a
                // division by zero.
                debug_assert!(value > 0.0, "zero confidence at common-domain pixel {idx}");
                inverse_confidence += 1.0 / value;
            }
        }
        Ok(sigma * sigma * inverse_confidence / idxs.len() as f64)
    }
}

/// Where a plane is measured: its median over `domain`, when the frames do not all cover every
/// pixel, and its values at the ascending `idxs`.
#[derive(Debug, Clone, Copy)]
pub(super) struct PlaneSampling<'a> {
    pub(super) domain: Option<&'a CommonDomain>,
    pub(super) idxs: &'a [usize],
}

impl PlaneSampling<'_> {
    /// `values` at the indices, into `samples`.
    pub(super) fn samples(
        &self,
        values: &[f32],
        samples: &mut Vec<f32>,
        cancel: &CancelToken,
    ) -> Result<(), StackError> {
        debug_assert!(self.idxs.is_sorted(), "the sample indices ascend");
        samples.clear();
        samples.reserve_exact(self.idxs.len());
        for chunk in self.idxs.chunks(CancelToken::POLL_CHUNK) {
            Cancelled::check(cancel)?;
            samples.extend(chunk.iter().map(|&idx| values[idx]));
        }
        Ok(())
    }

    /// `values`' median over the domain; `None` without one.
    ///
    /// The median is exact and holds no copy of the plane: `median`'s histogram, which the caller
    /// reuses across planes, ranks the domain's values in two walks over it. A copy for every plane
    /// measured at once would be a plane per worker, which no memory plan charges.
    pub(super) fn median(
        &self,
        values: &[f32],
        median: &mut RadixMedian,
        cancel: &CancelToken,
    ) -> Result<Option<f32>, StackError> {
        let Some(domain) = self.domain else {
            return Ok(None);
        };
        let mut high = median.high_pass();
        domain.for_each_in(values, cancel, |run| high.add_all(run))?;
        debug_assert_eq!(
            high.len(),
            domain.sample_count,
            "the walk visits the domain"
        );
        let mut low = high.finish();
        domain.for_each_in(values, cancel, |run| low.add_all(run))?;
        Ok(Some(low.median()))
    }
}
