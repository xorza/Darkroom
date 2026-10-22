use imaginarium::Buffer2;

use crate::bg_mesh::mesh_workspace::{MedianFilter, MeshParams, MeshWorkspace};
use crate::bg_mesh::tile_stats::MAX_TILE_SAMPLES;
use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;

/// Tiles of `tile_size`, two σ-clip passes, and `median_filter`.
const fn params(tile_size: usize, median_filter: MedianFilter) -> MeshParams {
    MeshParams {
        tile_size,
        sigma_clip_max_iterations: 2,
        median_filter,
    }
}

#[test]
fn repeated_measures_reuse_mesh_allocations_and_preserve_results() {
    let pixels = Buffer2::new_filled(256, 256, 0.5);
    let mut workspace = MeshWorkspace::default();
    let expected = {
        let grid = workspace.measure(&pixels, None, None, params(64, MedianFilter::Apply));
        grid.stats.pixels().to_vec()
    };
    let first_stats_ptr = workspace.grid.as_ref().unwrap().stats.pixels().as_ptr();
    let filter_ptr = workspace
        .median_filter_scratch
        .as_ref()
        .unwrap()
        .pixels()
        .as_ptr();
    {
        let grid = workspace.measure(&pixels, None, None, params(64, MedianFilter::Apply));
        assert_eq!(grid.stats.pixels().as_ptr(), filter_ptr);
        for (actual, expected) in grid.stats.pixels().iter().zip(expected) {
            assert_eq!(actual.sky, expected.sky);
            assert_eq!(actual.sigma, expected.sigma);
        }
    }
    assert_eq!(
        workspace
            .median_filter_scratch
            .as_ref()
            .unwrap()
            .pixels()
            .as_ptr(),
        first_stats_ptr
    );
}

#[test]
fn clear_releases_all_mesh_allocations() {
    let pixels = Buffer2::new_filled(128, 128, 0.5);
    let mut workspace = MeshWorkspace::default();
    workspace.measure(&pixels, None, None, params(32, MedianFilter::Apply));
    assert!(workspace.grid.is_some());
    assert!(workspace.median_filter_scratch.is_some());
    assert_ne!(workspace.tile_scratch.pooled(), 0);

    workspace.clear();

    assert!(workspace.grid.is_none());
    assert!(workspace.median_filter_scratch.is_none());
    assert_eq!(workspace.tile_scratch.pooled(), 0);
}

#[test]
fn a_measure_rebuilds_the_grid_for_layout_changes() {
    let first_pixels = Buffer2::new_filled(100, 70, 0.5);
    let second_pixels = Buffer2::new_filled(64, 32, 0.25);
    let mut workspace = MeshWorkspace::default();

    // 100 × 70 in 32s: remainders of 4 and 6, under half a tile, join their neighbours.
    let grid = workspace.measure(&first_pixels, None, None, params(32, MedianFilter::Skip));
    assert_eq!(grid.stats.width(), 3);
    assert_eq!(grid.stats.height(), 2);

    let grid = workspace.measure(&first_pixels, None, None, params(50, MedianFilter::Skip));
    assert_eq!(grid.stats.width(), 2);
    assert_eq!(grid.stats.height(), 1);

    let grid = workspace.measure(&second_pixels, None, None, params(16, MedianFilter::Skip));
    assert_eq!(grid.stats.width(), 4);
    assert_eq!(grid.stats.height(), 2);
    for stats in grid.stats.pixels() {
        assert_eq!(stats.sky, 0.25);
        assert_eq!(stats.sigma, 0.0);
    }
}

#[test]
fn a_masked_measure_keeps_per_job_scratch_bounded() {
    let pixels = Buffer2::new_filled(512, 512, 0.5);
    let mask = BitBuffer2::new_filled(Size2us::new(512, 512), false);
    let mut workspace = MeshWorkspace::default();

    workspace.measure(&pixels, Some(&mask), None, params(256, MedianFilter::Skip));
    assert_ne!(workspace.tile_scratch.pooled(), 0);
    assert!(
        workspace
            .tile_scratch
            .all_free(|scratch| { scratch.samples.values.capacity() <= MAX_TILE_SAMPLES }),
        "masked value scratch must retain at most {MAX_TILE_SAMPLES} samples per job"
    );
    assert!(
        workspace
            .tile_scratch
            .all_free(|scratch| { scratch.deviations.capacity() <= MAX_TILE_SAMPLES }),
        "deviation scratch must retain at most {MAX_TILE_SAMPLES} samples per job"
    );
}
