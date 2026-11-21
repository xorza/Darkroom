//! [`DarkScale`]: the factor a bias-removed dark is subtracted from a light with, fitted on the
//! defect map's hot pixels where enough of them are clean.

use common::Libm;
use common::{PerWorker, par};
use rayon::prelude::*;

use crate::calibration::calibration_pass::Subtracted;
use crate::calibration::dark_match::DarkMatch;
use crate::calibration::defect_map::HotPixels;
use crate::calibration::defect_map::ring_reference::{RingFrame, RingReference};
use crate::frame::capture_condition::{DARK_CURRENT_PER_DEGREE, TEMPERATURE_TOLERANCE};
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_flags::QualityFlags;
use crate::math::statistics::median_mad::MedianMad;

/// The fewest hot pixels the fit keeps for its standard error to be read at all: the MAD's σ of
/// `n` residuals is itself off by about `1.17/√n` (its asymptotic efficiency, 37%), a quarter at
/// 20. The gate on the fit is its precision; this keeps the precision a measurement.
const MIN_FIT_PIXELS: usize = 20;
/// The exposure ratio's relative error where no temperature was compared: a dark of an unknown
/// temperature can be off by any factor, and a fit known to a third beats a guess.
const UNCOMPARED_RATIO_ERROR: f64 = 1.0 / 3.0;
/// The clip, in σ of the residuals' MAD about their median: a star, a cosmic ray or a pixel whose
/// dark current is unstable sits far past it.
const FIT_CLIP_SIGMAS: f64 = 3.0;
/// A bound on the clip passes; a fit settles in a few.
const MAX_FIT_PASSES: usize = 10;

/// The factor a bias-removed dark was subtracted from a light with, and where it came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DarkScale {
    pub factor: f64,
    pub source: DarkScaleSource,
}

/// Where a [`DarkScale`] came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DarkScaleSource {
    /// The light's exposure over the dark's, 1 when they agree or either is not stated: the fit
    /// kept `fit_pixels` hot pixels and was less precise than the ratio, or there was none to fit.
    Exposure { fit_pixels: usize },
    /// The slope of the light's excess at the hot pixels on the dark's, through the origin,
    /// corrected for the dark's noise, after a 3σ clip: from `pixels` of them, with
    /// `standard_error`. It corrects a dark of another exposure or temperature alike, since it
    /// measures the dark current itself.
    Fitted { pixels: usize, standard_error: f64 },
}

/// The exposure ratio a fit competes with: the factor, and the relative error it may carry, which
/// the dark current's change with the temperature difference sets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ExposureRatio {
    pub(crate) factor: f64,
    pub(crate) relative_error: f64,
}

impl ExposureRatio {
    /// The ratio `matched` gives the dark: its factor, and an error of
    /// `12% · max(|ΔT|, 1 °C)` where a temperature was compared, the dark current's change over
    /// the difference or over the tolerance that counted the two as one, and of a third where none
    /// was.
    pub(crate) fn of(matched: DarkMatch) -> Self {
        let comparison = matched.comparison;
        let relative_error = match (comparison.temperature, comparison.temperature_difference) {
            (None, _) => UNCOMPARED_RATIO_ERROR,
            (Some(_), difference) => {
                let degrees =
                    difference.map_or(0.0, |difference| (difference.frame - difference.dark).abs());
                DARK_CURRENT_PER_DEGREE * degrees.max(TEMPERATURE_TOLERANCE)
            }
        };
        Self {
            factor: matched.factor(),
            relative_error,
        }
    }
}

/// What a fit reads of one light: its raw samples, its bias, how its pixels are flagged before
/// calibration — the masters' unmeasured pixels among them — and the gain that carries the dark's
/// samples into its domain.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FitLight<'a> {
    pub(crate) light: &'a CfaImage,
    pub(crate) bias: Option<Subtracted<'a>>,
    pub(crate) imposed: Option<&'a PixelFlags>,
    pub(crate) dark_gain: f64,
}

/// One hot pixel's pair: the dark's excess in the light's units, the variance of the dark's noise
/// in it, and the light's excess.
#[derive(Debug, Clone, Copy)]
struct Pair {
    dark: f64,
    dark_variance: f64,
    light: f64,
}

impl DarkScale {
    /// The scale of a bias-removed dark for `light`, fitted on the `hot` pixels, where the fit is
    /// more precise than `ratio`: a positive slope, which every dark current gives, whose relative
    /// standard error is below the ratio's relative error, from at least 20 pairs; else `ratio`'s
    /// factor.
    ///
    /// A hot pixel takes part unless the light has no measurement there or saturated it, or a
    /// master did — a dark that saturated a hot pixel cut its excess short — and is chosen on the
    /// dark's excess alone, as selecting on the light would bias the slope. The light's excess is
    /// its sample less the bias, less the same reference the dark's excess was taken against — a
    /// robust plane through the ring of its color's photosites — so the sky, a gradient and the
    /// dark's uniform current cancel. The flat does not shade dark current, so the fit reads the
    /// light before it.
    ///
    /// The dark's excess carries the dark's noise, which least squares on it attenuates by
    /// `Σσ²/Σd²` — about 4% for excesses near the detection's 5σ — so the fit is the moment
    /// estimator that removes it, for a regressor of known error variance (Fuller, *Measurement
    /// Error Models*, 1987, ch. 1): `k = Σd·l / (Σd² − Σσ²)` through the origin, refitted on the
    /// pairs within 3σ of the residuals' median until they settle; its standard error is that σ
    /// over `√(Σd² − Σσ²)`.
    pub(crate) fn fit(hot: &HotPixels, light: FitLight<'_>, ratio: ExposureRatio) -> Self {
        Self::chosen(fit_through_origin(&light.pairs(hot)), ratio)
    }

