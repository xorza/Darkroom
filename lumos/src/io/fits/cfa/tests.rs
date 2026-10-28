//! The Lumos CFA image's FITS file, written and read back from its bytes.

use std::io::Cursor;
use std::sync::Arc;

use common::{Buffer2, USize2};
use fits_well::FitsReader;
use fits_well::header::Header;
use fits_well::io::{ChecksumReport, ChecksumStatus};
use glam::USizeVec2;
use imaginarium::{ChannelLayout, PlanarDesc};

use crate::image::cfa::bayer_pattern::BayerPattern;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_flags::QualityFlags;
use crate::image::flat_gain::FlatGain;
use crate::image::flat_gain::frame_gain::FrameGain;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::metadata::derived_facts::DerivedFacts;
use crate::image::metadata::image_metadata::ImageMetadata;
use crate::image::metadata::observation_header::ObservationHeader;
use crate::image::metadata::sample_domain::{DomainMap, Pedestal, SampleDomain, ScaleOrigin};
use crate::internals::cfa::{XTRANS_PATTERN, make_cfa};
use crate::internals::fits::{fits_refusal, load_cfa, load_linear, open_fits, rewritten_fits};
use crate::io::fits::cfa;
use crate::io::fits::decode::fits_file::FitsFile;
use crate::io::fits::error::FitsRefusal;
use crate::io::fits::lumos_extension::LumosExtension;
use crate::io::fits::metadata::VERSION_KEYWORD;
use crate::io::load_ctx::LoadCtx;
use crate::noise::QUANTIZATION_SIGMA_PER_STEP;

