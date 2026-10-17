//! Putting every frame on one photometric scale before they are combined.
//!
//! Frames of the same field differ in sky level and transparency, so combining them raw would let
//! the brightest dominate and turn rejection into a vote about exposure rather than about
//! outliers. Each frame gets an affine `gain`/`offset` per slot — a channel, or a color of a
//! mosaic, whose colors drift apart over a session of twilight flats — measured against a
//! reference, the least noisy of the set, and the combine applies it as it gathers.
//!
//! `Normalization::Global` fits every frame's gain against the reference by
//! [`photometric_gain`]'s errors-in-variables fit over paired pixels, and its offset from the two
//! medians. `Normalization::Multiplicative` takes the ratio of medians. A slot is measured over
//! its own pixels — a color over its photosites — and coverage narrows them: frames that each
//! cover every pixel are measured over all of them, and when any frame contributes at only some
//! pixels — a warp's edge, or a source's declared nulls — over [`common_domain`], the pixels every
//! frame reached. Frames gathered in each channel by their own rule, a drizzle of mosaics, are
//! measured pair by pair against the reference, or one by one when some pair shares no pixel —
//! see [`FrameNorm::measure`].

pub(crate) mod common_domain;
mod norm_fit;
mod per_channel_norms;
pub(crate) mod photometric_gain;
pub(crate) mod planned_normalization;
mod slot_pixels;
pub(crate) mod slot_planes;

use arrayvec::ArrayVec;

use crate::combine::error::StackError;
use crate::combine::normalization::norm_fit::{Fitted, NormFit};
use crate::combine::normalization::slot_planes::SlotPlanes;
use crate::error::Cancelled;
use crate::frame_store::stored_frame::StoredFrame;
use crate::image::sample_domain::DomainMap;

/// Frame normalization method applied before stacking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Normalization {
    /// No normalization.
    None,
    /// Match global median and scale across frames (additive + scaling).
    /// Best for light frames.
    Global,
    /// Scale by ratio of medians (no additive offset).
    /// Best for flat frames where exposure varies.
    Multiplicative,
}

/// One slot's affine normalization, applied as `normalized = raw * gain + offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SlotNorm {
    pub(crate) gain: f32,
    pub(crate) offset: f32,
}

impl SlotNorm {
    pub(super) const IDENTITY: Self = Self {
        gain: 1.0,
        offset: 0.0,
    };

    /// `gain = median_ref / median` for slot `slot` of the frame of `(index, median)` against the
    /// reference's `(index, median)`.
    ///
    /// # Errors
    /// [`StackError::NonPositiveMedian`] when a median is not positive: a ratio to it scales
    /// nothing, and unit gain in its place would combine the frame at a scale no one measured.
    pub(super) fn median_ratio(
        (index, median): (usize, f32),
        (reference, reference_median): (usize, f32),
        slot: usize,
    ) -> Result<Self, StackError> {
        if median > 0.0 && reference_median > 0.0 {
            Ok(Self {
                gain: reference_median / median,
                offset: 0.0,
            })
        } else {
            let (index, median) = if median > 0.0 {
                (reference, reference_median)
            } else {
                (index, median)
            };
            Err(StackError::NonPositiveMedian {
                index,
                slot,
                median,
            })
        }
    }
}

/// A frame's affine normalization, one per
/// [`Slots`](crate::combine::frame_read::slots::Slots) slot.
#[derive(Debug, Clone)]
pub(crate) struct FrameNorm {
    pub(crate) slots: ArrayVec<SlotNorm, 3>,
}

impl FrameNorm {
    /// The per-frame affine every frame of `planes` is combined through, or `None` when every
    /// frame is taken as it stands.
    ///
    /// Expressed in the domain of the first frame that declares one — the domain the stacked
    /// product records. With no normalization each frame is only converted into it (gain `scale_i /
    /// scale_ref`, offset from the pedestals); a fitted normalization maps every frame onto its
    /// reference frame, so its gains and offsets are then converted from the reference frame's
    /// domain the same way. Frames whose domains agree get exactly the norms they had before
    /// conversion existed.
    pub(crate) fn measure(
        planes: SlotPlanes<'_>,
        normalization: Normalization,
    ) -> Result<Option<Vec<Self>>, StackError> {
        let to_domain = domain_maps(planes.frames);
        let Some(fitted) = Fitted::of(normalization) else {
            return Ok(to_domain
                .iter()
                .any(|&map| map != DomainMap::IDENTITY)
                .then(|| {
                    to_domain
                        .iter()
                        .map(|map| Self {
                            slots: (0..planes.slots.count())
                                .map(|_| SlotNorm {
                                    gain: map.gain as f32,
                                    offset: map.offset as f32,
                                })
                                .collect(),
                        })
                        .collect()
                }));
        };
        Cancelled::check(planes.cancel)?;
        let fit = NormFit {
            planes,
            normalization: fitted,
            reference: select_reference_frame(planes.frames, &to_domain),
        };
        let mut norms = fit.frame_norms()?;
        // A fitted norm lands each frame on the reference frame's raw values; the reference's own
        // map then carries those into the shared domain: `map(gain·x + offset)`.
        let map = to_domain[fit.reference];
        if map != DomainMap::IDENTITY {
            for slot in norms.iter_mut().flat_map(|norm| norm.slots.iter_mut()) {
                slot.gain = (map.gain * f64::from(slot.gain)) as f32;
                slot.offset = (map.gain * f64::from(slot.offset) + map.offset) as f32;
            }
        }
        Ok(Some(norms))
    }
}

/// The map that expresses each frame in the domain of the first frame declaring one; the identity
/// for a frame that declares none, or when none does.
///
/// The combine's constructors admit the set's facts first ([`SetFacts`]), so every declared domain
/// converts.
///
/// [`SetFacts`]: crate::ingest::set_facts::SetFacts
fn domain_maps(frames: &[StoredFrame]) -> Vec<DomainMap> {
    let reference = frames.iter().find_map(|frame| frame.facts.domain.as_ref());
    frames
        .iter()
        .map(|frame| match (&frame.facts.domain, reference) {
            (Some(domain), Some(reference)) => domain
                .conversion_to(reference)
                .expect("the frame set's sample domains were validated as convertible"),
            _ => DomainMap::IDENTITY,
        })
        .collect()
}

/// The least noisy frame: the lowest mean noise variance over its slots, each σ carried into the
/// shared domain by `to_domain`, so frames decoded at different scales compare in one unit. The
/// first wins a tie.
fn select_reference_frame(frames: &[StoredFrame], to_domain: &[DomainMap]) -> usize {
    let mean_variance = |(frame, map): (&StoredFrame, &DomainMap)| {
        let stats = &frame.source_stats;
        (0..stats.medians.len())
            .map(|slot| (map.gain * f64::from(stats.slot_noise(slot))).powi(2))
            .sum::<f64>()
            / stats.medians.len() as f64
    };
    let mut scores = frames.iter().zip(to_domain).map(mean_variance).enumerate();
    let (_, mut best_score) = scores.next().expect("normalization requires frames");
    let mut best_frame = 0;
    for (frame, score) in scores {
        if score < best_score {
            best_score = score;
            best_frame = frame;
        }
    }
    best_frame
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
