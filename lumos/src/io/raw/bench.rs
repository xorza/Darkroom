//! The decode of a raw buffer into normalized samples and flags: what a RAW load does past
//! LibRaw's unpack.

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{BIAS, FRAME, Field};
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::io::raw;
use crate::io::raw::black_level::{BlackLevel, DecodedRaw, FlagRule, LibrawBlack, Photosites};
use crate::io::raw::sensor_layout::SensorLayout;
use common::{Size2us, Vec2us};

/// RGGB as LibRaw's `filters` word spells it.
const RGGB: u32 = 0x9494_9494;

/// A 14-bit sensor's common black level.
const BLACK: u32 = 512;

/// Each LibRaw channel's black above [`BLACK`].
const CHANNEL_BLACK: [u32; 4] = [0, 3, 5, 3];

/// The 14-bit white point.
const WHITE: u16 = 16_383;

/// Where the flag rule takes a photosite as saturated: below the white point, as a sensor's
/// response bends before it clips.
const SATURATION: f64 = 16_000.0;

pub(crate) fn atomic(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = Kind::Atomic.group(c, "io/raw/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    // The decode of the standard frame's RGGB light from its raw buffer, masked margins included.
    group.bench_function(BenchmarkId::new("decode/rggb", field.name()), |b| {
        let sensor = RawSensor::of(field, FRAME);
        b.iter_with_large_drop(|| sensor.decode());
    });
}

/// A raw buffer as LibRaw unpacks it, and the levels that decode it.
#[derive(Debug)]
struct RawSensor {
    raw: Vec<u16>,
    layout: SensorLayout,
    black: BlackLevel,
}

impl RawSensor {
    /// `field`'s RGGB light over an `active` area, quantized to the levels above, with a masked
    /// margin of 16 columns and 12 rows each side at the black level. The black level is the
    /// light's bias, so a sample sits `sample − BIAS` of the span above it.
    #[expect(
        clippy::cast_sign_loss,
        reason = "a light reads its bias less a few read-noise σ at the least, so a level is above zero"
    )]
    fn of(field: Field, active: Size2us) -> Self {
        let layout = SensorLayout {
            raw: Size2us::new(active.width + 32, active.height + 24),
            active,
            margin: Vec2us::new(16, 12),
        };
        // LibRaw's `cblack` is 16 KiB, too large for the stack.
        let mut cblack: Box<[u32; libraw_sys::LIBRAW_CBLACK_SIZE as usize]> =
            vec![0u32; libraw_sys::LIBRAW_CBLACK_SIZE as usize]
                .into_boxed_slice()
                .try_into()
                .unwrap();
        cblack[..4].copy_from_slice(&CHANNEL_BLACK);
        let black = BlackLevel::from_libraw(&LibrawBlack {
            black: BLACK,
            cblack: &cblack,
            maximum: u32::from(WHITE),
            filters: RGGB,
            dng: None,
            masked: [0; 8],
        })
        .unwrap();
        let light = field.light(active, CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)));
        let span = black.span();
        let mut raw: Vec<u16> = (0..layout.raw.pixel_count())
            .map(|idx| {
                let (x, y) = (idx % layout.raw.width, idx / layout.raw.width);
                u16::try_from(BLACK + CHANNEL_BLACK[raw::filter_color(RGGB, y, x)]).unwrap()
            })
            .collect();
        for (idx, &sample) in light.data().pixels().iter().enumerate() {
            let (x, y) = (idx % active.width, idx / active.width);
            let level = black.of_channel(raw::filter_color(RGGB, y, x))
                + (f64::from(sample) - f64::from(BIAS)) * span;
            let (raw_x, raw_y) = (layout.margin.x + x, layout.margin.y + y);
            raw[raw_y * layout.raw.width + raw_x] = level.round().min(f64::from(WHITE)) as u16;
        }
        Self { raw, layout, black }
    }

    fn decode(&self) -> DecodedRaw {
        self.black.decode(
            &self.raw,
            self.layout,
            Photosites {
                channel: |x, y| raw::filter_color(RGGB, y, x),
                period: 2,
                rule: FlagRule {
                    zero_is_bad: true,
                    saturation: [SATURATION; 4],
                },
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decode gives the light back less its bias, to within half an ADU, `0.5 / span`,
    /// wherever the quantization did not clip it at the white point. The span is 16 383 − 512 =
    /// 15 871, and the raw buffer the 60 × 40 area with 16 columns and 12 rows of margin each side:
    /// 92 × 64.
    #[test]
    fn the_decode_gives_back_the_light_less_its_bias() {
        let active = Size2us::new(60, 40);
        let sensor = RawSensor::of(Field::Typical, active);
        assert_eq!(sensor.raw.len(), 92 * 64);
        let span = sensor.black.span();
        assert_eq!(span, 15_871.0);
        let light = Field::Typical.light(active, CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)));
        let decoded = sensor.decode();
        assert_eq!(decoded.pixels.len(), active.pixel_count());
        let headroom =
            f64::from(u32::from(WHITE) - BLACK - CHANNEL_BLACK.into_iter().max().unwrap());
        for (&decoded, &sample) in decoded.pixels.iter().zip(light.data().pixels()) {
            let above_black = f64::from(sample) - f64::from(BIAS);
            if above_black * span < headroom {
                let error = (f64::from(decoded) - above_black).abs();
                assert!(error <= 0.5 / span + 1e-7, "{decoded} for {sample}");
            }
        }
    }
}
