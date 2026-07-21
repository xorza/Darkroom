//! Benchmarks for the display-stretch stage (linear stacked master → viewable image), the
//! two automatic color-preserving curves. Run:
//! `cargo test -p lumos --release stretching::bench -- --ignored --nocapture`

use quickbench::quick_bench;
use std::hint::black_box;

use imaginarium::Image;

use crate::Stretch;
use crate::image_ops::rgb::Rgb;
use crate::image_ops::stretching::{self, AsinhCurve};
use crate::io::image::{ImageDimensions, LinearImage};

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

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_stretch_auto_stf_rgb(b: ::quickbench::Bencher) {
    let master = linear_master();
    let stretch = Stretch::auto_stf();
    // A fresh linear image per call: `apply` stretches in place, so re-stretching the same image
    // would feed an already-stretched master back in. The convert-from-`LinearImage` is the
    // realistic input cost (the master is handed to the display stage as a linear `LinearImage`).
    b.bench(|| {
        let mut img: Image = master.clone().into();
        stretch
            .apply(&mut img)
            .expect("stretch applies to an RGB f32 master");
        black_box(img)
    });
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_stretch_auto_asinh_rgb(b: ::quickbench::Bencher) {
    let master = linear_master();
    let stretch = Stretch::auto_asinh();
    b.bench(|| {
        let mut img: Image = master.clone().into();
        stretch
            .apply(&mut img)
            .expect("stretch applies to an RGB f32 master");
        black_box(img)
    });
}

/// Single-thread throughput of the color-preserving arcsinh kernel itself, isolated from the
/// `clone`/planar→interleaved-convert/subsample overhead the end-to-end benches above also pay.
/// The kernel is branchless in the pixel data, so re-running it in place over drifting values costs
/// a constant per call — no per-iteration reset needed.
#[quick_bench(warmup_iters = 1, iters = 10)]
fn bench_stretch_asinh_kernel_single_thread(b: ::quickbench::Bencher) {
    let curve = AsinhCurve::new(0.05);
    let n_px = W * H;
    let mut buf = vec![0.0f32; n_px * 3];
    for (i, px) in buf.chunks_exact_mut(3).enumerate() {
        let hash = (i as u32).wrapping_mul(2654435761) as f32 / u32::MAX as f32;
        let v = 0.03 + hash * 0.5; // background-to-star spread, some channels above 1
        px[0] = v;
        px[1] = v * 0.9;
        px[2] = v * 0.8;
    }
    b.bench(|| {
        // SAFETY: NEON is always available on aarch64.
        #[cfg(target_arch = "aarch64")]
        unsafe {
            stretching::simd_neon::asinh_color_preserve_neon(
                &mut buf,
                curve.inv_beta,
                curve.inv_norm,
            );
        }
        #[cfg(not(target_arch = "aarch64"))]
        for px in buf.chunks_exact_mut(3) {
            let out = stretching::color_preserve_pixel(
                Rgb {
                    r: px[0],
                    g: px[1],
                    b: px[2],
                },
                &curve,
            );
            px[0] = out.r;
            px[1] = out.g;
            px[2] = out.b;
        }
        black_box(&buf);
    });
}
