//! The dark's scale, fitted per light on the defect map's hot pixels.

use common::CancelToken;

use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::calibration_set::CalibrationSet;
use crate::calibration::dark_scale::{DarkScale, DarkScaleSource};
use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
use crate::image::calibration_state::CalibrationState;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::internals::cfa::{constant_cfa, make_cfa};
use crate::internals::test_rng::TestRng;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

const SIZE: Size2us = Size2us::new(256, 256);
const BIAS: f32 = 0.0625;
/// The dark current every pixel has, which the ring's plane takes out of both excesses.
const DARK_CURRENT: f32 = 0.004;
/// The light's read and shot noise, Gaussian.
const LIGHT_NOISE: f32 = 0.002;

/// A master dark and its hot pixels: each one's position and true excess.
#[derive(Debug)]
struct HotDark {
    dark: CfaImage,
    hot: Vec<(Vec2us, f32)>,
}

/// A bias-removed master dark of 300 s at −10 °C, `count` hot pixels on a 16-px grid 8 px in from
/// every edge, so each has its whole ring, with excesses evenly from 0.02 to 0.32. The last
/// `saturated` read 0.002 short of it, flagged saturated: a level the sensor clipped just below the
/// excess.
fn dark(count: usize, saturated: usize) -> HotDark {
    let hot: Vec<(Vec2us, f32)> = (0..count)
        .map(|k| {
            let position = Vec2us::new(8 + 16 * (k % 16), 8 + 16 * (k / 16));
            (position, 0.02 + 0.3 * k as f32 / 255.0)
        })
        .collect();
    let mut pixels = vec![DARK_CURRENT; SIZE.pixel_count()];
    for &(position, excess) in &hot {
        pixels[SIZE.index_of(position)] += excess;
    }
    let clipped: Vec<usize> = hot[count - saturated..]
        .iter()
        .map(|&(position, _)| SIZE.index_of(position))
        .collect();
    for &index in &clipped {
        pixels[index] -= 0.002;
    }
    let mut dark = make_cfa(SIZE, pixels, CfaType::Mono);
    dark.set_flags(PixelFlags::from_fn(SIZE, |index| {
        if clipped.contains(&index) {
            QualityFlags::SATURATED
        } else {
            QualityFlags::default()
        }
    }));
    dark.metadata.header.exposure_time = Some(300.0);
    dark.metadata.header.ccd_temp = Some(-10.0);
    dark.metadata
        .derived
        .set_calibration(CalibrationState::BIAS);
    HotDark { dark, hot }
}

/// A light of `exposure` s at `temperature` °C whose dark signal is `scale` times the dark's.
#[derive(Debug, Clone, Copy)]
struct Light {
    scale: f32,
    /// How many of the hot pixels, from the first, a star sits on.
    starred: usize,
    exposure: f64,
    temperature: f64,
}

impl Light {
    /// The light over the dark's `hot` pixels, on a sky that slopes along both axes, a star of
    /// amplitude 0.2 and σ 1 px on each of the first `starred` of them, and Gaussian noise.
    fn render(self, hot: &[(Vec2us, f32)]) -> CfaImage {
        let Self {
            scale,
            starred,
            exposure,
            temperature,
        } = self;
        let mut rng = TestRng::new(27);
        let mut pixels: Vec<f32> = (0..SIZE.pixel_count())
            .map(|index| {
                let point = SIZE.point_of(index);
                let sky = 0.1 + 2e-4 * point.x as f32 + 1e-4 * point.y as f32;
                BIAS + scale * DARK_CURRENT + sky + LIGHT_NOISE * rng.next_gaussian_f32()
            })
            .collect();
        for &(position, excess) in hot {
            pixels[SIZE.index_of(position)] += scale * excess;
        }
        for &(centre, _) in &hot[..starred] {
            for dy in -3i32..=3 {
                for dx in -3i32..=3 {
                    let at = Vec2us::new(
                        centre.x.wrapping_add_signed(dx as isize),
                        centre.y.wrapping_add_signed(dy as isize),
                    );
                    pixels[SIZE.index_of(at)] += 0.2 * (-((dx * dx + dy * dy) as f32) / 2.0).exp();
                }
            }
        }
        let mut light = make_cfa(SIZE, pixels, CfaType::Mono);
        light.metadata.header.exposure_time = Some(exposure);
        light.metadata.header.ccd_temp = Some(temperature);
        light
    }
}

