//! Connected component labeling using union-find.
//!
//! Optimized for sparse binary masks (typical in star detection):
//! - Run-length encoding (RLE) based labeling for efficient processing
//! - Word-level bit scanning to skip background regions
//! - Strip-parallel labeling with boundary merging, down to the single strip a small image needs
//! - Lock-free union-find with atomic operations
//! - Minimal allocations via buffer reuse

pub(crate) mod component_data;
pub(crate) mod labeler;
mod run;
mod union_find;

use std::ops::Index;

use crate::bit_buffer2::BitBuffer2;
use crate::star_detection::config::detection_config::Connectivity;
use crate::star_detection::labeling::component_data::ComponentData;
use crate::star_detection::resources::DetectionResources;
use imaginarium::Buffer2;

/// A 2D label map from connected component analysis, with each component's box and area.
#[derive(Debug)]
pub(crate) struct LabelMap {
    labels: Buffer2<u32>,
    /// Component `i` carries label `i + 1`.
    components: Vec<ComponentData>,
    /// Which of its labeler's labelings wrote it, so it can erase its runs while the labeler
    /// still holds them.
    generation: u64,
}

impl LabelMap {
    /// Label the foreground of `mask` into a buffer and scratch drawn from `resources`.
    pub(crate) fn from_pool(
        mask: &BitBuffer2,
        connectivity: Connectivity,
        resources: &mut DetectionResources,
    ) -> Self {
        debug_assert_eq!(mask.size, resources.dimensions);
        let labels = resources.acquire_u32();
        debug_assert!(labels.pixels().iter().all(|&label| label == 0));
        resources.labeler.label(mask, connectivity, labels)
    }

    /// Release this `LabelMap`'s buffers back to the pool, its labels zero again: the runs its
    /// labeling wrote erased, rather than the whole plane cleared before the next.
    pub(crate) fn release_to_pool(mut self, pool: &mut DetectionResources) {
        pool.labeler.erase(&mut self.labels, self.generation);
        pool.release_u32(self.labels);
        pool.labeler.recycle(self.components);
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

    #[inline]
    pub(crate) const fn width(&self) -> usize {
        self.labels.width()
    }

    #[inline]
    pub(crate) const fn height(&self) -> usize {
        self.labels.height()
    }
}

impl Index<usize> for LabelMap {
    type Output = u32;

    #[inline]
    fn index(&self, idx: usize) -> &Self::Output {
        &self.labels[idx]
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::bit_buffer2::BitBuffer2;
    use crate::math::vec2us::Vec2us;
    use crate::star_detection::config::detection_config::Connectivity;
    use crate::star_detection::labeling::LabelMap;
    use crate::star_detection::labeling::component_data::ComponentData;
    use crate::star_detection::labeling::labeler::Labeler;

    impl LabelMap {
        /// Adopt pre-computed labels `1..=num_labels`, bypassing connected-component analysis;
        /// the components are collected by a scan of every pixel.
        pub(crate) fn from_raw(labels: Buffer2<u32>, num_labels: usize) -> Self {
            let mut components = vec![ComponentData::default(); num_labels];
            for y in 0..labels.height() {
                for x in 0..labels.width() {
                    let label = labels[(x, y)];
                    if label == 0 {
                        continue;
                    }
                    let component = &mut components[label as usize - 1];
                    component.label = label;
                    component.bbox.include(Vec2us::new(x, y));
                    component.area += 1;
                }
            }
            Self {
                labels,
                components,
                generation: u64::MAX,
            }
        }

        /// The raw labels, row-major.
        pub(crate) fn labels(&self) -> &[u32] {
            self.labels.pixels()
        }

        /// Label `mask` into a freshly allocated buffer instead of one drawn from the pool.
        pub(crate) fn from_mask(mask: &BitBuffer2, connectivity: Connectivity) -> Self {
            let labels = Buffer2::new_filled(mask.size.width, mask.size.height, 0u32);
            Labeler::default().label(mask, connectivity, labels)
        }
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
