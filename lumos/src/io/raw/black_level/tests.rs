use crate::io::raw::black_level::{BlackLevel, DngLevels, LibrawBlack};
use crate::io::raw::error::BlackLevelError;
use crate::io::raw::sensor_layout::SensorLayout;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// RGGB in LibRaw's `filters` word: (0, 0) red, (0, 1) and (1, 0) green, (1, 1) blue, the second
/// green read as channel 3.
const RGGB: u32 = 0x9494_9494;

/// libraw's `color.cblack`, every entry zero, on the heap: 16 KiB is too large for the stack.
fn no_black() -> Box<[u32; 4104]> {
    vec![0; 4104]
        .into_boxed_slice()
        .try_into()
        .expect("4104 entries")
}

fn level(black: u32, cblack: &[u32; 4104], maximum: u32, filters: u32) -> BlackLevel {
    BlackLevel::from_libraw(&LibrawBlack {
        black,
        cblack,
        maximum,
        filters,
        dng: None,
        masked: [0; 8],
    })
    .unwrap()
}

/// `adjust_bl`'s folds, by hand:
/// - scalar black 512 alone: every channel 512, span 16383 − 512;
/// - channels 10, 5, 15, 5 over 100: their least, 5, joins the common level, 105, leaving 5, 0,
///   10, 0;
/// - a 2×2 pattern 4, 8, 12, 16 on RGGB folds into R, G, G2, B as 4, 8, 12, 16, whose least joins
///   the common level: 200 + 4 = 204, channels 0, 4, 12, 8 (R, G, B, G2);
/// - a 1×1 pattern of 20 on X-Trans adds to every channel, then joins the common level: 276.
#[test]
fn the_folds_are_libraws() {
    let uniform = level(512, &no_black(), 16_383, RGGB);
    assert_eq!(uniform.common, 512.0);
    assert_eq!(uniform.channel, [0.0; 4]);
    assert_eq!(uniform.span(), 15_871.0);
    assert!(uniform.repeat.is_none());

    let mut cblack = no_black();
    cblack[..4].copy_from_slice(&[10, 5, 15, 5]);
    let per_channel = level(100, &cblack, 4096, RGGB);
    assert_eq!(per_channel.common, 105.0);
    assert_eq!(per_channel.channel, [5.0, 0.0, 10.0, 0.0]);
    assert_eq!(per_channel.of_channel(2), 115.0);

    let mut cblack = no_black();
    cblack[4] = 2;
    cblack[5] = 2;
    cblack[6..10].copy_from_slice(&[4, 8, 12, 16]);
    let folded = level(200, &cblack, 16_383, RGGB);
    assert_eq!(folded.common, 204.0);
    assert_eq!(folded.channel, [0.0, 4.0, 12.0, 8.0]);
    assert!(folded.repeat.is_none());

    let mut cblack = no_black();
    cblack[4] = 1;
    cblack[5] = 1;
    cblack[6] = 20;
    let xtrans = level(256, &cblack, 4096, 9);
    assert_eq!(xtrans.common, 276.0);
    assert_eq!(xtrans.channel, [0.0; 4]);
}

