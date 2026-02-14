//! Test utilities for the labeling module.

use crate::common::{BitBuffer2, Buffer2};
use crate::star_detection::config::Connectivity;

use super::LabelMap;

/// Create a label map from pre-computed labels (for testing).
pub fn label_map_from_raw(labels: Buffer2<u32>, num_labels: usize) -> LabelMap {
    LabelMap { labels, num_labels }
}

/// Create a label map from a mask with specified connectivity (for testing).
pub fn label_map_from_mask_with_connectivity(
    mask: &BitBuffer2,
    connectivity: Connectivity,
) -> LabelMap {
    let labels = Buffer2::new_filled(mask.width(), mask.height(), 0u32);
    LabelMap::from_buffer(mask, connectivity, labels)
}
