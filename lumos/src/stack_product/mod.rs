//! The product of a stack: the calibrated, registered, combined image plus the ancillary
//! per-pixel planes that let a downstream tool measure the result rather than only view it.

pub(crate) mod coverage;
pub(crate) mod quality_planes;
pub(crate) mod run_report;
pub(crate) mod weight_kind;

use rayon::prelude::*;

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::linear_image::LinearImage;
use crate::image::quality_map::QualityMap;
use crate::stack_product::coverage::Coverage;
use crate::stack_product::run_report::RunReport;
use crate::stack_product::weight_kind::WeightKind;

/// A stacked science product, of a statistical combine or of a drizzle.
///
/// Every product comes out of the one combine: a drizzle drizzles each frame onto the output grid
/// on its own, then combines those frames, each sample weighted by the drops that formed it and
/// its noise divided by their Kish size. Each plane is `Some` only when it was requested (see
/// [`QualityPlanes`](crate::QualityPlanes)) and the combine could produce it. The weight, inverse
/// variance and dispersion are per channel for a color product, because rejection can keep
/// different samples in each channel, and one shared plane for a monochrome one.
#[derive(Debug)]
pub struct StackProduct {
    /// The combined linear image.
    pub image: LinearImage,
    /// The share of frames that reached each pixel, in `[0, 1]`, for masking and fill gating.
    ///
    /// A frame counts where its sample cleared the coverage floor, and a drizzled frame where any
    /// of its drops landed. Neither says how much signal landed there — a pixel every frame reached
    /// through a sliver of a drop reads 1.0 — which is what `weight` answers, and what drizzle's
    /// `min_weight_fraction` gates on, on the drops' own weight.
    pub coverage: Option<Coverage>,
    /// WHT map: `Σwᵢ` over whatever formed the pixel.
    ///
    /// The combine sums, per channel, each surviving frame's weight. `Equal` weighting gives unit
    /// frame weights, so the sum is the survivor count. `Noise` weighting gives each frame its
    /// inverse noise variance, not normalized, so where no warp averaged and no flat amplified the
    /// samples the sum is the inverse variance of the mean. `Manual` weights are relative, and so
    /// is the sum. A warp's confidence enters the variance, not the weight — see
    /// [`WarpResult`](crate::WarpResult). A drizzled frame's weight is multiplied by its drops'
    /// summed weight at the pixel: how much of its input pixels' flux landed there.
    ///
    /// It is the denominator the image was divided by, under the same weights as
    /// [`Self::inverse_variance`].
    pub weight: Option<QualityMap>,
    /// What the frame weights [`Self::weight`] sums were: a count, an inverse variance or relative
    /// weights. Stated whether the plane was asked for or not, as it is a fact of the combine.
    pub weight_kind: WeightKind,
    /// The inverse variance of each pixel's value, in the image's units to the power −2:
    /// `(Σwᵢ)² / Σwᵢ²·vᵢ` over the samples that formed it. A pixel nothing formed reads 0, no
    /// information, which is what an inverse-variance consumer gives it; a variance plane would
    /// read 0 there too and claim the pixel exact. A pixel whose noise model is 0 reads `+∞`.
    ///
    /// `vᵢ` is the sample's CCD noise model: the frame's measured background noise, plus the photon
    /// noise of the signal above the sky when the frame states its gain
    /// ([`RunReport::variance_background_only`] says when one did not), both scaled by the gain the
    /// frame's flat applied at the sample — the read noise and dark current by its square — carried
    /// through normalization and divided by the warp's confidence, or a drizzled frame's drops'
    /// Kish size. The combine takes it at the combined value. Drizzle spreads one input pixel over
    /// several output pixels, so its noise correlates between neighbours (Fruchter & Hook 2002):
    /// this plane is each pixel's own variance, not their covariance.
    ///
    /// Absent for median output, which has no exact variance.
    pub inverse_variance: Option<QualityMap>,
    /// The variance of each pixel's value as its surviving samples' scatter shows it, in the
    /// image's units squared: `Σwᵢ(xᵢ − x̄)² / ((n − 1)·Σwᵢ)`, with no noise model.
    ///
    /// Unbiased where each sample's variance is inversely proportional to its weight — as `Noise`
    /// weighting makes it at the sky where no warp averaged and no flat amplified the samples — and
    /// with equal weights the squared standard error of the mean. It checks
    /// [`Self::inverse_variance`], whose reciprocal it estimates: where the two disagree beyond the
    /// scatter of a scatter, the noise model or the frames are off. A clip in a band of σ — sigma
    /// clip, winsorized, linear fit — leaves survivors that scatter as a Gaussian truncated to the
    /// band, so their sum of squares is divided by that variance, 0.911 at ±2.5σ; trim and GESD cut
    /// by rank and by test, and read below the frames' full scatter. NaN where fewer than two
    /// samples survive.
    ///
    /// Absent for median output, and unless asked for.
    pub dispersion: Option<QualityMap>,
    /// The mosaic pattern every frame shared, for a stack of undemosaiced sensor frames; `None`
    /// for any other stack.
    pub cfa_type: Option<CfaType>,
    /// What the combine decided on its own: the samples it left out for their flags, and the
    /// flagged ones it had to keep.
    pub report: RunReport,
}

