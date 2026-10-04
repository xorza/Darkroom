//! Star deblending algorithms for separating overlapping sources.
//!
//! This module provides two deblending approaches:
//!
//! 1. **Local maxima deblending** (`local_maxima`): Fast algorithm that finds
//!    peaks in a connected component and assigns pixels to the nearest peak.
//!    Good for well-separated stars.
//!
//! 2. **Multi-threshold deblending** (`multi_threshold`): SExtractor-style
//!    tree-based algorithm that uses multiple threshold levels to separate
//!    blended sources. More accurate for crowded fields but slower.

use std::cmp::Ordering;

use crate::math::vec2us::Vec2us;

pub(super) mod component;
pub(super) mod component_pixels;
pub(super) mod deblend_buffers;
pub(super) mod local_maxima;
pub(super) mod multi_threshold;
pub(super) mod region;

/// A pixel with its coordinates and value.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pixel {
    pub(crate) pos: Vec2us,
    pub(crate) value: f32,
}

impl Pixel {
    /// The brightest of `pixels`, the first in iteration order among equals; `None` when there
    /// are none.
    fn brightest(pixels: impl IntoIterator<Item = Self>) -> Option<Self> {
        pixels
            .into_iter()
            .reduce(|best, p| if p.value > best.value { p } else { best })
    }

    /// Brightest first; equal values in raster order (row, then column), so a ranking does not
    /// depend on the order its input arrived in.
    fn brighter_first(a: &Self, b: &Self) -> Ordering {
        b.value
            .total_cmp(&a.value)
            .then(a.pos.y.cmp(&b.pos.y))
            .then(a.pos.x.cmp(&b.pos.x))
    }
}

/// Squared Euclidean distance between two pixel positions — the one peak-separation
/// metric shared by the Voronoi assignment and every min-separation check.
#[inline]
const fn dist_sq(a: Vec2us, b: Vec2us) -> usize {
    let dx = a.x.abs_diff(b.x);
    let dy = a.y.abs_diff(b.y);
    dx * dx + dy * dy
}

/// Index of the nearest peak to `pos` by squared Euclidean distance; the first peak wins ties.
fn nearest_peak_index(pos: Vec2us, peaks: &[Pixel]) -> usize {
    let mut min_dist_sq = usize::MAX;
    let mut nearest = 0;
    for (i, peak) in peaks.iter().enumerate() {
        let d = dist_sq(pos, peak.pos);
        if d < min_dist_sq {
            min_dist_sq = d;
            nearest = i;
        }
    }
    nearest
}

/// Whether two peak positions are closer than a squared-distance threshold, by the same
/// squared Euclidean metric [`nearest_peak_index`] uses for the Voronoi assignment every
/// deblender's peaks eventually feed into. `min_sep_sq` is
/// `min_separation * min_separation`, pre-squared by the caller since peak-separation
/// checks run in a loop over many candidate pairs.
#[inline]
const fn peaks_too_close(a: Vec2us, b: Vec2us, min_sep_sq: usize) -> bool {
    dist_sq(a, b) < min_sep_sq
}

#[cfg(test)]
pub(crate) mod internals {
    #[cfg(feature = "bench")]
    use crate::bit_buffer2::BitBuffer2;
    use imaginarium::Buffer2;

    use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
    use crate::math::size2us::Size2us;
    use crate::math::urect::URect;
    use crate::math::vec2us::Vec2us;
    use crate::star_detection::config::detection_config::Connectivity;
    use crate::star_detection::deblend::component::Component;
    use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
    use crate::star_detection::deblend::multi_threshold::MultiThresholdParams;
    use crate::star_detection::deblend::region::Region;
    use crate::star_detection::labeling::LabelMap;
    use crate::star_detection::labeling::component_data::ComponentData;
    use glam::Vec2;

    /// Rendered stars and the one label covering every pixel they light above 0.001.
    #[derive(Debug)]
    pub(crate) struct TestComponent {
        pub(crate) pixels: Buffer2<f32>,
        pub(crate) labels: LabelMap,
        pub(crate) data: ComponentData,
    }

