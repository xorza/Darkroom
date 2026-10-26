use common::{Buffer2, Size2us};

use crate::bg_mesh::BgMesh;
use crate::bg_mesh::mesh_spline::MeshSpline;
use crate::bg_mesh::mesh_surface::MeshSurface;
use crate::bg_mesh::mesh_workspace::{MedianFilter, MeshParams};
use crate::bg_mesh::tile_stats::TileComponent;
use crate::internals::test_rng::TestRng;
use crate::scratch_pool::ScratchPool;
use crate::star_detection::convolution::filter_kernels::FilterKernels;
use crate::star_detection::convolution::filter_kernels::MatchedFilter;
use crate::star_detection::convolution::internals::matched_filter_fresh;
use crate::star_detection::detection_filter::Prefilter;
use crate::star_detection::detection_filter::{DetectionFilter, FilterSource};
use crate::star_detection::median_filter::internals::apply_3x3;

/// A plane of noise around a slope, its sky the mesh of 16-px tiles measures on it.
fn frame(size: Size2us, seed: u64) -> (Buffer2<f32>, MeshSurface) {
    let mut rng = TestRng::new(seed);
    let pixels = Buffer2::from_fn(size, |x, y| {
        0.2 + 0.003 * x as f32 + 0.002 * y as f32 + 0.05 * rng.next_f32()
    });
    let mesh = BgMesh::computed(
        &pixels,
        None,
        MeshParams {
            tile_size: 16,
            sigma_clip_max_iterations: 2,
            median_filter: MedianFilter::Apply,
        },
    );
    let mut sky = MeshSurface::default();
    MeshSpline::default().solve(&mesh, TileComponent::Sky, &mut sky);
    (pixels, sky)
}

/// The filters over whole planes: the residual or the plane, its 3×3 median, then the matched
/// filter in place.
fn whole(
    source: FilterSource<'_>,
    prefilter: Prefilter,
    matched: Option<MatchedFilter>,
) -> Buffer2<f32> {
    let pixels = match source {
        FilterSource::Plane(pixels) => pixels.clone(),
        FilterSource::Residual { pixels, sky } => {
            let mut residual = pixels.clone();
            let sky = sky.plane();
            for (value, &level) in residual.pixels_mut().iter_mut().zip(sky.pixels()) {
                *value -= level;
            }
            residual
        }
    };
    let mut temp = Buffer2::new_default(pixels.size());
    let mut values = match prefilter {
        Prefilter::None => pixels,
        Prefilter::Median3x3 => {
            apply_3x3(&pixels, &mut temp);
            temp.clone()
        }
    };
    if let Some(filter) = matched {
        matched_filter_fresh(&mut values, filter, &mut temp);
    }
    values
}

/// A band at a time is the whole plane bit for bit, whatever the band's height: every source, with
/// and without the median, under no matched filter, a circular one, one along the axes and an
/// ellipse off them, which takes the 2D kernel. The frames are wider than tall, square, as narrow
/// as the median allows and narrower, and shorter than the kernel, whose ring is then the whole
/// image, so every edge the taps mirror at is crossed by some band.
#[test]
fn a_band_at_a_time_is_the_whole_plane_bit_for_bit() {
    let filters = [
        None,
        Some(MatchedFilter {
            fwhm: 3.0,
            axis_ratio: 1.0,
            angle: 0.0,
        }),
        Some(MatchedFilter {
            fwhm: 3.0,
            axis_ratio: 0.6,
            angle: 0.0,
        }),
        Some(MatchedFilter {
            fwhm: 3.0,
            axis_ratio: 0.7,
            angle: 0.5,
        }),
    ];
    let bands = ScratchPool::default();
    for (width, height) in [(37, 23), (24, 24), (3, 17), (2, 9), (40, 5), (19, 1)] {
        let (pixels, sky) = frame(Size2us::new(width, height), (width * 100 + height) as u64);
        for source in [
            FilterSource::Plane(&pixels),
            FilterSource::Residual {
                pixels: &pixels,
                sky: &sky,
            },
        ] {
            let kind = match source {
                FilterSource::Plane(_) => "plane",
                FilterSource::Residual { .. } => "residual",
            };
            for prefilter in [Prefilter::None, Prefilter::Median3x3] {
                for matched in filters {
                    let expected = whole(source, prefilter, matched);
                    for band in [1, 2, 3, 7, height] {
                        let mut kernels = FilterKernels::default();
                        let mut output = Buffer2::new_filled((width, height), f32::NAN);
                        DetectionFilter {
                            prefilter,
                            matched: matched.map(|filter| kernels.matched(filter)),
                        }
                        .run_in_bands(
                            source,
                            &mut output,
                            &bands,
                            band.min(height),
                        );
                        let case = format!(
                            "{width}×{height}, {kind}, {prefilter:?}, {matched:?}, bands of \
                             {band}"
                        );
                        for (idx, (&got, &want)) in
                            output.pixels().iter().zip(expected.pixels()).enumerate()
                        {
                            assert_eq!(got.to_bits(), want.to_bits(), "{case}: pixel {idx}");
                        }
                    }
                }
            }
        }
    }
}
