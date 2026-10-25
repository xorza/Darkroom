//! [`PreparedFrame`]: a frame reduced to the planes detection and measurement read.

use arrayvec::ArrayVec;
use common::{Buffer2, par};
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::image::linear_image::LinearImage;
use crate::image::quality_flags::QualityFlags;
use crate::noise::mrs_noise::NoisePlane;
use crate::scratch_pool::Pooled;
use crate::star_detection::bg::sky_noise::SkyNoise;
use crate::star_detection::centroid::MeasurePlanes;
use crate::star_detection::centroid::measure_grid::MeasureGrid;
use crate::star_detection::deblend::region::Region;
use crate::star_detection::detection_filter::FilterSource;
use crate::star_detection::detection_plane::{DetectionPlane, PlaneSpec, Prefilter};
use crate::star_detection::refined_bg::RefinedBg;
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star::Star;
use crate::star_detection::star_detection_config::StarDetectionConfig;
use crate::star_detection::star_detection_config::fwhm_config::FwhmMode;
use crate::star_detection::star_detection_config::measurement_config::MeasurementConfig;
use crate::star_detection::star_detector::detection_result::FwhmSource;
use crate::star_detection::star_detector::stages::fwhm::BrightStars;

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
        let prefilter = if image.metadata.derived.is_demosaiced() {
            Prefilter::Median3x3
        } else {
            Prefilter::None
        };
        let no_data = image
            .flags()
            .filter(|flags| flags.contains(QualityFlags::NO_DATA))
            .map(|flags| {
                let mut mask = resources.acquire_bit();
                mask.fill_from_predicate(|idx| flags.at(idx).intersects(QualityFlags::NO_DATA));
                mask
            });

        // The sources are found at the FWHM the configuration starts from.
        let refined = RefinedBg::measure(
            &plane,
            PlaneSpec {
                prefilter,
                matched: config
                    .fwhm
                    .mode
                    .map(|mode| config.fwhm.filter_at(mode.seed())),
                mask: no_data.as_deref(),
                bg: &config.bg,
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

    /// The FWHM the frame's matched filter runs at, and where it came from: none, the configured
    /// one, or an estimate from the frame's bright stars ([`BrightStars`]). A
    /// [`FwhmSource::Configured`] answer is one this frame did not measure — a fixed FWHM, or an
    /// estimate's fallback when too few stars passed.
    pub(crate) fn estimate_fwhm(
        &self,
        config: &StarDetectionConfig,
        resources: &mut DetectionResources,
    ) -> FwhmSource {
        match config.fwhm.mode {
            None => FwhmSource::Disabled,
            Some(FwhmMode::Fixed(fwhm)) => FwhmSource::Configured(fwhm),
            Some(FwhmMode::Auto { fallback }) => BrightStars {
                frame: self,
                config,
                fallback,
            }
            .fwhm(resources),
        }
    }

    /// A detection plane of this frame, matched-filtered at `fwhm` when one is given, its noise
    /// measured around the sources the refined sky found.
    pub(crate) fn detection_plane(
        &self,
        fwhm: Option<f32>,
        config: &StarDetectionConfig,
        resources: &mut DetectionResources,
    ) -> DetectionPlane {
        self.plane(
            PlaneSpec {
                prefilter: self.prefilter,
                matched: fwhm.map(|fwhm| config.fwhm.filter_at(fwhm)),
                mask: self.sources.as_deref().or(self.no_data.as_deref()),
                bg: &config.bg,
            },
            resources,
        )
    }

    /// The last detection plane of this frame, which takes the source mask with it: no plane after
    /// it reads one, so the mask goes back to the pool once this plane's noise is measured.
    pub(crate) fn final_plane(
        &mut self,
        fwhm: Option<f32>,
        config: &StarDetectionConfig,
        resources: &mut DetectionResources,
    ) -> DetectionPlane {
        let sources = self.sources.take();
        self.plane(
            PlaneSpec {
                prefilter: self.prefilter,
                matched: fwhm.map(|fwhm| config.fwhm.filter_at(fwhm)),
                mask: sources.as_deref().or(self.no_data.as_deref()),
                bg: &config.bg,
            },
            resources,
        )
    }

    /// The measurement plane under `spec`.
    fn plane(&self, spec: PlaneSpec<'_>, resources: &mut DetectionResources) -> DetectionPlane {
        DetectionPlane::filtered(FilterSource::Plane(&self.measure), spec, resources)
    }

    /// Measure the stars of `request.regions` on the measurement plane, in parallel, into `out`:
    /// sub-pixel positions, flux, FWHM and quality metrics, flagging those whose peak pixel is
    /// saturated. Each region's star, or none, goes to `measured` first, in the regions' order.
    pub(crate) fn measure_stars(
        &self,
        request: MeasureRequest<'_>,
        measured: &mut Vec<Option<Star>>,
        out: &mut Vec<Star>,
    ) {
        let MeasureRequest {
            regions,
            config,
            expected_fwhm,
        } = request;
        // One grid for the whole detection: `expected_fwhm` fixes the stamp, the window and the
        // annulus, so every candidate is measured over the same ones.
        let grid = MeasureGrid::new(expected_fwhm.unwrap_or(UNKNOWN_FWHM));
        let planes = MeasurePlanes {
            residual: &self.measure,
            sky: &self.sky,
            saturation: &self.saturation,
            no_data: self.no_data.as_deref(),
        };
        par::iter(regions)
            .map(|region| planes.measure_star(region, config, &grid))
            .collect_into_vec(measured);
        out.clear();
        out.extend(measured.drain(..).flatten());
    }
}

/// What a measurement of a frame's stars reads: the regions, the measurement settings, and the
/// FWHM every stamp is sized at — [`UNKNOWN_FWHM`] for `None`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MeasureRequest<'a> {
    pub(crate) regions: &'a [Region],
    pub(crate) config: &'a MeasurementConfig,
    pub(crate) expected_fwhm: Option<f32>,
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
    par::iter_mut(output.pixels_mut())
        .enumerate()
        .for_each(|(i, o)| {
            *o = weights[0] * r[i] + weights[1] * g[i] + weights[2] * b[i];
        });
}

/// Each channel's white noise, by the multiresolution estimator over the pixels no flag names: the
/// noise stacking weighs by, and not the MAD, which a red nebula inflates in red.
fn channel_noise(image: &LinearImage) -> [f32; 3] {
    let flags = image.flags();
    let excluded = |idx: usize| flags.is_some_and(|flags| flags.is_flagged(idx));
    [0, 1, 2].map(|channel| {
        NoisePlane {
            samples: image.channel(channel),
            excluded,
        }
        .sigma()
    })
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
    if image.metadata.derived.saturation_flagged() {
        match image.flags() {
            Some(flags) => {
                mask.fill_from_predicate(|idx| flags.at(idx).intersects(QualityFlags::SATURATED));
            }
            None => mask.fill(false),
        }
        return;
    }
    let level = image.metadata.header.saturation_level();
    let channels: ArrayVec<&[f32], 3> = (0..image.channels())
        .map(|channel| image.channel(channel).pixels())
        .collect();
    mask.fill_from_predicate(|idx| channels.iter().any(|channel| channel[idx] >= level));
}

#[cfg(test)]
mod tests;
