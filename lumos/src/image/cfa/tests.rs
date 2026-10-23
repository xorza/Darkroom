use std::f64::consts::FRAC_1_SQRT_2;
use std::path::Path;
use std::sync::Arc;

use common::{TempDir, fs};
use fits_well::FitsReader;
use fits_well::header::Header;
use fits_well::io::{ChecksumReport, ChecksumStatus};
use imaginarium::Buffer2;

use crate::frame::frame_stats::FrameStats;
use crate::image::calibration_state::CalibrationState;
use crate::image::cfa::cfa_image::*;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::*;
use crate::image::cfa::demosaic::Demosaic;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::channel_layout::ChannelLayout;
use crate::image::derived_facts::DerivedFacts;
use crate::image::flat_gain::FlatGain;
use crate::image::flat_gain::frame_gain::FrameGain;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::image_provenance::{ColorProvenance, DemosaicProvenance};
use crate::image::observation_header::ObservationHeader;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::image::sample_domain::{DomainMap, Pedestal, SampleDomain, ScaleOrigin};
use crate::internals::cfa::{XTRANS_PATTERN, make_cfa};
use crate::internals::fits::{fits_refusal, rewrite_fits};
use crate::internals::test_rng::TestRng;
use crate::io::fits::error::FitsRefusal;
use crate::io::fits::lumos_extension::LumosExtension;
use crate::io::fits::metadata::VERSION_KEYWORD;
use crate::io::load_ctx::LoadCtx;
use crate::io::{raw, writer};
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::noise::QUANTIZATION_SIGMA_PER_STEP;

#[test]
fn a_null_is_repaired_from_its_same_color_neighbours_before_demosaic() {
    // Mono, so the demosaic is a copy and what lands in the output is exactly what the repair put
    // there. The null holds a value with no relation to the frame — the decoder's frame-median fill
    // is what it would really be — and every one of its neighbours reads 0.5, so their median is
    // 0.5. Left alone, a mosaic demosaic would carry that 900 into every output pixel whose
    // interpolation reached it, which no mask covers.
    let size = Size2us::new(4usize, 4usize);
    let mut pixels = vec![0.5f32; size.pixel_count()];
    pixels[5] = 900.0;
    let mut nulls = vec![0.0f32; size.pixel_count()];
    nulls[5] = f32::NAN;
    let mut cfa = make_cfa(size, pixels, CfaType::Mono);
    cfa.set_flags(PixelFlags::of_non_finite(size, &[&nulls]));

    // A repaired null is flagged so, and a second repair leaves it as it is, though a neighbour
    // moved: the repair has one owner, and the demosaic does not repair it again.
    let mut repaired = cfa.clone();
    repaired.repair_nulls();
    assert_eq!(repaired.data()[5], 0.5);
    assert_eq!(
        repaired.flags().unwrap().at(5),
        QualityFlags::NO_DATA.union(QualityFlags::REPAIRED)
    );
    repaired.data_mut()[4] = 0.25;
    repaired.repair_nulls();
    assert_eq!(repaired.data()[5], 0.5);
    repaired.data_mut()[5] = 0.75;
    repaired
        .metadata
        .derived
        .set_calibration(CalibrationState::FLAT);
    let calibrated = repaired.demosaic_measured(MarkesteijnPasses::One);
    assert_eq!(calibrated.channel(0).pixels()[5], 0.75);

    // A calibrated frame read back from FITS holds the decoder's fill under its null, which the
    // file does not flag repaired (see the round trip below): the demosaic repairs it.
    let mut reloaded = cfa.clone();
    reloaded
        .metadata
        .derived
        .set_calibration(CalibrationState::FLAT);
    let reloaded = reloaded.demosaic_measured(MarkesteijnPasses::One);
    assert_eq!(reloaded.channel(0).pixels()[5], 0.5);

    let demosaiced = cfa.demosaic_measured(MarkesteijnPasses::One);
    assert_eq!(demosaiced.channel(0).pixels()[5], 0.5);
    // The mask stays at its own extent: this pixel was reconstructed, not measured, and the combine
    // still has to gate on that.
    assert!(
        demosaiced
            .flags()
            .unwrap()
            .mask_of(QualityFlags::NO_DATA)
            .get(5)
    );
    assert_eq!(demosaiced.flags().unwrap().count(QualityFlags::NO_DATA), 1);
}

