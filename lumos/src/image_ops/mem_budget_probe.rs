//! Live peak-RSS memory probe for the image-op chain, and the baseline instrument for the
//! interleaved → planar migration.
//!
//! What it watches: an op family entered once as an interleaved [`Image`] and driven through the
//! three ops that allocate image-sized scratch — `ExtractBackground` (one plane via
//! `process_channels`), `Denoise` (one plane via `process_channel_samples` plus a three-plane
//! wavelet workspace), and `Stretch` (a capped subsample). Peak heap is therefore expected at
//! roughly *master + one denoise working set*, and must stay flat in the op count: each op releases
//! its scratch before the next runs, so a chain of ten would peak no higher than a chain of three.
//!
//! Why it exists now: the planar migration replaces the per-op deinterleave with one at the family
//! boundary, and the naive write-back (`*image = Image::from(&planar)`) holds the interleaved
//! master, the planar copy, *and* the new interleaved buffer at once — 3× the master. This probe is
//! what makes that regression visible instead of a review argument.
//!
//! `#[ignore]`d because peak RSS is a per-process high-water mark, so run one config per process
//! with a filter:
//! ```sh
//! cargo test -p lumos --release image_ops_memory_probe -- --ignored --nocapture
//! ```
//!
//! Self-contained: renders its own synthetic RGB master, so it needs neither the `real-data`
//! dataset nor libraw.
//!
//! ```text
//! LUMOS_OPS_W   master width  in px   (default 6032 — the bundled stacked master)
//! LUMOS_OPS_H   master height in px   (default 4028)
//! ```
//!
//! Heap is read from `/proc/self/status` (`RssAnon`), so the numeric ceiling is only enforced on
//! Linux; elsewhere the chain still runs and the assertion is skipped.

use std::io::{self, Write};
use std::time::Instant;

use imaginarium::Image;

use crate::io::image::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::testing::mem_probe::{MB, RssSampler, env_parse, measured};
use crate::{Denoise, ExtractBackground, Stretch};

/// Image-sized f32 planes the chain holds beyond the master itself, in master-sized units of ⅓:
/// `Denoise`'s scratch plane plus its three-plane wavelet workspace (`c_curr`, `c_next`, `tmp`),
/// which is the widest point. `ExtractBackground`'s single plane has been released by then, and
/// `Stretch`'s subsample is capped at a million samples.
const WORKING_PLANES: u64 = 4;

/// A synthetic linear RGB master: sky gradient plus a hashed dither, with a sparse set of bright
/// cores above 1.0 as a real stack has. The content only has to be representative enough that no op
/// short-circuits — this probe measures allocation, not numerics.
fn linear_master(dimensions: ImageDimensions) -> LinearImage {
    let (width, height) = (dimensions.width(), dimensions.height());
    let count = width * height;
    let mut channels = [
        vec![0.0f32; count],
        vec![0.0f32; count],
        vec![0.0f32; count],
    ];
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let sky = 0.02 + (y as f32 / height as f32) * 0.03;
            let hash = (index as u32).wrapping_mul(2654435761) as f32 / u32::MAX as f32;
            let noise = (hash - 0.5) * 0.004;
            // Every 9973rd pixel (a prime, so the cores don't align to a row) is a star core.
            let core = if index % 9973 == 0 { 1.5 } else { 0.0 };
            for (channel, plane) in channels.iter_mut().enumerate() {
                plane[index] = sky * (1.0 - 0.1 * channel as f32) + noise + core;
            }
        }
    }
    LinearImage::from_planar_channels(dimensions, channels)
}

#[test]
#[ignore = "manual live peak-RSS probe; run explicitly with a filter, one config per process"]
fn image_ops_memory_probe() {
    let dimensions = ImageDimensions::new(
        (
            env_parse("LUMOS_OPS_W", 6032),
            env_parse("LUMOS_OPS_H", 4028),
        ),
        3,
    );
    let master_bytes = (dimensions.sample_count() * size_of::<f32>()) as u64;
    println!(
        "\nimage-op chain probe: {}x{} RGB f32 master ({} MB interleaved)",
        dimensions.width(),
        dimensions.height(),
        master_bytes / MB,
    );

    // Build planar, convert once, and drop the planar copy before sampling opens — the probe
    // measures the op chain's own high-water mark, not the fixture's.
    let mut image = Image::from(&linear_master(dimensions));

    let sampler = RssSampler::start();
    let chain_gate = sampler.gate();
    io::stdout().flush().ok();

    chain_gate.open();
    let started = Instant::now();
    ExtractBackground::default().apply(&mut image).unwrap();
    Denoise::default().apply(&mut image).unwrap();
    Stretch::auto_asinh().apply(&mut image).unwrap();
    let elapsed = started.elapsed();

    let peak = sampler.finish();
    let anon_mb = peak.anon_mb;
    println!("chain elapsed {elapsed:?}");
    println!("peak RssAnon  {anon_mb} MB   (heap — the OOM-relevant figure)");
    println!("  during ops  {} MB", peak.gated_anon_mb);
    println!("peak VmRSS    {} MB   (total resident)", peak.total_mb);
    println!(
        "peak / master {:.2}x",
        anon_mb as f64 / (master_bytes / MB) as f64
    );

    // The master stays resident for the whole chain and the widest op adds `WORKING_PLANES` of the
    // three it holds, so 1 + 4/3 master-sized units is the structural expectation — which the
    // measurement matches at 2.36x. Headroom is only 25% rather than the 2x the frame-pipeline
    // probes use: those size a ceiling around a tiering decision that legitimately varies, whereas
    // this chain's allocations are a handful of image-sized `Vec`s that glibc serves straight from
    // mmap, and it reproduces to within 1 MB. 2x here would sit above 3x the master and so would
    // wave through the boundary write-back this probe exists to catch.
    let ceiling_mb = 5 * (master_bytes + WORKING_PLANES * master_bytes / 3) / 4 / MB;
    if measured(anon_mb, "ceiling check") {
        assert!(
            anon_mb <= ceiling_mb,
            "peak heap {anon_mb} MB exceeded the {ceiling_mb} MB ceiling \
             (master {} MB + {WORKING_PLANES} working planes, 2x headroom)",
            master_bytes / MB,
        );
        println!("ceiling check OK: peak heap {anon_mb} MB <= {ceiling_mb} MB");
    }
}