    /// A 1.0 star at (30, 50) and a `secondary` one at (70, 50), σ 2.5, on 100×100: two disjoint
    /// blobs under one label.
    pub(super) fn separated_pair(secondary: f32) -> TestComponent {
        make_test_component(
            Size2us::new(100, 100),
            &[
                SyntheticStar::new(
                    Vec2::new(30.0, 50.0),
                    1.0,
                    StarProfile::Gaussian { sigma: 2.5 },
                ),
                SyntheticStar::new(
                    Vec2::new(70.0, 50.0),
                    secondary,
                    StarProfile::Gaussian { sigma: 2.5 },
                ),
            ],
        )
    }

    #[expect(
        clippy::cast_sign_loss,
        reason = "synthetic fixtures are small images with non-negative coordinates"
    )]
    pub(crate) fn make_test_component(size: Size2us, stars: &[SyntheticStar]) -> TestComponent {
        let mut pixels = Buffer2::new_filled(size.width, size.height, 0.0f32);
        let mut labels = Buffer2::new_filled(size.width, size.height, 0u32);
        let mut bbox = URect::empty();
        let mut area = 0;

        for &star in stars {
            let radius = star.radius();
            let star_pixels = star.pixels();
            for offset_y in -radius..=radius {
                for offset_x in -radius..=radius {
                    let x = (star.center.x as i32 + offset_x) as usize;
                    let y = (star.center.y as i32 + offset_y) as usize;
                    if !size.contains(Vec2us::new(x, y)) {
                        continue;
                    }

                    // The cutoff decides component membership, not just brightness: a pixel
                    // below it stays unlabelled, so it never joins the component's bbox or area.
                    let value = star_pixels.value(x, y);
                    if value <= 0.001 {
                        continue;
                    }

                    pixels[(x, y)] += value;
                    if labels[(x, y)] == 0 {
                        labels[(x, y)] = 1;
                        bbox.include(Vec2us::new(x, y));
                        area += 1;
                    }
                }
            }
        }

        TestComponent {
            pixels,
            labels: LabelMap::from_raw(labels, 1),
            data: ComponentData {
                bbox,
                label: 1,
                area,
            },
        }
    }

    /// The multi-threshold deblender on `component` with fresh buffers and 8-connectivity, its
    /// ladder floored at the component's faintest pixel — the detection threshold a synthetic
    /// component cut at a positive level stands for.
    pub(super) fn deblend_multi_threshold_test(
        component: &Component<'_>,
        n_thresholds: usize,
        min_separation: usize,
        min_contrast: f32,
    ) -> Vec<Region> {
        let floor = component
            .pixels()
            .map(|p| p.value)
            .fold(f32::INFINITY, f32::min);
        deblend_multi_threshold_floored(
            component,
            floor,
            n_thresholds,
            min_separation,
            min_contrast,
        )
    }

    /// The pixels of `pixels` above `threshold`, labelled with 4-connectivity: the components the
    /// deblender benches run on.
    #[cfg(feature = "bench")]
    pub(crate) fn label_above(pixels: &Buffer2<f32>, threshold: f32) -> LabelMap {
        let mut mask = BitBuffer2::new_filled(Size2us::new(pixels.width(), pixels.height()), false);
        for (idx, &value) in pixels.iter().enumerate() {
            if value > threshold {
                mask.set(idx, true);
            }
        }
        LabelMap::from_mask(&mask, Connectivity::Four)
    }

    /// [`deblend_multi_threshold_test`] with the ladder's floor given.
    pub(super) fn deblend_multi_threshold_floored(
        component: &Component<'_>,
        floor: f32,
        n_thresholds: usize,
        min_separation: usize,
        min_contrast: f32,
    ) -> Vec<Region> {
        let mut regions = Vec::new();
        MultiThresholdParams {
            n_thresholds,
            min_contrast,
            min_separation,
            min_area: 1,
            connectivity: Connectivity::Eight,
        }
        .deblend(
            component,
            floor,
            &mut DeblendBuffers::default(),
            &mut regions,
        );
        regions
    }
}

#[cfg(test)]
mod tests;
