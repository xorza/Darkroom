//! [`UnpackedRaw`]: a camera RAW file LibRaw unpacked, and what lumos reads of it.

use std::time::Instant;
use std::{array, mem};

use common::{Buffer2, par};
use imaginarium::{ChannelLayout, PlanarDesc};
use libraw::{Libraw, ProcessParams, ProcessedSamples, SensorLayout};
use rayon::prelude::*;

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::flags::quality_flags::SATURATION_FRACTION;
use crate::image::linear_image::LinearImage;
use crate::image::metadata::derived_facts::DerivedFacts;
use crate::image::metadata::image_metadata::ImageMetadata;
use crate::image::metadata::image_provenance::{
    ColorProvenance, DemosaicProvenance, ImageProvenance, RowOrder, SourceProvenance,
};
use crate::image::metadata::observation_header::ObservationHeader;
use crate::image::metadata::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
use crate::io::load_ctx::LoadCtx;
use crate::io::raw::black_level::{
    BlackLevel, DecodedRaw, DngLevels, FlagRule, LibrawBlack, Photosites,
};
use crate::io::raw::error::RawError;
use crate::io::raw::libraw_filters;
use crate::io::raw::raw_decoder::RawDecoder;
use crate::io::{null_policy, raw};
use crate::noise::QUANTIZATION_SIGMA_PER_STEP;
use crate::noise::mosaic_noise_measure::MosaicNoiseMeasure;
use common::Vec2us;

/// A camera RAW file LibRaw unpacked, with what lumos reads of it, ready for sensor-specific
/// processing.
#[derive(Debug)]
pub(super) struct UnpackedRaw {
    pub(super) libraw: Libraw,
    pub(super) layout: SensorLayout,
    pub(super) black_level: BlackLevel,
    /// The span LibRaw's own processing scales by: `maximum` less its integer black.
    processed_span: f64,
    pub(super) visible_filters: u32,
    /// The sensor pattern lumos reads itself, or `None` when LibRaw has to process the image.
    cfa_type: Option<CfaType>,
    camera_white_balance: Option<[f32; 4]>,
    iso: Option<u32>,
    /// The shutter time in seconds, when LibRaw read one.
    exposure_time: Option<f64>,
    /// The sensor temperature in °C its makernotes state, for the cameras that record one.
    ccd_temp: Option<f64>,
    /// The camera body's temperature in °C its makernotes state: Canon's, Kodak's, Leica's,
    /// Pentax's and Samsung's, which record no sensor's.
    camera_temp: Option<f64>,
    /// The camera, as LibRaw normalizes its make and model.
    instrument: Option<String>,
    /// The camera's clock at capture — see [`Libraw::camera_clock`].
    date_local: Option<String>,
    /// The lens's focal length in mm, when the file states one.
    focal_length: Option<f64>,
    /// Whether LibRaw reads a raw zero as a dead photosite: its `zero_is_bad`, set for Panasonic
    /// and some cameras its size table identifies.
    zero_is_bad: bool,
    /// The raw value at which each LibRaw color channel saturates, black included — see
    /// [`saturation_level`].
    saturation: [f64; 4],
    /// Whether LibRaw's linearization curve is the identity up to `maximum`. A compressed format
    /// (Sony cRAW, Nikon lossy NEF) maps its codes through a curve whose steps grow to several ADU
    /// in the highlights, so one ADU is no longer the quantization step.
    linear_curve: bool,
    /// The decoder LibRaw chose: a lossy codec — Canon's C-RAW, Fuji's lossy RAF — quantizes past
    /// one ADU without passing its codes through the curve at all.
    decoder: RawDecoder,
}

/// The two ways a RAW frame leaves LibRaw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RawPath {
    /// The sensor's mosaic as it was read, normalized and not clipped.
    Mosaic,
    /// LibRaw's demosaiced output: its 16-bit samples divided by their integer maximum, which
    /// clipped them.
    Processed,
}

