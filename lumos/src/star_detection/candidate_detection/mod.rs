//! Star candidate detection using thresholding and connected components.

#[cfg(test)]
mod bench;
mod labeling;
#[cfg(test)]
mod tests;

use super::background::BackgroundMap;
use super::buffer_pool::BufferPool;
use super::config::Config;
use super::deblend::{
    ComponentData, DeblendBuffers, DeblendedCandidate, deblend_local_maxima,
    deblend_multi_threshold,
};
use super::mask_dilation::dilate_mask;
use super::threshold_mask::{
    create_adaptive_threshold_mask, create_threshold_mask, create_threshold_mask_filtered,
};
use crate::common::{BitBuffer2, Buffer2};
use crate::math::{Aabb, Vec2us};

pub use labeling::LabelMap;
#[cfg(test)]
pub(crate) use labeling::{label_map_from_mask_with_connectivity, label_map_from_raw};
#[cfg(test)]
pub(crate) use tests::detect_stars_test;

/// A candidate star region before centroid refinement.
#[derive(Debug)]
pub struct StarCandidate {
    /// Bounding box.
    pub bbox: Aabb,
    /// Peak pixel coordinates.
    pub peak: Vec2us,
    /// Peak pixel value.
    pub peak_value: f32,
    /// Number of pixels in the region.
    pub area: usize,
}

/// Detect star candidates in an image using a buffer pool.
///
/// Uses connected component labeling to find regions above the detection threshold.
pub fn detect_stars(
    pixels: &Buffer2<f32>,
    filtered: Option<&Buffer2<f32>>,
    background: &BackgroundMap,
    config: &Config,
    pool: &mut BufferPool,
) -> Vec<StarCandidate> {
    let width = pixels.width();
    let height = pixels.height();

    // Acquire mask buffer from pool
    let mut mask = pool.acquire_bit();
    mask.fill(false);

    // Check if we have adaptive sigma available (from BackgroundMap)
    let use_adaptive = background.adaptive_sigma.is_some() && filtered.is_none();

    if use_adaptive {
        // Use per-pixel adaptive sigma thresholds
        let adaptive_sigma = background.adaptive_sigma.as_ref().unwrap();
        create_adaptive_threshold_mask(
            pixels,
            &background.background,
            &background.noise,
            adaptive_sigma,
            &mut mask,
        );
    } else if let Some(filtered) = filtered {
        debug_assert_eq!(width, filtered.width());
        debug_assert_eq!(height, filtered.height());
        create_threshold_mask_filtered(
            filtered,
            &background.noise,
            config.sigma_threshold,
            &mut mask,
        );
    } else {
        create_threshold_mask(
            pixels,
            &background.background,
            &background.noise,
            config.sigma_threshold,
            &mut mask,
        );
    }

    // Dilate mask to connect nearby pixels
    let mut dilated = pool.acquire_bit();
    dilated.fill(false);
    dilate_mask(&mask, 1, &mut dilated);
    std::mem::swap(&mut mask, &mut dilated);

    pool.release_bit(dilated);

    let label_map = LabelMap::from_pool(&mask, config.connectivity, pool);

    pool.release_bit(mask);

    let candidates = extract_and_filter_candidates(pixels, &label_map, config);

    label_map.release_to_pool(pool);

    candidates
}

/// Extract candidates from label map and filter by size/edge constraints.
fn extract_and_filter_candidates(
    pixels: &Buffer2<f32>,
    label_map: &LabelMap,
    config: &Config,
) -> Vec<StarCandidate> {
    let width = pixels.width();
    let height = pixels.height();

    let mut candidates = extract_candidates(pixels, label_map, config);

    candidates.retain(|c| {
        c.area >= config.min_area
            && c.bbox.min.x >= config.edge_margin
            && c.bbox.min.y >= config.edge_margin
            && c.bbox.max.x < width - config.edge_margin
            && c.bbox.max.y < height - config.edge_margin
    });

    candidates
}

