//! One tile's clipped sky, σ and noise, which the mesh measures for every tile of a frame on the
//! global pool, a tile a task: here one tile of the default 64 × 64 that holds a star.

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::background_mesh::tile_stats::TileStats;
use crate::background_mesh::workspace::TileScratch;
use crate::bench::fixture::{self, Scale};
use crate::bench::scene::Field;
use crate::math::size2us::Size2us;
use crate::math::urect::URect;
use crate::math::vec2us::Vec2us;
use crate::star_detection::config::background_config::BackgroundConfig;

pub(crate) fn benches(c: &mut Criterion) {
    let config = BackgroundConfig::default();
    let side = config.tile_size;
    let mut group = fixture::group(c, "kernel/detection/background_mesh", Scale::Sample);
    group.throughput(Throughput::Elements((side * side) as u64));
    group.bench_function(
        BenchmarkId::new("tile_stats", format!("tile_{side}")),
        |b| {
            let frame = Field::Typical.render(Size2us::new(4 * side, 4 * side));
            let pixels = frame.image.channel(0);
            let star = frame
                .truth
                .sources
                .first()
                .expect("a frame of 16 tiles holds a star");
            #[expect(clippy::cast_sign_loss, reason = "a star lies inside the frame")]
            let corner = |at: f64| ((at as usize / side) * side).min(3 * side);
            let min = Vec2us::new(corner(star.pos.x), corner(star.pos.y));
            let tile = URect::new(min, min + Vec2us::new(side, side));
            let mut scratch = TileScratch::default();
            b.iter(|| {
                TileStats::compute(
                    pixels,
                    None,
                    None,
                    tile,
                    config.sigma_clip_iterations,
                    &mut scratch,
                )
            });
        },
    );
}
