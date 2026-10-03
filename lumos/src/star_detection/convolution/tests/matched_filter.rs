//! Matched-filter stage tests: the production filter on the residual of a rendered star field.
//!
//! The filter's output is in units of the input noise, so a star's response over the robust floor
//! of the filtered image is its detection SNR.

#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use crate::internals::prelude::*;
use crate::internals::synthetic::background_map;
use crate::internals::visual::{ToneMap, save};
use crate::math::statistics::MedianMad;
use crate::star_detection::config::background_config::BackgroundConfig;
use crate::star_detection::convolution::{MatchedFilterBuffers, matched_filter};
use crate::star_detection::tests::Scenario;

/// One rendered field, matched-filtered at `kernel_fwhm`: each true star's response, inside a
/// 10 px border, and the robust floor of the whole filtered image — stars are sparse, so its
/// median and MAD describe the star-free sky.
#[derive(Debug)]
struct Filtered {
    responses: Vec<f32>,
    floor: MedianMad,
}

impl Filtered {
    fn of(frame: &Scenario, kernel_fwhm: f32, name: Option<&str>) -> Self {
        let frame = frame.frame();
        let pixels = frame.image.channel(0).clone();
        let (width, height) = (pixels.width(), pixels.height());
        let residual =
            background_map::estimate(&pixels, &BackgroundConfig::default()).residual_of(&pixels);

        let mut output = Buffer2::new_default(width, height);
        let mut temp = Buffer2::new_default(width, height);
        matched_filter(
            &residual,
            kernel_fwhm,
            1.0,
            0.0,
            &mut MatchedFilterBuffers {
                output: &mut output,
                temp: &mut temp,
            },
        );
        if let Some(name) = name {
            save(
                output.pixels(),
                Size2us::new(width, height),
                &format!("synthetic_starfield/stage_conv_{name}_filtered"),
                ToneMap::AutoRange,
            );
        }

        let responses = frame
            .truth
            .sources
            .iter()
            .filter_map(|s| {
                let (x, y) = (s.pos.x.round() as usize, s.pos.y.round() as usize);
                (x > 10 && x < width - 10 && y > 10 && y < height - 10).then(|| output[(x, y)])
            })
            .collect();
        let floor = MedianMad::of_mut(&mut output.pixels().to_vec());
        Self { responses, floor }
    }

    /// A response's height over the floor, in the floor's σ.
    fn snr(&self, response: f32) -> f32 {
        (response - self.floor.median) / self.floor.sigma()
    }

    fn mean_snr(&self) -> f32 {
        let mean = self.responses.iter().sum::<f32>() / self.responses.len() as f32;
        self.snr(mean)
    }
}

/// Every star of a sparse field clears the filtered floor by 5σ.
#[test]
fn matched_filter_lifts_every_star() {
    let filtered = Filtered::of(
        &Scenario {
            num_stars: 25,
            fwhm: 3.5,
            ..Default::default()
        },
        3.5,
        Some("sparse"),
    );
    assert!(!filtered.responses.is_empty());
    let faintest = filtered
        .responses
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min);
    assert!(
        filtered.snr(faintest) > 5.0,
        "faintest star at {:.1}σ over the floor",
        filtered.snr(faintest)
    );
}

/// The kernel matched to the stars' 4 px FWHM gives a higher SNR than a narrower or a wider one —
/// the matched-filter property. (Raw response rises without bound as the kernel narrows; SNR does
/// not.)
#[test]
fn matched_kernel_maximises_snr() {
    let frame = Scenario {
        num_stars: 30,
        ..Default::default()
    };
    let snr = |kernel_fwhm: f32| Filtered::of(&frame, kernel_fwhm, None).mean_snr();
    let (narrow, matched, wide) = (snr(2.5), snr(4.0), snr(5.5));
    assert!(
        matched > narrow && matched > wide,
        "narrow {narrow:.2}, matched {matched:.2}, wide {wide:.2}"
    );
}

/// With a shallow well and a high read noise, matched-filtered stars stay well detectable.
#[test]
fn matched_filter_keeps_noisy_stars_detectable() {
    let filtered = Filtered::of(
        &Scenario {
            num_stars: 20,
            fwhm: 3.5,
            full_well_e: 4_000.0,
            read_noise_e: 150.0,
            ..Default::default()
        },
        3.5,
        Some("noise"),
    );
    assert!(
        filtered.mean_snr() > 3.0,
        "mean star SNR {:.1}",
        filtered.mean_snr()
    );
}
