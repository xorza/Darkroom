use bytemuck::Pod;
use rayon::prelude::*;

use super::Preview;
use crate::common::color_format::{ChannelSize, ChannelType, ColorFormat};
use crate::image::{Image, ImageDesc};

/// Area-averages `input` to `params`' target size, fused with the conversion to
/// `RGBA_U8`. See [`Preview`](super::Preview) for the algorithm.
pub(super) fn generate(params: &Preview, input: &Image) -> Image {
    let dst_w = params.width.max(1);
    let dst_h = params.height.max(1);

    let fmt = input.desc().color_format;
    let channels = fmt.channel_count.channel_count();

    match (fmt.channel_size, fmt.channel_type, channels) {
        (ChannelSize::_8bit, ChannelType::UInt, 1) => typed::<u8, 1>(input, dst_w, dst_h),
        (ChannelSize::_8bit, ChannelType::UInt, 3) => typed::<u8, 3>(input, dst_w, dst_h),
        (ChannelSize::_8bit, ChannelType::UInt, 4) => typed::<u8, 4>(input, dst_w, dst_h),
        (ChannelSize::_16bit, ChannelType::UInt, 1) => typed::<u16, 1>(input, dst_w, dst_h),
        (ChannelSize::_16bit, ChannelType::UInt, 3) => typed::<u16, 3>(input, dst_w, dst_h),
        (ChannelSize::_16bit, ChannelType::UInt, 4) => typed::<u16, 4>(input, dst_w, dst_h),
        (ChannelSize::_32bit, ChannelType::Float, 1) => typed::<f32, 1>(input, dst_w, dst_h),
        (ChannelSize::_32bit, ChannelType::Float, 3) => typed::<f32, 3>(input, dst_w, dst_h),
        (ChannelSize::_32bit, ChannelType::Float, 4) => typed::<f32, 4>(input, dst_w, dst_h),
        _ => unreachable!("unsupported color format for preview: {fmt:?}"),
    }
}

/// A source channel element. `SCALE` maps an averaged raw value to the `0..=255`
/// output range (u8: identity, u16: `255/65535`, f32: `×255`), so the per-source
/// normalize divide is replaced by one multiply per output channel.
trait PreviewElem: Pod + Send + Sync {
    const SCALE: f32;
    fn to_f32(self) -> f32;
}

impl PreviewElem for u8 {
    const SCALE: f32 = 1.0;
    #[inline]
    fn to_f32(self) -> f32 {
        self as f32
    }
}

impl PreviewElem for u16 {
    const SCALE: f32 = 255.0 / 65535.0;
    #[inline]
    fn to_f32(self) -> f32 {
        self as f32
    }
}

impl PreviewElem for f32 {
    const SCALE: f32 = 255.0;
    #[inline]
    fn to_f32(self) -> f32 {
        self
    }
}

fn typed<T, const N: usize>(input: &Image, dst_w: usize, dst_h: usize) -> Image
where
    T: PreviewElem,
{
    let src_w = input.desc().width;
    let src_h = input.desc().height;
    let src: &[T] = bytemuck::cast_slice(input.bytes());

    let mut out = Image::new_black(ImageDesc::new(dst_w, dst_h, ColorFormat::RGBA_U8))
        .expect("RGBA_U8 preview dims are valid");

    out.bytes_mut()
        .par_chunks_mut(dst_w * 4)
        .enumerate()
        .for_each(|(oy, out_row)| {
            // Source-row band for this output row; the `.max(+1)` guards the
            // upscale corner case (empty footprint), `.min` keeps it in bounds.
            let sy0 = oy * src_h / dst_h;
            let sy1 = (((oy + 1) * src_h / dst_h).max(sy0 + 1)).min(src_h);

            for ox in 0..dst_w {
                let sx0 = ox * src_w / dst_w;
                let sx1 = (((ox + 1) * src_w / dst_w).max(sx0 + 1)).min(src_w);

                let mut sum = [0.0f32; 4];
                for sy in sy0..sy1 {
                    let row = sy * src_w;
                    // The footprint's columns are contiguous in the row; slice the
                    // band once (one bounds check) and walk pixels bound-free.
                    let band = &src[(row + sx0) * N..(row + sx1) * N];
                    for px in band.chunks_exact(N) {
                        for (acc, &v) in sum.iter_mut().zip(px) {
                            *acc += v.to_f32();
                        }
                    }
                }

                let scale = T::SCALE / ((sy1 - sy0) * (sx1 - sx0)) as f32;
                write_rgba8::<N>(&mut out_row[ox * 4..ox * 4 + 4], sum, scale);
            }
        });

    out
}