/// Every flag survives the trip. `NO_DATA` goes back as NaN, the blank of the float `BITPIX`, so
/// any reader finds it; without it the repaired sample would reload as a measurement. All of them
/// go back in the `LUMFLAGS` extension, which is written only when there is more than `NO_DATA`,
/// but a null's `REPAIRED`: the NaN, not the repair, is what the file holds there.
///
/// The 3×2 plane holds, row-major: nothing, `NO_DATA | REPAIRED` (1 + 16 = 17), `SATURATED |
/// DEFECT` (2 + 4 = 6), nothing, `COSMIC_RAY | REPAIRED` (8 + 16 = 24) and `FLAT_FLOOR` (32). It
/// reloads with the null `NO_DATA` alone, 1.
#[test]
fn a_masters_flags_survive_the_fits_round_trip() {
    let size = Size2us::new(3, 2);
    let bytes = [0u8, 17, 6, 0, 24, 32];
    let cfa = |bytes: [u8; 6]| {
        let mut image = CfaImage::from_plane(
            Buffer2::new(3, 2, vec![0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6]),
            CfaType::Mono,
            ImageMetadata::default(),
        );
        image.set_flags(PixelFlags::from_fn(size, |idx| {
            QualityFlags::from_byte(bytes[idx])
        }));
        image
    };
    let dir = TempDir::new("lumos-cfa-flags");
    let path = dir.join("master.fits");
    let hdu_count = |path: &Path| {
        let bytes = fs::read(path).unwrap();
        FitsReader::from_bytes(&bytes).unwrap().hdus().len()
    };

    writer::cfa_fits(&cfa(bytes), &path).unwrap();
    assert_eq!(hdu_count(&path), 2);
    let loaded = LoadCtx::default().cfa(&path).unwrap();
    assert_eq!(loaded.flags().unwrap().bytes(), &[0, 1, 6, 0, 24, 32]);
    assert_eq!(loaded.flags().unwrap().count(QualityFlags::NO_DATA), 1);
    // The measured samples are untouched by the trip; only the null's own value is not what was
    // written, because what was written for it was "no measurement".
    let data = loaded.data().to_vec();
    assert_eq!(
        [data[0], data[2], data[3], data[4], data[5]],
        [0.1f32, 0.3, 0.4, 0.5, 0.6]
    );

    let nulls_path = dir.join("nulls.fits");
    writer::cfa_fits(&cfa([0u8, 17, 0, 0, 0, 0]), &nulls_path).unwrap();
    assert_eq!(hdu_count(&nulls_path), 1, "the NaN carries a lone NO_DATA");
    let loaded = LoadCtx::default().cfa(&nulls_path).unwrap();
    assert_eq!(loaded.flags().unwrap().bytes(), &[0, 1, 0, 0, 0, 0]);
}

/// A flat gain survives the trip in its `LUMGAIN` extension, node for node, so a light saved after
/// calibration reloads with the gain its noise model reads, a source gain its run shares, under
/// the key its header states. A 12×6 RGGB mosaic has 4×3 nodes per color. An extension Lumos did
/// not write is refused, each behind a valid checksum: a node that is no gain, another step,
/// another version, another grid, a key that is no hex digest, another gain's key, grids for
/// another count of colors, and one that names no image of the file. One that states no key loads
/// whole.
#[test]
fn a_flat_gain_survives_the_fits_round_trip() {
    type Edit = fn(&mut Header, &mut Vec<u8>);
    let size = Size2us::new(12, 6);
    let bayer = CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb));
    let gain = Arc::new(FlatGain::of_divisor(
        &Buffer2::from_fn(12, 6, |x, _| 1.0 - 0.3 * x as f32 / 12.0),
        &bayer,
        |_| false,
    ));
    let dir = TempDir::new("lumos-cfa-gain");
    let path = dir.join("light.fits");
    let write = || {
        let mut image = CfaImage::from_plane(
            Buffer2::new(12, 6, vec![0.25; size.pixel_count()]),
            bayer,
            ImageMetadata::default(),
        );
        image.set_flat_gain(Some(FrameGain::Shared(Arc::clone(&gain))));
        writer::cfa_fits(&image, &path).unwrap();
    };

    write();
    let loaded = LoadCtx::default().cfa(&path).unwrap();
    let Some(FrameGain::Shared(reloaded)) = loaded.flat_gain() else {
        panic!("a decoded gain is a source gain");
    };
    assert_eq!(reloaded.size(), size);
    assert!(reloaded.planes().eq(gain.planes()));
    assert_eq!(reloaded.key(), gain.key());

    let extension = LumosExtension::Gain;
    let cases: [(&str, Edit, FitsRefusal); 7] = [
        (
            "no gain",
            |_, data| data[..4].copy_from_slice(&(-1.0f32).to_be_bytes()),
            FitsRefusal::NotAGain {
                node: 0,
                value: -1.0,
            },
        ),
        (
            "another step",
            |header, _| {
                header.set("LUMGSTEP", 8).unwrap();
            },
            FitsRefusal::GainStep { step: Some(8) },
        ),
        (
            "another version",
            |header, _| {
                header.set(VERSION_KEYWORD, 2).unwrap();
            },
            FitsRefusal::ExtensionFormat {
                extension,
                format: Some("FLATGAIN".to_owned()),
                version: Some(2),
            },
        ),
        (
            "another grid",
            |header, _| {
                header.set("NAXIS1", 3).unwrap();
                header.set("NAXIS2", 4).unwrap();
            },
            FitsRefusal::ExtensionShape {
                extension,
                shape: vec![3, 4, 3],
                image: size,
            },
        ),
        (
            "a key of no hex",
            |header, _| {
                header.set("LUMGSUM", "x").unwrap();
            },
            FitsRefusal::GainKeyText {
                text: "x".to_owned(),
            },
        ),
        (
            "another gain's key",
            |header, _| {
                header.set("LUMGSUM", "0f".repeat(32)).unwrap();
            },
            FitsRefusal::GainKeyMismatch,
        ),
        (
            "no image named",
            |header, _| {
                header.set("LUMFOR", "SCI").unwrap();
            },
            FitsRefusal::ClaimsMissingImage {
                extension,
                hdu: 1,
                image: "SCI".to_owned(),
            },
        ),
    ];
    for (name, edit, expected) in cases {
        write();
        rewrite_fits(&path, |idx, header, data| {
            if idx == 1 {
                edit(header, data);
            }
            true
        });
        let error = LoadCtx::default().cfa(&path).unwrap_err();
        assert_eq!(fits_refusal(error), expected, "{name}");
    }
    write();
    rewrite_fits(&path, |idx, header, _| {
        if idx == 1 {
            header.remove("LUMGSUM");
        }
        true
    });
    let unkeyed = LoadCtx::default().cfa(&path).unwrap();
    assert!(unkeyed.flat_gain().unwrap().planes().eq(gain.planes()));
    assert_eq!(
        FitsRefusal::ExtensionShape {
            extension,
            shape: vec![3, 4, 3],
            image: size,
        }
        .to_string(),
        "LUMGAIN: shape [3, 4, 3] is not one or three 4x3 grids over the image's 12x6"
    );
    // A keyword the header states reads as its value, one it leaves out as no value at all.
    for (refusal, text) in [
        (
            FitsRefusal::GainStep { step: Some(8) },
            "LUMGAIN: a node every 8 pixels; this reader takes 4",
        ),
        (
            FitsRefusal::GainStep { step: None },
            "LUMGAIN: no node spacing stated; this reader takes 4",
        ),
        (
            FitsRefusal::ExtensionFormat {
                extension,
                format: None,
                version: Some(1),
            },
            "LUMGAIN: format none version 1; expected FLATGAIN version 1",
        ),
    ] {
        assert_eq!(refusal.to_string(), text);
    }

    // A mono frame's one plane under three colors' grids, through either loader: a file no image
    // writes, as an image takes no gain over other colors, so the mosaic's header is made mono.
    write();
    rewrite_fits(&path, |idx, header, _| {
        if idx == 0 {
            header.set("CFATYPE", "MONO").unwrap();
        }
        true
    });
    for error in [
        LoadCtx::default().cfa(&path).unwrap_err(),
        LoadCtx::default().linear(&path).unwrap_err(),
    ] {
        assert_eq!(
            fits_refusal(error),
            FitsRefusal::GainChannels {
                grids: 3,
                channels: 1
            }
        );
    }
}

