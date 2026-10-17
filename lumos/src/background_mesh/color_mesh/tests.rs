use super::*;
use crate::bit_buffer2::BitBuffer2;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::math::size2us::Size2us;

/// An RGGB mosaic of 128 × 128 with red at 1/8 + x/1024, green at 1/4 and blue at 3/8: each
/// color's mesh reads its own level, and red's gradient. Tiles of 64 are centred at 31.5 and
/// 95.5. A tile's sky is read about the plane fitted to its samples, so red's tiles read the
/// ramp at their centres, and the bilinear reading between and past the centres follows it to
/// any column: red(31) and red(63). Every value is dyadic.
#[test]
fn each_color_reads_its_own_level() {
    let size = 128;
    let cfa = CfaType::Bayer(CfaPattern::Rggb);
    let red = |x: usize| 0.125 + x as f32 / 1024.0;
    let pixels = Buffer2::from_fn(size, size, |x, y| match cfa.slot_at(Vec2us::new(x, y)) {
        0 => red(x),
        1 => 0.25,
        _ => 0.375,
    });
    let mesh = ColorMesh::measure(MaskedPlane::from(&pixels), &cfa, 64);
    for position in [
        Vec2us::new(31, 31),
        Vec2us::new(64, 10),
        Vec2us::new(100, 90),
    ] {
        assert_eq!(mesh.at(1, position).sky, 0.25, "{position:?}");
        assert_eq!(mesh.at(2, position).sky, 0.375, "{position:?}");
        assert_eq!(mesh.at(1, position).noise, 0.0, "{position:?}");
    }
    assert_eq!(mesh.at(0, Vec2us::new(31, 31)).sky, red(31));
    assert_eq!(mesh.at(0, Vec2us::new(63, 31)).sky, red(63));
}

/// The mask counts against a color's half of a tile. An RGGB mosaic of 128 × 128 at 1/4, its
/// top-left 48 × 48 masked: the top-left 64-px tile keeps 1792 of its 4096 pixels, under half
/// for every color, and the halo of 1/2 its unmasked margin holds is not its sky — the good
/// tiles' 1/4 is, read at the tile's centre (31.5, 31.5) as it is everywhere. With the whole
/// frame masked no tile is good, and every tile is measured without the mask: 1/4 again.
#[test]
fn a_mostly_masked_tile_takes_its_neighbours_sky() {
    let size = Size2us::new(128, 128);
    let cfa = CfaType::Bayer(CfaPattern::Rggb);
    let pixels = Buffer2::from_fn(size.width, size.height, |x, y| match (x, y) {
        (0..48, 0..48) => 100.0,
        (0..64, 0..64) => 0.5,
        _ => 0.25,
    });
    let mut mask = BitBuffer2::new_default(size);
    for y in 0..48 {
        for x in 0..48 {
            mask.set_at(Vec2us::new(x, y), true);
        }
    }
    let masked = MaskedPlane {
        pixels: &pixels,
        mask: Some(&mask),
    };
    let mesh = ColorMesh::measure(masked, &cfa, 64);
    for color in 0..3 {
        assert_eq!(
            mesh.at(color, Vec2us::new(31, 31)).sky,
            0.25,
            "color {color}"
        );
    }

    let flat = Buffer2::new_filled(size.width, size.height, 0.25);
    let everywhere = BitBuffer2::new_filled(size, true);
    let mesh = ColorMesh::measure(
        MaskedPlane {
            pixels: &flat,
            mask: Some(&everywhere),
        },
        &cfa,
        64,
    );
    for color in 0..3 {
        assert_eq!(
            mesh.at(color, Vec2us::new(70, 20)).sky,
            0.25,
            "color {color}"
        );
    }
}
