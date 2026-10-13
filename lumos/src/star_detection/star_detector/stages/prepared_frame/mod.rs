//! [`PreparedFrame`]: a frame reduced to the planes detection and measurement read.

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::image::linear_image::LinearImage;
use crate::image::masked_plane::MaskedPlane;
use crate::image::quality_flags::QualityFlags;
use crate::math::noise::mrs_noise::MrsNoise;
use crate::scratch_pool::Pooled;
use crate::star_detection::background::background_estimate::BackgroundEstimate;
use crate::star_detection::background::sky_noise::SkyNoise;
use crate::star_detection::centroid::MeasurePlanes;
use crate::star_detection::centroid::measure_grid::MeasureGrid;
use crate::star_detection::deblend::region::Region;
use crate::star_detection::detection_plane::{DetectionPlane, PlaneSpec, Prefilter};
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star::Star;
use crate::star_detection::star_detection_config::StarDetectionConfig;
use crate::star_detection::star_detection_config::measurement_config::MeasurementConfig;

/// The width a measurement assumes when no FWHM is known: zero, which `measure_grid::stamp_radius`
/// and the moments window clamp up to their smallest stamp and narrowest window — the choice that
/// assumes nothing about the star beyond its being at least that wide.
const UNKNOWN_FWHM: f32 = 0.0;

/// A frame reduced to one plane and its sky: what detection and measurement read of it.
///
/// Measurement reads `measure`, which no filter touched: a 3×3 median keeps 56% of the flux and 42%
/// of the peak of a Gaussian star of FWHM 2. Detection reads a [`DetectionPlane`] built from it.
#[derive(Debug)]
pub(crate) struct PreparedFrame {
    /// The channels' noise-weighted combination, less the sky.
    pub(crate) measure: Pooled<Buffer2<f32>>,
    /// The sky's σ on `measure`.
    pub(crate) sky: SkyNoise,
    pub(crate) saturation: Pooled<BitBuffer2>,
    /// The pixels with no measurement: left out of the sky, of every noise and of every threshold.
    pub(crate) no_data: Option<Pooled<BitBuffer2>>,
    /// The sources the refined sky was measured around, and the pixels with no data; `None`
    /// without a refinement.
    sources: Option<Pooled<BitBuffer2>>,
    /// The 3×3 median for a demosaiced frame, whose detection plane takes it first.
    prefilter: Prefilter,
}

impl PreparedFrame {
    /// Combine the channels, measure the sky, refining it around the sources the configuration
    /// asks for, and take it out.
    pub(crate) fn new(
        image: &LinearImage,
        config: &StarDetectionConfig,
        resources: &mut DetectionResources,
    ) -> Self {
        let mut plane = resources.acquire_f32();
        combine_channels(image, &mut plane);
        let prefilter = if image.metadata.is_demosaiced() {
            Prefilter::Median3x3
        } else {
            Prefilter::None
        };
        let no_data = image
            .flags
            .as_ref()
            .filter(|flags| flags.contains(QualityFlags::NO_DATA))
            .map(|flags| {
                let mut mask = resources.acquire_bit();
                mask.fill_from_predicate(|index| flags.at(index).intersects(QualityFlags::NO_DATA));
                mask
            });

        // The sources are found at the FWHM the configuration starts from.
        let refined = BackgroundEstimate::estimate(
            MaskedPlane {
                pixels: &plane,
                mask: no_data.as_deref(),
            },
            &config.background,
            resources,
        )
        .refine(
            &plane,
            PlaneSpec {
                prefilter,
                matched: config
                    .fwhm
                    .mode
                    .map(|mode| config.fwhm.filter_at(mode.seed())),
                mask: no_data.as_deref(),
                background: &config.background,
            },
            resources,
        );

        // Saturation is a property of the recorded values, read before anything combines them.
        let mut saturation = resources.acquire_bit();
        mark_saturated(image, &mut saturation);
        let sky = refined.estimate.subtract_from(&mut plane);
        Self {
            measure: plane,
            sky,
            saturation,
            no_data,
            sources: refined.sources,
            prefilter,
        }
    }

    /// The detection plane of this frame, matched-filtered at `fwhm` when one is given.
    pub(crate) fn detection_plane(
        &self,
        fwhm: Option<f32>,
        config: &StarDetectionConfig,
        resources: &mut DetectionResources,
    ) -> DetectionPlane {
        let mut residual = resources.acquire_f32();
        residual.pixels_mut().copy_from_slice(self.measure.pixels());
        DetectionPlane::from_residual(
            residual,
            PlaneSpec {
                prefilter: self.prefilter,
                matched: fwhm.map(|fwhm| config.fwhm.filter_at(fwhm)),
                mask: self.sources.as_deref().or(self.no_data.as_deref()),
                background: &config.background,
            },
            resources,
        )
    }

