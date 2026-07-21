//! Statically typed planar pixels: one `Buffer2<T>` per channel.

use crate::common::buffer2::Buffer2;

/// `N` channel planes of equal dimensions and element type `T`.
#[derive(Debug, Clone)]
pub struct PlanarPixels<const N: usize, T> {
    pub planes: [Buffer2<T>; N],
}

impl<const N: usize, T> PlanarPixels<N, T> {
    pub fn from_planes(planes: [Buffer2<T>; N]) -> Self {
        const {
            assert!(
                N == 1 || N == 3 || N == 4,
                "PlanarPixels supports 1, 3, or 4 channels"
            )
        };
        let width = planes[0].width();
        let height = planes[0].height();
        for plane in &planes[1..] {
            assert_eq!(plane.width(), width, "all planes must share width");
            assert_eq!(plane.height(), height, "all planes must share height");
        }
        Self { planes }
    }
}
