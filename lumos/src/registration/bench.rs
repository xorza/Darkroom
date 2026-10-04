//! Benchmarks for the registration *solve* — triangle matching → RANSAC → optional
//! SIP — on synthetic star fields. The image warp it feeds is benched separately in
//! `resample::bench`; real-data register/warp timing lives in `registration::tests::real_data`.
//!
//! Run: `cargo test -p lumos --release --features bench registration::bench -- --ignored
//! --nocapture`

use crate::internals::prelude::*;
use quickbench::quick_bench;
use std::hint::black_box;

use crate::internals::synthetic::fixtures::star_field;
use crate::registration::transform::Transform;
use crate::star_detection::config::Config as StarDetectionConfig;
use crate::star_detection::detector::StarDetector;
use crate::star_detection::star::Star;
use crate::{RegistrationConfig, register};

/// A reference catalog and a transformed copy of it.
#[derive(Debug)]
struct StarPair {
    reference: Vec<Star>,
    target: Vec<Star>,
}

/// Detect a realistic star set on a synthetic field, then build a registration target by
/// applying a known similarity transform to those stars — a clean, deterministic correspondence
/// set that still drives the full matching + RANSAC machinery over `num_stars` points.
fn star_pair(num_stars: usize, seed: u64) -> StarPair {
    let frame = star_field(Size2us::new(1500, 1500), num_stars, seed);
    let mut detector = StarDetector::from_config(StarDetectionConfig::default()).unwrap();
    let ref_stars = detector.detect(&frame.image).stars;
    // A modest rotation + scale + shift, the kind dithered subs differ by.
    let t = Transform::similarity(DVec2::new(11.0, -7.0), 0.03, 1.002);
    let target = ref_stars
        .iter()
        .map(|s| {
            let mut moved = *s;
            moved.pos = t.apply(s.pos);
            moved
        })
        .collect();
    StarPair {
        reference: ref_stars,
        target,
    }
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_register_150_stars(b: ::quickbench::Bencher) {
    let StarPair {
        reference: ref_stars,
        target,
    } = star_pair(150, 7);
    let config = RegistrationConfig::default();
    b.bench(|| black_box(register(black_box(&ref_stars), black_box(&target), &config)));
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_register_500_stars(b: ::quickbench::Bencher) {
    let StarPair {
        reference: ref_stars,
        target,
    } = star_pair(500, 9);
    let config = RegistrationConfig::default();
    b.bench(|| black_box(register(black_box(&ref_stars), black_box(&target), &config)));
}

/// Benchmarks on the dataset's RAW lights.
#[cfg(feature = "real-data")]
mod real_data {
    use std::hint::black_box;
    use std::path::PathBuf;

    use ::quickbench::quick_bench;
    use common::TempDir;

    use crate::internals::real_data::{LightPair, first_and_last_lights, raw_frames, raw_light};
    use crate::io::image::linear::LinearImage;
    use crate::registration::register;
    use crate::registration::registration_config::RegistrationConfig;
    use crate::registration::resample::warp;
    use crate::star_detection::config::Config;
    use crate::star_detection::detector::StarDetector;

    #[quick_bench(warmup_iters = 0, iters = 1)]
    fn bench_register_and_warp_all(b: ::quickbench::Bencher) {
        let paths = raw_frames("Lights");
        let images: Vec<LinearImage> = paths.iter().map(|path| raw_light(path)).collect();
        // The warped frames go to a fresh directory, never into the dataset.
        let output_dir = TempDir::new("lumos-registered-lights");
        println!(
            "Loaded {} lights; writing under {}",
            images.len(),
            output_dir.path().display()
        );

        b.bench(|| {
            let star_config = Config::precise_ground();
            let mut detector = StarDetector::from_config(star_config).unwrap();

            // Detect stars in all frames
            let detections: Vec<_> = images.iter().map(|img| detector.detect(img)).collect();

            let reg_config = RegistrationConfig::default();
            let ref_stars = &detections[0].stars;

            println!(
                "Reference: {:?} ({} stars)",
                paths[0].file_name().unwrap(),
                ref_stars.len()
            );

            // Save reference frame as-is
            let tiff_name =
                |path: &PathBuf| format!("{}.tiff", path.file_stem().unwrap().to_string_lossy());
            let ref_output = output_dir.join(tiff_name(&paths[0]));
            images[0]
                .save(&ref_output)
                .expect("Failed to save reference frame");

            // Register and warp each subsequent frame
            for i in 1..images.len() {
                let name = paths[i].file_name().unwrap();
                let target_stars = &detections[i].stars;

                let result = match register(ref_stars, target_stars, &reg_config) {
                    Ok(r) => r,
                    Err(e) => {
                        println!("  {name:?}: FAILED ({e:?}), skipping");
                        continue;
                    }
                };

                println!(
                    "  {:?}: {} inliers, RMS {:.4} px, {:.1} ms",
                    name,
                    result.num_inliers(),
                    result.rms_error(),
                    result.elapsed_ms(),
                );

                let warped = warp(&images[i], &result.warp_transform(), reg_config.warp).image;

                let output_path = output_dir.join(tiff_name(&paths[i]));
                warped
                    .save(&output_path)
                    .expect("Failed to save warped frame");
            }

            println!(
                "Saved {} registered frames to {:?}",
                images.len(),
                output_dir
            );
        });
    }

    #[quick_bench(warmup_iters = 3, iters = 30)]
    fn bench_register_stars(b: ::quickbench::Bencher) {
        let LightPair {
            first: img1,
            last: img2,
        } = first_and_last_lights();

        // Pre-detect stars (not part of the benchmark)
        let star_config = Config::default();
        let mut detector = StarDetector::from_config(star_config).unwrap();
        let result1 = detector.detect(&img1);
        let result2 = detector.detect(&img2);

        let reg_config = RegistrationConfig::default();

        b.bench(|| {
            black_box(register(
                black_box(&result1.stars),
                black_box(&result2.stars),
                &reg_config,
            ))
        });
    }
}