    /// Measure the stars of `regions` on the measurement plane, in parallel: sub-pixel positions,
    /// flux, FWHM and quality metrics, flagging those whose peak pixel is saturated.
    /// `expected_fwhm` sizes every stamp; `None` measures at [`UNKNOWN_FWHM`].
    pub(crate) fn measure_stars(
        &self,
        regions: &[Region],
        config: &MeasurementConfig,
        expected_fwhm: Option<f32>,
    ) -> Vec<Star> {
        // One grid for the whole detection: `expected_fwhm` fixes the stamp, the window and the
        // annulus, so every candidate is measured over the same ones.
        let grid = MeasureGrid::new(expected_fwhm.unwrap_or(UNKNOWN_FWHM));
        let planes = MeasurePlanes {
            residual: &self.measure,
            sky: &self.sky,
            saturation: &self.saturation,
            no_data: self.no_data.as_deref(),
        };
        regions
            .par_iter()
            .filter_map(|region| planes.measure_star(region, config, &grid))
            .collect()
    }

    /// Return the source mask once no detection plane is built any more: only their noise reads it.
    pub(crate) fn drop_sources(&mut self) {
        self.sources = None;
    }
}

/// Reduce `image` to one plane in `output`: its only channel, or the inverse-variance combination
/// of its three (see [`inverse_variance_weights`]).
fn combine_channels(image: &LinearImage, output: &mut Buffer2<f32>) {
    if image.is_grayscale() {
        output
            .pixels_mut()
            .copy_from_slice(image.channel(0).pixels());
        return;
    }
    let weights = inverse_variance_weights(channel_noise(image));
    let r = image.channel(0).pixels();
    let g = image.channel(1).pixels();
    let b = image.channel(2).pixels();
    output
        .pixels_mut()
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, o)| {
            *o = weights[0] * r[i] + weights[1] * g[i] + weights[2] * b[i];
        });
}

/// Each channel's white noise, by the multiresolution estimator over the pixels no flag names: the
/// noise stacking weighs by, and not the MAD, which a red nebula inflates in red.
fn channel_noise(image: &LinearImage) -> [f32; 3] {
    let flags = image.flags.as_ref();
    let excluded =
        |index: usize| flags.is_some_and(|flags| flags.at(index) != QualityFlags::default());
    [0, 1, 2].map(|channel| MrsNoise::estimate(image.channel(channel), excluded))
}

/// Inverse-variance weights for collapsing RGB into the detection plane, summing to 1.
///
/// This is the optimal *linear* combiner for an unknown (flat) source SED, the linear analogue of
/// the SExtractor χ² detection image. It is kept linear rather than a χ² sum of squares because
/// flux, centroid, FWHM and SNR are measured on this plane downstream, and squaring would distort
/// the PSF and break flux linearity. Unlike Rec.709 luminance, it never zeroes a band, so red- and
/// blue-dominant stars stay detectable.
///
/// Each weight is `(σ_min/σ)²` before the sum, which needs no `1/σ²` that a tiny σ could overflow.
/// A channel with no measured noise is better than any with noise, so the channels at σ = 0 share
/// the whole weight: the limit of `1/σ²`. Only synthetic data has one.
fn inverse_variance_weights(sigmas: [f32; 3]) -> [f32; 3] {
    let quiet = sigmas.iter().filter(|&&sigma| sigma == 0.0).count();
    if quiet > 0 {
        return sigmas.map(|sigma| {
            if sigma == 0.0 {
                1.0 / quiet as f32
            } else {
                0.0
            }
        });
    }
    let least = sigmas.into_iter().fold(f32::INFINITY, f32::min);
    let relative = sigmas.map(|sigma| (least / sigma).powi(2));
    let sum: f32 = relative.iter().sum();
    relative.map(|weight| weight / sum)
}

/// Mark the saturated pixels of `image`: the decoder's [`QualityFlags::SATURATED`] when it flagged
/// saturation, which calibration records before it moves the samples; otherwise every pixel where
/// any input channel reaches its metadata's saturation level. Per channel, before the channels are
/// combined: a star clipped in green alone, (0.6, 1.0, 0.6), combines to 0.8.
fn mark_saturated(image: &LinearImage, mask: &mut BitBuffer2) {
    if image.metadata.saturation_flagged {
        match &image.flags {
            Some(flags) => {
                mask.fill_from_predicate(|index| {
                    flags.at(index).intersects(QualityFlags::SATURATED)
                });
            }
            None => mask.fill(false),
        }
        return;
    }
    let level = image.metadata.saturation_level();
    let channels: ArrayVec<&[f32], 3> = (0..image.channels())
        .map(|channel| image.channel(channel).pixels())
        .collect();
    mask.fill_from_predicate(|index| channels.iter().any(|channel| channel[index] >= level));
}

#[cfg(test)]
mod tests;
