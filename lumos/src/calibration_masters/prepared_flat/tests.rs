use crate::internals::cfa::XTRANS_PATTERN;
use crate::internals::prelude::*;
use crate::io::image::pixel_flags::Flags;

use crate::CfaType;
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
    prepared.apply(&mut light);
    assert_eq!(light.data.pixels(), &[10.0, 1.0, 1.0, 0.5]);
    // The floored divisor is flagged, and only it: that pixel is corrected by less than its flat
    // asked for.
    let flags = light.flags.as_ref().unwrap();
    assert_eq!(flags.count(Flags::FLAT_FLOOR), 1);
    assert_eq!(flags.at(0), Flags::FLAT_FLOOR);
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
        prepared.apply(&mut light);
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
