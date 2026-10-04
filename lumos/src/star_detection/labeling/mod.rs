//! Connected component labeling using union-find.
//!
//! Optimized for sparse binary masks (typical in star detection):
//! - Run-length encoding (RLE) based labeling for efficient processing
//! - Word-level bit scanning to skip background regions
//! - Strip-parallel labeling with boundary merging, down to the single strip a small image needs
//! - Lock-free union-find with atomic operations
//! - Minimal allocations via buffer reuse
//!
//! The result is each component's runs, not a plane of labels: every reader walks a component's
//! own pixels, so nothing pays for the frame's area.

pub(crate) mod component_data;
pub(crate) mod labeler;
mod run;
mod union_find;

use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::star_detection::config::detection_config::Connectivity;
use crate::star_detection::labeling::component_data::ComponentData;
use crate::star_detection::resources::DetectionResources;

/// The connected components of a mask: each one's box and area, and its pixels as runs.
#[derive(Debug, Default)]
pub(crate) struct LabelMap {
    size: Size2us,
    /// Component `i` carries label `i + 1`.
    components: Vec<ComponentData>,
    /// Every component's runs, component by component, each component's in raster order.
    runs: Vec<LabelRun>,
    /// Where component `i`'s runs start in `runs`, with the end after the last.
    run_starts: Vec<u32>,
}

/// One run of a component's pixels: row `y`, columns `start..end`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LabelRun {
    pub(crate) y: u32,
    pub(crate) start: u32,
    pub(crate) end: u32,
}

impl LabelMap {
    /// Label the foreground of `mask` into the buffers of `resources`' labeler.
    pub(crate) fn from_pool(
        mask: &BitBuffer2,
        connectivity: Connectivity,
        resources: &mut DetectionResources,
    ) -> Self {
        debug_assert_eq!(mask.size, resources.dimensions);
        resources.labeler.label(mask, connectivity)
    }

    /// Release this `LabelMap`'s buffers back to the labeler for the next frame to refill.
    pub(crate) fn release_to_pool(self, pool: &mut DetectionResources) {
        pool.labeler.recycle(self);
    }

    /// Number of connected components (excluding background).
    #[inline]
    pub(crate) const fn num_labels(&self) -> usize {
        self.components.len()
    }

    /// Every component, label `1` first.
    #[inline]
    pub(crate) fn components(&self) -> &[ComponentData] {
        &self.components
    }

    /// The runs of the component with `label`, in raster order.
    #[inline]
    pub(crate) fn runs_of(&self, label: u32) -> &[LabelRun] {
        let index = label as usize - 1;
        &self.runs[self.run_starts[index] as usize..self.run_starts[index + 1] as usize]
    }

    /// The size of the mask it labels.
    #[inline]
    pub(crate) const fn size(&self) -> Size2us {
        self.size
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::bit_buffer2::BitBuffer2;
    use crate::math::size2us::Size2us;
    use crate::math::vec2us::Vec2us;
    use crate::star_detection::config::detection_config::Connectivity;
    use crate::star_detection::labeling::component_data::ComponentData;
    use crate::star_detection::labeling::labeler::Labeler;
    use crate::star_detection::labeling::{LabelMap, LabelRun};

    impl LabelMap {
        /// Adopt pre-computed labels `1..=num_labels`, bypassing connected-component analysis;
        /// the components and their runs are collected by a scan of every pixel.
        pub(crate) fn from_raw(labels: &Buffer2<u32>, num_labels: usize) -> Self {
            let size = Size2us::new(labels.width(), labels.height());
            let mut components = vec![ComponentData::default(); num_labels];
            let mut runs_by_label: Vec<Vec<LabelRun>> = vec![Vec::new(); num_labels];
            for y in 0..size.height {
                let mut x = 0;
                while x < size.width {
                    let label = labels[(x, y)];
                    let start = x;
                    while x < size.width && labels[(x, y)] == label {
                        x += 1;
                    }
                    if label == 0 {
                        continue;
                    }
                    let component = &mut components[label as usize - 1];
                    component.label = label;
                    component.bbox.include(Vec2us::new(start, y));
                    component.bbox.include(Vec2us::new(x - 1, y));
                    component.area += x - start;
                    runs_by_label[label as usize - 1].push(LabelRun {
                        y: y as u32,
                        start: start as u32,
                        end: x as u32,
                    });
                }
            }
            let mut run_starts = vec![0u32];
            let mut runs = Vec::new();
            for component_runs in runs_by_label {
                runs.extend(component_runs);
                run_starts.push(runs.len() as u32);
            }
            Self {
                size,
                components,
                runs,
                run_starts,
            }
        }

        /// The labels as a plane, row-major: each component's label on its runs, 0 elsewhere.
        pub(crate) fn labels(&self) -> Vec<u32> {
            let mut plane = vec![0u32; self.size.pixel_count()];
            for component in &self.components {
                for run in self.runs_of(component.label) {
                    let row = run.y as usize * self.size.width;
                    plane[row + run.start as usize..row + run.end as usize].fill(component.label);
                }
            }
            plane
        }

        /// Label `mask` with a fresh labeler instead of the pool's.
        pub(crate) fn from_mask(mask: &BitBuffer2, connectivity: Connectivity) -> Self {
            Labeler::default().label(mask, connectivity)
        }
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
