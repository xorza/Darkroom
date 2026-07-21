use std::convert::Infallible;
use std::hint::black_box;

use quickbench::quick_bench;
use rayon::prelude::*;

use crate::LinearImage;
use crate::io::image::ImageDimensions;
use crate::stacking::pipeline::detector_pool::DetectorPool;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::detector::StarDetector;
use crate::testing::synthetic::fixtures::star_field;

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_detector_batch_reuse_1k(b: quickbench::Bencher) {
    let pixels = star_field(1024, 1024, 100, 42).image.channel(0).clone();
    let image = LinearImage::from_pixels(ImageDimensions::new((1024, 1024), 1), pixels.into_vec());
    let images: Vec<&LinearImage> = std::iter::repeat_n(&image, 16).collect();
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
            detectors
                .try_map(&images, |detector, image| {
                    Ok::<_, Infallible>(detector.detect(image))
                })
                .unwrap(),
        )
    });
}
