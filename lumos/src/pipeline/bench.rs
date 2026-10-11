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
