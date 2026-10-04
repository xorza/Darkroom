//! Benchmarks for the X-Trans demosaic.

use common::CancelToken;
use quickbench::quick_bench;

use crate::internals::test_rng::TestRng;
use crate::io::raw::demosaic::xtrans::internals::make_xtrans;
use crate::io::raw::demosaic::xtrans::markesteijn::{self, MarkesteijnPasses};
use crate::math::size2us::Size2us;

/// The Markesteijn demosaic alone at each pass count, on random samples the size of a 26 MP
/// X-Trans frame, so its direction choices take every branch.
#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_markesteijn_demosaic(b: quickbench::Bencher) {
    let size = Size2us::new(6240, 4160);
    let mut rng = TestRng::new(3);
    let data: Vec<f32> = (0..size.pixel_count())
        .map(|_| 0.1 + 0.8 * rng.next_f32())
        .collect();
    let xtrans = make_xtrans(&data, size);
    for passes in [MarkesteijnPasses::One, MarkesteijnPasses::Three] {
        b.bench_labeled(&format!("{passes:?}"), || {
            markesteijn::demosaic(&xtrans, passes, &CancelToken::never()).unwrap()
        });
    }
}