/// What each quality plane of a [`StackProduct`] holds at one pixel.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PixelQuality {
    pub(crate) coverage: f32,
    pub(crate) weight: f32,
    pub(crate) inverse_variance: f32,
    pub(crate) dispersion: f32,
}

impl PixelQuality {
    /// A pixel nothing measured: no frame reached it, no weight formed it, no information, and no
    /// scatter, which is NaN because a dispersion of 0 claims samples that agree exactly.
    pub(crate) const UNMEASURED: Self = Self {
        coverage: 0.0,
        weight: 0.0,
        inverse_variance: 0.0,
        dispersion: f32::NAN,
    };
}

impl StackProduct {
    /// Hold every pixel of each channel that `empty(channel, index)` names at `fill_value`, a value
    /// no frame measured, and each quality plane there at [`PixelQuality::UNMEASURED`], so no plane
    /// claims a measurement there. A plane every channel shares holds none only where no channel
    /// does.
    pub(crate) fn fill_where(
        &mut self,
        empty: impl Fn(usize, usize) -> bool + Sync,
        fill_value: f32,
    ) {
        let channels = self.image.channels();
        let fill = |plane: &mut [f32], channel: Option<usize>, value: f32| {
            plane
                .par_iter_mut()
                .enumerate()
                .for_each(|(index, sample)| {
                    let filled = match channel {
                        Some(channel) => empty(channel, index),
                        None => (0..channels).all(|channel| empty(channel, index)),
                    };
                    if filled {
                        *sample = value;
                    }
                });
        };
        for channel in 0..channels {
            fill(
                self.image.channel_mut(channel).pixels_mut(),
                Some(channel),
                fill_value,
            );
        }
        let unmeasured = PixelQuality::UNMEASURED;
        let coverage = match &mut self.coverage {
            Some(Coverage::PerPixel(map)) => Some(map),
            Some(Coverage::Uniform { .. }) | None => None,
        };
        let maps = [
            (coverage, unmeasured.coverage),
            (self.weight.as_mut(), unmeasured.weight),
            (self.inverse_variance.as_mut(), unmeasured.inverse_variance),
            (self.dispersion.as_mut(), unmeasured.dispersion),
        ];
        for (map, value) in maps {
            match map {
                Some(QualityMap::Shared(plane)) => fill(plane.pixels_mut(), None, value),
                Some(QualityMap::PerChannel(planes)) => {
                    for (channel, plane) in planes.iter_mut().enumerate() {
                        fill(plane.pixels_mut(), Some(channel), value);
                    }
                }
                None => {}
            }
        }
    }

    /// Reinterpret a combined mosaic stack as the calibration master it is.
    ///
    /// # Panics
    ///
    /// If the product has more than one channel, or is not a stack of mosaic frames. A CFA frame
    /// is a single mosaic plane, so a stack of them is too — `CfaImage` has nowhere to put a
    /// second channel and no loader produces one.
    pub(crate) fn into_cfa_master(self) -> CfaImage {
        assert_eq!(
            self.image.channels(),
            1,
            "a CFA master must be single-channel; got {} channels",
            self.image.channels()
        );
        let cfa_type = self
            .cfa_type
            .expect("a CFA master is stacked from mosaic frames");
        let mut image = self.image;
        let flags = image.take_flags();
        let metadata = std::mem::take(&mut image.metadata);
        let mut master = CfaImage::from_plane(image.into_pixels().into_l(), cfa_type, metadata);
        master.set_flags(flags);
        master
    }
}

#[cfg(test)]
mod tests {
    use imaginarium::Buffer2;