impl UnpackedRaw {
    /// Unpack the file `libraw` opened on up to `threads` threads, and read what lumos
    /// needs of it.
    ///
    /// # Errors
    ///
    /// A [`RawError`] when LibRaw cannot unpack the file or met corrupt data in it, or when what it
    /// reports — the geometry, the sensor layout, the black level — is unusable.
    pub(super) fn unpack(mut libraw: Libraw, threads: usize) -> Result<Self, RawError> {
        let decoder = raw::identified(&libraw)?;
        libraw.unpack(threads)?;
        let layout = libraw.layout()?;
        // The raw buffer is read as rows of `raw_width` samples: any other pitch would shear it.
        let pitch = libraw.raw_pitch();
        if pitch != 2 * layout.raw.width {
            return Err(RawError::RawPitch {
                pitch,
                width: layout.raw.width,
            });
        }
        let zero_is_bad = libraw.zero_is_bad();

        let idata = libraw.image_data();
        let visible_filters = idata.filters;
        let cfa_type = libraw_filters::cfa_type(&idata)?;
        tracing::debug!(
            "libraw: filters=0x{:08x}, cfa_type={:?}",
            visible_filters,
            cfa_type
        );

        let color = libraw.color();
        // The exact level, from the unrounded values LibRaw keeps where they round to its own;
        // `exact` false is LibRaw's integers alone, the level its own processing subtracts.
        let black = |exact: bool| {
            BlackLevel::from_libraw(&LibrawBlack {
                black: color.black,
                cblack: color.cblack,
                maximum: color.maximum,
                filters: visible_filters,
                dng: (exact && idata.dng_version != 0).then_some(DngLevels {
                    black: color.dng_black,
                    cblack: color.dng_cblack,
                }),
                masked: if exact { color.black_stat } else { [0; 8] },
            })
        };
        let black_level = black(true)?;
        let processed_span = black(false)?.span();

        let camera_white_balance =
            raw::camera_white_balance(cfa_type, color.cam_mul, color.as_shot_wb_applied);
        let capture = libraw.capture();
        let iso = iso(capture.iso_speed);
        let shutter = capture.shutter;
        let exposure_time = (shutter > 0.0).then_some(f64::from(shutter));
        // LibRaw leaves -1000 where the makernotes state no temperature.
        let temperature = |celsius: f32| (celsius > -273.15).then_some(f64::from(celsius));
        let ccd_temp = temperature(capture.sensor_temperature);
        let camera_temp = temperature(capture.camera_temperature);
        let instrument = instrument(&idata.make, &idata.model);
        let date_local = libraw.camera_clock();
        let focal_length = (capture.focal_len > 0.0).then_some(f64::from(capture.focal_len));
        let saturation = array::from_fn(|channel| {
            saturation_level(
                black_level.of_channel(channel),
                color.linear_max[channel],
                color.maximum,
            )
        });
        let last_code = (color.maximum as usize).min(color.curve.len() - 1);
        let linear_curve = color.curve[..=last_code]
            .iter()
            .enumerate()
            .all(|(code, &value)| usize::from(value) == code);

        Ok(Self {
            libraw,
            layout,
            black_level,
            processed_span,
            visible_filters,
            cfa_type,
            camera_white_balance,
            iso,
            exposure_time,
            ccd_temp,
            camera_temp,
            instrument,
            date_local,
            focal_length,
            zero_is_bad,
            saturation,
            linear_curve,
            decoder,
        })
    }

    /// Sensor ADU above black over `span`, from the file itself.
    const fn sample_domain(span: f64) -> SampleDomain {
        SampleDomain {
            scale: span,
            origin: ScaleOrigin::Declared,
            pedestal: Pedestal::Removed,
            // No RAW format states a unit for sensor counts, and inventing one here would make a
            // RAW frame disagree with a FITS frame that spells the same thing differently.
            unit: None,
        }
    }

