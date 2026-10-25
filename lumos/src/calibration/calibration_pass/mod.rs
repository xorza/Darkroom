//! [`CalibrationPass`]: the masters a frame is calibrated by, applied in one pass.

use arrayvec::ArrayVec;
use common::{Buffer2, par};
use rayon::prelude::*;

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::flags::flag_counts::FlagCounts;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_flags::QualityFlags;
use crate::image::metadata::sample_domain::DomainMap;

/// A master subtracted from a frame: its samples, carried into the frame's domain by `map`, its
/// signal scaled by `scale` — a bias-removed dark to the frame's exposure, 1 otherwise. The map's
/// offset is a level, not signal, and is not scaled.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Subtracted<'a> {
    pub(crate) samples: &'a [f32],
    pub(crate) map: DomainMap,
    pub(crate) scale: f64,
}

impl Subtracted<'_> {
    /// What the master takes from the frame at `idx`.
    #[inline(always)]
    pub(crate) fn at(&self, idx: usize) -> f64 {
        f64::from(self.samples[idx]) * (self.map.gain * self.scale) + self.map.offset
    }
}

/// The arithmetic and flags of calibrating one frame, applied in one row-parallel pass:
/// `(L − Σ subtracted) / flat` in f64, rounded to f32 once, and every master's flags ORed in.
/// A value below zero stays: a stack averages the noise around zero out, and a clamp would bias it
/// upward.
///
/// Under [`Saturation::Detect`], [`QualityFlags::SATURATED`] is taken from each raw sample at the
/// frame's saturation level before it moves, unless the decoder flagged saturation, and `data_max`
/// dropped: past this no level marks the saturated samples, and the flags are the record.
#[derive(Debug)]
pub(crate) struct CalibrationPass<'a> {
    /// The additive masters, each in the frame's domain.
    pub(crate) subtracted: ArrayVec<Subtracted<'a>, 2>,
    /// The divisor: a normalized, floored flat.
    pub(crate) flat: Option<&'a [f32]>,
    /// What the masters leave the frame flagged: see
    /// [`CalibrationMasters`](crate::CalibrationMasters).
    pub(crate) imposed: Option<&'a PixelFlags>,
    pub(crate) saturation: Saturation,
}

/// Whether a pass flags the frame's saturated samples before it moves them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Saturation {
    /// A frame as decoded, a light or a frame of a calibration stack: its samples at its
    /// saturation level are flagged, unless its decoder flagged them, and its `data_max` dropped.
    /// The level is the one the detector applies to a frame as decoded.
    Detect,
    /// A stacked master: a ceiling no file declared is a guess the detector makes for frames as
    /// decoded alone, so its flags and `data_max` stay as they are.
    Kept,
}

impl CalibrationPass<'_> {
    /// Calibrate `image`'s samples and flags in place; under [`Saturation::Detect`] its metadata
    /// says that its saturation is flagged, the rest is the caller's to record.
    pub(crate) fn apply(&self, image: &mut CfaImage) {
        let size = image.size();
        let pixels = size.pixel_count();
        for samples in self
            .subtracted
            .iter()
            .map(|term| term.samples)
            .chain(self.flat)
        {
            debug_assert_eq!(samples.len(), pixels, "a master of another size");
        }
        let saturation = (self.saturation == Saturation::Detect
            && !image.metadata.derived.saturation_flagged())
        .then(|| image.metadata.header.saturation_level());
        let imposed = self.imposed.map(PixelFlags::bytes);
        debug_assert!(imposed.is_none_or(|bytes| bytes.len() == pixels));
        let flags = match image.take_flags() {
            Some(flags) => Some(flags.into_buffer()),
            None => (imposed.is_some() || saturation.is_some()).then(|| Buffer2::new_default(size)),
        };
        let width = size.width.max(1);
        let rows = par::chunks_mut(image.data_mut().pixels_mut(), width);
        let flags = if let Some(mut flags) = flags {
            let counts = rows
                .zip(par::chunks_mut(flags.pixels_mut(), width))
                .enumerate()
                .map(|(y, (row, row_flags))| {
                    let mut counts = FlagCounts::default();
                    for (x, (sample, byte)) in row.iter_mut().zip(row_flags).enumerate() {
                        let idx = y * width + x;
                        if saturation.is_some_and(|level| *sample >= level) {
                            *byte |= QualityFlags::SATURATED.byte();
                        }
                        if let Some(imposed) = imposed {
                            *byte |= imposed[idx];
                        }
                        *sample = self.calibrated(*sample, idx);
                        counts.tally(QualityFlags::from_byte(*byte));
                    }
                    counts
                })
                .reduce(FlagCounts::default, FlagCounts::merged);
            PixelFlags::from_counted(flags, counts)
        } else {
            rows.enumerate().for_each(|(y, row)| {
                for (x, sample) in row.iter_mut().enumerate() {
                    *sample = self.calibrated(*sample, y * width + x);
                }
            });
            None
        };
        image.set_flags(flags);
        if self.saturation == Saturation::Detect {
            image.metadata.derived.mark_saturation_flagged();
            image.metadata.header.data_max = None;
        }
    }

    /// The calibrated value of `sample`, the frame's at `idx`, rounded once.
    #[inline(always)]
    fn calibrated(&self, sample: f32, idx: usize) -> f32 {
        let mut value = f64::from(sample);
        for term in &self.subtracted {
            value -= term.at(idx);
        }
        if let Some(flat) = self.flat {
            value /= f64::from(flat[idx]);
        }
        value as f32
    }
}

#[cfg(test)]
mod tests;
