use quickbench::quick_bench;

use crate::io::raw::internals::load_raw_libraw_demosaic;
use crate::io::raw::*;
use crate::testing::init_tracing;
use crate::testing::real_data::raw_frames;

#[quick_bench(warmup_iters = 1, iters = 5)]
fn raw_load(b: quickbench::Bencher) {
    init_tracing();

    let path = raw_frames("Lights").swap_remove(0);
    println!("Benchmarking load_raw on: {}", path.display());

    b.bench(|| load_raw(&path, &CancelToken::never()).unwrap());
}

/// libraw's built-in demosaic at each quality level, beside ours.
/// For X-Trans: qual <= 2 -> Markesteijn 1-pass, qual >= 3 -> Markesteijn 3-pass.
/// For Bayer: 0=linear, 1=VNG, 2=PPG, 3=AHD, 11=DHT.
#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_demosaic_vs_libraw(b: quickbench::Bencher) {
    init_tracing();

    let path = raw_frames("Lights").swap_remove(0);
    println!("Benchmarking demosaic on: {}", path.display());

    b.bench_labeled("ours", || load_raw(&path, &CancelToken::never()).unwrap());
    for (qual, label) in [
        (0, "libraw linear"),
        (1, "libraw VNG / Markesteijn 1-pass"),
        (2, "libraw PPG / Markesteijn 1-pass"),
        (3, "libraw AHD / Markesteijn 3-pass"),
        (11, "libraw DHT / Markesteijn 3-pass"),
    ] {
        b.bench_labeled(label, || load_raw_libraw_demosaic(&path, qual).unwrap());
    }
}
