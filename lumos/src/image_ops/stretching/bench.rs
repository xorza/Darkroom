//! Benchmarks for the display-stretch stage (linear stacked master → viewable image), the
//! two automatic color-preserving curves. Run:
//! `cargo test -p lumos --release stretching::bench -- --ignored --nocapture`

use quickbench::quick_bench;
use std::hint::black_box;

use crate::Stretch;
#[cfg(not(target_arch = "aarch64"))]
use crate::image_ops::rgb::Rgb;
use crate::image_ops::stretching::{self, AsinhCurve};

/// Scalar reference path for targets (or x86 CPUs) without the SIMD kernel.
#[cfg(not(target_arch = "aarch64"))]
fn scalar_asinh(red: &mut [f32], green: &mut [f32], blue: &mut [f32], curve: &AsinhCurve) {
    for i in 0..red.len() {
        let out = stretching::color_preserve_pixel(
            Rgb {
                r: red[i],
                g: green[i],
                b: blue[i],
            },
            curve,
        );
        red[i] = out.r;
        green[i] = out.g;
        blue[i] = out.b;
    }
}
use crate::io::image::ImageDimensions;
use crate::io::image::linear::LinearImage;

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

#[quick_bench(warmup_iters = 1, iters = 5)]
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
#[quick_bench(warmup_iters = 1, iters = 10)]
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
        // Mirror `apply_color_preserving_asinh`'s dispatch so the bench
        // times the kernel production actually runs on this machine.
        #[cfg(target_arch = "aarch64")]
        // SAFETY: NEON is always available on aarch64.
        unsafe {
            stretching::simd_neon::asinh_color_preserve_neon(
                r,
                g,
                bch,
                curve.inv_beta,
                curve.inv_norm,
            );
        }
        #[cfg(target_arch = "x86_64")]
        if imaginarium::cpu_features::has_avx2_fma() {
            // SAFETY: AVX2+FMA availability checked above.
            unsafe {
                stretching::simd_avx2::asinh_color_preserve_avx2(
                    r,
                    g,
                    bch,
                    curve.inv_beta,
                    curve.inv_norm,
                );
            }
        } else {
            scalar_asinh(r, g, bch, &curve);
        }
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        scalar_asinh(r, g, bch, &curve);
        black_box(&planes);
    });
}
