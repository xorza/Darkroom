//! The decode of a raw buffer into normalized samples and flags, on a synthetic 6000 × 4000 Bayer
//! buffer: what a RAW load does past LibRaw's `unpack`.
//!
//! Run: `cargo test -p lumos --release --features bench io::raw::black_level::bench -- --ignored
//! --nocapture`

use std::hint::black_box;

use quickbench::quick_bench;

use crate::internals::test_rng::TestRng;
use crate::io::raw::black_level::{BlackLevel, FlagRule, LibrawBlack};
use crate::io::raw::libraw_filter_color;
use crate::io::raw::sensor_layout::SensorLayout;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// RGGB as LibRaw's `filters` word spells it.
const RGGB: u32 = 0x9494_9494;

#[quick_bench(warmup_iters = 2, iters = 10)]
fn bench_raw_decode(b: quickbench::Bencher) {
    let layout = SensorLayout {
        raw: Size2us::new(6032, 4024),
        active: Size2us::new(6000, 4000),
        margin: Vec2us::new(16, 12),
    };
    let mut rng = TestRng::new(12);
    // Black 512 with a per-channel offset, a 14-bit white point, a zero in every 4099th photosite
    // and a saturated one in every 997th.
    let raw: Vec<u16> = (0..layout.raw.pixel_count())
        .map(|index| match index {
            _ if index % 4099 == 0 => 0,
            _ if index % 997 == 0 => 16_383,
            _ => 512 + (rng.next_u64() % 12_000) as u16,
        })
        .collect();
    // LibRaw's `cblack` is 16 KiB, too large for the stack.
    let mut cblack: Box<[u32; libraw_sys::LIBRAW_CBLACK_SIZE as usize]> =
        vec![0u32; libraw_sys::LIBRAW_CBLACK_SIZE as usize]
            .into_boxed_slice()
            .try_into()
            .unwrap();
    cblack[..4].copy_from_slice(&[0, 3, 5, 3]);
    let black = BlackLevel::from_libraw(&LibrawBlack {
        black: 512,
        cblack: &cblack,
        maximum: 16_383,
        filters: RGGB,
        dng: None,
        masked: [0; 8],
    })
    .unwrap();
    let rule = FlagRule {
        zero_is_bad: true,
        saturation: [16_000.0; 4],
    };
    let channel = |x: usize, y: usize| libraw_filter_color(RGGB, y, x);
    b.bench_labeled("decode", || {
        black_box(black.decode(&raw, layout, channel, 2, rule));
    });
}
