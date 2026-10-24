//! [`DarkScale`]: the factor a bias-removed dark is subtracted from a light with, fitted on the
//! defect map's hot pixels where enough of them are clean.

use common::{PerWorker, par};
use rayon::prelude::*;

use crate::calibration::calibration_pass::Subtracted;
use crate::calibration::defect_map::HotPixels;
use crate::calibration::defect_map::ring_reference::{RingFrame, RingReference};
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::math::statistics::median_mad::MedianMad;

/// The fewest hot pixels the fit keeps for its factor to stand; with fewer the exposure ratio does.
const MIN_FIT_PIXELS: usize = 100;
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
    /// kept `fit_pixels` hot pixels, fewer than it needs, or there was none to fit.
    Exposure { fit_pixels: usize },
    /// The least-squares slope of the light's excess at the hot pixels on the dark's, through the
    /// origin, after a 3σ clip: from `pixels` of them, with `standard_error`. It corrects a dark
    /// of another exposure or temperature alike, since it measures the dark current itself.
    Fitted { pixels: usize, standard_error: f64 },
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

/// One hot pixel's pair: the dark's excess in the light's units, and the light's.
#[derive(Debug, Clone, Copy)]
struct Pair {
    dark: f64,
    light: f64,
}

impl DarkScale {
    /// The scale of a bias-removed dark for `light`, fitted on the `hot` pixels, else
    /// `exposure_ratio`: when fewer than 100 pairs are kept, or the slope is not a positive
    /// number, which no dark current gives.
    ///
    /// A hot pixel takes part unless the light has no measurement there or saturated it, or a
    /// master did — a dark that saturated a hot pixel cut its excess short — and is chosen on the
    /// dark's excess alone: selecting on the regressor leaves a least-squares slope
    /// unbiased, where selecting on the light would bias it. The light's excess is its sample less
    /// the bias, less the same reference the dark's excess was taken against — a robust plane
    /// through the ring of its color's photosites — so the sky, a gradient and the dark's
    /// uniform current cancel. The flat does not shade dark current, so the fit reads the light
    /// before it. The fit is `l = k·d` through the origin, refitted on the pairs within 3σ of the
    /// residuals' median until they settle; its standard error is that σ over `√Σd²`.
    pub(crate) fn fit(hot: &HotPixels, light: FitLight<'_>, exposure_ratio: f64) -> Self {
        let pairs = light.pairs(hot);
        match fit_through_origin(&pairs) {
            Some(Fit {
                slope,
                standard_error,
                pixels,
            }) if pixels >= MIN_FIT_PIXELS && slope.is_finite() && slope > 0.0 => Self {
                factor: slope,
                source: DarkScaleSource::Fitted {
                    pixels,
                    standard_error,
                },
            },
            fit => Self {
                factor: exposure_ratio,
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
        let saturation = (!image.metadata.derived.saturation_flagged())
            .then(|| image.metadata.header.saturation_level());
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
            .map(|(&idx, &excess)| {
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
                    light: less_bias(idx) - f64::from(level),
                })
            })
            .flatten()
            .collect()
    }
}

/// A clipped least-squares slope through the origin.
#[derive(Debug, Clone, Copy)]
struct Fit {
    slope: f64,
    standard_error: f64,
    pixels: usize,
}

/// The slope of `light = k·dark` through the origin over `pairs`, refitted on the pairs within
/// [`FIT_CLIP_SIGMAS`] of the residuals' median, in their MAD's σ, until the kept set settles;
/// `None` for no pair with a dark excess.
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
                    energy + pair.dark * pair.dark,
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
