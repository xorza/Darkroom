//! [`Component`]: one connected component of the residual, as the deblenders read it.

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::math::urect::URect;
use crate::math::vec2us::Vec2us;
use crate::stacking::star_detection::deblend::region::Region;
use crate::stacking::star_detection::deblend::{MAX_PEAKS, Pixel, nearest_peak_index};
use crate::stacking::star_detection::labeling::LabelMap;
use crate::stacking::star_detection::labeling::component_data::ComponentData;

/// One connected component of the residual: the pixels its label covers, read through its
/// bounding box, and the brightest of them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Component<'a> {
    data: &'a ComponentData,
    residual: &'a Buffer2<f32>,
    labels: &'a LabelMap,
    /// The brightest pixel, the first in raster order among equals.
    peak: Pixel,
}

impl<'a> Component<'a> {
    pub(crate) fn new(
        data: &'a ComponentData,
        residual: &'a Buffer2<f32>,
        labels: &'a LabelMap,
    ) -> Self {
        debug_assert_eq!(
            (residual.width(), residual.height()),
            (labels.width(), labels.height()),
            "residual and labels must have same dimensions"
        );
        let peak = Pixel::brightest(Self::scan(data, residual, labels))
            .expect("a component holds at least one pixel");
        Self {
            data,
            residual,
            labels,
            peak,
        }
    }

    pub(crate) const fn peak(&self) -> Pixel {
        self.peak
    }

    pub(crate) const fn residual(&self) -> &'a Buffer2<f32> {
        self.residual
    }

    /// Every pixel of the component, in raster order.
    pub(crate) fn pixels(&self) -> impl Iterator<Item = Pixel> + 'a {
        Self::scan(self.data, self.residual, self.labels)
    }

    /// The component undivided: one region with its brightest pixel as the peak.
    pub(crate) const fn whole(&self) -> Region {
        Region {
            bbox: self.data.bbox,
            peak: self.peak.pos,
            peak_value: self.peak.value,
            area: self.data.area,
        }
    }

    /// Assign every pixel to its nearest of `peaks` (squared-Euclidean Voronoi; the first peak
    /// wins ties) and build one [`Region`] per peak, dropping peaks that captured no pixels — the
    /// shared tail of both deblenders.
    pub(crate) fn assign_to_nearest(&self, peaks: &[Pixel]) -> ArrayVec<Region, MAX_PEAKS> {
        debug_assert!(
            peaks.len() <= MAX_PEAKS,
            "a deblender keeps at most MAX_PEAKS peaks"
        );
        let mut bboxes = [URect::empty(); MAX_PEAKS];
        let mut areas = [0usize; MAX_PEAKS];
        for pixel in self.pixels() {
            let nearest = nearest_peak_index(pixel.pos, peaks);
            bboxes[nearest].include(pixel.pos);
            areas[nearest] += 1;
        }

        let mut result = ArrayVec::new();
        for ((peak, bbox), area) in peaks.iter().zip(bboxes).zip(areas) {
            if area > 0 {
                debug_assert!(
                    bbox.contains(peak.pos),
                    "assigned region must contain its peak"
                );
                result.push(Region {
                    bbox,
                    peak: peak.pos,
                    peak_value: peak.value,
                    area,
                });
            }
        }
        result
    }

    fn scan(
        data: &'a ComponentData,
        residual: &'a Buffer2<f32>,
        labels: &'a LabelMap,
    ) -> impl Iterator<Item = Pixel> + 'a {
        let width = residual.width();
        let bbox = data.bbox;
        let label = data.label;
        (bbox.min.y..bbox.max.y).flat_map(move |y| {
            (bbox.min.x..bbox.max.x).filter_map(move |x| {
                let idx = y * width + x;
                (labels[idx] == label).then(|| Pixel {
                    pos: Vec2us::new(x, y),
                    value: residual[idx],
                })
            })
        })
    }
}
