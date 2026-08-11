//! Benchmarks for the display-stretch stage (linear stacked master → viewable image), the
//! two automatic color-preserving curves. Run:
//! `cargo test -p lumos --release stretching::bench -- --ignored --nocapture`

use crate::testing::prelude::*;
use quickbench::quick_bench;
use std::hint::black_box;

use crate::Stretch;
use crate::image_ops::stretching::{self, AsinhCurve};

const W: usize = 3000;
const H: usize = 2000;

/// A synthetic *linear* RGB master: sky gradient + read noise + a few hundred bright stars whose
/// cores exceed 1.0 (as a real linear stack's do — every stretch curve must clamp them).
fn linear_master() -> LinearImage {
    let dims = ImageDimensions::new((W, H), 3);
    let n = W * H;
    let mut r = vec![0.0f32; n];
    let mut g = vec![0.0f32; n];
    let mut bch = vec![0.0f32; n];
    for y in 0..H {
        for x in 0..W {
            let idx = y * W + x;
            let sky = 0.02 + (y as f32 / H as f32) * 0.03;
            let hash = (idx as u32).wrapping_mul(2654435761) as f32 / u32::MAX as f32;
            let noise = (hash - 0.5) * 0.004;
            r[idx] = sky + noise;
            g[idx] = sky * 0.9 + noise;
            bch[idx] = sky * 0.8 + noise;
        }
    }
    for s in 0..400 {
        let sx = (s as u32).wrapping_mul(2654435761) as usize % W;
        let sy = (s as u32).wrapping_mul(40503) as usize % H;
        let idx = sy * W + sx;
        let bright = 0.5 + (s % 50) as f32 * 0.05;
        r[idx] += bright;
        g[idx] += bright;
        bch[idx] += bright;
    }
    LinearImage::from_planar_channels(dims, vec![r, g, bch])
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_stretch_auto_stf_rgb(b: ::quickbench::Bencher) {
    let master = linear_master();
    let stretch = Stretch::auto_stf();
    // A fresh clone per call: `apply` stretches in place, so re-stretching the same image would
    // feed an already-stretched master back in.
    b.bench(|| {
        let mut img = master.clone();
        stretch
            .apply(&mut img)
            .expect("stretch applies to an RGB f32 master");
        black_box(img)
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_stretch_auto_asinh_rgb(b: ::quickbench::Bencher) {
    let master = linear_master();
    let stretch = Stretch::auto_asinh();
    b.bench(|| {
        let mut img = master.clone();
        stretch
            .apply(&mut img)
            .expect("stretch applies to an RGB f32 master");
        black_box(img)
    });
}

/// Single-thread throughput of the color-preserving arcsinh kernel itself, isolated from the
/// `clone`/subsample overhead the end-to-end benches above also pay. The kernel is branchless in
/// the pixel data, so re-running it in place over drifting values costs a constant per call — no
/// per-iteration reset needed.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_stretch_asinh_kernel_single_thread(b: ::quickbench::Bencher) {
    let curve = AsinhCurve::new(0.05);
    let n_px = W * H;
    // Per channel, so the three planes get the same value scaled — matching what the interleaved
    // fixture wrote per pixel.
    let planes: [Vec<f32>; 3] = std::array::from_fn(|channel| {
        let scale = 1.0 - 0.1 * channel as f32;
        (0..n_px)
            .map(|i| {
                let hash = (i as u32).wrapping_mul(2654435761) as f32 / u32::MAX as f32;
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