/// Writes one averaged source pixel (`sum` × `scale` → `0..=255`) as RGBA8,
/// expanding the source's `N` channels: L → gray broadcast, RGB → opaque alpha.
#[inline]
fn write_rgba8<const N: usize>(out: &mut [u8], sum: [f32; 4], scale: f32) {
    let q = |v: f32| (v * scale).round().clamp(0.0, 255.0) as u8;
    match N {
        1 => {
            let g = q(sum[0]);
            out.copy_from_slice(&[g, g, g, 255]);
        }
        3 => out.copy_from_slice(&[q(sum[0]), q(sum[1]), q(sum[2]), 255]),
        4 => out.copy_from_slice(&[q(sum[0]), q(sum[1]), q(sum[2]), q(sum[3])]),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::color_format::ALL_FORMATS;
    use crate::common::test_utils::create_test_image;

    fn img(w: usize, h: usize, fmt: ColorFormat, bytes: Vec<u8>) -> Image {
        Image::new_with_data(ImageDesc::new(w, h, fmt), bytes).unwrap()
    }

    /// Same-size "downscale" is a per-pixel convert: each footprint is one pixel,
    /// so an RGBA_U8 source round-trips byte-exact.
    #[test]
    fn test_same_size_is_identity_rgba8() {
        let src = img(
            2,
            2,
            ColorFormat::RGBA_U8,
            vec![
                10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160,
            ],
        );
        let out = Preview::new(2, 2).to_rgba8(&src);
        assert_eq!(out.desc(), src.desc());
        assert_eq!(out.bytes(), src.bytes());
    }

    /// 2×2 → 1×1 averages all four pixels. R: (0+100+200+40)/4 = 85, etc.; the
    /// opaque alphas average back to 255.
    #[test]
    fn test_box_average_2x_to_1x() {
        let src = img(
            2,
            2,
            ColorFormat::RGBA_U8,
            vec![
                0, 0, 0, 255, 100, 100, 100, 255, 200, 200, 200, 255, 40, 40, 40, 255,
            ],
        );
        let out = Preview::new(1, 1).to_rgba8(&src);
        assert_eq!(out.desc().width, 1);
        assert_eq!(out.desc().height, 1);
        assert_eq!(out.bytes(), &[85, 85, 85, 255]);
    }

    /// Grayscale broadcasts to RGB with an opaque alpha. L8 [0,100,200,40] → 1×1
    /// gray 85 → (85,85,85,255).
    #[test]
    fn test_l8_broadcasts_with_opaque_alpha() {
        let src = img(2, 2, ColorFormat::L_U8, vec![0, 100, 200, 40]);
        let out = Preview::new(1, 1).to_rgba8(&src);
        assert_eq!(out.bytes(), &[85, 85, 85, 255]);
    }

    /// RGB source gains an opaque alpha; a single 1×1 RGB pixel passes through.
    #[test]
    fn test_rgb8_gains_opaque_alpha() {
        let src = img(1, 1, ColorFormat::RGB_U8, vec![12, 34, 56]);
        let out = Preview::new(1, 1).to_rgba8(&src);
        assert_eq!(out.bytes(), &[12, 34, 56, 255]);
    }

    /// U16 is rescaled to 8-bit: 65535 → 255, and 32768 (just over half of the
    /// 65535 max) → round(32768·255/65535) = round(127.502) = 128.
    #[test]
    fn test_u16_rescaled_to_u8() {
        let px: Vec<u8> = [65535u16, 32768, 0, 65535]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let src = img(1, 1, ColorFormat::RGBA_U16, px);
        let out = Preview::new(1, 1).to_rgba8(&src);
        assert_eq!(out.bytes(), &[255, 128, 0, 255]);
    }

    /// F32 in [0,1] maps ×255 with rounding: 1.0→255, 0.0→0, 0.2→51, and the
    /// missing alpha is opaque (RGB_F32 source).
    #[test]
    fn test_f32_scaled_to_u8() {
        let px: Vec<u8> = [1.0f32, 0.0, 0.2]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let src = img(1, 1, ColorFormat::RGB_F32, px);
        let out = Preview::new(1, 1).to_rgba8(&src);
        assert_eq!(out.bytes(), &[255, 0, 51, 255]);
    }

    /// Non-integer reduction (3→2) tiles the source by integer floor into
    /// footprints {0} and {1,2}: out0 = 0, out1 = avg(60,120) = 90.
    #[test]
    fn test_non_integer_downscale_footprints() {
        let src = img(3, 1, ColorFormat::L_U8, vec![0, 60, 120]);
        let out = Preview::new(2, 1).to_rgba8(&src);
        assert_eq!(out.bytes(), &[0, 0, 0, 255, 90, 90, 90, 255]);
    }

    /// Every format reduces to a valid, correctly-sized RGBA_U8 preview.
    #[test]
    fn test_all_formats_to_rgba8() {
        for &format in ALL_FORMATS {
            let src = create_test_image(format, 17, 11, 3);
            let out = Preview::new(8, 5).to_rgba8(&src);
            assert_eq!(out.desc().color_format, ColorFormat::RGBA_U8);
            assert_eq!(out.desc().width, 8);
            assert_eq!(out.desc().height, 5);
            assert_eq!(out.bytes().len(), 8 * 5 * 4);
        }
    }
}
