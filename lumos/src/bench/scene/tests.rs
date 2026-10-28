use common::CancelToken;

use super::*;
use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::defect_map::DefectMap;
use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
use crate::image::cfa::bayer_pattern::BayerPattern;
use crate::image::cfa::mosaic::Mosaic;
use crate::internals::synthetic::camera::FlatField;
use crate::math::statistics;
use glam::USizeVec2;

/// 600 × 400 is a hundredth of the frame: 3000 / 100 = 30 stars, 300 / 100 = 3 sparse ones,
/// 20 000 / 100 = 200 in the cluster, 200 / 100 = 2 cosmic rays, and, rounded up,
/// ⌈240 / 100⌉ = 3 hot pixels and ⌈24 / 100⌉ = 1 dead one.
const SMALL: USize2 = USize2::new(600, 400);

const RGGB: CfaType = CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb));

#[test]
fn a_field_keeps_its_density_on_a_smaller_frame() {
    assert_eq!(at_density(COSMIC_RAYS, SMALL), 2);
    assert_eq!(at_density(Field::Typical.stars(), FRAME), 3000);
    assert_eq!(Field::Sparse.scene(SMALL).sources.len(), 3);
    assert_eq!(at_density(Field::Dense.stars(), FRAME), 20_000);
    assert_eq!(Field::Dense.scene(SMALL).sources.len(), 200);
    let SensorDefects { hot, dead } = defects(SMALL);
    assert_eq!((hot.len(), dead.len()), (3, 1));
    let frame = Field::Typical.render(SMALL);
    assert_eq!(frame.truth.sources.len(), 30);
    // The faintest star, of flux 0.5, peaks at 0.036 over the sky, 25σ of its 0.0014 noise.
    let mut tile = Field::Typical.tile(64).into_vec();
    assert_eq!(tile.len(), 64 * 64);
    let peak = tile.iter().copied().fold(f32::MIN, f32::max);
    assert!(
        peak - statistics::median_mut(&mut tile) > 10.0 * 0.0014,
        "no star in the tile"
    );
    // Exposure 0: sky (92 + 4·0) / 100 = 0.92, transparency 1. Exposure 1: 7 mod 5 = 2 and
    // 3 mod 4 = 3, so sky (92 + 8) / 100 = 1 and transparency (100 − 9) / 100 = 0.91.
    assert_eq!((exposure_sky(1), exposure_transparency(1)), (1.0, 0.91));
    assert_eq!((exposure_sky(0), exposure_transparency(0)), (0.92, 1.0));
    // The dither puts exposure 1's stars where its transform takes them.
    let dithered = Field::Typical.exposure(SMALL, 1);
    let placement = dither(SMALL, 1);
    assert_ne!(placement.apply(DVec2::ZERO), DVec2::ZERO);
    for (dithered, source) in dithered
        .truth
        .sources
        .iter()
        .zip(&Field::Typical.scene(SMALL).sources)
    {
        assert_eq!(dithered.pos, placement.apply(source.pos));
    }
    // The stacked master's sky medians stand as its sky levels, 1.2 : 1 : 0.8.
    let stacked = Field::Typical.stacked_rgb(SMALL);
    let sky =
        |channel: usize| statistics::median_mut(&mut stacked.channel(channel).pixels().to_vec());
    for (channel, level) in [(0, 1.2), (2, 0.8)] {
        let ratio = sky(channel) / sky(1);
        assert!(
            (ratio / level - 1.0).abs() < 0.01,
            "channel {channel}: {ratio}"
        );
    }
    let first = Field::Typical.aligned(SMALL, 0);
    let second = Field::Typical.aligned(SMALL, 1);
    for (first, second) in first.truth.sources.iter().zip(&second.truth.sources) {
        assert_eq!((first.pos, second.flux), (second.pos, first.flux * 0.91));
    }
    let level =
        |frame: &SimFrame| statistics::median_mut(&mut frame.image.channel(0).pixels().to_vec());
    let ratio = level(&second) / level(&first);
    assert!(
        (ratio / (1.0 / 0.92) - 1.0).abs() < 0.005,
        "sky ratio {ratio}"
    );
    assert_eq!(frame.image.channel(0).pixels().len(), 600 * 400);
    assert_eq!(frame.image.metadata.header.camera_white_balance, None);
    let mosaic = Field::Typical.mosaic(SMALL, RGGB);
    assert_eq!(
        mosaic.metadata.header.camera_white_balance,
        Some(WHITE_BALANCE)
    );
    // A calibrated exposure carries the sequence's sky on the calibrated sensor: exposure 1's
    // sky is the field's, (92 + 4·2) / 100 = 1, and exposure 2's (92 + 4·(14 mod 5)) / 100 =
    // 1.08 of it. The medians, the sky with no bias under them, stand in the same ratios.
    let median = |image: &CfaImage| statistics::median_mut(&mut image.data().pixels().to_vec());
    let field_sky = median(&mosaic);
    for (idx, level) in [(1, 1.0), (2, 1.08)] {
        let exposure = Field::Typical.mosaic_exposure(SMALL, RGGB, idx);
        assert_eq!(
            exposure.metadata.header.camera_white_balance,
            Some(WHITE_BALANCE)
        );
        let ratio = median(&exposure) / field_sky;
        assert!(
            (ratio / level - 1.0).abs() < 0.005,
            "exposure {idx}: sky ratio {ratio}"
        );
    }
}