    /// The metadata both paths record: the file's own, the samples' `domain`, and how `path`
    /// made them.
    fn metadata(&self, domain: SampleDomain, path: RawPath) -> ImageMetadata {
        let (color, demosaic, clipped) = match path {
            RawPath::Mosaic => (ColorProvenance::SensorCfa, DemosaicProvenance::None, false),
            RawPath::Processed => (
                ColorProvenance::Unspecified,
                DemosaicProvenance::LibRaw,
                true,
            ),
        };
        ImageMetadata {
            header: ObservationHeader {
                iso: self.iso,
                exposure_time: self.exposure_time,
                ccd_temp: self.ccd_temp,
                camera_temp: self.camera_temp,
                instrument: self.instrument.clone(),
                date_local: self.date_local.clone(),
                focal_length: self.focal_length,
                camera_white_balance: self.camera_white_balance,
                ..Default::default()
            },
            derived: DerivedFacts::builder()
                .domain(domain)
                // LibRaw hands back the visible area top-down; no RAW format stores it otherwise.
                .provenance(ImageProvenance {
                    source: SourceProvenance::CameraRaw,
                    color,
                    clipped,
                    demosaic,
                    row_order: RowOrder::TopDown,
                })
                .build(),
        }
    }

    /// The active area of `raw` normalized, and the flags its raw values themselves settle:
    /// [`QualityFlags`](crate::image::flags::quality_flags::QualityFlags)`::SATURATED` at each
    /// channel's saturation level, and `NO_DATA` on the zeros of a `zero_is_bad` camera; no flags
    /// when no photosite carries either.
    ///
    /// Saturation is decided here, on the raw value before black is subtracted, because after a
    /// dark subtraction and a flat division the level differs at every pixel. LibRaw's own
    /// processing replaces `zero_is_bad` zeros with a same-color mean (`remove_zeroes`); lumos
    /// reads the raw buffer itself, so without the flag a dead photosite would normalize to
    /// `−black / span` and enter every stage as a measurement.
    pub(super) fn decode(&self, raw: &[u16]) -> DecodedRaw {
        self.black_level.decode(
            raw,
            self.layout,
            Photosites {
                channel: self.channel_map(),
                period: self.cfa_type.map_or(1, |cfa_type| cfa_type.period()),
                rule: self.flag_rule(),
            },
        )
    }

    /// Copied out: the rule runs on rayon workers, and `self` holds the libraw handle.
    const fn flag_rule(&self) -> FlagRule {
        FlagRule {
            zero_is_bad: self.zero_is_bad,
            saturation: self.saturation,
        }
    }

    /// One ADU's uniform-error σ in the normalized domain, or `None` when a compressed curve makes
    /// the step vary or a lossy codec quantizes past it.
    fn quantization_sigma(&self) -> Option<f32> {
        (self.linear_curve && !self.decoder.lossy())
            .then(|| (f64::from(QUANTIZATION_SIGMA_PER_STEP) / self.black_level.span()) as f32)
    }

    /// Each visible pixel's LibRaw color channel: the X-Trans pattern's color, the Bayer
    /// `filters` word's (green on the second row as channel 3), or 0 for a monochrome sensor.
    /// Copied out of `self`, so the map can run on rayon workers while `self` holds libraw.
    fn channel_map(&self) -> impl Fn(usize, usize) -> usize + Sync + use<> {
        let (cfa_type, filters) = (self.cfa_type, self.visible_filters);
        move |x: usize, y: usize| match cfa_type {
            Some(CfaType::Mosaic(Mosaic::XTrans(pattern))) => {
                pattern.color_at(Vec2us::new(x, y)).idx()
            }
            Some(CfaType::Mosaic(Mosaic::Bayer(_))) => raw::filter_color(filters, y, x),
            Some(CfaType::Mono) | None => 0,
        }
    }

