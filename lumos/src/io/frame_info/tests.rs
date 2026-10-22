use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::bayer::rcd;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::channel_layout::ChannelLayout;
use crate::image::image_dimensions::ImageDimensions;
use crate::io::frame_info::{DecoderBytes, FrameInfo};
use crate::memory::footprint::Footprint;
use crate::noise::mosaic_noise_measure::MosaicNoiseMeasure;

/// A 10 × 8 frame decodes to 320 B of samples and 80 of flags, 400 B kept, 500 at the peak with
/// a decoder of 100 B beside the frame, and as much with a source of 300 B freed before it. A Bayer demosaic on 4 workers then holds the mosaic, three planes and its
/// tiles, 4 · 320 B and RCD's workspace, and keeps the three planes, 960 B, with the 80 flag
/// bytes on both. A mono frame's demosaic keeps its one plane: 320 + 80 = 400 B, and its way
/// peaks at the decoder's 500 B.
#[test]
fn the_way_to_a_demosaic_holds_the_decode_the_noise_then_the_planes() {
    let dimensions = ImageDimensions::new((10, 8), ChannelLayout::Mono);
    let info = |cfa_type| FrameInfo {
        dimensions,
        cfa_type,
        decoder: DecoderBytes {
            source: 0,
            beside: 100,
        },
        gain_bytes: 0,
    };
    let rggb = CfaType::Bayer(CfaPattern::Rggb);
    assert_eq!(
        info(rggb).decoded(),
        Footprint {
            kept: 400,
            peak: 500,
        }
    );
    // A source past the frame sets the peak beside the decoder's own bytes, 600 + 100; one under
    // it is gone before the frame is made.
    for (source, peak) in [(300, 500), (600, 700)] {
        let with_source = FrameInfo {
            decoder: DecoderBytes {
                source,
                beside: 100,
            },
            ..info(rggb)
        };
        assert_eq!(
            with_source.decoded(),
            Footprint { kept: 400, peak },
            "source of {source} B"
        );
    }
    let noise = MosaicNoiseMeasure::heap_bytes(dimensions.size(), &rggb);
    let demosaic_peak = 4 * 320 + rcd::internals::workspace_bytes(4) + 80;
    assert_eq!(
        info(rggb).demosaiced(info(rggb).decoded(), MarkesteijnPasses::One, 4),
        Footprint {
            kept: 960 + 80,
            peak: demosaic_peak.max(400 + noise).max(500),
        }
    );
    let mono = info(CfaType::Mono);
    assert_eq!(
        mono.demosaiced(mono.decoded(), MarkesteijnPasses::One, 4),
        Footprint {
            kept: 400,
            peak: 500,
        }
    );
}
