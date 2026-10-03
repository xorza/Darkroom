//! Benchmarks for the display-stretch stage (linear stacked master → viewable image), the
//! two automatic color-preserving curves. Run:
//! `cargo test -p lumos --release --features bench stretching::bench -- --ignored --nocapture`

use crate::testing::prelude::*;
use crate::testing::synthetic::patterns;
use quickbench::quick_bench;
use std::array;
use std::hint::black_box;

use crate::Stretch;
use crate::image_ops::stretching::{self, AsinhCurve};

const W: usize = 3000;
const H: usize = 2000;

/// Each automatic stretch end to end on the RGB master, plus the clone each pays: `apply`
/// stretches in place, so every call takes a fresh copy (re-stretching one image would feed an
/// already-stretched master back in), and the `clone` row is what that copy costs.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_stretch_rgb(b: ::quickbench::Bencher) {
    let master = patterns::linear_rgb_master(ImageDimensions::new((W, H), 3));
    b.bench_labeled("clone", || black_box(master.clone()));
    for (label, stretch) in [
        ("auto_stf", Stretch::auto_stf()),
        ("auto_asinh", Stretch::auto_asinh()),
    ] {
        b.bench_labeled(label, || {
            let mut img = master.clone();
            stretch
                .apply(&mut img)
                .expect("stretch applies to an RGB f32 master");
            black_box(img)
        });
    }
}

/// Single-thread throughput of the color-preserving arcsinh kernel itself, isolated from the
/// `clone`/subsample overhead the end-to-end benches above also pay. The kernel is branchless in
/// the pixel data, so re-running it in place over drifting values costs a constant per call — no
/// per-iteration reset needed.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_stretch_asinh_kernel_single_thread(b: ::quickbench::Bencher) {
    let curve = AsinhCurve::new(0.05);
    let n_px = W * H;
    // One hashed level per pixel, scaled per channel.
    let planes: [Vec<f32>; 3] = array::from_fn(|channel| {
        let scale = 1.0 - 0.1 * channel as f32;
        (0..n_px)
            .map(|i| {
                let hash = (i as u32).wrapping_mul(2_654_435_761) as f32 / u32::MAX as f32;
                // background-to-star spread, some channels above 1
                (0.03 + hash * 0.5) * scale
            })
            .collect()
    });
    let mut planes = planes;
    b.bench(|| {
        let [r, g, bch] = &mut planes;
        // The same entry point `apply_color_preserving_asinh` calls, so the bench times whichever
        // kernel production picks on this machine.
        stretching::simd::asinh_color_preserve(r, g, bch, curve);
        black_box(&planes);
    });
}