/// Frames of one flat share its gain: a run holds it once, and a file whose `LUMGAIN` states the
/// key of a gain the run holds is taken without its nodes being read. Two 12×6 RGGB mosaics of one
/// gain, the second's nodes broken to −1 behind a valid checksum: through one run's gains both
/// load with the first's gain, one allocation; through a run of its own the second is refused.
#[test]
fn a_gain_the_run_holds_is_not_read_again() {
    let bayer = CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb));
    let gain = Arc::new(FlatGain::of_divisor(
        &Buffer2::from_fn(12, 6, |x, _| 1.0 - 0.3 * x as f32 / 12.0),
        &bayer,
        |_| false,
    ));
    let dir = TempDir::new("lumos-cfa-shared-gain");
    let paths = ["first.fits", "second.fits"].map(|name| {
        let path = dir.join(name);
        let mut image = CfaImage::from_plane(
            Buffer2::new(12, 6, vec![0.25; 72]),
            bayer,
            ImageMetadata::default(),
        );
        image.set_flat_gain(Some(FrameGain::Shared(Arc::clone(&gain))));
        writer::cfa_fits(&image, &path).unwrap();
        path
    });
    rewrite_fits(&paths[1], |idx, _, data| {
        if idx == 1 {
            data[..4].copy_from_slice(&(-1.0f32).to_be_bytes());
        }
        true
    });
    let ctx = LoadCtx::default();
    let gains = SharedGains::default();
    let [first, second] = paths
        .each_ref()
        .map(|path| ctx.cfa_sharing(path, &gains).unwrap());
    let (Some(FrameGain::Shared(first)), Some(FrameGain::Shared(second))) =
        (first.flat_gain(), second.flat_gain())
    else {
        panic!("a decoded gain is a source gain");
    };
    assert!(Arc::ptr_eq(first, second));
    assert!(first.planes().eq(gain.planes()));
    assert_eq!(
        fits_refusal(ctx.cfa(&paths[1]).unwrap_err()),
        FitsRefusal::NotAGain {
            node: 0,
            value: -1.0
        }
    );
}

