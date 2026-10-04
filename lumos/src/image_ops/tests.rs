use crate::internals::prelude::*;
use crate::io::image::mosaic_noise::MosaicNoise;
use crate::{
    Denoise, ExtractBackground, Hdr, LocalContrast, NeutralizeBackground, OpError, Scnr, Stretch,
};

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
