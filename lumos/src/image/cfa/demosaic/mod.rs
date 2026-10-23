//! [`Demosaic`]: the demosaic lumos runs on a sensor's mosaic, and what it reads and holds.
//!
//! RCD on a Bayer mosaic (RGGB, BGGR, GRBG, GBRG), Markesteijn on a Fujifilm X-Trans one.

pub(crate) mod bayer;
pub(crate) mod mosaic_view;
pub(crate) mod tiled;
pub(crate) mod xtrans;

use common::CancelToken;
use imaginarium::Buffer2;

use crate::error::Cancelled;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::bayer::rcd;
use crate::image::cfa::demosaic::mosaic_view::MosaicView;
use crate::image::cfa::demosaic::xtrans::markesteijn;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::cfa::xtrans_pattern::XTransPattern;
use crate::image::channel_layout::ChannelLayout;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_provenance::DemosaicProvenance;
use crate::image::linear_pixels::LinearPixels;
use crate::memory::footprint::Footprint;

/// The demosaic of a sensor's frame: none on a mono sensor, whose samples pass through, RCD on a
/// Bayer mosaic, and Markesteijn with its passes on an X-Trans one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Demosaic {
    None,
    Rcd(CfaPattern),
    Markesteijn {
        pattern: XTransPattern,
        passes: MarkesteijnPasses,
    },
}

impl Demosaic {
    /// The demosaic of a `cfa_type` frame, an X-Trans one in `passes`.
    pub(crate) const fn of(cfa_type: CfaType, passes: MarkesteijnPasses) -> Self {
        match cfa_type {
            CfaType::Mono => Self::None,
            CfaType::Mosaic(Mosaic::Bayer(pattern)) => Self::Rcd(pattern),
            CfaType::Mosaic(Mosaic::XTrans(pattern)) => Self::Markesteijn { pattern, passes },
        }
    }

    /// `data`, the mosaic, as the planes of its demosaic, its colors balanced by `gains` relative
    /// to green as the kernels read them and the balance taken back out as they write.
    pub(crate) fn run(
        self,
        data: Buffer2<f32>,
        gains: [f32; 3],
        cancel: &CancelToken,
    ) -> Result<LinearPixels, Cancelled> {
        let dimensions = ImageDimensions::new(&data, ChannelLayout::Rgb);
        let planes = match self {
            Self::None => return Ok(LinearPixels::from(data)),
            Self::Rcd(pattern) => {
                rcd::demosaic(&MosaicView::new(&data, pattern).with_gains(gains), cancel)?
            }
            Self::Markesteijn { pattern, passes } => markesteijn::demosaic(
                &MosaicView::new(&data, pattern).with_gains(gains),
                passes,
                cancel,
            )?,
        };
        Ok(LinearPixels::from_planar_channels(dimensions, planes))
    }

    /// How far the demosaic reads from an output pixel: an input photosite this far away can
    /// change it. Both demosaics decide directions from neighbourhoods of interpolated values, so
    /// the reach is the chained steps', not one kernel's, and each further Markesteijn pass
    /// interpolates again from interpolated colors. An impulse on random texture moves pixels up
    /// to 10 away by more than 2⁻¹⁶ of itself for RCD, and changes pixels up to 11 and 16 away for
    /// one and three Markesteijn passes (`the_demosaic_support_bounds_every_impulse_response`).
    pub(crate) const fn support(self) -> usize {
        match self {
            Self::None => 0,
            Self::Rcd(_) => 10,
            Self::Markesteijn {
                passes: MarkesteijnPasses::One,
                ..
            } => 11,
            Self::Markesteijn {
                passes: MarkesteijnPasses::Three,
                ..
            } => 16,
        }
    }

    /// What the demosaic of a `dimensions` frame holds on `workers`.
    pub(crate) const fn footprint(self, dimensions: ImageDimensions, workers: usize) -> Footprint {
        match self {
            Self::None => {
                Footprint::kept(dimensions.pixel_count().saturating_mul(size_of::<f32>()))
            }
            Self::Rcd(_) => rcd::footprint(dimensions.size(), workers),
            Self::Markesteijn { .. } => markesteijn::footprint(dimensions.size(), workers),
        }
    }

    /// The record of the demosaic a frame went through.
    pub(crate) const fn provenance(self) -> DemosaicProvenance {
        match self {
            Self::None => DemosaicProvenance::None,
            Self::Rcd(_) => DemosaicProvenance::LumosRcd,
            Self::Markesteijn { passes, .. } => DemosaicProvenance::LumosMarkesteijn { passes },
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod memory_tests {
    use crate::image::cfa::cfa_pattern::CfaPattern;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::image::cfa::demosaic::Demosaic;
    use crate::image::cfa::demosaic::bayer::rcd;
    use crate::image::cfa::demosaic::xtrans::markesteijn;
    use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
    use crate::image::cfa::mosaic::Mosaic;
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::internals::cfa::XTRANS_PATTERN;
    use crate::memory::footprint::Footprint;

    /// Mono holds its one plane. Both demosaics hold the input, the three output planes and the
    /// tile buffers of each of the run's workers, whatever the frame's size: four workers' here.
    #[test]
    fn demosaic_footprint_counts_each_plane_the_kernel_holds() {
        let even = ImageDimensions::new((10, 8), ChannelLayout::Mono);
        let odd = ImageDimensions::new((5, 3), ChannelLayout::Mono);

        let mono = Demosaic::of(CfaType::Mono, MarkesteijnPasses::One).footprint(even, 4);
        assert_eq!(mono, Footprint::kept(80 * 4));

        for (cfa_type, workspace) in [
            (
                CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
                rcd::internals::workspace_bytes(4),
            ),
            (
                CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN)),
                markesteijn::internals::workspace_bytes(4),
            ),
        ] {
            for dimensions in [even, odd] {
                let plane = dimensions.pixel_count() * 4;
                assert_eq!(
                    Demosaic::of(cfa_type, MarkesteijnPasses::One).footprint(dimensions, 4),
                    Footprint {
                        kept: 3 * plane,
                        peak: 4 * plane + workspace,
                    },
                    "{cfa_type:?}"
                );
            }
        }
    }
}
