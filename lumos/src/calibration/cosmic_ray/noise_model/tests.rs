use imaginarium::Buffer2;

use crate::calibration::cosmic_ray::noise_model::*;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::masked_plane::MaskedPlane;
use crate::image::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
use crate::math::size2us::Size2us;

/// With a gain the source term is `(signal − sky)·g/electrons`: σ 0.5, sky 1, 4 electrons per
/// unit, signal 3 gives √(0.25 + 2/4) = √0.75 where no flat amplified the pixel, and
/// √(0.25 + 2·2/4) = √1.25 where a flat doubled it. Below the sky only σ counts. Without a gain
/// the slope is σ²/max(sky, σ) = 0.25, measured at the pixel's own flat: √(0.25 + 2·0.25) at
/// either gain. A σ of 0 is floored at one step at the sky, 2⁻²³.
#[test]
fn the_noise_adds_the_photons_above_the_local_sky() {
    let pixel = |noise: f32, flat_gain: f32| PixelBackground {
        local: LocalBackground { sky: 1.0, noise },
        flat_gain,
    };
    let gain = NoiseModel {
        electrons_per_unit: Some(4.0),
    };
    assert_eq!(gain.noise(3.0, pixel(0.5, 1.0)), 0.75f32.sqrt());
    assert_eq!(gain.noise(3.0, pixel(0.5, 2.0)), 1.25f32.sqrt());
    assert_eq!(gain.noise(0.5, pixel(0.5, 2.0)), 0.5);
    let measured = NoiseModel {
        electrons_per_unit: None,
    };
    for flat_gain in [1.0, 2.0] {
        assert_eq!(measured.noise(3.0, pixel(0.5, flat_gain)), 0.75f32.sqrt());
    }
    assert_eq!(gain.noise(1.0, pixel(0.0, 1.0)), f32::EPSILON);
}

/// A pixel's background is its colour's mesh value and its flat's gain there: a frame no flat
/// divided reads gain 1, one under a flat of 0.5 reads 2, the sky the mesh measured either way.
#[test]
fn a_pixel_reads_its_mesh_and_its_flat_gain() {
    let size = Size2us::new(16, 16);
    let mesh = ColourMesh::measure(
        MaskedPlane {
            pixels: &Buffer2::new(16, 16, vec![0.5; size.pixel_count()]),
            mask: None,
        },
        &CfaType::Mono,
        8,
    );
    let gain = FlatGain::of_divisor(
        &Buffer2::new(16, 16, vec![0.5; 256]),
        &CfaType::Mono,
        |_| false,
    );
    for (flat_gain, expected) in [(None, 1.0), (Some(&gain), 2.0)] {
        let pixel = PixelBackgrounds {
            mesh: &mesh,
            flat_gain,
        }
        .at(0, Vec2us::new(5, 9));
        assert_eq!(pixel.flat_gain, expected);
        assert_eq!(pixel.local.sky, 0.5);
    }
}

/// A stated gain of 1.5 e⁻/ADU over a declared 4095 ADU per unit is 6142.5 electrons per unit;
/// without a declared scale the quantization σ (1/√12)/4095 gives the same 4095 ADU, up to the
/// two roundings of its quotient; with neither, the gain has no unit to apply to. The measured
/// model reads the frame's own EGAIN.
#[test]
fn a_stated_gain_takes_its_unit_from_the_frame() {
    let estimation = NoiseEstimation::Gain {
        electrons_per_adu: 1.5,
    };
    let declared = ImageMetadata {
        domain: Some(SampleDomain {
            scale: 4095.0,
            origin: ScaleOrigin::Declared,
            pedestal: Pedestal::Removed,
            unit: None,
        }),
        ..Default::default()
    };
    assert_eq!(
        NoiseModel::resolve(&estimation, &declared)
            .unwrap()
            .electrons_per_unit,
        Some(6142.5)
    );
    let stepped = ImageMetadata {
        quantization_sigma: Some(QUANTIZATION_SIGMA_PER_STEP / 4095.0),
        ..Default::default()
    };
    assert_eq!(
        NoiseModel::resolve(&estimation, &stepped)
            .unwrap()
            .electrons_per_unit,
        Some(1.5 * (QUANTIZATION_SIGMA_PER_STEP / (QUANTIZATION_SIGMA_PER_STEP / 4095.0)))
    );
    assert_eq!(
        NoiseModel::resolve(&estimation, &ImageMetadata::default()),
        Err(CosmicRayError::UnknownAdcStep)
    );
    let egain = ImageMetadata {
        egain: Some(2.0),
        ..declared
    };
    assert_eq!(
        NoiseModel::resolve(&NoiseEstimation::Measured, &egain)
            .unwrap()
            .electrons_per_unit,
        Some(8190.0)
    );
}