    /// `fit` where it is precise enough to beat `ratio`, else `ratio`.
    fn chosen(fit: Option<Fit>, ratio: ExposureRatio) -> Self {
        match fit {
            Some(Fit {
                slope,
                standard_error,
                pixels,
            }) if pixels >= MIN_FIT_PIXELS
                && slope.is_finite()
                && slope > 0.0
                && standard_error < ratio.relative_error * slope =>
            {
                Self {
                    factor: slope,
                    source: DarkScaleSource::Fitted {
                        pixels,
                        standard_error,
                    },
                }
            }
            fit => Self {
                factor: ratio.factor,
                source: DarkScaleSource::Exposure {
                    fit_pixels: fit.map_or(0, |fit| fit.pixels),
                },
            },
        }
    }
}

impl FitLight<'_> {
    /// The pair at each hot pixel the light measured unsaturated and whose ring fixes a plane.
    fn pairs(&self, hot: &HotPixels) -> Vec<Pair> {
        let image = self.light;
        let size = image.size();
        let samples = image.data().pixels();
        let saturation = image.metadata.derived.unflagged_saturation();
        let excluded = QualityFlags::SATURATED.union(QualityFlags::NO_DATA);
        let unmeasured = |idx: usize| {
            image
                .flags()
                .is_some_and(|flags| flags.at(idx).intersects(excluded))
                || self
                    .imposed
                    .is_some_and(|flags| flags.at(idx).intersects(excluded))
                || saturation.is_some_and(|level| samples[idx] >= level)
        };
        let less_bias =
            |idx: usize| f64::from(samples[idx]) - self.bias.map_or(0.0, |bias| bias.at(idx));
        let rings = PerWorker::new(RingReference::default);
        par::iter(&hot.idxs)
            .zip(&hot.excess)
            .zip(&hot.noise)
            .map(|((&idx, &excess), &noise)| {
                if unmeasured(idx) {
                    return None;
                }
                let frame = RingFrame {
                    size,
                    value: less_bias,
                };
                let level = rings
                    .get()
                    .at(frame, &image.cfa_type(), size.point_of(idx))?;
                Some(Pair {
                    dark: f64::from(excess) * self.dark_gain,
                    dark_variance: (f64::from(noise) * self.dark_gain).libm_powi(2),
                    light: less_bias(idx) - f64::from(level),
                })
            })
            .flatten()
            .collect()
    }
}

/// A clipped slope through the origin, corrected for the noise in its regressor.
#[derive(Debug, Clone, Copy)]
struct Fit {
    slope: f64,
    standard_error: f64,
    pixels: usize,
}

/// The slope of `light = k·dark` through the origin over `pairs`, `Σd·l / (Σd² − Σσ²)`, refitted
/// on the pairs within [`FIT_CLIP_SIGMAS`] of the residuals' median, in their MAD's σ, until the
/// kept set settles; `None` for no pair whose dark excess stands above its noise.
fn fit_through_origin(pairs: &[Pair]) -> Option<Fit> {
    let mut kept = vec![true; pairs.len()];
    let mut residuals = Vec::with_capacity(pairs.len());
    let mut fit = None;
    for _ in 0..MAX_FIT_PASSES {
        let (moment, energy, pixels) = pairs.iter().zip(&kept).filter(|(_, kept)| **kept).fold(
            (0.0, 0.0, 0usize),
            |(moment, energy, pixels), (pair, _)| {
                (
                    moment + pair.dark * pair.light,
                    energy + pair.dark * pair.dark - pair.dark_variance,
                    pixels + 1,
                )
            },
        );
        if energy <= 0.0 {
            return fit;
        }
        let slope = moment / energy;
        residuals.clear();
        residuals.extend(
            pairs
                .iter()
                .zip(&kept)
                .filter(|(_, kept)| **kept)
                .map(|(pair, _)| pair.light - slope * pair.dark),
        );
        let residual = MedianMad::of_mut(&mut residuals);
        let sigma = residual.sigma();
        fit = Some(Fit {
            slope,
            standard_error: sigma / energy.sqrt(),
            pixels,
        });
        if sigma <= 0.0 {
            break;
        }
        let mut changed = false;
        for (pair, kept) in pairs.iter().zip(&mut kept) {
            let keep = ((pair.light - slope * pair.dark) - residual.median).abs()
                <= FIT_CLIP_SIGMAS * sigma;
            changed |= keep != *kept;
            *kept = keep;
        }
        if !changed {
            break;
        }
    }
    fit
}

