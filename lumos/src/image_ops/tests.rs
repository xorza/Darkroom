use crate::image_ops::color_calibration::neutralize_background::NeutralizeBackground;
use crate::image_ops::color_calibration::scnr::Scnr;
use crate::image_ops::denoise::Denoise;
use crate::image_ops::error::OpError;
use crate::image_ops::extract_background::ExtractBackground;
use crate::image_ops::hdr::Hdr;
use crate::image_ops::local_contrast::LocalContrast;
use crate::image_ops::stretch::{ColorMode, Stretch, StretchMethod};
use crate::internals::prelude::*;
use crate::math::noise::mosaic_noise::MosaicNoise;

/// An op under test, applied in place.
type Op = fn(&mut LinearImage) -> Result<(), OpError>;

/// Every op that changes the samples drops the noise facts that described them: a 128 × 128 RGB
/// frame with a quantization σ and a mosaic noise leaves each op with neither, and with samples
/// that differ from the ones it came in with. The background tiles are 16 px, so the frame holds
/// 64 of them and its gradient is there to remove.
#[test]
fn every_op_drops_the_noise_of_the_samples_it_changed() {
    let dimensions = ImageDimensions::new((128, 128), 3);
    let frame = || {
        let mut image = LinearImage::from_pixels(
            dimensions,
            (0..dimensions.sample_count())
                .map(|index| {
                    let (pixel, channel) = (index / 3, index % 3);
                    let (x, y) = (pixel % 128, pixel / 128);
                    0.1 + 0.05 * channel as f32
                        + x as f32 / 2048.0
                        + ((x * 7 + y * 13) % 17) as f32 / 400.0
                })
                .collect(),
        );
        image.metadata.quantization_sigma = Some(1.0 / 65_536.0);
        image.metadata.mosaic_noise = Some(MosaicNoise {
            sigma: [0.01; 3],
            read_share: [0.0; 3],
            sky: [0.1, 0.15, 0.2],
            quantization_sigma: Some(1.0 / 65_536.0),
        });
        image
    };
    let ops: [(&str, Op); 7] = [
        ("stretch", |image| Stretch::default().apply(image)),
        ("denoise", |image| Denoise::default().apply(image)),
        ("background extraction", |image| {
            ExtractBackground {
                tile_size: 16,
                ..ExtractBackground::default()
            }
            .apply(image)
        }),
        ("local contrast", |image| {
            LocalContrast::default().apply(image)
        }),
        ("hdr", |image| Hdr::default().apply(image)),
        ("neutralize background", |image| {
            NeutralizeBackground.apply(image)
        }),
        ("scnr", |image| Scnr::default().apply(image)),
    ];
    for (name, op) in ops {
        let before = frame();
        let mut after = frame();
        op(&mut after).unwrap();
        assert_ne!(
            (0..3)
                .map(|channel| after.channel(channel).pixels().to_vec())
                .collect::<Vec<_>>(),
            (0..3)
                .map(|channel| before.channel(channel).pixels().to_vec())
                .collect::<Vec<_>>(),
            "{name} left the samples as they were"
        );
        assert_eq!(after.metadata.quantization_sigma, None, "{name}");
        assert_eq!(after.metadata.mosaic_noise, None, "{name}");
    }
}

/// Every op that owns a `validate`, at each bound its config can break, so an op added
/// without the `self.validate()?` prologue the `image_ops` docs describe fails here rather than
/// panicking somewhere in its kernel. Where an op skips its work at a zero strength or amount,
/// the row sets that zero too: the config is still rejected, so validation runs before the
/// shortcut.
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

    for degree in [0, 7] {
        rejects("background extraction degree", |image| {
            ExtractBackground {
                degree,
                ..Default::default()
            }
            .apply(image)
        });
    }
    rejects("SCNR amount", |image| Scnr::additive_mask(1.5).apply(image));
    rejects("denoise strength", |image| {
        Denoise {
            strength: 1.5,
            ..Default::default()
        }
        .apply(image)
    });
    rejects("denoise k", |image| {
        Denoise {
            k: 0.0,
            strength: 0.0,
            ..Default::default()
        }
        .apply(image)
    });
    rejects("hdr amount", |image| {
        Hdr {
            amount: 1.5,
            ..Default::default()
        }
        .apply(image)
    });
    rejects("hdr scales", |image| {
        Hdr {
            scales: 0,
            amount: 0.0,
        }
        .apply(image)
    });
    rejects("local contrast tiles", |image| {
        LocalContrast {
            tiles: 0,
            strength: 0.0,
            ..Default::default()
        }
        .apply(image)
    });
    rejects("local contrast clip_limit", |image| {
        LocalContrast {
            clip_limit: 0.5,
            strength: 0.0,
            ..Default::default()
        }
        .apply(image)
    });
    rejects("asinh beta", |image| {
        Stretch {
            method: StretchMethod::Asinh {
                black_point: 0.0,
                beta: 0.0,
            },
            color: ColorMode::ColorPreserving,
        }
        .apply(image)
    });
    rejects("ghs d", |image| {
        Stretch {
            method: StretchMethod::Ghs {
                black_point: 0.0,
                d: -1.0,
                b: 0.0,
                sp: 0.5,
                lp: 0.0,
                hp: 1.0,
            },
            color: ColorMode::ColorPreserving,
        }
        .apply(image)
    });
}