    /// The visible area as a CFA frame: every sample `(v − black) / span`, unclamped — a dark's or
    /// a bias's noise below its pedestal is data the stack averages — with the flags the raw values
    /// settle. The file is freed before the frame is allocated ([`Libraw::release_file`]), and
    /// consuming `self` frees LibRaw's state, tens of MB, before a demosaic allocates its own.
    ///
    /// # Errors
    ///
    /// [`RawError::NotACfaFrame`] for a sensor LibRaw delivers already processed, and
    /// [`RawError::NoRawImage`] when LibRaw unpacked no buffer.
    pub(super) fn into_cfa_image(mut self) -> Result<CfaImage, RawError> {
        let cfa_type = self.cfa_type.ok_or(RawError::NotACfaFrame)?;
        self.libraw.release_file();
        let raw = self.libraw.raw_image().ok_or(RawError::NoRawImage)?;
        let DecodedRaw { mut pixels, flags } = self.decode(raw);
        // A photosite read as no measurement holds the frame's median, as a FITS null does.
        if let Some(flags) = &flags {
            null_policy::fill_flagged(&mut pixels, flags);
        }
        let mut metadata = self.metadata(
            Self::sample_domain(self.black_level.span()),
            RawPath::Mosaic,
        );
        metadata.derived = mem::take(&mut metadata.derived)
            .rebuild()
            .quantization_sigma(self.quantization_sigma())
            .saturation_flagged(true)
            .build();
        let mut image =
            CfaImage::from_plane(Buffer2::new(self.layout.active, pixels), cfa_type, metadata);
        image.set_flags(flags);
        Ok(image)
    }

    /// [`raw::load_preview`] past the open: the CFA path and a clamp, or LibRaw's own processing.
    pub(super) fn into_linear_image(self, ctx: &LoadCtx) -> Result<LinearImage, RawError> {
        if self.cfa_type.is_none() {
            let idata = self.libraw.image_data();
            if idata.colors == 4 {
                let colors = String::from_utf8_lossy(&idata.cdesc).into_owned();
                return Err(RawError::FourColorSensor { colors });
            }
            tracing::info!("no mosaic lumos demosaics; LibRaw processes the image");
            return self.processed_by_libraw(None);
        }
        let cfa = self.into_cfa_image()?;
        let noise = MosaicNoiseMeasure::of(&cfa).map(MosaicNoiseMeasure::measure);
        let mut image = cfa.demosaic(ctx.xtrans_passes, noise, &ctx.cancel)?;
        // The demosaic's own reach leaves the range, ~1.06 out of RCD and ~1.16 out of
        // Markesteijn at a step edge, and the frame arrives with its noise below black: the
        // preview clamps both, after the demosaic, so the interpolation sees the samples as
        // they are.
        for channel in 0..image.channel_count() {
            par::iter_mut(image.channel_mut(channel).pixels_mut())
                .for_each(|sample| *sample = sample.clamp(0.0, 1.0));
        }
        if let Some(provenance) = image.metadata.derived.provenance_mut() {
            provenance.clipped = true;
        }
        Ok(image)
    }