/// A Canon-like frame — channel blacks 2047 + (1, 0, 2, 0) under a 3×2 spatial pattern — normalizes
/// each sample to the correctly rounded quotient of its integer sum, by hand: the black at a pixel
/// is `2047 + channel + pattern`; the channels' least, 0, and the pattern's least, 5, move to the
/// common level, 2052, so the span is `15000 − 2052 = 12948`. The quotient `(v − black) / 12948`
/// of integers is taken in f64 and rounded once. Under a one-pixel margin, the pattern is anchored
/// at the visible area's first pixel.
#[test]
fn a_canon_black_level_is_exact_against_a_hand_sum() {
    let mut cblack = no_black();
    cblack[..4].copy_from_slice(&[1, 0, 2, 0]);
    cblack[4] = 2;
    cblack[5] = 3;
    cblack[6..12].copy_from_slice(&[5, 7, 9, 11, 13, 15]);
    let black = level(2047, &cblack, 15_000, RGGB);
    assert_eq!(black.common, 2052.0);
    assert_eq!(black.span(), 12_948.0);
    let repeat = black.repeat.as_ref().unwrap();
    assert_eq!(repeat.size, Size2us::new(3, 2));
    assert_eq!(&*repeat.values, &[0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);

    let layout = SensorLayout {
        raw: Size2us::new(8, 5),
        active: Size2us::new(6, 3),
        margin: Vec2us::new(1, 1),
    };
    let raw: Vec<u16> = (0..layout.raw.pixel_count())
        .map(|index| 2000 + (index * 37 % 9000) as u16)
        .collect();
    let channel = |x: usize, y: usize| [[0, 1], [3, 2]][y % 2][x % 2];
    let pixels = black.normalize(&raw, layout, channel);
    for y in 0..3 {
        for x in 0..6 {
            let value = raw[(y + 1) * 8 + x + 1];
            let pattern = [5, 7, 9, 11, 13, 15][(y % 2) * 3 + x % 3];
            let integer_black = 2047 + [1, 0, 2, 0][channel(x, y)] + pattern;
            let expected = ((f64::from(value) - f64::from(integer_black)) / 12_948.0) as f32;
            assert_eq!(
                pixels[y * 6 + x].to_bits(),
                expected.to_bits(),
                "({x}, {y}): {value} over black {integer_black}"
            );
        }
    }
}

/// Where LibRaw truncated a mean of the masked pixels, the mean itself is used. Sums 204 837,
/// 204 900, 205 012, 204 899 over 100 pixels each truncate to 2048, 2049, 2050, 2048, and `unpack`
/// moves the least, 2048, into `black`, leaving `cblack` 0, 1, 2, 0: the state LibRaw hands over.
/// The means are 2048.37, 2049, 2050.12, 2048.99; the least, 2048.37, is the common level, and a
/// channel is that plus its difference from it, which rounds at f64's 2⁻⁴² of 2048, so 1e-9.
///
/// A `cblack` that is not those truncations — LibRaw took its black elsewhere — keeps its integers,
/// as does one beside a spatial pattern, which a masked-area black never leaves.
#[test]
fn the_masked_mean_replaces_its_truncation() {
    let sums = [204_837, 204_900, 205_012, 204_899];
    let masked = [sums[0], sums[1], sums[2], sums[3], 100, 100, 100, 100];
    let mut cblack = no_black();
    cblack[..4].copy_from_slice(&[0, 1, 2, 0]);
    let from = |cblack: &[u32; 4104]| {
        BlackLevel::from_libraw(&LibrawBlack {
            black: 2048,
            cblack,
            maximum: 16_383,
            filters: RGGB,
            dng: None,
            masked,
        })
        .unwrap()
    };
    let exact = from(&cblack);
    assert_eq!(exact.common, 2048.37);
    for (c, &sum) in sums.iter().enumerate() {
        let mean = f64::from(sum) / 100.0;
        assert!((exact.of_channel(c) - mean).abs() < 1e-9, "channel {c}");
    }

    let mut elsewhere = cblack.clone();
    elsewhere[1] += 1;
    assert_eq!(from(&elsewhere).common, 2048.0);
    // A 1×1 pattern of 0 on top of the same channels: a pattern says the black came from no mask.
    let mut patterned = cblack.clone();
    patterned[4] = 1;
    patterned[5] = 1;
    let patterned = from(&patterned);
    assert_eq!(patterned.common, 2048.0);
    assert_eq!(patterned.of_channel(2), 2050.0);
}

/// A DNG's float levels replace LibRaw's roundings of them when each is within one ADU: a
/// `BlackLevel` of 256.4 that LibRaw truncated to 256. Floats that disagree with the integers by a
/// whole ADU or more were not where LibRaw's black came from, and the integers stand.
#[test]
fn a_dngs_float_levels_replace_their_roundings() {
    let cblack = no_black();
    let floats: Box<[f32; 4104]> = vec![0.0; 4104].into_boxed_slice().try_into().unwrap();
    let dng = |black: f32| {
        BlackLevel::from_libraw(&LibrawBlack {
            black: 256,
            cblack: &cblack,
            maximum: 4095,
            filters: RGGB,
            dng: Some(DngLevels {
                black,
                cblack: &floats,
            }),
            masked: [0; 8],
        })
        .unwrap()
        .common
    };
    assert_eq!(dng(256.4), f64::from(256.4f32));
    assert_eq!(dng(257.5), 256.0);
}

/// Every value is the file's: a black that reaches `maximum` in any channel or cell is refused,
/// not only a common level that does — here the common level 100 is below 140, but red adds 50 —
/// and so is a spatial pattern larger than LibRaw's table.
#[test]
fn file_values_that_leave_no_range_are_refused() {
    let error = BlackLevel::from_libraw(&LibrawBlack {
        black: 512,
        cblack: &no_black(),
        maximum: 512,
        filters: RGGB,
        dng: None,
        masked: [0; 8],
    })
    .unwrap_err();
    assert!(matches!(
        error,
        BlackLevelError::BlackExceedsMaximum { maximum: 512, .. }
    ));

    let mut cblack = no_black();
    cblack[0] = 50;
    cblack[1] = 0;
    let error = BlackLevel::from_libraw(&LibrawBlack {
        black: 100,
        cblack: &cblack,
        maximum: 140,
        filters: RGGB,
        dng: None,
        masked: [0; 8],
    })
    .unwrap_err();
    assert!(matches!(
        error,
        BlackLevelError::BlackExceedsMaximum { black, maximum: 140 } if black == 150.0
    ));

    let mut oversized = no_black();
    oversized[4] = 64;
    oversized[5] = 65;
    let error = BlackLevel::from_libraw(&LibrawBlack {
        black: 0,
        cblack: &oversized,
        maximum: 4096,
        filters: RGGB,
        dng: None,
        masked: [0; 8],
    })
    .unwrap_err();
    assert!(matches!(
        error,
        BlackLevelError::SpatialPatternTooLarge {
            width: 65,
            height: 64,
            capacity: 4098
        }
    ));
}