/// A flags extension that is not the one Lumos wrote is refused, each behind a valid checksum so
/// the check behind it is what fires: a bit no flag has, a `NO_DATA` the NaNs do not state either
/// way, another geometry, another version, and an extension that names no image of the file. Its
/// checksum is judged before its bytes: a byte changed behind the stored checksum reads as that,
/// not as the `NO_DATA` it also drops.
#[test]
fn a_flags_extension_lumos_did_not_write_is_refused() {
    type Edit = fn(&mut Header, &mut Vec<u8>);
    let size = Size2us::new(3, 2);
    let bytes = [0u8, 1, 6, 0, 24, 32];
    let dir = TempDir::new("lumos-cfa-flags-refused");
    let path = dir.join("master.fits");
    let write = || {
        let image = {
            let mut image = CfaImage::from_plane(
                Buffer2::new(3, 2, vec![0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6]),
                CfaType::Mono,
                ImageMetadata::default(),
            );
            image.set_flags(PixelFlags::from_fn(size, |idx| {
                QualityFlags::from_byte(bytes[idx])
            }));
            image
        };
        writer::cfa_fits(&image, &path).unwrap();
    };
    let extension = LumosExtension::Flags;
    let cases: [(&str, Edit, FitsRefusal); 6] = [
        (
            "unknown bit",
            |_, data| data[5] |= 0x40,
            FitsRefusal::UnknownFlagBits {
                pixel: Vec2us::new(2, 1),
                byte: 0x60,
            },
        ),
        (
            "NO_DATA lost",
            |_, data| data[1] = 0,
            FitsRefusal::NoDataDisagrees {
                pixel: Vec2us::new(1, 0),
            },
        ),
        (
            "NO_DATA added",
            |_, data| data[3] = 1,
            FitsRefusal::NoDataDisagrees {
                pixel: Vec2us::new(0, 1),
            },
        ),
        (
            "another geometry",
            |header, _| {
                header.set("NAXIS1", 2).unwrap();
                header.set("NAXIS2", 3).unwrap();
            },
            FitsRefusal::ExtensionShape {
                extension,
                shape: vec![2, 3],
                image: size,
            },
        ),
        (
            "another version",
            |header, _| {
                header.set(VERSION_KEYWORD, 2).unwrap();
            },
            FitsRefusal::ExtensionFormat {
                extension,
                format: Some("PIXFLAGS".to_owned()),
                version: Some(2),
            },
        ),
        (
            "no image named",
            |header, _| {
                header.set("LUMFOR", "SCI").unwrap();
            },
            FitsRefusal::ClaimsMissingImage {
                extension,
                hdu: 1,
                image: "SCI".to_owned(),
            },
        ),
    ];
    for (name, edit, expected) in cases {
        write();
        rewrite_fits(&path, |idx, header, data| {
            if idx == 1 {
                edit(header, data);
            }
            true
        });
        let error = LoadCtx::default().cfa(&path).unwrap_err();
        assert_eq!(fits_refusal(error), expected, "{name}");
    }
    assert_eq!(
        FitsRefusal::ExtensionFormat {
            extension,
            format: Some("PIXFLAGS".to_owned()),
            version: Some(2),
        }
        .to_string(),
        "LUMFLAGS: format PIXFLAGS version 2; expected PIXFLAGS version 1"
    );

    // The flags are the last HDU, and their 6 bytes open its one 2880-byte data block.
    write();
    let mut file = fs::read(&path).unwrap();
    let flags_start = file.len() - 2880;
    file[flags_start + 1] = 0;
    fs::write(&path, &file).unwrap();
    let error = LoadCtx::default().cfa(&path).unwrap_err();
    assert_eq!(
        fits_refusal(error),
        FitsRefusal::ChecksumRequired {
            hdu: 1,
            report: ChecksumReport {
                datasum: ChecksumStatus::Invalid,
                checksum: ChecksumStatus::Invalid,
            },
        }
    );
}

/// The demosaic measures each color's noise on the mosaic, before the interpolation correlates
/// neighbours, and the frame's statistics take it from there: the demosaiced frame's noise, sky
/// and quantization σ are the mosaic's own, bit for bit, though the demosaic cleared the frame's
/// quantization σ. Measured on the frame's own correlated pixels instead, every channel reads less
/// than its color's σ. A 64 × 64 RGGB mosaic: red 0.125 ± 0.02, green 0.25 ± 0.01, blue
/// 0.375 ± 0.03, with a step of 1/4096. Divided by a flat, the frame keeps the flat's gain through
/// the demosaic, and its noise's split by that gain is the mosaic's too.
#[test]
fn a_demosaiced_frame_keeps_its_mosaics_noise() {
    let size = Size2us::new(64, 64);
    let cfa_type = CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb));
    let (level, sigma) = ([0.125f32, 0.25, 0.375], [0.02f32, 0.01, 0.03]);
    let mut rng = TestRng::new(7);
    let pixels = (0..size.pixel_count())
        .map(|idx| {
            let color = cfa_type.slot_at(Vec2us::new(idx % 64, idx / 64));
            level[color] + sigma[color] * rng.next_gaussian_f32()
        })
        .collect();
    let mut cfa = make_cfa(size, pixels, cfa_type);
    cfa.metadata
        .derived
        .set_quantization_sigma(Some(QUANTIZATION_SIGMA_PER_STEP / 4096.0));
    let gain = Arc::new(FlatGain::of_divisor(
        &Buffer2::from_fn(64, 64, |x, _| 1.0 - 0.5 * x as f32 / 64.0),
        &cfa_type,
        |_| false,
    ));
    let mut flattened = cfa.clone();
    flattened.set_flat_gain(Some(FrameGain::Shared(Arc::clone(&gain))));
    let flattened_mosaic = FrameStats::measure(&flattened);
    let flattened = flattened.demosaic_measured(MarkesteijnPasses::One);
    let Some(FrameGain::Shared(carried)) = flattened.flat_gain() else {
        panic!("the demosaic carries the shared gain");
    };
    assert!(Arc::ptr_eq(carried, &gain));
    let flattened_frame = FrameStats::measure(&flattened);
    assert_eq!(flattened_frame.bg, flattened_mosaic.bg);

    let mosaic = FrameStats::measure(&cfa);
    let mut demosaiced = cfa.demosaic_measured(MarkesteijnPasses::One);
    assert_eq!(demosaiced.metadata.derived.quantization_sigma(), None);
    let frame = FrameStats::measure(&demosaiced);
    assert_eq!(frame.bg, mosaic.bg);
    assert_eq!(frame.sky, mosaic.sky);
    assert_eq!(frame.quantization_sigma, mosaic.quantization_sigma);
    assert!(frame.bg.iter().all(|split| split.read_share == 0.0));

    demosaiced.metadata.derived.set_mosaic_noise(None);
    let correlated = FrameStats::measure(&demosaiced);
    for color in 0..3 {
        assert!(
            correlated.bg[color].variance < mosaic.bg[color].variance,
            "color {color}: {:?} against the mosaic's {:?}",
            correlated.bg[color],
            mosaic.bg[color]
        );
    }
}

