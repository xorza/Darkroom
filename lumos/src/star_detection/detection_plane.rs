//! [`DetectionPlane`]: the plane detection thresholds and deblends, with the noise measured on it.

use std::mem;

use imaginarium::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::star_detection::background::background_estimate::BackgroundEstimate;
use crate::star_detection::background::sky_noise::SkyNoise;
use crate::star_detection::config::background_config::BackgroundConfig;
use crate::star_detection::config::fwhm_config::MatchedFilter;
use crate::star_detection::convolution::matched_filter;
use crate::star_detection::median_filter::median_filter_3x3;
use crate::star_detection::resources::DetectionResources;

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
    pub(crate) values: Buffer2<f32>,
    pub(crate) noise: SkyNoise,
}

/// How a detection plane is filtered, and what its noise is measured around.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlaneFilters<'a> {
    /// The 3×3 median, for a demosaiced frame: interpolation leaves artifacts a single pixel wide.
    pub(crate) median: bool,
    pub(crate) matched: Option<MatchedFilter>,
    /// The pixels left out of the noise: the ones with no data, and the sources once they are
    /// known.
    pub(crate) mask: Option<&'a BitBuffer2>,
}

impl DetectionPlane {
    /// Filter `residual`, a plane from `resources` that the plane takes over, and measure its noise.
    pub(crate) fn from_residual(
        mut residual: Buffer2<f32>,
        filters: PlaneFilters<'_>,
        background: &BackgroundConfig,
        resources: &mut DetectionResources,
    ) -> Self {
        if filters.median {
            let mut filtered = resources.acquire_f32();
            median_filter_3x3(&residual, &mut filtered);
            mem::swap(&mut residual, &mut filtered);
            resources.release_f32(filtered);
        }
        if let Some(filter) = filters.matched {
            let mut temp = resources.acquire_f32();
            matched_filter(&mut residual, filter, &mut temp);
            resources.release_f32(temp);
        }
        let noise = BackgroundEstimate::noise_of(&residual, filters.mask, background, resources);
        Self {
            values: residual,
            noise,
        }
    }

    pub(crate) fn release_to_pool(self, resources: &mut DetectionResources) {
        resources.release_f32(self.values);
        self.noise.release_to_pool(resources);
    }
}
