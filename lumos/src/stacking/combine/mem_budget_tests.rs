//! Memory-tier behaviour of the from-paths stacker.
//!
//! The disk (spill + mmap) and in-memory tiers must produce an identical master: the tier chosen
//! for a given memory budget is an implementation detail that must never change the result. These
//! exercise the real [`load_to_disk`]/[`load_in_memory`] loaders — and the fixed decode-concurrency
//! accounting — that the in-memory `stack_images` path bypasses. The live peak-RSS measurement is
//! the `master_stack_mem` example; here we assert the deterministic invariants instead.
//!
//! [`load_to_disk`]: super::cache
//! [`load_in_memory`]: super::cache

use common::CancelToken;
use fits_well::{FitsWriter, Image};

use crate::AstroImage;
use crate::stacking::combine::config::StackConfig;
use crate::stacking::combine::progress::ProgressCallback;
use crate::stacking::combine::stack::stack;

/// Write a spatially-uniform 16-bit FITS frame (`value` in every pixel) to `path`.
fn write_const_fits(path: &std::path::Path, w: usize, h: usize, value: u16) {
    let image = Image::from_u16(vec![w, h], &vec![value; w * h]);
    let mut buf = Vec::new();
    FitsWriter::new(&mut buf)
        .write_image(&image)
        .expect("encode fits");
    std::fs::write(path, &buf).expect("write fits");
}

/// The spill tier (`available_memory = 1`) and the resident tier (`u64::MAX`) must combine the same
/// frames into the same master, and that master must equal the plain mean of the frames — verified
/// through the loader itself, so FITS normalization can't skew the expectation.
#[test]
fn disk_and_memory_tiers_produce_identical_masters() {
    let dir = std::env::temp_dir().join("lumos_mem_tier_test");
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h, n) = (24usize, 24usize, 6usize);

    // Distinct per-frame constant → a spatially-uniform master bracketed by the frame values, so a
    // per-frame or per-tier bug shows up as a non-uniform or mis-averaged plane.
    let values: Vec<u16> = (0..n).map(|i| 10_000 + i as u16 * 5_000).collect();
    let paths: Vec<std::path::PathBuf> = values
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let p = dir.join(format!("f{i}.fits"));
            write_const_fits(&p, w, h, v);
            p
        })
        .collect();

    let master = |available_memory: u64, tag: &str| {
        let mut config = StackConfig::mean(); // Mean, no rejection → exact average.
        config.cache.available_memory = Some(available_memory);
        config.cache.cache_dir = dir.join(format!("cache_{tag}"));
        config.cache.keep_cache = false;
        stack(
            &paths,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .expect("stack")
        .image
    };

    let disk = master(1, "disk");
    let ram = master(u64::MAX, "ram");

    let disk_px = disk.channel(0).pixels();
    let ram_px = ram.channel(0).pixels();
    assert_eq!(
        disk_px, ram_px,
        "spill and resident tiers must agree pixel-for-pixel"
    );

    // Uniform inputs → uniform master.
    let first = disk_px[0];
    assert!(
        disk_px.iter().all(|&p| p == first),
        "uniform per-frame inputs must yield a uniform master, got a varying plane"
    );

    // Expected = mean of each frame's normalized constant, read back through the same loader (so the
    // check is independent of how FITS maps u16 → f32). Mean(None) must reproduce it exactly.
    let per_frame: Vec<f32> = paths
        .iter()
        .map(|p| AstroImage::from_file(p).unwrap().channel(0).pixels()[0])
        .collect();
    let expected = per_frame.iter().sum::<f32>() / n as f32;
    assert!(
        (first - expected).abs() < 1e-6,
        "master mean {first} != mean-of-frames {expected}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