/// A master records its whole domain and its quantization σ, so a reload gives back exactly what
/// was saved, for every origin and every pedestal: the σ is not divided by the span a second time
/// (15360× too small for a RAW master), and an assumed scale reloads as assumed rather than as a
/// scale of 1 the master would be refused for. The scale goes out as `BSCALE`, so a reader that
/// knows nothing of lumos gets `0.25 × scale` in the stated `BUNIT`.
#[test]
fn a_masters_sample_domain_and_quantization_survive_the_fits_round_trip() {
    let dir = TempDir::new("lumos-cfa-domain");
    let mut case = 0;
    for origin in [ScaleOrigin::Declared, ScaleOrigin::Assumed] {
        for pedestal in [Pedestal::Removed, Pedestal::Kept(2048.0), Pedestal::Unknown] {
            for (scale, unit) in [
                (15_360.0, None),
                (1_234.567_8, Some("ADU")),
                (65_535.0, None),
            ] {
                case += 1;
                let mut master = make_cfa(Size2us::new(2, 2), vec![0.25; 4], CfaType::Mono);
                let domain = SampleDomain {
                    scale,
                    origin,
                    pedestal,
                    unit: unit.map(str::to_owned),
                };
                master.metadata.derived.set_domain(Some(domain.clone()));
                master
                    .metadata
                    .derived
                    .set_quantization_sigma(Some(QUANTIZATION_SIGMA_PER_STEP / 15_360.0));
                let path = dir.path().join(format!("master_{case}.fits"));
                writer::cfa_fits(&master, &path).unwrap();
                let bytes = fs::read(&path).unwrap();
                let reader = FitsReader::from_bytes(&bytes).unwrap();
                let header = &reader.hdus()[0].header;
                assert_eq!(header.get_real("BSCALE").unwrap(), Some(scale), "{domain}");
                assert_eq!(header.get_text("BUNIT").unwrap(), unit, "{domain}");
                let loaded = LoadCtx::default().cfa(&path).unwrap();

                assert_eq!(loaded.metadata.derived.domain(), Some(&domain), "{domain}");
                assert_eq!(
                    loaded.metadata.derived.quantization_sigma(),
                    master.metadata.derived.quantization_sigma(),
                    "{domain}"
                );
                assert_eq!(
                    loaded.data().to_vec(),
                    vec![0.25; 4],
                    "samples are stored normalized"
                );
                assert_eq!(
                    loaded
                        .metadata
                        .derived
                        .domain()
                        .unwrap()
                        .conversion_to(&domain),
                    Some(DomainMap::IDENTITY),
                    "{domain}"
                );
            }
        }
    }
}

#[test]
fn master_cfa_save_load_round_trips_data_and_pattern() {
    let cfa = CfaImage::from_plane(
        Buffer2::new(2, 2, vec![0.1f32, 0.2, 0.3, 0.4]),
        CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Bggr)),
        ImageMetadata {
            header: ObservationHeader {
                camera_white_balance: Some([2.0, 1.0, 1.5, 1.0]),
                ..Default::default()
            },
            derived: DerivedFacts::builder()
                .quantization_sigma(Some(0.000_01))
                .build(),
        },
    );
    let dir = TempDir::new("lumos-cfa-roundtrip");
    let path = dir.join("master.fits");
    writer::cfa_fits(&cfa, &path).unwrap();
    let info = LoadCtx::default().cfa_peek(&path).unwrap().info;
    assert_eq!(
        info.dimensions,
        ImageDimensions::new((2, 2), ChannelLayout::Mono)
    );
    assert_eq!(
        info.cfa_type,
        CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Bggr))
    );
    let loaded = LoadCtx::default().cfa(&path).unwrap();

    assert_eq!((loaded.data().width(), loaded.data().height()), (2, 2));
    assert_eq!(loaded.data().to_vec(), vec![0.1f32, 0.2, 0.3, 0.4]);
    assert_eq!(
        loaded.cfa_type(),
        CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Bggr))
    );
    assert_eq!(
        loaded.metadata.header.camera_white_balance,
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(loaded.metadata.derived.quantization_sigma(), Some(0.000_01));

    let original = fs::read(&path).unwrap();
    let mut invalid_version = original.clone();
    let version_card = invalid_version
        .windows(8)
        .position(|window| window == VERSION_KEYWORD.as_bytes())
        .unwrap();
    let version_digit = invalid_version[version_card..version_card + 80]
        .iter()
        .rposition(u8::is_ascii_digit)
        .unwrap();
    invalid_version[version_card + version_digit] = b'0';
    fs::write(&path, invalid_version).unwrap();
    let error = LoadCtx::default().cfa(&path).unwrap_err();
    assert_eq!(
        fits_refusal(error),
        FitsRefusal::CfaVersion { version: Some(0) }
    );

    let mut corrupted = original;
    let sample = 0.1f32.to_be_bytes();
    let offset = corrupted
        .windows(sample.len())
        .position(|window| window == sample)
        .unwrap();
    corrupted[offset] ^= 0x01;
    fs::write(&path, corrupted).unwrap();
    let error = LoadCtx::default().cfa(&path).unwrap_err();
    assert_eq!(
        fits_refusal(error),
        FitsRefusal::ChecksumRequired {
            hdu: 0,
            report: ChecksumReport {
                datasum: ChecksumStatus::Invalid,
                checksum: ChecksumStatus::Invalid,
            },
        }
    );
}

