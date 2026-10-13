//! One tile's clipped sky, σ and noise, which the mesh measures for every tile of a frame on the
//! global pool, a tile a task: here one tile of the default 64 × 64 that holds a star.

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::background_mesh::tile_stats::{TileInput, TileScratch, TileStats};
use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::Field;
use crate::math::urect::URect;
use crate::math::vec2us::Vec2us;
use crate::star_detection::star_detection_config::background_config::BackgroundConfig;

pub(crate) fn atomic(c: &mut Criterion) {
    let config = BackgroundConfig::default();
    let side = config.tile_size;
    let mut group = Kind::Atomic.group(c, "detection/background_mesh", Scale::Sample);
    group.throughput(Throughput::Elements((side * side) as u64));
    group.bench_function(
        BenchmarkId::new("tile_stats", format!("tile_{side}")),
        |b| {
            let pixels = Field::Typical.tile(side);
            let tile = URect::new(Vec2us::new(0, 0), Vec2us::new(side, side));
            let input = TileInput {
                pixels: &pixels,
                mask: None,
                eligible: None,
                sigma_clip_iterations: config.sigma_clip_iterations,
            };
            let mut scratch = TileScratch::default();
            b.iter(|| TileStats::compute(&input, tile, &mut scratch));
        },
    );
}
