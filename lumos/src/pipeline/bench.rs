use crate::internals::prelude::*;
use std::convert::Infallible;

use crate::concurrency;
use std::hint::black_box;
use std::iter;

use quickbench::quick_bench;
use rayon::prelude::*;

use crate::internals::synthetic::fixtures::star_field;
use crate::pipeline::detector_pool::DetectorPool;
use crate::star_detection::config::Config;
use crate::star_detection::detector::StarDetector;

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_detector_batch_reuse_1k(b: quickbench::Bencher) {
    let pixels = star_field(Size2us::new(1024, 1024), 100, 42)
        .image
        .channel(0)
        .clone();
    let image = LinearImage::from_pixels(ImageDimensions::new((1024, 1024), 1), pixels.into_vec());
    let images: Vec<&LinearImage> = iter::repeat_n(&image, 16).collect();
    let config = Config::default();
    let concurrency = rayon::current_num_threads().min(images.len());
    let mut detectors = DetectorPool::from_config(&config, concurrency).unwrap();

    b.bench_labeled("fresh", || {
        black_box(
            images
                .par_iter()
                .map(|image| {
                    StarDetector::from_config(config.clone())
                        .unwrap()
                        .detect(image)
                })
                .collect::<Vec<_>>(),
        )
    });
    b.bench_labeled("reused", || {
        black_box(
            concurrency::try_par_map_bounded(images.len(), detectors.slots(), |detector, index| {
                Ok::<_, Infallible>(detector.detect(images[index]))
            })
            .unwrap(),
        )
    });
}

/// The warp of a parked 2048² frame, cold — its planes evicted before each pass — straight from
/// its map against read back whole first, at 0°, 90° and 180°: a quarter turn reads one column of
/// the source per output row, a half turn its rows in reverse, against a map advised `Sequential`.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 2000)]
fn bench_parked_warp_2k(b: quickbench::Bencher) {
    use common::TempDir;

    use crate::frame_store::run_scratch::RunScratch;
    use crate::frame_store::stored_image::StoredImage;
    use crate::registration::registration_config::RegistrationConfig;
    use crate::registration::resample::WarpBuffers;
    use crate::registration::resample::source_image::{SourceImage, SourcePlane};
    use crate::registration::transform::{Transform, WarpTransform};

    let size = Size2us::new(2048, 2048);
    let image = star_field(size, 400, 7).image;
    let directory = TempDir::new("lumos_bench_parked_warp");
    let scratch = RunScratch::create(directory.path()).unwrap();
    let parked = StoredImage::spill(&scratch, &image).unwrap();
    let evict = || black_box(parked.evict());
    let params = RegistrationConfig::default().warp;
    let centre = DVec2::new(1023.5, 1023.5);
    let mut buffers = WarpBuffers::new(image.dimensions());
    for (label, angle) in [
        ("0°", 0.0),
        ("90°", std::f64::consts::FRAC_PI_2),
        ("180°", std::f64::consts::PI),
    ] {
        let rotation = Transform::similarity(DVec2::ZERO, angle, 1.0);
        let transform = Transform::translation(centre)
            .compose(&rotation)
            .compose(&Transform::translation(-centre - DVec2::new(0.25, 0.5)));
        let warp = WarpTransform::new(transform);
        b.bench_labeled(&format!("map {label}"), || {
            evict();
            let width = size.width;
            let flags = parked.flags();
            let source = SourceImage {
                dimensions: parked.dimensions,
                planes: parked
                    .planes()
                    .map(|pixels| SourcePlane { pixels, width })
                    .collect(),
                flags: flags.as_ref(),
            };
            buffers.warp_into(&source, &warp, params);
            black_box(&buffers);
        });
        b.bench_labeled(&format!("owned {label}"), || {
            evict();
            let resident = parked.load();
            buffers.warp_into(&SourceImage::of(&resident), &warp, params);
            black_box(&buffers);
        });
    }
}

/// Twelve dithered 2048² RGGB lights through `calibrate_align_stack`, spilled on a 256 MiB budget
/// with the reference found, so every light is parked between its detection and its warp: the
/// time and the bytes the process read from storage reported. Run alone under a cap that leaves
/// the parked lights no room to stay cached, as a spill meets them:
/// `systemd-run --user --scope -p MemoryMax=768M <test binary> --exact <this> --ignored
/// --nocapture`. Measured here: 5.9 s and nothing read uncapped, 7.0 s and 1560 MiB read at 768M;
/// a parked mosaic would save at most the 2/3 of the parked bytes a demosaiced light adds, about
/// 0.15–0.3 s, against a second demosaic of every light, about 0.24 s.
#[test]
#[ignore = "a manual measurement under a memory cgroup"]
fn parked_lights_under_a_memory_cap() {
    use std::fs;
    use std::path::PathBuf;
    use std::time::Instant;

    use common::TempDir;

    use crate::calibration_masters::CalibrationMasters;
    use crate::internals::cfa::make_cfa;
    use crate::io::image::cfa::CfaType;
    use crate::io::image::cfa::cfa_pattern::CfaPattern;
    use crate::io::image::fits::cfa::save_cfa_fits;
    use crate::pipeline::calibrate::calibrate_align_stack;
    use crate::pipeline::config::AlignStackConfig;
    use crate::progress::progress_callback::ProgressCallback;
    use crate::registration::registration_config::RegistrationConfig;
    use crate::registration::resample::warp;
    use crate::registration::transform::{Transform, WarpTransform};

    let read_bytes = || -> u64 {
        fs::read_to_string("/proc/self/io")
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("read_bytes: "))
            .unwrap()
            .parse()
            .unwrap()
    };
    let size = Size2us::new(2048, 2048);
    let base = star_field(size, 300, 77).image;
    let scratch = TempDir::new("lumos_parked_lights_cap");
    let paths: Vec<PathBuf> = (0..12u32)
        .map(|index| {
            let shift = DVec2::new(
                f64::from(index % 4) * 3.0 - 4.5,
                f64::from(index / 4) * 3.0 - 3.0,
            );
            let light = warp(
                &base,
                &WarpTransform::new(Transform::translation(shift)),
                RegistrationConfig::default().warp,
            )
            .image;
            let mosaic = make_cfa(
                size,
                light.channel(0).pixels().to_vec(),
                CfaType::Bayer(CfaPattern::Rggb),
            );
            let path = scratch.join(format!("light_{index}.fits"));
            save_cfa_fits(&path, &mosaic).unwrap();
            path
        })
        .collect();
    let mut config = AlignStackConfig::default();
    config.ingest.memory_override = Some(256 << 20);
    config.ingest.cache_dir = scratch.join("cache");
    let before = read_bytes();
    let started = Instant::now();
    let result = calibrate_align_stack(
        &paths,
        &CalibrationMasters::default(),
        &config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap();
    eprintln!(
        "parked {} lights, read {} MiB in {:?}",
        result.product.report.parked_lights,
        (read_bytes() - before) >> 20,
        started.elapsed()
    );
}
