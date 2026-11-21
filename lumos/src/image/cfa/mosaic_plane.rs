//! [`MosaicPlane`]: a sensor's single plane of samples under its color filter pattern.

use arrayvec::ArrayVec;
use common::Buffer2;
use imaginarium::{ChannelLayout, PlanarDesc};

use crate::image::cfa::cfa_type::CfaType;
use crate::image::frame::sealed;

/// A sensor's samples, row-major, under the pattern of its color filter; calibration may put
/// values outside `[0, 1]`.
#[derive(Debug, Clone)]
pub struct MosaicPlane {
    pub(crate) data: Buffer2<f32>,
    pub(crate) cfa_type: CfaType,
}

impl sealed::Planes for MosaicPlane {
    type Pattern = CfaType;

    fn desc(&self) -> PlanarDesc {
        PlanarDesc::new(self.data.size(), ChannelLayout::Mono)
    }

    fn channel(&self, channel: usize) -> &Buffer2<f32> {
        assert!(channel == 0, "a mosaic has only 1 channel, got {channel}");
        &self.data
    }

    fn pattern(&self) -> CfaType {
        self.cfa_type
    }

    fn cfa_type(&self) -> Option<CfaType> {
        Some(self.cfa_type)
    }

    fn pattern_of(cfa_type: Option<CfaType>) -> Option<CfaType> {
        cfa_type
    }

    fn gain_channels(&self) -> usize {
        self.cfa_type.color_count()
    }

    fn into_planes(self) -> ArrayVec<Buffer2<f32>, 3> {
        ArrayVec::from_iter([self.data])
    }

    fn from_planes(planes: ArrayVec<Buffer2<f32>, 3>, cfa_type: CfaType) -> Self {
        assert_eq!(planes.len(), 1, "a mosaic is one plane");
        let data = planes.into_iter().next().expect("one plane");
        Self { data, cfa_type }
    }
}
