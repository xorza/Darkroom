//! Errors the image ops report from their `apply` boundary.

use crate::error::InvalidConfigField;

/// Why a display/processing op failed.
#[derive(Debug, thiserror::Error)]
pub enum OpError {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_ops::background_extraction::ExtractBackground;
    use crate::image_ops::color_calibration::Scnr;
    use crate::image_ops::denoise::Denoise;
    use crate::image_ops::hdr::Hdr;
    use crate::image_ops::local_contrast::LocalContrast;
    use crate::image_ops::stretching::{ColorMode, Stretch, StretchMethod};
    use crate::io::image::linear::LinearImage;
    use crate::math::size2us::Size2us;
    use crate::testing::images::gray_image;

    /// One case per op that owns a `validate`, so an op added without the `self.validate()?`
    /// prologue this module describes fails here rather than panicking somewhere in its kernel.
    #[test]
    fn every_op_apply_rejects_an_invalid_config() {
        fn rejects(field: &str, apply: impl Fn(&mut LinearImage) -> Result<(), OpError>) {
            let mut image = gray_image(Size2us::new(4, 4), vec![0.0; 16]);
            let error = apply(&mut image).unwrap_err();
            assert!(
                matches!(&error, OpError::InvalidConfig(invalid) if invalid.field == field),
                "expected an InvalidConfig on {field}, got {error:?}"
            );
        }

        rejects("background extraction degree", |image| {
            ExtractBackground {
                degree: 0,
                ..Default::default()
            }
            .apply(image)
        });
        rejects("SCNR amount", |image| Scnr::additive_mask(1.5).apply(image));
        rejects("denoise strength", |image| {
            Denoise {
                strength: 1.5,
                ..Default::default()
            }
            .apply(image)
        });
        rejects("hdr scales", |image| {
            Hdr {
                scales: 0,
                ..Default::default()
            }
            .apply(image)
        });
        rejects("local contrast tiles", |image| {
            LocalContrast {
                tiles: 0,
                ..Default::default()
            }
            .apply(image)
        });
        rejects("asinh beta", |image| {
            Stretch {
                method: StretchMethod::Asinh { beta: 0.0 },
                color: ColorMode::ColorPreserving,
            }
            .apply(image)
        });
    }
}
