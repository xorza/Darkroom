//! Whether the combine respects its memory budget, across two levels.
//!
//! - **Budget accounting** ([`load_budget_is_respected_across_configs`]): a deterministic sweep
//!   over (frame size, count, budget), asserting the loader's chosen decode concurrency never lets
//!   projected peak load heap exceed the usable budget — the invariant behind the ~2× overshoot fix.
//! - **Tier correctness** ([`disk_and_memory_tiers_produce_identical_masters`]): the disk (spill +
//!   mmap) and in-memory tiers must combine the same frames into an identical master — the tier a
//!   budget selects is an implementation detail that must never change the result. This exercises
//!   the real [`load_to_disk`]/[`load_in_memory`] loaders that the in-memory `stack_images` bypasses.
//!
//! The live peak-RSS measurement is the `#[ignore]`d `master_stack_memory_probe` test in
//! `mem_budget_probe`; here we assert the deterministic invariants instead.
//!
//! [`load_to_disk`]: crate::stacking::combine::cache
//! [`load_in_memory`]: crate::stacking::combine::cache

use common::CancelToken;
use fits_well::FitsWriter;
use fits_well::image::Image;

use crate::AstroImage;
use crate::stacking::combine::config::StackConfig;
use crate::stacking::combine::stack::stack;
use crate::stacking::frame_store::{fits_in_memory, load_concurrency, memory_budget};
use crate::stacking::progress::ProgressCallback;
use crate::testing::ScratchDirectory;

/// The tiered loader's core guarantee, as an invariant sweep: for any (frame size, count, budget),
/// the chosen concurrency must not let peak *load* heap exceed the usable budget. Project that peak
/// — resident set + concurrency × the real 2× per-decode transient — and assert it fits, except at
/// the irreducible floor where not even one decode fits the budget (concurrency pinned to 1). This
/// is what the `master_stack_memory_probe` measured live; here it's deterministic. It fails if the
/// divisor ever reverts to the resident frame size (the ~2× overshoot bug), since concurrency would
/// then double and the projection blow the budget.
#[test]
fn load_budget_is_respected_across_configs() {
    const MIB: u64 = 1024 * 1024;
    let workers = 32;
    for &frame_mb in &[16u64, 64, 137, 512] {
        let frame = (frame_mb * MIB) as usize;
        let transient = 2 * frame; // matches DECODE_TRANSIENT_FACTOR in `cache`
        for &count in &[4usize, 12, 24, 60] {
            for &budget_gb in &[1u64, 2, 4, 8, 16] {
                let budget = budget_gb * 1024 * MIB;
                let usable = memory_budget(budget);
                let resident_frames = if fits_in_memory(frame, count, budget) {
                    count
                } else {
                    0
                };
                let c = load_concurrency(frame, transient, resident_frames, budget, workers);
                let projected = resident_frames as u64 * frame as u64 + c as u64 * transient as u64;
                assert!(
                    projected <= usable || c == 1,
                    "frame={frame_mb}MB count={count} budget={budget_gb}GB: concurrency {c} \
                     projects {}MB peak > {}MB usable",
                    projected / MIB,
                    usable / MIB,
                );
            }
        }
    }
}

/// Write a spatially-uniform 16-bit FITS frame (`value` in every pixel) to `path`.
fn write_const_fits(path: &std::path::Path, w: usize, h: usize, value: u16) {
    let image = Image::from_u16(vec![w, h], &vec![value; w * h]).expect("valid image");
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
    let dir = ScratchDirectory::new("lumos_mem_tier_test");
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
}
