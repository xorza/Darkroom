use imaginarium::{ChannelLayout, PlanarDesc};

use crate::frame::frame_info::{DecoderBytes, FrameInfo};
use crate::image::cfa::bayer_pattern::BayerPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::bayer::rcd;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::cfa::mosaic::Mosaic;
use crate::memory::footprint::Footprint;
use crate::noise::mosaic_noise_measure::MosaicNoiseMeasure;

/// A 10 × 8 frame decodes to 320 B of samples and 80 of flags, 400 B kept, 500 at the peak with
/// a decoder of 100 B beside the frame. A
/// Bayer demosaic on 4 workers then holds the mosaic, three planes and its tiles, 4 · 320 B and
/// RCD's workspace, and keeps the three planes, 960 B, with the 80 flag bytes on both; before it
/// the flags grow in block ORs beside the mosaic. A mono frame's demosaic keeps its one plane:
/// 320 + 80 = 400 B, and its way peaks at the decoder's 500 B.
#[test]
fn the_way_to_a_demosaic_holds_the_decode_the_noise_then_the_planes() {
    let desc = PlanarDesc::new((10, 8), ChannelLayout::Mono);
    let info = |cfa_type| FrameInfo {
        desc,
        cfa_type,
        decoder: DecoderBytes {
            source: 0,
            beside: 100,
        },
        gain_bytes: 0,
    };
    let rggb = CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb));
    assert_eq!(
        info(rggb).decoded(),
        Footprint {
            kept: 400,
            peak: 500,
        }
    );
    let noise = MosaicNoiseMeasure::heap_bytes(desc.size(), rggb.mosaic().unwrap());
    let demosaic_peak = 4 * 320 + rcd::internals::workspace_bytes(4) + 80;
    assert_eq!(
        info(rggb).demosaiced(info(rggb).decoded(), MarkesteijnPasses::One, 4),
        Footprint {
            kept: 960 + 80,
            peak: demosaic_peak.max(400 + noise.max(640)).max(500),
        }
    );
    // A way that keeps more than the demosaic holds at its peak shows what is held beside it, one
    // step after the other: the noise, 320 B, then the flags' growth, two block ORs of the
    // 10-column strip down the 8 rows on each of the 4 workers, 4 × 2 × 80 = 640 B, the larger.
    assert_eq!(
        info(rggb)
            .demosaiced(Footprint::kept(2_000_000), MarkesteijnPasses::One, 4)
            .peak,
        2_000_000 + 640
    );
    assert_eq!(noise, 320, "premise: the growth is the larger step");
    let mono = info(CfaType::Mono);
    assert_eq!(
        mono.demosaiced(mono.decoded(), MarkesteijnPasses::One, 4),
        Footprint {
            kept: 400,
            peak: 500,
        }
    );
}

/// A 100 × 100 one-channel frame resides as 40 000 B of samples and 10 000 B of flags.
///
/// As a mosaic of RGGB its statistics hold its pairs, 98 a row over 100 rows at 4 bytes and a bin
/// byte, 49 000, more than its 40 000 B of level samples: its way peaks at 99 000 B, and a decoder
/// holding 50 000 B beside the frame raises that to 100 000 B. A source of 120 000 B freed before
/// the frame is made sets the peak alone; one of 30 000 B beside the 50 000 is gone by then, and
/// leaves it at 100 000 B. As a plain frame its noise is measured by MRS over one tile that is the
/// frame, 10 000 coefficients of 33 bytes and a region of 28 bytes a pixel, 610 000, the tile's 136
/// bytes of bookkeeping, and a row of 100 pixels, 400 B, for the one worker that takes the tile: a
/// peak of 660 536 B however many workers there are.
#[test]
fn the_statistics_and_the_decoders_bytes_set_the_way_s_peak() {
    let rggb = CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb));
    let info = |cfa_type, decoder| FrameInfo {
        desc: PlanarDesc::new((100, 100), ChannelLayout::Mono),
        cfa_type,
        decoder,
        gain_bytes: 0,
    };
    let bytes = |source, beside| DecoderBytes { source, beside };
    for (cfa_type, decoder, peak) in [
        (rggb, bytes(0, 0), 99_000),
        (rggb, bytes(0, 50_000), 100_000),
        (rggb, bytes(120_000, 0), 120_000),
        (rggb, bytes(30_000, 50_000), 100_000),
        (CfaType::Mono, bytes(0, 0), 660_536),
    ] {
        for workers in [1, 8] {
            assert_eq!(
                info(cfa_type, decoder).footprint(workers),
                Footprint { kept: 50_000, peak },
                "{cfa_type:?}, {decoder:?}, {workers} workers"
            );
        }
    }
}
