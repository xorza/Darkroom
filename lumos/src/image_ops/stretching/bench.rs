//! Benchmarks for the display-stretch stage (linear stacked master → viewable image), the
//! two automatic color-preserving curves. Run:
//! `cargo test -p lumos --release stretching::bench -- --ignored --nocapture`

use quickbench::quick_bench;
use std::hint::black_box;

use imaginarium::Image;

use crate::Stretch;
use crate::io::astro_image::{AstroImage, ImageDimensions};

const W: usize = 3000;
const H: usize = 2000;

/// A synthetic *linear* RGB master: sky gradient + read noise + a few hundred bright stars whose
/// cores exceed 1.0 (as a real linear stack's do — every stretch curve must clamp them).
fn linear_master() -> AstroImage {
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
    AstroImage::from_planar_channels(dims, vec![r, g, bch])
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_stretch_auto_stf_rgb(b: ::quickbench::Bencher) {
    let master = linear_master();
    let stretch = Stretch::auto_stf();
    // A fresh linear image per call: `apply` stretches in place, so re-stretching the same image
    // would feed an already-stretched master back in. The convert-from-`AstroImage` is the
    // realistic input cost (the master is handed to the display stage as a linear `AstroImage`).
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
