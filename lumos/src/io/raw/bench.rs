use std::ffi::CString;

use common::CancelToken;
use libraw_sys as sys;
use quickbench::quick_bench;

use crate::internals::init_tracing;
use crate::internals::real_data::raw_frames;
use crate::io::raw::internals::load_raw_libraw_demosaic;
use crate::io::raw::*;

#[quick_bench(warmup_iters = 1, iters = 5)]
fn raw_load(b: quickbench::Bencher) {
    init_tracing();

    let path = raw_frames("Lights").swap_remove(0);
    println!("Benchmarking load_raw on: {}", path.display());

    b.bench(|| load_raw(&path, &LoadContext::default()).unwrap());
}

/// libraw's built-in demosaic at each quality level, beside ours.
/// For X-Trans: qual <= 2 -> Markesteijn 1-pass, qual >= 3 -> Markesteijn 3-pass.
/// For Bayer: 0=linear, 1=VNG, 2=PPG, 3=AHD, 11=DHT.
#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_demosaic_vs_libraw(b: quickbench::Bencher) {
    init_tracing();

    let path = raw_frames("Lights").swap_remove(0);
    println!("Benchmarking demosaic on: {}", path.display());

    b.bench_labeled("ours", || load_raw(&path, &LoadContext::default()).unwrap());
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

/// LibRaw's file datastream against the memory one every load uses: the open and unpack of the
/// first light of each set, read from disk each time either way. The memory path reads the file
/// whole first; the file path reads it as LibRaw's decoder asks.
#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_unpack_file_vs_buffer(b: quickbench::Bencher) {
    init_tracing();

    for set in ["Lights", "raw_samples"] {
        for path in raw_frames(set).into_iter().take(1) {
            println!("Unpacking {}", path.display());
            b.bench_labeled(&format!("{set}: open_buffer"), || {
                let mut libraw =
                    Libraw::open(fs::read(&path).unwrap(), &CancelToken::never()).unwrap();
                libraw.unpack().unwrap();
            });
            let path_c = CString::new(path.to_str().expect("a UTF-8 dataset path")).unwrap();
            b.bench_labeled(&format!("{set}: open_file"), || {
                // SAFETY: the handle is checked, opened from a live C string, and closed once.
                unsafe {
                    let handle = sys::libraw_init(0);
                    assert!(!handle.is_null());
                    assert_eq!(sys::libraw_open_file(handle, path_c.as_ptr()), 0);
                    assert_eq!(sys::libraw_unpack(handle), 0);
                    sys::libraw_close(handle);
                }
            });
        }
    }
}