#[cfg(test)]
mod tests {
    use crate::calibration::dark_match::DarkMatch;
    use crate::calibration::dark_scale::{
        DarkScale, DarkScaleSource, ExposureRatio, Fit, Pair, fit_through_origin,
    };
    use crate::image::metadata::dark_comparison::DarkComparison;
    use crate::image::metadata::temperature_difference::TemperatureDifference;
    use crate::image::metadata::temperature_source::TemperatureSource;
    use crate::math::statistics;

    /// The ratio's error is the dark current's change over the temperature difference, at least
    /// the 1 °C tolerance: 12% for a match within it, 36% for 3 °C apart; a third with no
    /// temperature compared.
    #[test]
    fn the_ratio_s_error_follows_the_temperature_difference() {
        let matched = |temperature, temperature_difference| DarkMatch {
            scale: Some(2.5),
            comparison: DarkComparison {
                exposure_compared: true,
                temperature,
                temperature_difference,
            },
        };
        let sensor = Some(TemperatureSource::Sensor);
        let apart = Some(TemperatureDifference {
            reading: TemperatureSource::Sensor,
            frame: -7.0,
            dark: -10.0,
        });
        for (matched, relative_error) in [
            (matched(sensor, None), 0.12),
            (matched(sensor, apart), 0.12 * 3.0),
            (matched(None, None), 1.0 / 3.0),
        ] {
            assert_eq!(
                ExposureRatio::of(matched),
                ExposureRatio {
                    factor: 2.5,
                    relative_error,
                }
            );
        }
    }

    /// A fit stands where its relative standard error is below the ratio's error: of 100 pairs, a
    /// slope of 1 known to 0.1 beats a ratio good to 12% and loses to one good to 12% when known to
    /// 0.2, a hundred weak pairs; a third, a ratio of no temperature, it beats.
    #[test]
    fn a_fit_is_taken_where_it_beats_the_ratio() {
        let fit = |standard_error| {
            Some(Fit {
                slope: 1.0,
                standard_error,
                pixels: 100,
            })
        };
        let ratio = |relative_error| ExposureRatio {
            factor: 2.5,
            relative_error,
        };
        let fitted = |standard_error| DarkScale {
            factor: 1.0,
            source: DarkScaleSource::Fitted {
                pixels: 100,
                standard_error,
            },
        };
        let exposure = DarkScale {
            factor: 2.5,
            source: DarkScaleSource::Exposure { fit_pixels: 100 },
        };
        assert_eq!(DarkScale::chosen(fit(0.1), ratio(0.12)), fitted(0.1));
        assert_eq!(DarkScale::chosen(fit(0.2), ratio(0.12)), exposure);
        assert_eq!(DarkScale::chosen(fit(0.2), ratio(1.0 / 3.0)), fitted(0.2));
        assert_eq!(
            DarkScale::chosen(None, ratio(0.12)),
            DarkScale {
                factor: 2.5,
                source: DarkScaleSource::Exposure { fit_pixels: 0 },
            }
        );
    }

    fn pair(dark: f64, dark_variance: f64, light: f64) -> Pair {
        Pair {
            dark,
            dark_variance,
            light,
        }
    }

    /// The noise in the dark's excess is taken out of the fit. Two hot pixels of true excess 10
    /// read 11 and 9 in a dark of noise σ = 1, and 20 in a light of twice the current: least
    /// squares gives `(220 + 180) / (121 + 81) = 400/202`, the corrected fit `400 / (202 − 2) = 2`,
    /// exact. The residuals −2 and 2 have median 0 and MAD 2, so the standard error is that MAD's σ
    /// over `√200`. A pair whose excess does not stand above its noise, `d² ≤ σ²`, leaves no fit.
    #[test]
    fn the_fit_removes_the_dark_s_noise() {
        let fit = fit_through_origin(&[pair(11.0, 1.0, 20.0), pair(9.0, 1.0, 20.0)]).unwrap();
        assert_eq!(fit.slope, 2.0);
        assert_eq!(fit.pixels, 2);
        assert_eq!(
            fit.standard_error,
            statistics::sigma_of_mad(2.0, 2) / 200f64.sqrt()
        );
        assert!(fit_through_origin(&[pair(1.0, 1.0, 5.0)]).is_none());
        assert!(fit_through_origin(&[]).is_none());
    }

    /// The clip drops a pair far off the line and refits on the rest: four pairs on `l = 3d`, and
    /// one at `d = 4` reading 100, far past 3σ of the residuals' MAD. Without noise the refit is
    /// least squares, 3 exactly, over the four kept.
    #[test]
    fn the_clip_refits_without_the_outlier() {
        let mut pairs: Vec<Pair> = [1.0, 2.0, 3.0, 5.0]
            .into_iter()
            .map(|dark| pair(dark, 0.0, 3.0 * dark))
            .collect();
        pairs.push(pair(4.0, 0.0, 100.0));
        let fit = fit_through_origin(&pairs).unwrap();
        assert_eq!(fit.slope, 3.0);
        assert_eq!(fit.pixels, 4);
        assert_eq!(fit.standard_error, 0.0);
    }
}
