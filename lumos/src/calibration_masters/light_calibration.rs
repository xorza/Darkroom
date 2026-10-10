//! [`LightCalibration`]: the masters a light is calibrated by, applied in one pass.

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::image::cfa::CfaImage;
use crate::io::image::pixel_flags::{FlagCounts, PixelFlags, QualityFlags};
use crate::io::image::sample_domain::DomainMap;

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
    fn at(&self, index: usize) -> f64 {
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
                        counts.tally(*byte);
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
mod tests {
    use crate::calibration_masters::light_calibration::{LightCalibration, Subtracted};
    use crate::internals::cfa::make_cfa;
    use crate::internals::test_rng::TestRng;
    use crate::io::image::cfa::CfaType;
    use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
    use crate::io::image::sample_domain::DomainMap;
    use crate::math::size2us::Size2us;

    /// Each sample is `(L − B·g_b − o_b − D·g_d·k − o_d)/F`, rounded to f32 once: within half a
    /// unit in its last place of that value, worked in f64 in another order, where three rounded
    /// f32 steps would stray past it. Saturation is read from the raw sample, before it moves, and
    /// the masters' flags are ORed in: the light saturated at pixels 0 and 7 (at or past the level
    /// 0.95 of a span of 1) and imposed `NO_DATA` at 3 holds exactly those.
    #[test]
    fn a_light_is_its_f64_calibration_rounded_once() {
        let size = Size2us::new(16, 8);
        let mut rng = TestRng::new(5);
        let mut plane = |base: f32, spread: f32| -> Vec<f32> {
            (0..size.pixel_count())
                .map(|_| base + spread * rng.next_f32())
                .collect()
        };
        let mut light_samples = plane(0.3, 0.5);
        light_samples[0] = 0.95;
        light_samples[7] = 0.99;
        let bias = plane(0.01, 0.003);
        let dark = plane(0.004, 0.01);
        let flat = plane(0.6, 0.7);
        let bias_map = DomainMap {
            gain: 0.75,
            offset: -0.002,
        };
        let dark_map = DomainMap {
            gain: 1.25,
            offset: 0.0005,
        };
        let scale = 1.7;
        let imposed = PixelFlags::from_fn(size, |index| {
            if index == 3 {
                QualityFlags::NO_DATA
            } else {
                QualityFlags::default()
            }
        })
        .unwrap();
        let mut light = make_cfa(size, light_samples.clone(), CfaType::Mono);
        LightCalibration {
            bias: Some(Subtracted {
                samples: &bias,
                map: bias_map,
                scale: 1.0,
            }),
            dark: Some(Subtracted {
                samples: &dark,
                map: dark_map,
                scale,
            }),
            flat: Some(&flat),
            imposed: Some(&imposed),
        }
        .apply(&mut light);

        let mut chain_strays = 0;
        for index in 0..size.pixel_count() {
            let removed = (f64::from(bias[index]) * bias_map.gain + bias_map.offset)
                + (f64::from(dark[index]) * (dark_map.gain * scale) + dark_map.offset);
            let exact = (f64::from(light_samples[index]) - removed) / f64::from(flat[index]);
            let within_half_ulp = |value: f32| {
                let half_ulp = f64::from(value.abs()) * f64::from(f32::EPSILON) / 2.0;
                (f64::from(value) - exact).abs() <= half_ulp * (1.0 + 1e-9)
            };
            let value = light.data.pixels()[index];
            assert!(
                within_half_ulp(value),
                "pixel {index}: {value} against {exact}"
            );
            let chain = ((light_samples[index]
                - (bias[index] * bias_map.gain as f32 + bias_map.offset as f32))
                - (dark[index] * (dark_map.gain * scale) as f32 + dark_map.offset as f32))
                / flat[index];
            chain_strays += usize::from(!within_half_ulp(chain));
        }
        assert!(
            chain_strays > 0,
            "the f32 chain must stray for the test to tell"
        );
        let flags = light.flags.as_ref().unwrap();
        let flagged: Vec<(usize, QualityFlags)> = (0..size.pixel_count())
            .map(|index| (index, flags.at(index)))
            .filter(|(_, flags)| *flags != QualityFlags::default())
            .collect();
        assert_eq!(
            flagged,
            [
                (0, QualityFlags::SATURATED),
                (3, QualityFlags::NO_DATA),
                (7, QualityFlags::SATURATED)
            ]
        );
        assert!(light.metadata.saturation_flagged);
    }
}
