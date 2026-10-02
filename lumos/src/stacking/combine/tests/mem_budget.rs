//! Whether the tier a budget selects changes the combine: the disk (spill + mmap) and in-memory
//! tiers must combine the same frames into an identical master. This exercises the real
//! [`load_to_disk`]/[`load_in_memory`] loaders that the in-memory `stack_images` bypasses.
//!
//! The budget accounting itself is `memory::tests::planned_concurrency_never_overshoots_its_tier_budget`,
//! which sweeps the loader's plan too; the live peak-RSS measurement is the `#[ignore]`d
//! `master_stack_memory_probe` test in `mem_budget_probe`.
//!
//! [`load_to_disk`]: crate::stacking::combine::cache
//! [`load_in_memory`]: crate::stacking::combine::cache

use common::CancelToken;
use fits_well::FitsWriter;
use fits_well::image::Image;

use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::math::size2us::Size2us;
use crate::stacking::combine::config::StackConfig;
use crate::stacking::combine::stack::stack;
use crate::stacking::progress::ProgressCallback;
use common::TempDir;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

/// Write a spatially-uniform 16-bit FITS frame (`value` in every pixel) to `path`.
fn write_const_fits(path: &Path, size: Size2us, value: u16) {
    let image = Image::from_u16(
        vec![size.width, size.height],
        &vec![value; size.pixel_count()],
    )
    .expect("valid image");
    let mut buf = Vec::new();
    FitsWriter::new(&mut buf)
        .write_image(&image, None)
        .expect("encode fits");
    fs::write(path, &buf).expect("write fits");
}

/// The spill tier (`memory_override = 1`) and the resident tier (`u64::MAX`) must combine the same
/// frames into the same master, and that master must equal the plain mean of the frames — verified
/// through the loader itself, so FITS normalization can't skew the expectation.
#[test]
fn disk_and_memory_tiers_produce_identical_masters() {
    let dir = TempDir::new("lumos_mem_tier_test");
    let size = Size2us::new(24, 24);
    let n = 6usize;

    // Distinct per-frame constant → a spatially-uniform master bracketed by the frame values, so a
    // per-frame or per-tier bug shows up as a non-uniform or mis-averaged plane.
    let values: Vec<u16> = (0..n).map(|i| 10_000 + i as u16 * 5_000).collect();
    let paths: Vec<PathBuf> = values
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let p = dir.join(format!("f{i}.fits"));
            write_const_fits(&p, size, v);
            p
        })
        .collect();

    let master = |memory_override: u64, tag: &str| {
        let mut config = StackConfig::mean(); // Mean, no rejection → exact average.
        config.cache.memory_override = Some(memory_override);
        config.cache.cache_dir = dir.join(format!("cache_{tag}"));
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
        .map(|p| {
            LinearImage::from_file(p, &LoadContext::default())
                .unwrap()
                .channel(0)
                .pixels()[0]
        })
        .collect();
    let expected = per_frame.iter().sum::<f32>() / n as f32;
    assert!(
        (first - expected).abs() < 1e-6,
        "master mean {first} != mean-of-frames {expected}"
    );
}
