use crate::calibration::calibration_pass::{CalibrationPass, Saturation, Subtracted};
use crate::image::cfa::cfa_type::CfaType;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::image::sample_domain::DomainMap;
use crate::internals::cfa::make_cfa;
use crate::internals::test_rng::TestRng;
use common::Size2us;

/// Each sample is `(L − B·g_b − o_b − D·g_d·k − o_d)/F`, rounded to f32 once: within half a
/// unit in its last place of that value, worked in f64 in another order, where three rounded
/// f32 steps would stray past it. Saturation is read from the raw sample, before it moves, and
/// the masters' flags are ORed in: the light saturated at pixels 0 and 7 (at or past the level
/// 0.95 of a span of 1) and imposed `NO_DATA` at 3 holds exactly those.
#[test]
fn a_light_is_its_f64_calibration_rounded_once() {
    let size = Size2us::new(16, 8);
    let mut rng = TestRng::new(5);
    let mut plane = |base: f32, spread: f32| -> Vec<f32> {
        (0..size.pixel_count())
            .map(|_| base + spread * rng.next_f32())
            .collect()
    };
    let mut light_samples = plane(0.3, 0.5);
    light_samples[0] = 0.95;
    light_samples[7] = 0.99;
    let bias = plane(0.01, 0.003);
    let dark = plane(0.004, 0.01);
    let flat = plane(0.6, 0.7);
    let bias_map = DomainMap {
        gain: 0.75,
        offset: -0.002,
    };
    let dark_map = DomainMap {
        gain: 1.25,
        offset: 0.0005,
    };
    let scale = 1.7;
    let imposed = PixelFlags::from_fn(size, |idx| {
        if idx == 3 {
            QualityFlags::NO_DATA
        } else {
            QualityFlags::default()
        }
    })
    .unwrap();
    let mut light = make_cfa(size, light_samples.clone(), CfaType::Mono);
    CalibrationPass {
        subtracted: [
            Subtracted {
                samples: &bias,
                map: bias_map,
                scale: 1.0,
            },
            Subtracted {
                samples: &dark,
                map: dark_map,
                scale,
            },
        ]
        .into_iter()
        .collect(),
        flat: Some(&flat),
        imposed: Some(&imposed),
        saturation: Saturation::Detect,
    }
    .apply(&mut light);

    let mut chain_strays = 0;
    for idx in 0..size.pixel_count() {
        let removed = (f64::from(bias[idx]) * bias_map.gain + bias_map.offset)
            + (f64::from(dark[idx]) * (dark_map.gain * scale) + dark_map.offset);
        let exact = (f64::from(light_samples[idx]) - removed) / f64::from(flat[idx]);
        let within_half_ulp = |value: f32| {
            let half_ulp = f64::from(value.abs()) * f64::from(f32::EPSILON) / 2.0;
            (f64::from(value) - exact).abs() <= half_ulp * (1.0 + 1e-9)
        };
        let value = light.data().pixels()[idx];
        assert!(
            within_half_ulp(value),
            "pixel {idx}: {value} against {exact}"
        );
        let chain = ((light_samples[idx]
            - (bias[idx] * bias_map.gain as f32 + bias_map.offset as f32))
            - (dark[idx] * (dark_map.gain * scale) as f32 + dark_map.offset as f32))
            / flat[idx];
        chain_strays += usize::from(!within_half_ulp(chain));
    }
    assert!(
        chain_strays > 0,
        "the f32 chain must stray for the test to tell"
    );
    let flags = light.flags().unwrap();
    let flagged: Vec<(usize, QualityFlags)> = (0..size.pixel_count())
        .map(|idx| (idx, flags.at(idx)))
        .filter(|(_, flags)| *flags != QualityFlags::default())
        .collect();
    assert_eq!(
        flagged,
        [
            (0, QualityFlags::SATURATED),
            (3, QualityFlags::NO_DATA),
            (7, QualityFlags::SATURATED)
        ]
    );
    assert!(light.metadata.derived.saturation_flagged());
}
