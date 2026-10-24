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

use crate::math::vec2us::Vec2us;
use crate::star_detection::deblend::pixel::Pixel;

pub(super) mod assignment;
pub(super) mod component;
pub(super) mod component_pixels;
pub(super) mod deblend_buffers;
pub(super) mod deblenders;
pub(super) mod local_maxima;
pub(super) mod multi_threshold;
pub(super) mod pixel;
pub(super) mod region;

/// Squared Euclidean distance between two pixel positions — the one peak-separation
/// metric shared by the Voronoi assignment and every min-separation check.
#[inline]
const fn dist_sq(a: Vec2us, b: Vec2us) -> usize {
    let dx = a.x.abs_diff(b.x);
    let dy = a.y.abs_diff(b.y);
    dx * dx + dy * dy
}

/// Index of the nearest peak to `pos` by squared Euclidean distance; the first peak wins ties.
fn nearest_peak_idx(pos: Vec2us, peaks: &[Pixel]) -> usize {
    let mut min_dist_sq = usize::MAX;
    let mut nearest = 0;
    for (i, peak) in peaks.iter().enumerate() {
        let d = dist_sq(pos, peak.pos());
        if d < min_dist_sq {
            min_dist_sq = d;
            nearest = i;
        }
    }
    nearest
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
    use crate::math::size2us::Size2us;
    use crate::math::urect::URect;
    use crate::math::vec2us::Vec2us;
    use crate::star_detection::deblend::component::Component;
    use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
    use crate::star_detection::deblend::multi_threshold::MultiThresholdParams;
    use crate::star_detection::deblend::region::Region;
    use crate::star_detection::labeling::LabelMap;
    use crate::star_detection::labeling::component_data::ComponentData;
    use crate::star_detection::star_detection_config::extraction_config::Connectivity;
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
                SyntheticStar::new(Vec2::new(30.0, 50.0), 1.0, StarProfile::gaussian(2.5)),
                SyntheticStar::new(Vec2::new(70.0, 50.0), secondary, StarProfile::gaussian(2.5)),
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
            labels: LabelMap::from_raw(&labels, 1),
            data: ComponentData {
                bbox,
                label: 1,
                area,
            },
        }
    }

    /// What a test sets of a multi-threshold deblend: the ladder's levels, the peaks' least
    /// separation, and a branch's least share of the flux.
    #[derive(Debug, Clone, Copy)]
    pub(super) struct Ladder {
        pub(super) threshold_count: usize,
        pub(super) min_separation: usize,
        pub(super) min_contrast: f32,
    }

    impl Ladder {
        /// SExtractor's ladder: 32 levels, peaks 3 px apart, branches of 0.5% of the flux.
        pub(super) const STANDARD: Self = Self {
            threshold_count: 32,
            min_separation: 3,
            min_contrast: 0.005,
        };
    }

    /// The multi-threshold deblender on `component` with fresh buffers and 8-connectivity, its
    /// ladder floored at the component's faintest pixel — the detection threshold a synthetic
    /// component cut at a positive level stands for.
    pub(super) fn deblend_multi_threshold_test(
        component: &Component<'_>,
        ladder: Ladder,
    ) -> Vec<Region> {
        let floor = component
            .pixels()
            .map(|p| p.value)
            .fold(f32::INFINITY, f32::min);
        deblend_multi_threshold_floored(component, floor, ladder)
    }

    /// [`deblend_multi_threshold_test`] with the ladder's floor given.
    pub(super) fn deblend_multi_threshold_floored(
        component: &Component<'_>,
        floor: f32,
        ladder: Ladder,
    ) -> Vec<Region> {
        let Ladder {
            threshold_count,
            min_separation,
            min_contrast,
        } = ladder;
        let mut regions = Vec::new();
        MultiThresholdParams {
            threshold_count,
            min_contrast,
            min_separation,
            min_area: 1,
            connectivity: Connectivity::Eight,
            floor,
        }
        .deblend(component, &mut DeblendBuffers::default(), &mut regions);
        regions
    }
}

#[cfg(test)]
mod tests;