/// `image` as the bytes of the FITS file [`cfa::write`] makes of it.
fn written(image: &CfaImage) -> Vec<u8> {
    let mut file = Cursor::new(Vec::new());
    cfa::write(image, &mut file).unwrap();
    file.into_inner()
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
    let size = USize2::new(3, 2);
    let bytes = [0u8, 17, 6, 0, 24, 32];
    let cfa = |bytes: [u8; 6]| {
        let mut image = CfaImage::from_plane(
            Buffer2::new((3, 2), vec![0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6]),
            CfaType::Mono,
            ImageMetadata::default(),
        );
        image.set_flags(PixelFlags::from_fn(size, |idx| {
            QualityFlags::from_byte(bytes[idx])
        }));
        image
    };
    let hdu_count = |file: &[u8]| FitsReader::from_bytes(file).unwrap().hdus().len();

    let file = written(&cfa(bytes));
    assert_eq!(hdu_count(&file), 2);
    let loaded = load_cfa(&file, &LoadCtx::default()).unwrap();
    assert_eq!(loaded.flags().unwrap().bytes(), &[0, 1, 6, 0, 24, 32]);
    assert_eq!(loaded.flags().unwrap().count(QualityFlags::NO_DATA), 1);
    // The measured samples are untouched by the trip; only the null's own value is not what was
    // written, because what was written for it was "no measurement".
    let data = loaded.data().to_vec();
    assert_eq!(
        [data[0], data[2], data[3], data[4], data[5]],
        [0.1f32, 0.3, 0.4, 0.5, 0.6]
    );

    let nulls = written(&cfa([0u8, 17, 0, 0, 0, 0]));
    assert_eq!(hdu_count(&nulls), 1, "the NaN carries a lone NO_DATA");
    let loaded = load_cfa(&nulls, &LoadCtx::default()).unwrap();
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
    let size = USize2::new(12, 6);
    let bayer = CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb));
    let gain = Arc::new(FlatGain::of_divisor(
        &Buffer2::from_fn((12, 6), |x, _| 1.0 - 0.3 * x as f32 / 12.0),
        &bayer,
        |_| false,
    ));
    let file = {
        let mut image = CfaImage::from_plane(
            Buffer2::new((12, 6), vec![0.25; size.pixel_count()]),
            bayer,
            ImageMetadata::default(),
        );
        image.set_flat_gain(Some(FrameGain::Shared(Arc::clone(&gain))));
        written(&image)
    };

    let loaded = load_cfa(&file, &LoadCtx::default()).unwrap();
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
        let edited = rewritten_fits(&file, |idx, header, data| {
            if idx == 1 {
                edit(header, data);
            }
            true
        });
        let error = load_cfa(&edited, &LoadCtx::default()).unwrap_err();
        assert_eq!(fits_refusal(error), expected, "{name}");
    }
    let unkeyed = rewritten_fits(&file, |idx, header, _| {
        if idx == 1 {
            header.remove("LUMGSUM").unwrap();
        }
        true
    });
    let unkeyed = load_cfa(&unkeyed, &LoadCtx::default()).unwrap();
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
    let mono = rewritten_fits(&file, |idx, header, _| {
        if idx == 0 {
            header.set("CFATYPE", "MONO").unwrap();
        }
        true
    });
    for error in [
        load_cfa(&mono, &LoadCtx::default()).unwrap_err(),
        load_linear(&mono, &LoadCtx::default()).unwrap_err(),
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
    let bayer = CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb));
    let gain = Arc::new(FlatGain::of_divisor(
        &Buffer2::from_fn((12, 6), |x, _| 1.0 - 0.3 * x as f32 / 12.0),
        &bayer,
        |_| false,
    ));
    let first_file = {
        let mut image = CfaImage::from_plane(
            Buffer2::new((12, 6), vec![0.25; 72]),
            bayer,
            ImageMetadata::default(),
        );
        image.set_flat_gain(Some(FrameGain::Shared(Arc::clone(&gain))));
        written(&image)
    };
    let second_file = rewritten_fits(&first_file, |idx, _, data| {
        if idx == 1 {
            data[..4].copy_from_slice(&(-1.0f32).to_be_bytes());
        }
        true
    });
    let ctx = LoadCtx::default();
    let gains = SharedGains::default();
    let [first, second] = [&first_file, &second_file]
        .map(|file| open_fits(file, &ctx).unwrap().cfa(&ctx, &gains).unwrap());
    let (Some(FrameGain::Shared(first)), Some(FrameGain::Shared(second))) =
        (first.flat_gain(), second.flat_gain())
    else {
        panic!("a decoded gain is a source gain");
    };
    assert!(Arc::ptr_eq(first, second));
    assert!(first.planes().eq(gain.planes()));
    assert_eq!(
        fits_refusal(load_cfa(&second_file, &ctx).unwrap_err()),
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
    let size = USize2::new(3, 2);
    let bytes = [0u8, 1, 6, 0, 24, 32];
    let file = {
        let image = {
            let mut image = CfaImage::from_plane(
                Buffer2::new((3, 2), vec![0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6]),
                CfaType::Mono,
                ImageMetadata::default(),
            );
            image.set_flags(PixelFlags::from_fn(size, |idx| {
                QualityFlags::from_byte(bytes[idx])
            }));
            image
        };
        written(&image)
    };
    let extension = LumosExtension::Flags;
    let cases: [(&str, Edit, FitsRefusal); 6] = [
        (
            "unknown bit",
            |_, data| data[5] |= 0x40,
            FitsRefusal::UnknownFlagBits {
                pixel: USizeVec2::new(2, 1),
                byte: 0x60,
            },
        ),
        (
            "NO_DATA lost",
            |_, data| data[1] = 0,
            FitsRefusal::NoDataDisagrees {
                pixel: USizeVec2::new(1, 0),
            },
        ),
        (
            "NO_DATA added",
            |_, data| data[3] = 1,
            FitsRefusal::NoDataDisagrees {
                pixel: USizeVec2::new(0, 1),
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
        let edited = rewritten_fits(&file, |idx, header, data| {
            if idx == 1 {
                edit(header, data);
            }
            true
        });
        let error = load_cfa(&edited, &LoadCtx::default()).unwrap_err();
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
    let mut file = file;
    let flags_start = file.len() - 2880;
    file[flags_start + 1] = 0;
    let error = load_cfa(&file, &LoadCtx::default()).unwrap_err();
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

/// A master records its whole domain and its quantization σ, so a reload gives back exactly what
/// was saved, for every origin and every pedestal: the σ is not divided by the span a second time
/// (15360× too small for a RAW master), and an assumed scale reloads as assumed rather than as a
/// scale of 1 the master would be refused for. The scale goes out as `BSCALE`, so a reader that
/// knows nothing of lumos gets `0.25 × scale` in the stated `BUNIT`.
#[test]
fn a_masters_sample_domain_and_quantization_survive_the_fits_round_trip() {
    for origin in [ScaleOrigin::Declared, ScaleOrigin::Assumed] {
        for pedestal in [Pedestal::Removed, Pedestal::Kept(2048.0), Pedestal::Unknown] {
            for (scale, unit) in [
                (15_360.0, None),
                (1_234.567_8, Some("ADU")),
                (65_535.0, None),
            ] {
                let mut master = make_cfa(USize2::new(2, 2), vec![0.25; 4], CfaType::Mono);
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
                let file = written(&master);
                let reader = FitsReader::from_bytes(&file).unwrap();
                let header = &reader.hdus()[0].header;
                assert_eq!(header.get_real("BSCALE").unwrap(), Some(scale), "{domain}");
                assert_eq!(header.get_text("BUNIT").unwrap(), unit, "{domain}");
                let loaded = load_cfa(&file, &LoadCtx::default()).unwrap();

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
        Buffer2::new((2, 2), vec![0.1f32, 0.2, 0.3, 0.4]),
        CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Bggr)),
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
    let original = written(&cfa);
    let info = open_fits(&original, &LoadCtx::default())
        .and_then(FitsFile::cfa_peek)
        .unwrap()
        .info;
    assert_eq!(info.desc, PlanarDesc::new((2, 2), ChannelLayout::Mono));
    assert_eq!(
        info.cfa_type,
        CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Bggr))
    );
    let loaded = load_cfa(&original, &LoadCtx::default()).unwrap();

    assert_eq!((loaded.data().width(), loaded.data().height()), (2, 2));
    assert_eq!(loaded.data().to_vec(), vec![0.1f32, 0.2, 0.3, 0.4]);
    assert_eq!(
        loaded.cfa_type(),
        CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Bggr))
    );
    assert_eq!(
        loaded.metadata.header.camera_white_balance,
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(loaded.metadata.derived.quantization_sigma(), Some(0.000_01));

    let mut invalid_version = original.clone();
    let version_card = invalid_version
        .windows(8)
        .position(|window| window == VERSION_KEYWORD.as_str().as_bytes())
        .unwrap();
    let version_digit = invalid_version[version_card..version_card + 80]
        .iter()
        .rposition(u8::is_ascii_digit)
        .unwrap();
    invalid_version[version_card + version_digit] = b'0';
    let error = load_cfa(&invalid_version, &LoadCtx::default()).unwrap_err();
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
    let error = load_cfa(&corrupted, &LoadCtx::default()).unwrap_err();
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
    for (name, cfa_type) in [
        ("mono", CfaType::Mono),
        ("xtrans", CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN))),
    ] {
        let image = CfaImage::from_plane(
            Buffer2::new((2, 2), vec![0.1f32, 0.2, 0.3, 0.4]),
            cfa_type,
            ImageMetadata::default(),
        );
        let loaded = load_cfa(&written(&image), &LoadCtx::default()).unwrap();

        assert_eq!(loaded.cfa_type(), cfa_type, "{name}");
        assert_eq!(loaded.data().pixels(), image.data().pixels(), "{name}");
    }
}
