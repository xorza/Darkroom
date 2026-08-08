//! The contract every in-place image op enforces at its `apply` boundary: a linear f32 master
//! ([`require_f32_master`]) and valid configuration ([`crate::InvalidConfigField`]), reported via
//! [`OpError`] instead of a panic. The ops themselves (`denoise`, `hdr`, `stretching`, …) run over
//! [`crate::image_ops`].
//!
//! A convention rather than a trait, deliberately. A trait would turn seven inherent `apply`
//! methods into trait methods, so every downstream call site would need it in scope, to save the
//! two-line prologue and express a composability nothing uses — `lens` drives each op from its own
//! node with its own deserialized config type, and would keep doing so. `NeutralizeBackground`
//! takes no parameters and so has no `validate` to call; that is the contract met, not skipped.

use imaginarium::{ColorFormat, Image, ImageDesc};

use crate::error::InvalidConfigField;
use crate::io::image::linear::LinearImage;

/// Why a display/processing op failed.
#[derive(Debug, thiserror::Error)]
pub enum OpError {
    /// The image isn't a linear f32 master (`L_F32` or `RGB_F32`).
    #[error("image op requires an L_F32 or RGB_F32 image, got {0}")]
    UnsupportedFormat(ColorFormat),
    /// A configuration parameter is outside its valid range.
    #[error("invalid config: {0}")]
    InvalidConfig(#[from] InvalidConfigField),
    /// A model's design matrix does not contain enough independent information.
    #[error("{operation} is rank deficient: rank {rank}, requires {required_rank}")]
    RankDeficient {
        operation: &'static str,
        rank: usize,
        required_rank: usize,
    },
}

/// Run an op that wants **planar** channel planes over an interleaved master: deinterleave once,
/// hand the planes to `run`, and re-interleave the result back into `image`.
///
/// One deinterleave for the whole op rather than one per channel — but note that it costs a full
/// master's worth of planes where per-channel streaming only ever held one plane. So the
/// interleaved buffer is **released for the duration of the op**: every sample already lives in the
/// planes, it is the largest allocation in the process at full-frame sizes, and holding it
/// alongside both the planes and whatever working set the op allocates is what takes peak heap to
/// 3× the master (`image_ops::mem_budget_probe` measures exactly this).
///
/// On failure the master's *contents* are unspecified — the planes are re-interleaved either way,
/// so it carries however far `run` got, matching what per-channel streaming left behind. Its
/// dimensions and format are always intact.
pub(crate) fn on_planes(
    image: &mut Image,
    run: impl FnOnce(&mut LinearImage) -> Result<(), OpError>,
) -> Result<(), OpError> {
    require_f32_master(image)?;
    let mut planar = LinearImage::from_f32_image(image);
    *image = Image::new_black(ImageDesc::new(1, 1, image.desc().color_format))
        .expect("a 1x1 image in the master's own format is valid");
    let outcome = run(&mut planar);
    *image = Image::from(&planar);
    outcome
}

/// The display ops are defined on a linear f32 master in L or RGB; reject anything else
/// (integer formats, RGBA) at the op boundary, before the per-pixel helpers (which assume it).
pub(crate) fn require_f32_master(image: &Image) -> Result<(), OpError> {
    let format = image.desc().color_format;
    if format == ColorFormat::L_F32 || format == ColorFormat::RGB_F32 {
        Ok(())
    } else {
        Err(OpError::UnsupportedFormat(format))
    }
}

#[cfg(test)]
mod tests {
    use imaginarium::{ColorFormat, Image, ImageDesc};

    use crate::error::InvalidConfigField;
    use crate::image_ops::op::{OpError, on_planes};

    /// Dyadic values throughout, so every edit below is exact in f32 and the assertions can be
    /// equalities rather than tolerances.
    fn rgb_f32(width: usize, height: usize, samples: &[f32]) -> Image {
        Image::new_with_data(
            ImageDesc::new(width, height, ColorFormat::RGB_F32),
            bytemuck::cast_slice(samples).to_vec(),
        )
        .unwrap()
    }

    fn l_f32(width: usize, samples: &[f32]) -> Image {
        Image::new_with_data(
            ImageDesc::new(width, 1, ColorFormat::L_F32),
            bytemuck::cast_slice(samples).to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn planes_arrive_deinterleaved_in_channel_order_and_land_back_interleaved() {
        let mut rgb = rgb_f32(2, 1, &[0.125, 0.25, 0.375, 0.5, 0.625, 0.75]);
        let mut seen = Vec::new();
        on_planes(&mut rgb, |planar| {
            for plane in planar.planes_mut() {
                seen.push(plane.pixels().to_vec());
                for p in plane.pixels_mut() {
                    *p += 1.0;
                }
            }
            Ok(())
        })
        .unwrap();
        // R then G then B, each the contiguous plane of its own channel...
        assert_eq!(
            seen,
            [vec![0.125, 0.5], vec![0.25, 0.625], vec![0.375, 0.75]]
        );
        // ...and each edit back in its own interleaved slot, not shifted by a channel.
        assert_eq!(
            bytemuck::cast_slice::<u8, f32>(rgb.bytes()),
            &[1.125, 1.25, 1.375, 1.5, 1.625, 1.75]
        );

        // A mono master is one plane, not three, and takes the same path.
        let mut l = l_f32(3, &[0.25, 0.5, 0.75]);
        let mut planes = 0;
        on_planes(&mut l, |planar| {
            for plane in planar.planes_mut() {
                planes += 1;
                for p in plane.pixels_mut() {
                    *p *= 2.0;
                }
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(planes, 1);
        assert_eq!(bytemuck::cast_slice::<u8, f32>(l.bytes()), &[0.5, 1.0, 1.5]);
    }

    #[test]
    fn a_failing_op_still_leaves_a_whole_master_behind() {
        // The interleaved buffer is released while the op runs, so the failure path has to
        // re-interleave too — otherwise the caller is left holding the 1x1 placeholder.
        let mut rgb = rgb_f32(2, 1, &[0.125, 0.25, 0.375, 0.5, 0.625, 0.75]);
        let error = on_planes(&mut rgb, |planar| {
            for p in planar.planes_mut().next().unwrap().pixels_mut() {
                *p += 1.0;
            }
            Err(OpError::InvalidConfig(InvalidConfigField {
                field: "test",
                expected: "a failure",
                value: 0.0,
                bound: None,
            }))
        })
        .unwrap_err();
        assert!(matches!(error, OpError::InvalidConfig(_)));
        assert_eq!(rgb.desc(), ImageDesc::new(2, 1, ColorFormat::RGB_F32));
        // Contents are unspecified on failure, but they are the planes as `run` left them — here
        // the red channel it had already edited, not stale or zeroed samples.
        assert_eq!(
            bytemuck::cast_slice::<u8, f32>(rgb.bytes()),
            &[1.125, 0.25, 0.375, 1.5, 0.625, 0.75]
        );
    }

    #[test]
    fn a_non_f32_master_is_rejected_before_any_deinterleave() {
        let mut u8_rgb =
            Image::new_with_data(ImageDesc::new(2, 1, ColorFormat::RGB_U8), vec![0u8; 2 * 3])
                .unwrap();
        let mut ran = false;
        let error = on_planes(&mut u8_rgb, |_| {
            ran = true;
            Ok(())
        })
        .unwrap_err();
        assert!(matches!(
            error,
            OpError::UnsupportedFormat(ColorFormat::RGB_U8)
        ));
        assert!(!ran, "the op body ran on a format it cannot handle");
    }
}