    /// The image through LibRaw's own processing, linear, unbalanced and unturned: the path for a
    /// sensor lumos does not demosaic, slower but correct for exotic patterns, demosaiced at
    /// `quality`, or LibRaw's default for `None`. Its samples are LibRaw's 16-bit output over
    /// 65535, which LibRaw scaled by its integer span.
    ///
    /// # Errors
    ///
    /// [`RawError::Libraw`] when LibRaw fails or reports an unusable shape, and
    /// [`RawError::ProcessedColors`] for other than one color or three.
    pub(super) fn processed_by_libraw(
        mut self,
        quality: Option<i32>,
    ) -> Result<LinearImage, RawError> {
        let start = Instant::now();
        let params = ProcessParams {
            // Linear output: no gamma curve and no brightness change.
            gamma: [1.0, 1.0],
            bright: 1.0,
            auto_bright: false,
            // LibRaw otherwise falls back to daylight WB when camera and auto WB are disabled.
            user_mul: [1.0; 4],
            auto_wb: false,
            camera_wb: false,
            output_bps: 16,
            // The sensor's own colors: no conversion, which would also fold a four-color sensor to
            // RGB.
            output_color: 0,
            // Scale by `maximum` as LibRaw read it, the span the domain records, not by a frame's
            // own brightest pixel when that lies within 25% of it.
            adjust_maximum_thr: 0.0,
            // The sensor's rows and columns as they are: no turn by the EXIF orientation, which
            // would make a portrait frame H×W here alone, and no stretch by pixel aspect.
            user_flip: 0,
            fuji_rotate: false,
            quality,
        };
        let mut image = {
            let processed = self.libraw.process(&params)?;
            let samples = processed.samples()?;
            let layout =
                ChannelLayout::try_from(samples.colors).map_err(|_| RawError::ProcessedColors {
                    colors: samples.colors,
                })?;
            LinearImage::from_planar_channels(
                PlanarDesc::new(samples.size, layout),
                planes(&samples),
            )
        };
        tracing::info!(
            "LibRaw processed {}x{}x{} in {:.2}ms",
            image.width(),
            image.height(),
            image.channel_count(),
            start.elapsed().as_secs_f64() * 1000.0
        );
        image.metadata =
            self.metadata(Self::sample_domain(self.processed_span), RawPath::Processed);
        Ok(image)
    }
}

/// How far below a stated linear limit a sample counts as saturated: 0.5% of the span from black
/// to it. RawTherapee's conservative white level (`rtengine/camconst.json`, "How to Measure White
/// Levels"): 50 to 100 units below the clip at 14 bits, 10 to 20 at 12.
const LINEAR_MAX_MARGIN: f64 = 0.005;

/// The raw value at which a channel of black `black` saturates, black included.
///
/// Where the maker notes state the channel's linear limit, `linear_max`, the sensor clips there:
/// [`LINEAR_MAX_MARGIN`] below it. Only a limit above black and within `maximum` is one: LibRaw
/// rescales several makers' values itself, and a value outside those bounds is one it read at the
/// wrong scale, which would flag every sample or none. Elsewhere `maximum`, which overstates many
/// sensors' clip — LibRaw's own processing allows down to 75% of it — so the level is
/// [`SATURATION_FRACTION`] of the way from black to it.
pub(super) fn saturation_level(black: f64, linear_max: u32, maximum: u32) -> f64 {
    let limit = f64::from(linear_max);
    if black < limit && linear_max <= maximum {
        limit - LINEAR_MAX_MARGIN * (limit - black)
    } else {
        black + f64::from(SATURATION_FRACTION) * (f64::from(maximum) - black)
    }
}

/// The camera LibRaw names by its normalized `make` and `model`, joined by a space; `None` when it
/// names neither.
pub(super) fn instrument(make: &str, model: &str) -> Option<String> {
    let text = |field: &str| {
        let text = field.trim();
        (!text.is_empty()).then(|| text.to_owned())
    };
    match (text(make), text(model)) {
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (make, model) => make.or(model),
    }
}

/// The ISO speed LibRaw read, when it read a positive one.
#[expect(
    clippy::cast_sign_loss,
    reason = "only a positive speed reaches the cast"
)]
fn iso(speed: f32) -> Option<u32> {
    (speed > 0.0).then(|| speed.round() as u32)
}

/// `samples`' interleaved colors as one plane each, over 65535, each plane written once as it is
/// made.
pub(super) fn planes<'a>(samples: &'a ProcessedSamples<'_>) -> impl Iterator<Item = Vec<f32>> + 'a {
    let colors = samples.colors;
    let pixels = samples.size.pixel_count();
    (0..colors).map(move |channel| {
        par::into_iter(0..pixels)
            .map(|pixel| f32::from(samples.samples[pixel * colors + channel]) / 65535.0)
            .collect()
    })
}
