//! [`LightCalibration`]: the masters a light is calibrated by, applied in one pass.

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::flag_counts::FlagCounts;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::image::sample_domain::DomainMap;

/// A master subtracted from a light: its samples, carried into the light's domain by `map`, its
/// signal scaled by `scale` — a bias-removed dark to the light's exposure, 1 otherwise. The map's
/// offset is a level, not signal, and is not scaled.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Subtracted<'a> {
    pub(crate) samples: &'a [f32],
    pub(crate) map: DomainMap,
    pub(crate) scale: f64,
}

impl Subtracted<'_> {
    /// What the master takes from the light at `index`.
    #[inline(always)]
    pub(crate) fn at(&self, index: usize) -> f64 {
        f64::from(self.samples[index]) * (self.map.gain * self.scale) + self.map.offset
    }
}

/// The arithmetic and flags of calibrating one light, applied in one row-parallel pass:
/// `(L − bias − dark) / flat` in f64, rounded to f32 once, and every master's flags ORed in.
///
/// [`QualityFlags::SATURATED`] is taken from each raw sample at the light's saturation level before
/// it moves, unless the decoder flagged saturation, and `data_max` dropped: past this no level
/// marks the saturated samples, and the flags are the record. The level is the one the detector
/// applies to a frame as decoded; a master is not tested against it, since a ceiling no file
/// declared is a guess the detector makes for lights alone.
#[derive(Debug)]
pub(crate) struct LightCalibration<'a> {
    pub(crate) bias: Option<Subtracted<'a>>,
    pub(crate) dark: Option<Subtracted<'a>>,
    /// The divisor: a normalized, floored flat.
    pub(crate) flat: Option<&'a [f32]>,
    /// What the masters leave the light flagged: see
    /// [`CalibrationMasters`](crate::CalibrationMasters).
    pub(crate) imposed: Option<&'a PixelFlags>,
}

impl LightCalibration<'_> {
    /// Calibrate `image`'s samples and flags in place; its metadata says only that its saturation
    /// is flagged, the rest is the caller's to record.
    pub(crate) fn apply(&self, image: &mut CfaImage) {
        let size = image.size();
        let pixels = size.pixel_count();
        for samples in [
            self.bias.map(|term| term.samples),
            self.dark.map(|term| term.samples),
            self.flat,
        ]
        .into_iter()
        .flatten()
        {
            debug_assert_eq!(samples.len(), pixels, "a master of another size");
        }
        let saturation =
            (!image.metadata.saturation_flagged).then(|| image.metadata.saturation_level());
        let imposed = self.imposed.map(PixelFlags::bytes);
        debug_assert!(imposed.is_none_or(|bytes| bytes.len() == pixels));
        let flags = match image.flags.take() {
            Some(flags) => Some(flags.into_buffer()),
            None => (imposed.is_some() || saturation.is_some())
                .then(|| Buffer2::new_default(size.width, size.height)),
        };
        let width = size.width.max(1);
        let rows = image.data.pixels_mut().par_chunks_mut(width);
        image.flags = if let Some(mut flags) = flags {
            let counts = rows
                .zip(flags.pixels_mut().par_chunks_mut(width))
                .enumerate()
                .map(|(y, (row, row_flags))| {
                    let mut counts = FlagCounts::default();
                    for (x, (sample, byte)) in row.iter_mut().zip(row_flags).enumerate() {
                        let index = y * width + x;
                        if saturation.is_some_and(|level| *sample >= level) {
                            *byte |= QualityFlags::SATURATED.byte();
                        }
                        if let Some(imposed) = imposed {
                            *byte |= imposed[index];
                        }
                        *sample = self.calibrated(*sample, index);
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
        image.metadata.saturation_flagged = true;
        image.metadata.data_max = None;
    }

    /// The calibrated value of `sample`, the light's at `index`, rounded once.
    #[inline(always)]
    fn calibrated(&self, sample: f32, index: usize) -> f32 {
        let mut value = f64::from(sample);
        if let Some(bias) = &self.bias {
            value -= bias.at(index);
        }
        if let Some(dark) = &self.dark {
            value -= dark.at(index);
        }
        if let Some(flat) = self.flat {
            value /= f64::from(flat[index]);
        }
        value as f32
    }
}

#[cfg(test)]
mod tests;