/// Extract candidate properties from labeled image with deblending.
pub(crate) fn extract_candidates(
    pixels: &Buffer2<f32>,
    label_map: &LabelMap,
    config: &Config,
) -> Vec<StarCandidate> {
    use rayon::prelude::*;

    if label_map.num_labels() == 0 {
        return Vec::new();
    }
    let component_data = collect_component_data(label_map, pixels.width(), config.max_area);
    let total_components = component_data.len();

    tracing::debug!(
        total_components,
        max_area = config.max_area,
        multi_threshold = config.is_multi_threshold(),
        "Processing components for candidate extraction"
    );

    let map_to_candidate = |obj: DeblendedCandidate| StarCandidate {
        bbox: obj.bbox,
        peak: obj.peak,
        peak_value: obj.peak_value,
        area: obj.area,
    };

    let filtered: Vec<_> = component_data
        .into_iter()
        .filter(|data| data.area > 0 && data.area <= config.max_area)
        .collect();

    let candidates: Vec<StarCandidate> = if config.is_multi_threshold() {
        filtered
            .into_par_iter()
            .fold(
                || (Vec::new(), DeblendBuffers::new()),
                |(mut candidates, mut buffers), data| {
                    for obj in deblend_multi_threshold(
                        &data,
                        pixels,
                        label_map,
                        config.deblend_n_thresholds,
                        config.deblend_min_separation,
                        config.deblend_min_contrast,
                        &mut buffers,
                    ) {
                        candidates.push(map_to_candidate(obj));
                    }
                    (candidates, buffers)
                },
            )
            .map(|(candidates, _)| candidates)
            .reduce(Vec::new, |mut a, b| {
                a.extend(b);
                a
            })
    } else {
        filtered
            .into_par_iter()
            .flat_map_iter(|data| {
                deblend_local_maxima(
                    &data,
                    pixels,
                    label_map,
                    config.deblend_min_separation,
                    config.deblend_min_prominence,
                )
                .into_iter()
                .map(map_to_candidate)
            })
            .collect()
    };

    tracing::debug!(
        candidates = candidates.len(),
        "Candidate extraction complete"
    );

    candidates
}

/// Collect component metadata (bounding boxes and areas) from label map.
fn collect_component_data(
    label_map: &LabelMap,
    width: usize,
    max_area: usize,
) -> Vec<ComponentData> {
    use common::parallel;
    use parking_lot::Mutex;
    use rayon::prelude::*;

    let num_labels = label_map.num_labels();
    let labels = label_map.labels();

    let result = Mutex::new(vec![
        ComponentData {
            bbox: Aabb::empty(),
            label: 0,
            area: 0,
        };
        num_labels
    ]);

    parallel::par_iter_auto(label_map.height()).for_each_init(
        || {
            (
                vec![
                    ComponentData {
                        bbox: Aabb::empty(),
                        label: 0,
                        area: 0,
                    };
                    num_labels
                ],
                Vec::<usize>::with_capacity(1024),
            )
        },
        |(local_data, touched), (_, start_row, end_row)| {
            for &idx in touched.iter() {
                local_data[idx] = ComponentData {
                    bbox: Aabb::empty(),
                    label: 0,
                    area: 0,
                };
            }
            touched.clear();

            for y in start_row..end_row {
                let row_start = y * width;
                for x in 0..width {
                    let label = labels[row_start + x];
                    if label == 0 {
                        continue;
                    }
                    let idx = (label - 1) as usize;
                    let data = &mut local_data[idx];
                    if data.area == 0 {
                        touched.push(idx);
                    }
                    data.bbox.include(Vec2us::new(x, y));
                    data.label = label;
                    data.area += 1;
                }
            }

            let mut result = result.lock();
            for &idx in touched.iter() {
                let partial_comp = &local_data[idx];
                let data = &mut result[idx];
                data.bbox = data.bbox.merge(&partial_comp.bbox);
                data.label = partial_comp.label;
                data.area += partial_comp.area;
            }
        },
    );

    let mut component_data = result.into_inner();

    for data in &mut component_data {
        if data.area > max_area {
            data.area = max_area + 1;
        }
    }

    component_data
}
