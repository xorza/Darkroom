use crate::internals::cfa::XTRANS_PATTERN;
use crate::internals::prelude::*;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};

use crate::CfaType;
use crate::calibration_masters::CalibrationMasters;
use crate::calibration_masters::prepared_flat::{MIN_NORMALIZED_FLAT, PreparedFlat};
use crate::internals::assertions::bits;
use crate::internals::cfa::make_cfa;
use crate::io::image::cfa::CfaImage;
use crate::io::image::sample_domain::DomainMap;
use crate::io::raw::demosaic::bayer::CfaPattern;

fn prepare(mut flat: CfaImage, subtractor: Option<&CfaImage>) -> PreparedFlat {
    if let Some(subtractor) = subtractor {
        flat.subtract(subtractor, DomainMap::IDENTITY);
    }
    PreparedFlat::new(flat).unwrap()
}

/// Calibrate `light` by `prepared` alone, its saturation taken as flagged.
fn divide(prepared: PreparedFlat, light: &mut CfaImage) {
    light.metadata.saturation_flagged = true;
    CalibrationMasters::assemble(None, None, Some(prepared), None)
        .unwrap()
        .calibrate(light)
        .unwrap();
}

fn standard_xtrans() -> CfaType {
    CfaType::XTrans(XTRANS_PATTERN)
}

#[test]
fn prepared_flat_matches_hand_computed_mono_calibration() {
    let flat = make_cfa(Size2us::new(2, 2), vec![0.0, 1.0, 1.0, 2.0], CfaType::Mono);
    let prepared = prepare(flat, None);
    assert_eq!(
        prepared.divisor().data.pixels(),
        &[MIN_NORMALIZED_FLAT, 1.0, 1.0, 2.0]
    );
    assert_eq!(prepared.floored(), 1);

    let mut light = make_cfa(Size2us::new(2, 2), vec![1.0; 4], CfaType::Mono);
    divide(prepared, &mut light);
    assert_eq!(light.data.pixels(), &[10.0, 1.0, 1.0, 0.5]);
    // The light carries the flat's gain for its noise: the one node over the 2×2 frame is the mean
    // gain of the photosites the floor left alone, (1 + 1 + ½)/3 = 5/6; the floored photosite's
    // 10 would raise it to 3.125.
    let gain = light.metadata.flat_gain.as_ref().unwrap();
    assert_eq!(gain.at(0, 0.0, 0.0), (5.0f64 / 6.0) as f32);
    // The floored divisor is flagged, and only it: that pixel is corrected by less than its flat
    // asked for.
    let flags = light.flags.as_ref().unwrap();
    assert_eq!(flags.count(QualityFlags::FLAT_FLOOR), 1);
    assert_eq!(flags.at(0), QualityFlags::FLAT_FLOOR);
}

/// A flat's mean leaves out the photosites that hold no measurement, and the light holds none
/// where the flat does not. A flat of 1, 3, a saturated 5 and a 9 with no data has the mean
/// (1 + 3)/2 = 2, not 18/4 = 4.5: divisors 0.5, 1.5, 2.5 and 4.5, so a light of 1 reads 2 at the
/// first photosite, and `NO_DATA` at the last two, which calibration then repairs.
#[test]
fn a_flats_mean_leaves_out_what_it_did_not_measure() {
    let size = Size2us::new(2, 2);
    let mut flat = make_cfa(size, vec![1.0, 3.0, 5.0, 9.0], CfaType::Mono);
    flat.flags = PixelFlags::from_fn(size, |index| match index {
        2 => QualityFlags::SATURATED,
        3 => QualityFlags::NO_DATA,
        _ => QualityFlags::default(),
    });
    let prepared = prepare(flat, None);
    assert_eq!(prepared.divisor().data.pixels(), &[0.5, 1.5, 2.5, 4.5]);

    let mut light = make_cfa(size, vec![1.0; 4], CfaType::Mono);
    divide(prepared, &mut light);
    assert_eq!(light.data.pixels()[0], 2.0);
    let flags = light.flags.as_ref().unwrap();
    assert_eq!(
        (0..4).map(|index| flags.at(index)).collect::<Vec<_>>(),
        [
            QualityFlags::default(),
            QualityFlags::default(),
            QualityFlags::NO_DATA.union(QualityFlags::REPAIRED),
            QualityFlags::NO_DATA.union(QualityFlags::REPAIRED)
        ]
    );
}

#[test]
fn prepared_flat_is_bit_exact_for_bayer_and_xtrans_with_subtraction() {
    for cfa_type in [CfaType::Bayer(CfaPattern::Rggb), standard_xtrans()] {
        let size = match cfa_type {
            CfaType::Bayer(_) => Size2us::new(4, 4),
            CfaType::XTrans(_) => Size2us::new(6, 6),
            CfaType::Mono => unreachable!(),
        };
        let means = [0.5f32, 1.0, 0.25];
        let mut counts = [0usize; 3];
        let mut flat_pixels = Vec::with_capacity(size.pixel_count());
        let mut expected_divisors = Vec::with_capacity(size.pixel_count());
        for y in 0..size.height {
            for x in 0..size.width {
                let color = cfa_type.color_at(Vec2us::new(x, y)) as usize;
                let divisor = if counts[color].is_multiple_of(2) {
                    0.5
                } else {
                    1.5
                };
                counts[color] += 1;
                expected_divisors.push(divisor);
                flat_pixels.push(0.125 + means[color] * divisor);
            }
        }

        let flat = make_cfa(size, flat_pixels, cfa_type);
        let subtractor = make_cfa(size, vec![0.125; size.pixel_count()], cfa_type);
        let prepared = prepare(flat, Some(&subtractor));
        assert_eq!(
            bits(prepared.divisor().data.pixels()),
            bits(&expected_divisors)
        );
        assert_eq!(prepared.floored(), 0);

        let mut light = make_cfa(size, vec![0.75; size.pixel_count()], cfa_type);
        divide(prepared, &mut light);
        let expected: Vec<f32> = expected_divisors
            .iter()
            .map(|divisor| 0.75 / divisor)
            .collect();
        assert_eq!(bits(light.data.pixels()), bits(&expected));
    }
}

#[test]
#[should_panic(expected = "CfaImage dimensions mismatch")]
fn preparation_rejects_mismatched_subtractor_dimensions() {
    let flat = make_cfa(Size2us::new(2, 2), vec![1.0; 4], CfaType::Mono);
    let subtractor = make_cfa(Size2us::new(3, 2), vec![0.1; 6], CfaType::Mono);
    prepare(flat, Some(&subtractor));
}

/// A flat prepares to the same bits on one thread as on seven, mono and mosaic: each mean is a
/// sum added in a fixed order.
#[test]
fn preparation_does_not_depend_on_the_thread_count() {
    let size = Size2us::new(300, 260);
    let mut rng = TestRng::new(5);
    let pixels: Vec<f32> = (0..size.pixel_count())
        .map(|_| 0.4 + 0.2 * rng.next_f32())
        .collect();
    for cfa_type in [
        CfaType::Mono,
        CfaType::Bayer(CfaPattern::Rggb),
        standard_xtrans(),
    ] {
        let run = |threads: usize| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            let flat = make_cfa(size, pixels.clone(), cfa_type);
            bits(pool.install(|| prepare(flat, None)).divisor().data.pixels())
        };
        assert_eq!(run(1), run(7), "{cfa_type:?}");
    }
}