fn masters(dark: CfaImage) -> CalibrationMasters {
    CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(dark),
            bias: Some(constant_cfa(SIZE, BIAS, CfaType::Mono)),
            ..CalibrationSet::default()
        },
        SigmaThreshold::new(5.0).unwrap(),
        &CancelToken::never(),
    )
    .unwrap()
}

/// 256 hot pixels, 10 under stars, and the 5 brightest read 0.002 short in the dark and flagged
/// saturated there, which the masters impose on the light as no measurement, so the fit leaves
/// them out: their residuals, `0.002·k` ≈ 0.003, are half the clip's 3σ ≈ 0.006, so only the
/// flags keep them out. The fit recovers the scale the light's dark signal was made
/// with — 1.37 at the dark's own temperature, and 1.42 for a light 3 °C warmer, the kind of
/// change a temperature makes — whatever the exposures say, which agree. The 10 starred pixels
/// stand 0.2 above the line, a hundred times the noise, and are clipped: at most 241 are kept, and
/// of those a 3σ clip of Gaussian residuals takes about 0.27%, 0.7 on average. A starred pixel
/// kept would pull the slope by `Σ d·0.2 / Σd²` ≈ 10 · 0.025 · 0.2 / 9.1 ≈ 0.0055, past the bound
/// below, so the bound is what shows them clipped.
///
/// The light's excess at a hot pixel is its noise plus the noise of its ring's plane, an intercept
/// over 168 photosites: `σ·√(1 + 1/168)`. Through the origin the slope's error is that over `√Σd²`,
/// so the fitted scale lies within 3 of those of the true one, and the error the fit reports, from
/// the MAD of about 240 residuals, within 20% of it: the MAD's own spread at that count is about
/// 6%.
#[test]
fn the_fit_recovers_the_scale_of_the_lights_dark_signal() {
    let HotDark { dark, hot } = dark(256, 5);
    let masters = masters(dark);
    for (scale, temperature) in [(1.37f32, -10.0), (1.42, -7.0)] {
        let mut light = Light {
            scale,
            starred: 10,
            exposure: 300.0,
            temperature,
        }
        .render(&hot);
        let outcome = masters.calibrate(&mut light).unwrap();
        let Some(DarkScale {
            factor,
            source:
                DarkScaleSource::Fitted {
                    pixels,
                    standard_error,
                },
        }) = outcome.dark_scale
        else {
            panic!("expected a fitted scale, got {:?}", outcome.dark_scale);
        };
        assert!((235..=241).contains(&pixels), "{scale}: {pixels} kept");
        let energy: f64 = hot[10..251]
            .iter()
            .map(|&(_, excess)| f64::from(excess).powi(2))
            .sum();
        let derived = f64::from(LIGHT_NOISE) * (1.0 + 1.0 / 168.0f64).sqrt() / energy.sqrt();
        assert!(
            (factor - f64::from(scale)).abs() <= 3.0 * derived,
            "{factor} against {scale} ± 3·{derived}"
        );
        assert!(
            (0.8..=1.2).contains(&(standard_error / derived)),
            "{standard_error} against {derived}"
        );
        assert_eq!(
            outcome
                .dark
                .and_then(|dark| dark.temperature_difference)
                .is_some(),
            temperature != -10.0
        );
    }
}

/// With 50 hot pixels, fewer than the 100 a fit needs, the dark is scaled by the exposures — 300 s
/// over 300 s, then 300 s over 120 s — and the outcome says how many pixels the fit had.
#[test]
fn too_few_hot_pixels_fall_back_to_the_exposures() {
    let HotDark { dark, hot } = dark(50, 0);
    let masters = masters(dark);
    for (exposure, ratio) in [(300.0, 1.0), (750.0, 2.5)] {
        let mut light = Light {
            scale: 1.0,
            starred: 0,
            exposure,
            temperature: -10.0,
        }
        .render(&hot);
        assert_eq!(
            masters.calibrate(&mut light).unwrap().dark_scale,
            Some(DarkScale {
                factor: ratio,
                source: DarkScaleSource::Exposure { fit_pixels: 50 },
            })
        );
    }
}
