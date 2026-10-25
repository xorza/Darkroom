//! [`SlotPixels`]: the pixels one slot is measured over.

use arrayvec::ArrayVec;
use common::CancelToken;

use crate::combine::error::StackError;
use crate::combine::normalization::common_domain::CommonDomain;
use crate::combine::normalization::slot_planes::SlotPlanes;
use crate::frame_store::stratified_samples::StratifiedSamples;

/// The pixels one slot is measured over.
#[derive(Debug)]
pub(super) enum SlotPixels<'a> {
    /// Every pixel of the slot — a channel's, or a color's photosites — each frame covering all of
    /// them, sampled the way a frame on disk sampled itself.
    Everywhere(StratifiedSamples),
    /// The pixels every frame covers.
    Common(&'a CommonDomain),
    /// The photosites of one color among the pixels every frame covers.
    CommonColor(CommonDomain),
}

impl<'a> SlotPixels<'a> {
    /// Each slot of `planes`' pixels: its channel's, or its color's photosites, among the pixels
    /// every frame covers when `domain` holds them.
    ///
    /// # Errors
    /// [`StackError::NoCommonCoverage`] when a slot has no pixel to measure.
    pub(super) fn of_slots(
        planes: SlotPlanes<'_>,
        domain: Option<&'a CommonDomain>,
    ) -> Result<ArrayVec<Self, 3>, StackError> {
        let size = planes.desc.size();
        let mosaic = planes.slots.mosaic();
        (0..planes.slots.count())
            .map(|slot| match (domain, mosaic) {
                (None, mosaic) => {
                    let samples = StratifiedSamples::new(size, mosaic, slot);
                    if samples.len() == 0 {
                        return Err(StackError::NoCommonCoverage);
                    }
                    Ok(Self::Everywhere(samples))
                }
                (Some(domain), None) => Ok(Self::Common(domain)),
                (Some(domain), Some(mosaic)) => Ok(Self::CommonColor(domain.restricted(
                    size,
                    |pos| mosaic.slot_at(pos) == slot,
                    planes.cancel,
                )?)),
            })
            .collect()
    }

    /// The pixels a median is measured over again, when the frames do not all cover every pixel;
    /// `None` when the statistics measured on each source at load describe the pixels combined.
    pub(super) const fn shared(&self) -> Option<&CommonDomain> {
        match self {
            Self::Everywhere(_) => None,
            Self::CommonColor(domain) => Some(domain),
            Self::Common(domain) => Some(domain),
        }
    }

    /// The sampled pixel indices, ascending.
    pub(super) fn idxs(&self, cancel: &CancelToken) -> Result<Vec<usize>, StackError> {
        match self {
            Self::Everywhere(samples) => Ok(samples.idxs().collect()),
            Self::Common(domain) => domain.stratified_idxs(cancel),
            Self::CommonColor(domain) => domain.stratified_idxs(cancel),
        }
    }
}