    use crate::image::cfa::cfa_pattern::CfaPattern;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::image::linear_image::LinearImage;
    use crate::image::quality_map::QualityMap;
    use crate::math::size2us::Size2us;
    use crate::stack_product::StackProduct;
    use crate::stack_product::coverage::Coverage;
    use crate::stack_product::run_report::RunReport;
    use crate::stack_product::weight_kind::WeightKind;

    fn product(channels: usize, cfa_type: Option<CfaType>) -> StackProduct {
        let dimensions = ImageDimensions::new((2, 1), ChannelLayout::try_from(channels).unwrap());
        let mut image = LinearImage::from_interleaved(
            dimensions,
            (0..dimensions.sample_count()).map(|i| i as f32).collect(),
        );
        image.metadata.header.exposure_time = Some(30.0);
        image.metadata.derived.set_quantization_sigma(Some(0.25));
        StackProduct {
            image,
            coverage: None,
            weight: None,
            weight_kind: WeightKind::Count,
            inverse_variance: None,
            dispersion: None,
            cfa_type,
            report: RunReport::default(),
        }
    }

    /// A mono mosaic stack becomes the master it is: the plane, the pattern, the metadata and the
    /// quantization σ all carry over.
    #[test]
    fn a_mosaic_stack_becomes_its_master() {
        let pattern = CfaType::Bayer(CfaPattern::Rggb);
        let master = product(1, Some(pattern)).into_cfa_master();
        assert_eq!(master.data().pixels(), &[0.0, 1.0]);
        assert_eq!(master.cfa_type(), pattern);
        assert_eq!(master.metadata.header.exposure_time, Some(30.0));
        assert_eq!(master.metadata.derived.quantization_sigma(), Some(0.25));
        assert!(master.flags().is_none());
    }

    #[test]
    #[should_panic(expected = "a CFA master must be single-channel; got 3 channels")]
    fn a_color_stack_is_no_master() {
        product(3, Some(CfaType::Mono)).into_cfa_master();
    }

    #[test]
    #[should_panic(expected = "a CFA master is stacked from mosaic frames")]
    fn a_stack_of_demosaiced_frames_is_no_master() {
        product(1, None).into_cfa_master();
    }

    /// Filling names each plane's own empty value: of a 2×1 RGB product whose pixel 1 is empty in
    /// every channel and pixel 0 in channel 0 alone, the image reads the fill value at each empty
    /// sample, the per-channel planes 0 or NaN there, and the shared coverage only at pixel 1.
    #[test]
    fn filling_holds_each_plane_at_its_empty_value() {
        let measured =
            || QualityMap::PerChannel([0, 1, 2].map(|_| Buffer2::new(2, 1, vec![5.0; 2])));
        let mut filled = StackProduct {
            coverage: Some(Coverage::PerPixel(QualityMap::Shared(Buffer2::new(
                2,
                1,
                vec![1.0; 2],
            )))),
            weight: Some(measured()),
            inverse_variance: Some(measured()),
            dispersion: Some(measured()),
            ..product(3, None)
        };
        filled.fill_where(|channel, index| index == 1 || channel == 0, -1.0);

        assert_eq!(filled.image.channel(0).pixels(), &[-1.0, -1.0]);
        assert_eq!(filled.image.channel(1).pixels(), &[1.0, -1.0]);
        let plane = |map: &Option<QualityMap>, channel| {
            map.as_ref().unwrap().channel(channel).pixels().to_vec()
        };
        for channel in 0..3 {
            let kept = if channel == 0 { None } else { Some(5.0) };
            let expect = |empty: f32| [kept.unwrap_or(empty), empty];
            assert_eq!(plane(&filled.weight, channel), expect(0.0));
            assert_eq!(plane(&filled.inverse_variance, channel), expect(0.0));
            let dispersion = plane(&filled.dispersion, channel);
            assert_eq!(dispersion[0].is_nan(), channel == 0, "channel {channel}");
            assert!(dispersion[1].is_nan(), "channel {channel}");
        }
        let Some(Coverage::PerPixel(coverage)) = &filled.coverage else {
            unreachable!()
        };
        assert_eq!(coverage.channel(0).pixels(), &[1.0, 0.0]);
    }

    /// A uniform coverage becomes an image of its one value at its size.
    #[test]
    fn uniform_coverage_becomes_a_filled_plane() {
        let image = LinearImage::from(Coverage::Uniform {
            value: 0.5,
            size: Size2us::new(3, 2),
        });
        assert_eq!(
            image.dimensions(),
            ImageDimensions::new((3, 2), ChannelLayout::Mono)
        );
        assert_eq!(image.channel(0).pixels(), &[0.5; 6]);
    }
}
