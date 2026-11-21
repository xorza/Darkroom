use common::Buffer2;

use crate::calibration::cosmic_ray::noise_model::*;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::masked_plane::MaskedPlane;
use crate::image::metadata::calibration_state::CalibrationState;
use crate::image::metadata::derived_facts::DerivedFacts;
use crate::image::metadata::observation_header::ObservationHeader;
use crate::image::metadata::sample_domain::{SampleDomain, ScaleOrigin};

/// With a gain the source term is `(signal − sky)·g/electrons`: σ 0.5, sky 1, 4 electrons per
/// unit, signal 3 gives √(0.25 + 2/4) = √0.75 where no flat amplified the pixel, and
/// √(0.25 + 2·2/4) = √1.25 where a flat doubled it. Below the sky only σ counts. Without a gain
/// the slope is σ²/max(sky, σ) = 0.25, measured at the pixel's own flat: √(0.25 + 2·0.25) at
/// either gain. A pedestal of 0.5 under the sky holds no photons, so the sky above it is 0.5 and
/// the slope 0.5: √(0.25 + 2·0.5) = √1.25. A σ of 0 is floored at one step at the sky, 2⁻²³.
#[test]
fn the_noise_adds_the_photons_above_the_local_sky() {
    let pixel = |noise: f32, flat_gain: f32| PixelBg {
        local: LocalBg { sky: 1.0, noise },
        flat_gain,
    };
    let gain = NoiseModel::Gain {
        electrons_per_unit: 4.0,
    };
    assert_eq!(gain.noise(3.0, pixel(0.5, 1.0)), 0.75f32.sqrt());
    assert_eq!(gain.noise(3.0, pixel(0.5, 2.0)), 1.25f32.sqrt());
    assert_eq!(gain.noise(0.5, pixel(0.5, 2.0)), 0.5);
    let measured = NoiseModel::Sky { pedestal: 0.0 };
    for flat_gain in [1.0, 2.0] {
        assert_eq!(measured.noise(3.0, pixel(0.5, flat_gain)), 0.75f32.sqrt());
    }
    let kept = NoiseModel::Sky { pedestal: 0.5 };
    assert_eq!(kept.noise(3.0, pixel(0.5, 1.0)), 1.25f32.sqrt());
    assert_eq!(gain.noise(1.0, pixel(0.0, 1.0)), f32::EPSILON);
}

/// A pixel's background is its color's mesh value and its flat's gain there: a frame no flat
/// divided reads gain 1, one under a flat of 0.5 reads 2, the sky the mesh measured either way.
#[test]
fn a_pixel_reads_its_mesh_and_its_flat_gain() {
    let size = USize2::new(16, 16);
    let mesh = ColorMesh::measure(
        MaskedPlane {
            pixels: &Buffer2::new((16, 16), vec![0.5; size.pixel_count()]),
            mask: None,
        },
        &CfaType::Mono,
        8,
    );
    let gain = FlatGain::of_divisor(
        &Buffer2::new((16, 16), vec![0.5; 256]),
        &CfaType::Mono,
        |_| false,
    );
    for (flat_gain, expected) in [(None, 1.0), (Some(&gain), 2.0)] {
        let pixel = PixelBgs {
            mesh: &mesh,
            flat_gain,
        }
        .at(0, USizeVec2::new(5, 9));
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
        header: ObservationHeader::default(),
        derived: DerivedFacts::builder()
            .domain(SampleDomain {
                scale: 4095.0,
                origin: ScaleOrigin::Declared,
                pedestal: Pedestal::Removed,
                unit: None,
            })
            .build(),
    };
    assert_eq!(
        NoiseModel::resolve(&estimation, &declared).unwrap(),
        NoiseModel::Gain {
            electrons_per_unit: 6142.5
        }
    );
    let stepped = ImageMetadata {
        header: ObservationHeader::default(),
        derived: DerivedFacts::builder()
            .quantization_sigma(Some(QUANTIZATION_SIGMA_PER_STEP / 4095.0))
            .build(),
    };
    assert_eq!(
        NoiseModel::resolve(&estimation, &stepped).unwrap(),
        NoiseModel::Gain {
            electrons_per_unit: 1.5
                * (QUANTIZATION_SIGMA_PER_STEP / (QUANTIZATION_SIGMA_PER_STEP / 4095.0))
        }
    );
    assert_eq!(
        NoiseModel::resolve(&estimation, &ImageMetadata::default()),
        Err(CosmicRayError::UnknownAdcStep)
    );
    let egain = ImageMetadata {
        header: ObservationHeader {
            egain: Some(2.0),
            ..declared.header
        },
        derived: declared.derived,
    };
    assert_eq!(
        NoiseModel::resolve(&NoiseEstimation::Measured, &egain).unwrap(),
        NoiseModel::Gain {
            electrons_per_unit: 8190.0
        }
    );
}

/// Without a gain, the measured model stands the sky on the pedestal the values keep: 2048 ADU
/// over a scale of 4096 is 0.5 of a unit; a removed pedestal, a frame of no declared domain and an
/// unknown pedestal a bias took off stand it on 0; an unknown pedestal on an uncalibrated frame is
/// refused, as the sky above zero signal is not stated.
#[test]
fn the_measured_model_stands_the_sky_on_its_pedestal() {
    let with = |pedestal, bias| {
        let mut derived = DerivedFacts::builder()
            .domain(SampleDomain {
                scale: 4096.0,
                origin: ScaleOrigin::Declared,
                pedestal,
                unit: None,
            })
            .build();
        if bias {
            derived.set_calibration(CalibrationState::BIAS);
        }
        ImageMetadata {
            header: ObservationHeader::default(),
            derived,
        }
    };
    let resolve =
        |metadata: &ImageMetadata| NoiseModel::resolve(&NoiseEstimation::Measured, metadata);
    assert_eq!(
        resolve(&with(Pedestal::Kept(2048.0), false)),
        Ok(NoiseModel::Sky { pedestal: 0.5 })
    );
    for metadata in [
        with(Pedestal::Removed, false),
        with(Pedestal::Unknown, true),
        ImageMetadata::default(),
    ] {
        assert_eq!(resolve(&metadata), Ok(NoiseModel::Sky { pedestal: 0.0 }));
    }
    assert_eq!(
        resolve(&with(Pedestal::Unknown, false)),
        Err(CosmicRayError::UnknownPedestal)
    );
}
