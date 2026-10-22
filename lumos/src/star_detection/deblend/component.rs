//! [`Component`]: one connected component of the residual, as the deblenders read it.

use imaginarium::Buffer2;

use crate::math::size2us::Size2us;
use crate::math::urect::URect;
use crate::star_detection::deblend::assignment::Assignment;
use crate::star_detection::deblend::pixel::Pixel;
use crate::star_detection::deblend::region::Region;
use crate::star_detection::labeling::component_data::ComponentData;
use crate::star_detection::labeling::{LabelMap, LabelRun};

/// One connected component of the residual: the pixels of its runs, and the brightest of them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Component<'a> {
    data: &'a ComponentData,
    residual: &'a Buffer2<f32>,
    runs: &'a [LabelRun],
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
            Size2us::from(residual),
            labels.size(),
            "residual and labels must have same dimensions"
        );
        let runs = labels.runs_of(data.label);
        let peak =
            Pixel::brightest(scan(residual, runs)).expect("a component holds at least one pixel");
        Self {
            data,
            residual,
            runs,
            peak,
        }
    }

    pub(crate) const fn peak(&self) -> Pixel {
        self.peak
    }

    pub(crate) const fn residual(&self) -> &'a Buffer2<f32> {
        self.residual
    }

    pub(crate) const fn bbox(&self) -> URect {
        self.data.bbox
    }

    /// Every pixel of the component, in raster order.
    pub(crate) fn pixels(&self) -> impl Iterator<Item = Pixel> + 'a {
        scan(self.residual, self.runs)
    }

    /// The component undivided: one region with its brightest pixel as the peak.
    pub(crate) const fn whole(&self) -> Region {
        Region {
            bbox: self.data.bbox,
            peak: self.peak.pos(),
            area: self.data.area,
        }
    }

    /// The regions `peaks` split the component into, pushed onto `out`: the whole component for
    /// fewer than two peaks, else every pixel assigned to its nearest peak (squared-Euclidean
    /// Voronoi; the first peak wins ties) and one [`Region`] per peak that captured any — the
    /// shared tail of both deblenders. Returns how many it pushed.
    pub(super) fn split_at(
        &self,
        peaks: &[Pixel],
        scratch: &mut Assignment,
        out: &mut Vec<Region>,
    ) -> usize {
        if peaks.len() <= 1 {
            out.push(self.whole());
            return 1;
        }
        scratch.reset(peaks, self.data.bbox);
        for pixel in self.pixels() {
            let nearest = scratch.nearest(pixel.pos(), peaks);
            scratch.bboxes[nearest].include(pixel.pos());
            scratch.areas[nearest] += 1;
        }
        let Assignment { bboxes, areas, .. } = scratch;

        let before = out.len();
        for ((peak, &bbox), &area) in peaks.iter().zip(bboxes.iter()).zip(areas.iter()) {
            if area > 0 {
                debug_assert!(
                    bbox.contains(peak.pos()),
                    "assigned region must contain its peak"
                );
                out.push(Region {
                    bbox,
                    peak: peak.pos(),
                    area,
                });
            }
        }
        out.len() - before
    }
}

/// The pixels of `runs` in `residual`, in raster order: in proportion to the component's pixels,
/// not to its box.
fn scan<'a>(residual: &'a Buffer2<f32>, runs: &'a [LabelRun]) -> impl Iterator<Item = Pixel> + 'a {
    let width = residual.width();
    runs.iter().flat_map(move |run| {
        let row = run.y as usize * width;
        (run.start..run.end).map(move |x| Pixel::at(x, run.y, residual[row + x as usize]))
    })
}
