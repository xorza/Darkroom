//! One tile's clipped sky, σ and noise, which the mesh measures for every tile of a frame on the
//! global pool, a tile a task: here one tile of the default 64 × 64 that holds a star.

use common::bench::{Kind, Scale};
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::scene::Field;
use crate::bg_mesh::tile_stats::{TileInput, TileScratch, TileStats};
use crate::star_detection::star_detection_config::bg_config::BgConfig;
use common::{Rect2us, Vec2us};

pub(crate) fn atomic(c: &mut Criterion) {
    let config = BgConfig::default();
    let side = config.tile_size;
    let mut group = Kind::Atomic.group(c, "detection/bg_mesh", Scale::Micros);
    group.throughput(Throughput::Elements((side * side) as u64));
    group.bench_function(
        BenchmarkId::new("tile_stats", format!("tile_{side}")),
        |b| {
            let pixels = Field::Typical.tile(side);
            let tile = Rect2us::new(Vec2us::new(0, 0), Vec2us::new(side, side));
            let input = TileInput {
                pixels: &pixels,
                mask: None,
                color: None,
                sigma_clip_max_iterations: config.sigma_clip_max_iterations,
            };
            let mut scratch = TileScratch::default();
            b.iter(|| TileStats::compute(&input, tile, &mut scratch));
        },
    );
}
