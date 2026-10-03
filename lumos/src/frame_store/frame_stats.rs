//! Per-frame robust statistics, measured before any interpolation touches the pixels.

use arrayvec::ArrayVec;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::frame_store::frame_facts::FrameFacts;
use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::cfa::CfaType;
use crate::io::image::pixel_flags::Flags;
use crate::math::noise::difference_noise::DifferenceNoise;
use crate::math::noise::mrs_noise::MrsNoise;
use crate::math::statistics::MedianMad;
use crate::math::vec2us::Vec2us;

/// Per-frame statistics: one median/MAD pair per channel, and the white noise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FrameStats {
    pub(crate) channels: ArrayVec<MedianMad, 3>,
    /// The white-noise standard deviation, without the signal MAD includes: per channel by
    /// [`MrsNoise`], or per colour of a mosaic by [`DifferenceNoise`]. Measured over the pixels no
    /// flag names.
    pub(crate) noise: ArrayVec<f32, 3>,
    pub(crate) quantization_sigma: Option<f32>,
    /// What the decoder said the samples are; what makes two frames' statistics comparable at all.
    pub(crate) facts: FrameFacts,
}

impl FrameStats {
    /// Measure per-channel median and MAD on `image`, before any interpolation touches it.
    ///
    /// Pixels the source declared no measurement for are left out. They matter most to the MAD: the
    /// decoder fills a null with the frame's own median, so every one of them is a zero-deviation
    /// sample, and a frame with a large masked region would report a spread far below its real
    /// noise — which is the figure weighting divides by.
    ///
    /// The other stages that measure a whole plane need no such exclusion, and the fill is why.
    /// Star detection looks for peaks above a local background and a patch sitting *at* the
    /// background produces none; the defect detectors look for outliers against a median the fill
    /// by construction is; and normalization already re-measures over the pixels every frame shares
    /// once any frame is partially covering.
    pub(crate) fn measure(image: &impl StackableImage) -> Self {
        let dimensions = image.dimensions();
        let quantization_sigma = image.metadata().quantization_sigma;
        let facts = FrameFacts::of(image);
        let flags = image.flags();
        let excluded =
            |index: usize| flags.is_some_and(|flags| flags.at(index) != Flags::default());
        let noise = match image.cfa_type() {
            Some(cfa_type @ (CfaType::Bayer(_) | CfaType::XTrans(_))) => {
                DifferenceNoise::estimate(image.channel(0), dimensions.size(), &cfa_type, excluded)
            }
            _ => (0..dimensions.channels())
                .into_par_iter()
                .map(|channel| {
                    MrsNoise::estimate(image.channel(channel), dimensions.size(), excluded)
                })
                .collect::<Vec<_>>()
                .into_iter()
                .collect(),
        };
        let nulls = flags.filter(|flags| flags.contains(Flags::NO_DATA));
        let channels = (0..dimensions.channels())
            .into_par_iter()
            .map(|channel| {
                // One copy per channel, the measured samples, which the median and the MAD then
                // sort in place.
                let plane = image.channel(channel);
                let mut measured: Vec<f32> = match nulls {
                    Some(nulls) => plane
                        .chunks(dimensions.width())
                        .enumerate()
                        .flat_map(|(y, row)| {
                            row.iter()
                                .enumerate()
                                .filter(move |&(x, _)| {
                                    !nulls.at_pos(Vec2us::new(x, y)).intersects(Flags::NO_DATA)
                                })
                                .map(|(_, &sample)| sample)
                        })
                        .collect(),
                    None => plane.to_vec(),
                };
                // A frame with nothing measured anywhere has no statistics to report. It also
                // contributes at no pixel, so what goes here is never read — but it has to be
                // something, and the median of nothing would panic.
                if measured.is_empty() {
                    return MedianMad {
                        median: 0.0,
                        mad: 0.0,
                    };
                }
                MedianMad::of_mut(&mut measured)
            })
            .collect::<Vec<_>>()
            .into_iter()
            .collect();
        Self {
            channels,
            noise,
            quantization_sigma,
            facts,
        }
    }
}