/// The masters carry the sensor's defects: against the dark's noise, σ ≈ 8·10⁻⁵ from 3 e⁻ of
/// read noise and 6 e⁻ of dark current in a 50 ke⁻ well, the least hot excess, 0.05, is over
/// 600σ, and a dead pixel reads nothing under the flat once its bias is out. The map finds
/// exactly the sensor's hot pixels in the dark and its dead pixel in the flat.
#[test]
fn the_masters_find_the_sensors_defects() {
    let CalibrationSet {
        bias, dark, flat, ..
    } = masters(SMALL, RGGB);
    let (bias, dark, mut flat) = (bias.unwrap(), dark.unwrap(), flat.unwrap());
    for (flat, bias) in flat
        .data_mut()
        .pixels_mut()
        .iter_mut()
        .zip(bias.data().pixels())
    {
        *flat -= bias;
    }
    let map = DefectMap::new(SMALL)
        .detect_hot(&dark, SigmaThreshold::DEFAULT, &CancelToken::never())
        .unwrap()
        .detect_cold(&flat, &CancelToken::never())
        .unwrap();
    let SensorDefects { hot, dead } = defects(SMALL);
    let idx = |x: usize, y: usize| SMALL.idx_of(USizeVec2::new(x, y));
    let mut hot: Vec<usize> = hot.into_iter().map(|(x, y, _)| idx(x, y)).collect();
    hot.sort_unstable();
    assert_eq!(map.hot().idxs, hot);
    let dead: Vec<usize> = dead.into_iter().map(|(x, y)| idx(x, y)).collect();
    assert_eq!(map.cold_idxs(), dead);
}

/// Calibrating a light with its masters takes out the bias and the vignette: each color of it
/// is then that color of the calibrated mosaic times the vignette's mean over the color's
/// photosites, the level the flat is normalized to. The medians of 60 000 samples a color
/// agree to far within 0.5%, where a bias of 0.03 left on a sky of 0.1 · 1/2.13 would put the
/// red one 64% off.
#[test]
fn a_light_calibrated_by_its_masters_is_the_calibrated_mosaic() {
    let masters = CalibrationMasters::from_images(
        masters(SMALL, RGGB),
        SigmaThreshold::DEFAULT,
        &CancelToken::never(),
    )
    .unwrap();
    let mut light = Field::Typical.light(SMALL, RGGB);
    masters.calibrate(&mut light).unwrap();
    let mosaic = Field::Typical.mosaic(SMALL, RGGB);
    let vignette = FlatField {
        vignette: Some(VIGNETTE),
        ..FlatField::default()
    }
    .render(SMALL, &CfaType::Mono);
    for color in 0..3 {
        let of_color = |pixels: &[f32]| -> Vec<f32> {
            pixels
                .iter()
                .enumerate()
                .filter(|&(idx, _)| RGGB.slot_at(SMALL.point_of(idx)) == color)
                .map(|(_, &value)| value)
                .collect()
        };
        let falloff = of_color(&vignette)
            .iter()
            .map(|&v| f64::from(v))
            .sum::<f64>()
            / of_color(&vignette).len() as f64;
        let calibrated = f64::from(statistics::median_mut(&mut of_color(light.data().pixels())));
        let expected = falloff
            * f64::from(statistics::median_mut(&mut of_color(
                mosaic.data().pixels(),
            )));
        assert!(
            (calibrated / expected - 1.0).abs() < 0.005,
            "color {color}: {calibrated} against {expected}"
        );
    }
}
