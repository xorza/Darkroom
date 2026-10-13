//! The detection benches: the entry `crate::bench` runs them by, and the inputs they share, a
//! field taken through the detector's first stages.

use criterion::Criterion;

use crate::bench::scene::{FRAME, Field};
use crate::bit_buffer2::BitBuffer2;
use crate::star_detection::detection_plane::DetectionPlane;
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star_detection_config::StarDetectionConfig;
use crate::star_detection::star_detector::stages::fwhm;
use crate::star_detection::star_detector::stages::prepared_frame::PreparedFrame;
use crate::star_detection::star_detector::stages::{detect, filter};
use crate::star_detection::{
    background, centroid, convolution, labeling, median_filter, threshold_mask,
};

pub(crate) fn atomic(c: &mut Criterion) {
    background::bench::atomic(c);
    median_filter::simd::bench::atomic(c);
    threshold_mask::simd::bench::atomic(c);
    convolution::simd::bench::atomic(c);
    convolution::bench::atomic(c);
    labeling::bench::atomic(c);
    detect::bench::atomic(c);
    centroid::bench::atomic(c);
    filter::bench::atomic(c);
}

/// A field at the standard frame taken through the detector's first stages as
/// `StarDetector::detect` runs them, under the configuration a user picks for it.
#[derive(Debug)]
pub(crate) struct Stages {
    pub(crate) config: StarDetectionConfig,
    /// The frame less its sky, which the stars are measured on.
    pub(super) frame: PreparedFrame,
    /// The FWHM the matched filter took, or `None` where the configuration filters with none.
    pub(crate) fwhm: Option<f32>,
    pub(crate) plane: DetectionPlane,
    /// The plane's pixels above the detection threshold.
    pub(crate) mask: BitBuffer2,
}

impl Stages {
    pub(crate) fn of(field: Field) -> Self {
        let config = config(field);
        let image = field.render(FRAME).image;
        let mut resources = DetectionResources::new(FRAME);
        let frame = PreparedFrame::new(&image, &config, &mut resources);
        let fwhm = fwhm::estimate(&frame, &config, &mut resources).value();
        let plane = frame.detection_plane(fwhm, &config, &mut resources);
        let mut mask = BitBuffer2::new_default(FRAME);
        plane.threshold(config.extraction.sigma_threshold, &mut mask);
        Self {
            config,
            frame,
            fwhm,
            plane,
            mask,
        }
    }
}

/// The configuration a user picks for `field`: the default for a sparse or a typical field, the
/// crowded-field preset for a cluster.
pub(crate) fn config(field: Field) -> StarDetectionConfig {
    match field {
        Field::Sparse | Field::Typical => StarDetectionConfig::default(),
        Field::Dense => StarDetectionConfig::crowded_field(),
    }
}