#[test]
fn master_cfa_fits_round_trips_mono_and_xtrans_patterns() {
    let dir = TempDir::new("lumos-cfa-types");
    for (name, cfa_type) in [
        ("mono", CfaType::Mono),
        ("xtrans", CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN))),
    ] {
        let image = CfaImage::from_plane(
            Buffer2::new(2, 2, vec![0.1f32, 0.2, 0.3, 0.4]),
            cfa_type,
            ImageMetadata::default(),
        );
        let path = dir.join(format!("master_{name}.fits"));

        writer::cfa_fits(&image, &path).unwrap();
        let loaded = LoadCtx::default().cfa(path).unwrap();

        assert_eq!(loaded.cfa_type(), cfa_type, "{name}");
        assert_eq!(loaded.data().pixels(), image.data().pixels(), "{name}");
    }
}

#[test]
fn data_len() {
    let img = CfaImage::from_plane(
        Buffer2::new(10, 20, vec![0.0; 200]),
        CfaType::Mono,
        ImageMetadata::default(),
    );
    assert_eq!(img.data().len(), 200);
    assert_eq!(img.metadata.derived.quantization_sigma(), None);
}

/// Each pattern names the demosaic lumos runs on it and what its output color means.
#[test]
fn each_pattern_names_its_demosaic() {
    for (cfa_type, demosaic, color) in [
        (
            CfaType::Mono,
            DemosaicProvenance::None,
            ColorProvenance::Monochrome,
        ),
        (
            CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
            DemosaicProvenance::LumosRcd,
            ColorProvenance::SensorRgb,
        ),
        (
            CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN)),
            DemosaicProvenance::LumosMarkesteijn {
                passes: MarkesteijnPasses::Three,
            },
            ColorProvenance::SensorRgb,
        ),
    ] {
        assert_eq!(
            Demosaic::of(cfa_type, MarkesteijnPasses::Three).provenance(),
            demosaic,
            "{cfa_type:?}"
        );
        assert_eq!(cfa_type.planes_color(), color, "{cfa_type:?}");
    }
}

/// The color at every position of two periods, for each pattern, from each source that states it:
/// the `BAYERPAT` spelling read letter by letter, LibRaw's `filters` word, `CfaPattern`, and
/// `CfaType`; mono is red-index 0 everywhere and X-Trans is its rows, both wrapping.
///
/// The photosites of each color, counted per phase, match a count of `color_at` over the frame. A
/// 5 × 3 RGGB has its reds at even columns of even rows, 3 × 2 = 6, its blues at odd of odd, 2 × 1
/// = 2, and the greens the other 7; a 6 × 6 X-Trans period has 8 reds, 20 greens and 8 blues.
#[test]
fn every_pattern_names_the_color_at_each_position() {
    let color_of = |letter: u8| -> usize {
        match letter {
            b'R' => 0,
            b'G' => 1,
            b'B' => 2,
            _ => unreachable!(),
        }
    };
    for (pattern, filters) in [
        (CfaPattern::Rggb, 0x9494_9494),
        (CfaPattern::Bggr, 0x1616_1616),
        (CfaPattern::Grbg, 0x6161_6161),
        (CfaPattern::Gbrg, 0x4949_4949),
    ] {
        let spelling = pattern.bayerpat().as_bytes();
        for y in 0..4 {
            for x in 0..4 {
                let expected = color_of(spelling[(y % 2) * 2 + x % 2]);
                let pos = Vec2us::new(x, y);
                assert_eq!(pattern.color_at(pos).idx(), expected, "{pattern:?} {pos:?}");
                assert_eq!(
                    CfaType::Mosaic(Mosaic::Bayer(pattern)).slot_at(pos),
                    expected
                );
                assert_eq!(raw::filter_color(filters, y, x), expected);
            }
        }
    }
    for y in 0..12 {
        for x in 0..12 {
            let pos = Vec2us::new(x, y);
            assert_eq!(CfaType::Mono.slot_at(pos), 0);
            assert_eq!(
                CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN)).slot_at(pos),
                usize::from(XTRANS_PATTERN.rows()[y % 6][x % 6])
            );
        }
    }

    let bayer = CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb));
    let xtrans = CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN));
    assert_eq!(
        bayer.photosites_per_color(Size2us::new(5, 3)).as_slice(),
        [6, 7, 2]
    );
    assert_eq!(
        xtrans.photosites_per_color(Size2us::new(6, 6)).as_slice(),
        [8, 20, 8]
    );
    // An 8 × 8 Bayer frame at 10 samples a color steps by ⌊64 / 30⌋ = 2 periods, a 24 × 24
    // X-Trans one at 48 by ⌊576 / 144⌋ = 4: whole periods of rows, never single rows, so every row
    // of the pattern is sampled. A mono frame's period is a row, and its one color at 30 samples
    // steps by ⌊64 / 30⌋ = 2.
    let rows = |cfa: CfaType, side: usize, per_color: usize| -> Vec<usize> {
        cfa.sampled_rows(Size2us::new(side, side), per_color)
            .collect()
    };
    assert_eq!(rows(bayer, 8, 10), [0, 1, 4, 5]);
    assert_eq!(rows(xtrans, 24, 48), [0, 1, 2, 3, 4, 5]);
    assert_eq!(rows(CfaType::Mono, 8, 30), [0, 2, 4, 6]);
    assert_eq!(rows(bayer, 8, 64), (0..8).collect::<Vec<_>>());
    for cfa in [CfaType::Mono, bayer, xtrans] {
        for size in [Size2us::new(1, 1), Size2us::new(5, 3), Size2us::new(7, 13)] {
            let mut counted = vec![0; cfa.color_count()];
            for y in 0..size.height {
                for x in 0..size.width {
                    counted[cfa.slot_at(Vec2us::new(x, y))] += 1;
                }
            }
            assert_eq!(
                cfa.photosites_per_color(size).as_slice(),
                counted,
                "{cfa:?} {size:?}"
            );
        }
    }
}

