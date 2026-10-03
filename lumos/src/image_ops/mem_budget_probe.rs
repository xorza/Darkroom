//! Live peak-RSS memory probe for the image-op chain.
//!
//! What it watches: a planar RGB master driven in place through the three ops that allocate
//! image-sized scratch — `ExtractBackground`, `Denoise` (a three-plane wavelet workspace) and
//! `Stretch` (a capped subsample). The master's planes stay resident throughout and each op
//! releases its working set before the next one starts, so the expected peak is *the master + the
//! widest op working set*, flat in the op count.
//!
//! Why it exists: three image-sized allocations is a plausible shape for this chain and 3× a
//! full-frame master is a lot of RAM, so the arrangement wants a measurement rather than an
//! argument: a working set held across ops, or one op allocating a second master-sized copy,
//! shows here and nowhere else.
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

use crate::internals::mem_probe::{MB, RssSampler, env_parse, measured};
use crate::internals::synthetic::patterns;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::{Denoise, ExtractBackground, Stretch};

/// The widest single op working set, in image-sized f32 planes: `Denoise`'s wavelet workspace
/// (`c_curr`, `c_next`, `tmp`). `ExtractBackground`'s mesh is tile-resolution and `Stretch`'s
/// subsample is capped at a million samples, so neither comes near it.
const WORKING_PLANES: u64 = 3;

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
        "\nimage-op chain probe: {}x{} RGB f32 master ({} MB)",
        dimensions.width(),
        dimensions.height(),
        master_bytes / MB,
    );

    let mut image = patterns::linear_rgb_master(dimensions);

    let sampler = RssSampler::start();
    let chain_gate = sampler.gate();
    #[expect(
        clippy::unused_result_ok,
        reason = "a progress line that fails to flush costs the probe nothing"
    )]
    io::stdout().flush().ok();

    chain_gate.open();
    let started = Instant::now();
    ExtractBackground::default().apply(&mut image).unwrap();
    Denoise::default().apply(&mut image).unwrap();
    Stretch::auto_asinh().apply(&mut image).unwrap();
    let elapsed = started.elapsed();

    let peak = sampler.finish();
    let anon_mb = peak.anon;
    println!("chain elapsed {elapsed:?}");
    println!("peak RssAnon  {anon_mb} MB   (heap — the OOM-relevant figure)");
    println!("  during ops  {} MB", peak.gated_anon);
    println!("peak VmRSS    {} MB   (total resident)", peak.total);
    println!(
        "peak / master {:.2}x",
        anon_mb as f64 / (master_bytes / MB) as f64
    );

    // The planes are resident for the whole chain and the widest thing beside them is `Denoise`'s
    // three single-channel planes, one master-sized unit for an RGB master — so 2 master-sized
    // units is the structural expectation. Headroom is only 25% rather than the 2x the
    // frame-pipeline probes use: those size a ceiling around a tiering decision that legitimately
    // varies, whereas this chain's allocations are a handful of image-sized `Vec`s that glibc
    // serves straight from mmap. 2x headroom here would sit above 3 resident masters and so would
    // wave through exactly the arrangement this probe exists to catch.
    let working_bytes = WORKING_PLANES * master_bytes / 3;
    let ceiling_mb = 5 * (master_bytes + working_bytes) / 4 / MB;
    if measured(anon_mb, "ceiling check") {
        assert!(
            anon_mb <= ceiling_mb,
            "peak heap {anon_mb} MB exceeded the {ceiling_mb} MB ceiling \
             (master {} MB + {WORKING_PLANES} working planes, 25% headroom)",
            master_bytes / MB,
        );
        println!("ceiling check OK: peak heap {anon_mb} MB <= {ceiling_mb} MB");
    }
}
