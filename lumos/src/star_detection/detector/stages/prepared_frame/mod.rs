//! [`PreparedFrame`]: a frame reduced to the planes detection and measurement read.

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::io::image::linear::LinearImage;
use crate::io::image::pixel_flags::{QualityFlags, SATURATION_FRACTION};
use crate::math::noise::mrs_noise::MrsNoise;
use crate::math::size2us::Size2us;
use crate::star_detection::background::background_estimate::{BackgroundEstimate, Refinement};
use crate::star_detection::background::sky_noise::SkyNoise;
use crate::star_detection::config::Config;
use crate::star_detection::config::background_config::BackgroundRefinement;
use crate::star_detection::detection_plane::{DetectionPlane, PlaneFilters};
use crate::star_detection::resources::DetectionResources;

/// A frame reduced to one plane and its sky: what detection and measurement read of it.
///
/// Measurement reads `measure`, which no filter touched: a 3×3 median keeps 56% of the flux and 42%
/// of the peak of a Gaussian star of FWHM 2. Detection reads a [`DetectionPlane`] built from it.
#[derive(Debug)]
pub(crate) struct PreparedFrame {
    /// The channels' noise-weighted combination, less the sky.
    pub(crate) measure: Buffer2<f32>,
    /// The sky's σ on `measure`.
    pub(crate) sky: SkyNoise,
    pub(crate) saturation: BitBuffer2,
    /// The pixels with no measurement: left out of the sky, of every noise and of every threshold.
    pub(crate) no_data: Option<BitBuffer2>,
    /// The sources the refined sky was measured around, and the pixels with no data; `None`
    /// without a refinement.
    sources: Option<BitBuffer2>,
    /// Whether the frame was demosaiced, so its detection plane takes the 3×3 median first.
    demosaiced: bool,
}

impl PreparedFrame {
    /// Combine the channels, measure the sky, refining it around the sources the configuration
    /// asks for, and take it out.
    pub(crate) fn new(
        image: &LinearImage,
        config: &Config,
        resources: &mut DetectionResources,
    ) -> Self {
        let mut plane = resources.acquire_f32();
        combine_channels(image, &mut plane);
        let demosaiced = image.metadata.is_demosaiced();
        let no_data = image
            .flags
            .as_ref()
            .filter(|flags| flags.contains(QualityFlags::NO_DATA))
            .map(|flags| {
                let mut mask = resources.acquire_bit();
                mask.fill_from_predicate(|index| flags.at(index).intersects(QualityFlags::NO_DATA));
                mask
            });

        let mut background =
            BackgroundEstimate::estimate(&plane, no_data.as_ref(), &config.background, resources);
        let mut sources = None;
        if let BackgroundRefinement::Iterative {
            iterations,
            mask_dilation,
            mask_sigma,
        } = config.background.refinement
        {
            // The sources are found at the FWHM the configuration starts from.
            let refined = background.refine(
                &plane,
                Refinement {
                    iterations,
                    mask_dilation,
                    mask_sigma,
                },
                PlaneFilters {
                    median: demosaiced,
                    matched: config
                        .fwhm
                        .mode
                        .map(|mode| config.fwhm.filter_at(mode.seed())),
                    mask: no_data.as_ref(),
                },
                &config.background,
                resources,
            );
            background = refined.estimate;
            sources = Some(refined.sources);
        }

        // Saturation is a property of the recorded values, read before anything combines them.
        let mut saturation = resources.acquire_bit();
        mark_saturated(image, &mut saturation);
        let sky = background.subtract_from(&mut plane, resources);
        Self {
            measure: plane,
            sky,
            saturation,
            no_data,
            sources,
            demosaiced,
        }
    }

    /// The detection plane of this frame, matched-filtered at `fwhm` when one is given.
    pub(crate) fn detection_plane(
        &self,
        fwhm: Option<f32>,
        config: &Config,
        resources: &mut DetectionResources,
    ) -> DetectionPlane {
        let mut residual = resources.acquire_f32();
        residual.pixels_mut().copy_from_slice(self.measure.pixels());
        DetectionPlane::from_residual(
            residual,
            PlaneFilters {
                median: self.demosaiced,
                matched: fwhm.map(|fwhm| config.fwhm.filter_at(fwhm)),
                mask: self.sources.as_ref().or(self.no_data.as_ref()),
            },
            &config.background,
            resources,
        )
    }

    /// Return the source mask once no detection plane is built any more: only their noise reads it.
    pub(crate) fn release_sources(&mut self, resources: &mut DetectionResources) {
        if let Some(sources) = self.sources.take() {
            resources.release_bit(sources);
        }
    }

    pub(crate) fn release_to_pool(self, resources: &mut DetectionResources) {
        resources.release_f32(self.measure);
        self.sky.release_to_pool(resources);
        resources.release_bit(self.saturation);
        for mask in [self.no_data, self.sources].into_iter().flatten() {
            resources.release_bit(mask);
        }
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
    let size = Size2us::new(image.width(), image.height());
    [0, 1, 2].map(|channel| MrsNoise::estimate(image.channel(channel).pixels(), size, excluded))
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

/// The level a sample of `image` saturates at when its decoder flagged nothing:
/// [`SATURATION_FRACTION`] of its declared ceiling, `DATAMAX` in the normalized domain, or of that
/// domain's 1 when it declares none.
fn saturation_level(image: &LinearImage) -> f32 {
    SATURATION_FRACTION * image.metadata.data_max.map_or(1.0, |max| max as f32)
}

/// Mark the saturated pixels of `image`: the decoder's [`QualityFlags::SATURATED`] when it flagged
/// saturation, which a dark subtraction and a flat division leave exact; otherwise every pixel where
/// any input channel reaches [`saturation_level`]. Per channel, before the channels are combined:
/// a star clipped in green alone, (0.6, 1.0, 0.6), combines to 0.8.
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
    let level = saturation_level(image);
    let channels: ArrayVec<&[f32], 3> = (0..image.channels())
        .map(|channel| image.channel(channel).pixels())
        .collect();
    mask.fill_from_predicate(|index| channels.iter().any(|channel| channel[index] >= level));
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::io::image::linear::LinearImage;
    use crate::star_detection::detector::stages::prepared_frame::saturation_level;

    /// The level the detector marks `image`'s pixels saturated at.
    pub(crate) fn saturation_level_of(image: &LinearImage) -> f32 {
        saturation_level(image)
    }
}

#[cfg(test)]
mod tests;