/// No input photosite moves an output pixel farther away than `Demosaic::support`. An
/// impulse of 20 on random texture, at several phases of each pattern, changes no output pixel past
/// the support by more than 2⁻¹⁶ of the impulse. A wider survey, 40 textures at 6 phases on 128²
/// frames, reached exactly 10 for RCD and 11 and 16 for Markesteijn's one and three passes, by any
/// change at all for Markesteijn; this sample reaches 9, 11 and 14.
#[test]
fn the_demosaic_support_bounds_every_impulse_response() {
    const SIDE: usize = 64;
    for (cfa_type, passes) in [
        (
            CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
            MarkesteijnPasses::One,
        ),
        (
            CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN)),
            MarkesteijnPasses::One,
        ),
        (
            CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN)),
            MarkesteijnPasses::Three,
        ),
    ] {
        let size = Size2us::new(SIDE, SIDE);
        let mut reach = 0usize;
        for seed in 1..=6u64 {
            let mut rng = TestRng::new(seed);
            let base: Vec<f32> = (0..size.pixel_count())
                .map(|_| 0.2 + 0.3 * rng.next_f32())
                .collect();
            let reference = make_cfa(size, base.clone(), cfa_type).demosaic_measured(passes);
            for (cx, cy) in [(30usize, 30usize), (31, 30), (31, 31), (33, 32)] {
                let mut pixels = base.clone();
                pixels[cy * SIDE + cx] = 20.0;
                let threshold = (20.0 - base[cy * SIDE + cx]) / 65_536.0;
                let hit = make_cfa(size, pixels, cfa_type).demosaic_measured(passes);
                for channel in 0..3 {
                    for y in 0..SIDE {
                        for x in 0..SIDE {
                            let moved = (hit.channel(channel)[(x, y)]
                                - reference.channel(channel)[(x, y)])
                                .abs();
                            if moved > threshold {
                                reach = reach.max(x.abs_diff(cx).max(y.abs_diff(cy)));
                            }
                        }
                    }
                }
            }
        }
        assert!(
            reach <= Demosaic::of(cfa_type, passes).support(),
            "{cfa_type:?}, {passes:?}: {reach}"
        );
    }
}

/// A flag on one photosite covers every output pixel the demosaic reads it into, and `NO_DATA`
/// stays where it was: a saturated photosite at (20, 20) flags the 21 × 21 square around it.
#[test]
fn demosaic_spreads_flags_by_its_support() {
    let size = Size2us::new(48usize, 48usize);
    let mut cfa = make_cfa(
        size,
        vec![0.25; size.pixel_count()],
        CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
    );
    cfa.set_flags(PixelFlags::from_fn(size, |idx| match idx {
        idx if idx == 20 * 48 + 20 => QualityFlags::SATURATED,
        idx if idx == 40 * 48 + 40 => QualityFlags::NO_DATA,
        _ => QualityFlags::default(),
    }));
    let flags = cfa
        .demosaic_measured(MarkesteijnPasses::One)
        .flags()
        .cloned()
        .unwrap();
    assert_eq!(flags.count(QualityFlags::SATURATED), 21 * 21);
    assert_eq!(flags.count(QualityFlags::NO_DATA), 1);
    assert!(
        flags
            .at_pos(Vec2us::new(10, 30))
            .intersects(QualityFlags::SATURATED)
    );
    assert!(
        !flags
            .at_pos(Vec2us::new(31, 20))
            .intersects(QualityFlags::SATURATED)
    );
}

