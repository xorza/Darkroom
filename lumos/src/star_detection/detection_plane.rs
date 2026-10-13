//! [`DetectionPlane`]: the plane detection thresholds and deblends, with the noise measured on it.

use std::mem;

use imaginarium::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::image::masked_plane::MaskedPlane;
use crate::scratch_pool::Pooled;
use crate::star_detection::background::sky_noise::SkyNoise;
use crate::star_detection::median_filter;
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star_detection_config::background_config::BackgroundConfig;
use crate::star_detection::star_detection_config::fwhm_config::MatchedFilter;
use crate::star_detection::threshold_mask::threshold_params::ThresholdParams;

/// The plane the threshold and both deblenders read, with the noise measured on it: the residual
/// of the measurement plane, median-filtered when the frame was demosaiced, then matched-filtered
/// when a FWHM is known.
///
/// Both filters correlate neighbouring pixels, so the σ of their output is not the input's σ
/// scaled as for white noise: the matched filter on a median-filtered demosaiced plane leaves 2.2
/// to 2.6 times the σ that scaling predicts, at filter FWHM 2.5 to 6. The mesh measures the σ the
/// filtered plane has, which holds under any correlation, so a threshold of kσ passes the fraction
/// of sky it states. SEP thresholds the filtered values, and photutils deblends the convolved data.
#[derive(Debug)]
pub(crate) struct DetectionPlane {
    pub(crate) values: Pooled<Buffer2<f32>>,
    pub(crate) noise: SkyNoise,
}

/// What a detection plane's residual passes through before the matched filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Prefilter {
    None,
    /// The 3×3 median, for a demosaiced frame: interpolation leaves artifacts a single pixel wide.
    Median3x3,
}

/// How a detection plane is filtered, and how its noise is measured.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlaneSpec<'a> {
    pub(crate) prefilter: Prefilter,
    pub(crate) matched: Option<MatchedFilter>,
    /// The pixels left out of the noise: the ones with no data, and the sources once they are
    /// known.
    pub(crate) mask: Option<&'a BitBuffer2>,
    /// The mesh the noise is measured by.
    pub(crate) background: &'a BackgroundConfig,
}

impl DetectionPlane {
    /// Filter `residual`, a plane from `resources` that the plane takes over, and measure its
    /// noise.
    pub(crate) fn from_residual(
        mut residual: Pooled<Buffer2<f32>>,
        spec: PlaneSpec<'_>,
        resources: &mut DetectionResources,
    ) -> Self {
        match spec.prefilter {
            Prefilter::None => {}
            Prefilter::Median3x3 => {
                let mut filtered = resources.acquire_f32();
                median_filter::apply_3x3(&residual, &mut filtered);
                mem::swap(&mut residual, &mut filtered);
            }
        }
        if let Some(filter) = spec.matched {
            let mut temp = resources.acquire_f32();
            resources
                .kernels
                .matched_filter(&mut residual, filter, &mut temp);
        }
        let noise = SkyNoise::measure(
            MaskedPlane {
                pixels: &residual,
                mask: spec.mask,
            },
            spec.background,
            resources,
        );
        Self {
            values: residual,
            noise,
        }
    }

    /// Fill `mask` with the pixels above `sigma` of the plane's own noise, held to its floor.
    pub(crate) fn threshold(&self, sigma: f32, mask: &mut BitBuffer2) {
        ThresholdParams {
            sigma,
            min_noise: self.noise.floor,
        }
        .fill_mask(&self.values, &self.noise.noise, mask);
    }
}