/// Neutral detail through a sensor whose channels respond with gains 1/2, 1 and 1/1.5 keeps less
/// false color when the camera balance is applied before the demosaic: the direction decisions
/// compare neighbours of different colors, and on the unbalanced mosaic the cast reads as
/// structure. Balanced back up, red and blue should equal green; their RMS difference over the
/// interior, measured, is 0.0013 against 0.0156 for RCD on a star of σ 1.3 px and 0.0008 against
/// 0.0032 on a soft edge, and 0.0045 against 0.023 and 0.0008 against 0.0094 for Markesteijn. The
/// test asks for a third of the unbalanced error at most, below the least ratio measured, 4.2.
#[test]
fn a_balanced_demosaic_keeps_neutral_detail_neutral() {
    let size = Size2us::new(48, 48);
    let gains = [2.0f32, 1.0, 1.5];
    let star = |x: usize, y: usize| {
        let (dx, dy) = (x as f32 - 23.6, y as f32 - 24.3);
        0.1 + 0.8 * (-(dx * dx + dy * dy) / (2.0 * 1.3 * 1.3)).exp()
    };
    let soft_edge =
        |x: usize, y: usize| 0.5 + 0.3 * ((x as f32 + 0.37 * y as f32 - 30.0) / 1.5).tanh();
    let scenes: [&dyn Fn(usize, usize) -> f32; 2] = [&star, &soft_edge];
    for cfa_type in [
        CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
        CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN)),
    ] {
        for (scene_idx, scene) in scenes.iter().enumerate() {
            let samples: Vec<f32> = (0..size.pixel_count())
                .map(|idx| {
                    let (x, y) = (idx % size.width, idx / size.width);
                    scene(x, y) / gains[cfa_type.slot_at(Vec2us::new(x, y))]
                })
                .collect();
            let false_color = |balance: Option<[f32; 4]>| {
                let mut cfa = make_cfa(size, samples.clone(), cfa_type);
                cfa.metadata.header.camera_white_balance = balance;
                let image = cfa.demosaic_measured(MarkesteijnPasses::One);
                let (mut sum, mut count) = (0.0f64, 0);
                for y in 12..36 {
                    for x in 12..36 {
                        let green = image.channel(1)[(x, y)];
                        for (channel, gain) in [(0, gains[0]), (2, gains[2])] {
                            let error = image.channel(channel)[(x, y)] * gain - green;
                            sum += f64::from(error * error);
                            count += 1;
                        }
                    }
                }
                (sum / f64::from(count)).sqrt()
            };
            let balanced = false_color(Some([2.0, 1.0, 1.5, 1.0]));
            let unbalanced = false_color(None);
            assert!(
                balanced * 3.0 < unbalanced,
                "{cfa_type:?} scene {scene_idx}: balanced {balanced}, unbalanced {unbalanced}"
            );
        }
    }
}

/// The green proxy keeps every green photosite and fills the others from the greens of their 3×3
/// neighbourhood, weighed 1 beside and 1/√2 on a diagonal, over the mosaic `v = x + 10y`.
///
/// RGGB: red (0, 0) has greens (1, 0) and (0, 1) beside it, 1 and 10, so 5.5; blue (1, 1) has
/// greens 10, 12, 1 and 21 beside it, so 11 — a Bayer site's diagonals are never green. X-Trans:
/// red (2, 0) has greens (1, 0) and (3, 0) beside it, 1 and 3, and (1, 1) and (3, 1) on its
/// diagonals, 11 and 13, so `(4 + 24r) / (2 + 2r)` with `r = 1/√2`. A filled photosite carries
/// the flags of the greens it was filled from, a green its own: green (1, 0) is saturated, and
/// so are the red (0, 0) and blue (1, 1) it fills, while blue (3, 3), whose greens are (2, 3),
/// (4, 3), (3, 2) and (3, 4), carries nothing.
#[test]
fn the_green_proxy_fills_each_photosite_from_its_greens() {
    let size = Size2us::new(6, 6);
    let pixels: Vec<f32> = (0..size.pixel_count())
        .map(|idx| (idx % 6 + 10 * (idx / 6)) as f32)
        .collect();
    let mut bayer = make_cfa(
        size,
        pixels.clone(),
        CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
    );
    bayer.add_flags_where(QualityFlags::SATURATED, |idx| idx == 1);
    let proxy = bayer.green_proxy();
    let plane = proxy.channel(0);
    assert_eq!(plane[(0, 0)], 5.5);
    assert_eq!(plane[(1, 1)], 11.0);
    assert_eq!(plane[(1, 0)], 1.0, "a green photosite is kept");
    let flags = proxy.flags().unwrap();
    for (pixel, expected) in [
        (1, QualityFlags::SATURATED),
        (0, QualityFlags::SATURATED),
        (7, QualityFlags::SATURATED),
        (21, QualityFlags::default()),
    ] {
        assert_eq!(flags.at(pixel), expected, "pixel {pixel}");
    }
    assert_eq!(
        proxy.metadata.derived.provenance().map(|p| p.demosaic),
        bayer
            .metadata
            .derived
            .provenance()
            .map(|_| DemosaicProvenance::GreenProxy)
    );

    let xtrans = make_cfa(
        size,
        pixels.clone(),
        CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN)),
    )
    .green_proxy();
    let r = FRAC_1_SQRT_2;
    let expected = ((4.0 + 24.0 * r) / (2.0 + 2.0 * r)) as f32;
    let actual = xtrans.channel(0)[(2, 0)];
    assert!(
        (actual - expected).abs() <= f32::EPSILON * expected,
        "{actual}, expected {expected}"
    );

    // A filled mosaic mixes its samples, so its quantization step goes; a mono frame is its own
    // proxy, samples and metadata alike.
    let quantized = |cfa_type| {
        let mut frame = make_cfa(size, pixels.clone(), cfa_type);
        frame.metadata.derived.set_quantization_sigma(Some(0.25));
        frame
    };
    let bayer = quantized(CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb))).green_proxy();
    assert_eq!(bayer.metadata.derived.quantization_sigma(), None);
    let mono = quantized(CfaType::Mono);
    let proxy = mono.green_proxy();
    assert_eq!(proxy.channel(0).pixels(), mono.data().pixels());
    assert_eq!(proxy.metadata.derived.quantization_sigma(), Some(0.25));
    assert_eq!(
        proxy.metadata.derived.provenance().map(|p| p.demosaic),
        mono.metadata.derived.provenance().map(|p| p.demosaic)
    );
}
