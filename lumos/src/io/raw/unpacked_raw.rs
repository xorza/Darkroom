//! [`UnpackedRaw`]: a camera RAW file LibRaw unpacked, and what lumos reads of it.

use std::array;
use std::ffi;
use std::path::{Path, PathBuf};
use std::time::Instant;

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::cancelled::Cancelled;
use crate::io::image::cfa::{CfaImage, CfaType, QUANTIZATION_SIGMA_PER_STEP};
use crate::io::image::error::ImageError;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, RowOrder,
    SourceContainer, TransferProvenance,
};
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags, SATURATION_FRACTION};
use crate::io::image::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
use crate::io::raw;
use crate::io::raw::black_level::{BlackLevel, DngLevels, LibrawBlack};
use crate::io::raw::error::RawError;
use crate::io::raw::libraw::{Libraw, ProcessedSamples};
use crate::io::raw::raw_decoder::RawDecoder;
use crate::io::raw::sensor_layout::SensorLayout;
use crate::math::vec2us::Vec2us;

/// A camera RAW file LibRaw unpacked, with what lumos reads of it, ready for sensor-specific
/// processing.
#[derive(Debug)]
pub(super) struct UnpackedRaw {
    pub(super) libraw: Libraw,
    path: PathBuf,
    pub(super) layout: SensorLayout,
    pub(super) black_level: BlackLevel,
    /// The span LibRaw's own processing scales by: `maximum` less its integer black.
    processed_span: f64,
    pub(super) visible_filters: u32,
    /// The mosaic lumos demosaics itself, or `None` when LibRaw has to process the image.
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
    /// The raw value at which each LibRaw colour channel saturates, black included — see
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

impl UnpackedRaw {
    /// Unpack the file `libraw` opened from `path` on up to `threads` threads, and read what lumos
    /// needs of it.
    ///
    /// # Errors
    ///
    /// A [`RawError`] when LibRaw cannot unpack the file or met corrupt data in it, or when what it
    /// reports — the geometry, the sensor layout, the black level — is unusable.
    pub(super) fn unpack(
        mut libraw: Libraw,
        path: &Path,
        threads: usize,
    ) -> Result<Self, RawError> {
        let decoder = raw::identified(&libraw)?;
        libraw.unpack(threads)?;
        let data = libraw.data();
        let sizes = &data.sizes;
        let layout = SensorLayout::of(sizes)?;
        // The raw buffer is read as rows of `raw_width` samples: any other pitch would shear it.
        let pitch = sizes.raw_pitch as usize;
        if pitch != 2 * layout.raw.width {
            return Err(RawError::RawPitch {
                pitch,
                width: layout.raw.width,
            });
        }
        // `unpack` copies LibRaw's internal output parameters here.
        let zero_is_bad = data.rawdata.ioparams.zero_is_bad != 0;

        let visible_filters = data.idata.filters;
        let cfa_type = raw::sensor_cfa_type(data)?;
        tracing::debug!(
            "libraw: filters=0x{:08x}, cfa_type={:?}",
            visible_filters,
            cfa_type
        );

        let color = &data.color;
        // The exact level, from the unrounded values LibRaw keeps where they round to its own;
        // `exact` false is LibRaw's integers alone, the level its own processing subtracts.
        let black = |exact: bool| {
            BlackLevel::from_libraw(&LibrawBlack {
                black: color.black,
                cblack: &color.cblack,
                maximum: color.maximum,
                filters: visible_filters,
                dng: (exact && data.idata.dng_version != 0).then_some(DngLevels {
                    black: color.dng_levels.dng_fblack,
                    cblack: &color.dng_levels.dng_fcblack,
                }),
                masked: if exact { color.black_stat } else { [0; 8] },
            })
        };
        let black_level = black(true)?;
        let processed_span = black(false)?.span();

        let camera_white_balance =
            raw::camera_white_balance(cfa_type, color.cam_mul, color.as_shot_wb_applied);
        let iso = iso(data.other.iso_speed);
        let shutter = data.other.shutter;
        let exposure_time = (shutter > 0.0).then_some(f64::from(shutter));
        // LibRaw leaves -1000 where the makernotes state no temperature.
        let temperature = |celsius: f32| (celsius > -273.15).then_some(f64::from(celsius));
        let ccd_temp = temperature(data.makernotes.common.SensorTemperature);
        let camera_temp = temperature(data.makernotes.common.CameraTemperature);
        let instrument = instrument(&data.idata.normalized_make, &data.idata.normalized_model);
        let date_local = libraw.camera_clock();
        let focal_length = (data.other.focal_len > 0.0).then_some(f64::from(data.other.focal_len));
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
            path: path.to_path_buf(),
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

    /// The metadata both paths record: the file's own, the samples' `domain`, and how the decode
    /// made them — `color`, `demosaic`, and whether it `clipped` them.
    fn metadata(
        &self,
        domain: SampleDomain,
        color: ColorProvenance,
        demosaic: DemosaicProvenance,
        clipped: bool,
    ) -> ImageMetadata {
        ImageMetadata {
            domain: Some(domain),
            iso: self.iso,
            exposure_time: self.exposure_time,
            ccd_temp: self.ccd_temp,
            camera_temp: self.camera_temp,
            instrument: self.instrument.clone(),
            date_local: self.date_local.clone(),
            focal_length: self.focal_length,
            camera_white_balance: self.camera_white_balance,
            provenance: Some(ImageProvenance {
                container: SourceContainer::CameraRaw,
                decoder: DecoderProvenance::LibRaw,
                transfer: TransferProvenance::RawNormalized,
                color,
                clipped,
                demosaic,
                // LibRaw hands back the visible area top-down; no RAW format stores it otherwise.
                row_order: RowOrder::TopDown,
            }),
            ..Default::default()
        }
    }

    /// The flags the raw values themselves settle for the active area of `raw`:
    /// [`QualityFlags::SATURATED`] at each channel's saturation level, and
    /// [`QualityFlags::NO_DATA`] on the zeros of a `zero_is_bad` camera; `None` when no photosite
    /// carries either.
    ///
    /// Saturation is decided here, on the raw value before black is subtracted, because after a
    /// dark subtraction and a flat division the level differs at every pixel. LibRaw's own
    /// processing replaces `zero_is_bad` zeros with a same-colour mean (`remove_zeroes`); lumos reads
    /// the raw buffer itself, so without the flag a dead photosite would normalize to
    /// `−black / span` and enter every stage as a measurement.
    pub(super) fn decode_flags(&self, raw: &[u16]) -> Option<PixelFlags> {
        let SensorLayout {
            raw: raw_size,
            active,
            margin,
        } = self.layout;
        // Copied out: the closure runs on rayon workers, and `self` holds the libraw handle.
        let (zero_is_bad, saturation) = (self.zero_is_bad, self.saturation);
        let channel_at = self.channel_map();
        PixelFlags::from_fn(active, |index| {
            let (x, y) = (index % active.width, index / active.width);
            let value = raw[(y + margin.y) * raw_size.width + x + margin.x];
            if value == 0 && zero_is_bad {
                QualityFlags::NO_DATA
            } else if f64::from(value) >= saturation[channel_at(x, y)] {
                QualityFlags::SATURATED
            } else {
                QualityFlags::default()
            }
        })
    }

    /// One ADU's uniform-error σ in the normalized domain, or `None` when a compressed curve makes
    /// the step vary or a lossy codec quantizes past it.
    fn quantization_sigma(&self) -> Option<f32> {
        (self.linear_curve && !self.decoder.lossy())
            .then(|| (f64::from(QUANTIZATION_SIGMA_PER_STEP) / self.black_level.span()) as f32)
    }

    /// Each visible pixel's LibRaw colour channel: the X-Trans pattern's colour, the Bayer
    /// `filters` word's (green on the second row as channel 3), or 0 for a monochrome sensor.
    /// Copied out of `self`, so the map can run on rayon workers while `self` holds libraw.
    fn channel_map(&self) -> impl Fn(usize, usize) -> usize + Sync + use<> {
        let (cfa_type, filters) = (self.cfa_type, self.visible_filters);
        move |x: usize, y: usize| match cfa_type {
            Some(CfaType::XTrans(pattern)) => usize::from(pattern.color_at(Vec2us::new(x, y))),
            Some(CfaType::Bayer(_)) => raw::libraw_filter_color(filters, y, x),
            Some(CfaType::Mono) | None => 0,
        }
    }

    /// The visible area as a CFA frame: every sample `(v − black) / span`, unclamped — a dark's or
    /// a bias's noise below its pedestal is data the stack averages — with the flags the raw values
    /// settle. Consumes `self`, so LibRaw's state, tens of MB, is freed before a demosaic allocates
    /// its own.
    ///
    /// # Errors
    ///
    /// [`RawError::NotACfaFrame`] for a sensor LibRaw delivers already processed, and
    /// [`RawError::NoRawImage`] when LibRaw unpacked no buffer.
    pub(super) fn into_cfa_image(self) -> Result<CfaImage, RawError> {
        let cfa_type = self.cfa_type.ok_or(RawError::NotACfaFrame)?;
        let raw = self.libraw.raw_image().ok_or(RawError::NoRawImage)?;
        let pixels = self
            .black_level
            .normalize(raw, self.layout, self.channel_map());
        let flags = self.decode_flags(raw);
        let metadata = ImageMetadata {
            quantization_sigma: self.quantization_sigma(),
            saturation_flagged: true,
            ..self.metadata(
                Self::sample_domain(self.black_level.span()),
                ColorProvenance::SensorCfa,
                DemosaicProvenance::None,
                false,
            )
        };
        Ok(CfaImage {
            data: Buffer2::new(self.layout.active.width, self.layout.active.height, pixels),
            cfa_type,
            metadata,
            flags,
        })
    }

    /// [`raw::load_raw`] past the open: the CFA path and a clamp, or LibRaw's own processing.
    pub(super) fn into_linear_image(
        self,
        context: &LoadContext,
    ) -> Result<LinearImage, ImageError> {
        if self.cfa_type.is_none() {
            let idata = &self.libraw.data().idata;
            if idata.colors == 4 {
                let colours =
                    String::from_utf8_lossy(&raw::colour_description(idata.cdesc)).into_owned();
                return Err(ImageError::raw(
                    &self.path,
                    RawError::FourColourSensor { colours },
                ));
            }
            tracing::info!("no mosaic lumos demosaics; LibRaw processes the image");
            let path = self.path.clone();
            return self
                .processed_by_libraw()
                .map_err(|source| ImageError::raw(&path, source));
        }
        let path = self.path.clone();
        let mut image = self
            .into_cfa_image()
            .map_err(|source| ImageError::raw(&path, source))?
            .demosaic(context.xtrans_passes, &context.cancel)
            .map_err(|Cancelled| ImageError::cancelled(&path))?;
        // The demosaic's own reach leaves the range, ~1.06 out of RCD and ~1.16 out of
        // Markesteijn at a step edge, and the frame arrives with its noise below black: the
        // preview clamps both, after the demosaic, so the interpolation sees the samples as
        // they are.
        for channel in 0..image.channels() {
            image
                .channel_mut(channel)
                .pixels_mut()
                .par_iter_mut()
                .for_each(|sample| *sample = sample.clamp(0.0, 1.0));
        }
        if let Some(provenance) = &mut image.metadata.provenance {
            provenance.clipped = true;
        }
        Ok(image)
    }

    /// The image through LibRaw's own processing, linear, unbalanced and unturned: the path for a
    /// sensor lumos does not demosaic, slower but correct for exotic patterns. Its samples are
    /// LibRaw's 16-bit output over 65535, which LibRaw scaled by its integer span.
    ///
    /// # Errors
    ///
    /// [`RawError::Process`] when LibRaw fails, and the shape errors of
    /// [`ProcessedImage::samples`](crate::io::raw::libraw::ProcessedImage::samples).
    pub(super) fn processed_by_libraw(mut self) -> Result<LinearImage, RawError> {
        let start = Instant::now();
        let params = self.libraw.params_mut();
        // Linear output: no gamma curve and no brightness change.
        params.gamm[0] = 1.0;
        params.gamm[1] = 1.0;
        params.bright = 1.0;
        params.no_auto_bright = 1;
        // LibRaw otherwise falls back to daylight WB when camera and auto WB are disabled.
        params.user_mul = [1.0; 4];
        params.use_auto_wb = 0;
        params.use_camera_wb = 0;
        params.output_bps = 16;
        // The sensor's own colours: no conversion, which would also fold a four-colour sensor to
        // RGB.
        params.output_color = 0;
        // Scale by `maximum` as LibRaw read it, the span the domain records, not by a frame's own
        // brightest pixel when that lies within 25% of it.
        params.adjust_maximum_thr = 0.0;
        // The sensor's rows and columns as they are: no turn by the EXIF orientation, which would
        // make a portrait frame H×W here alone, and no stretch by pixel aspect.
        params.user_flip = 0;
        params.use_fuji_rotate = 0;

        let mut image = {
            let processed = self.libraw.process()?;
            let samples = processed.samples()?;
            LinearImage::from_planar_channels(samples.dimensions, planes(&samples))
        };
        tracing::info!(
            "LibRaw processed {}x{}x{} in {:.2}ms",
            image.width(),
            image.height(),
            image.channels(),
            start.elapsed().as_secs_f64() * 1000.0
        );
        image.metadata = self.metadata(
            Self::sample_domain(self.processed_span),
            ColorProvenance::Unspecified,
            DemosaicProvenance::LibRaw,
            // LibRaw's 16-bit output divided by its integer maximum.
            true,
        );
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
/// names neither. Each is a NUL-terminated field, read up to its terminator.
pub(super) fn instrument(make: &[ffi::c_char], model: &[ffi::c_char]) -> Option<String> {
    let text = |field: &[ffi::c_char]| {
        let bytes: Vec<u8> = field
            .iter()
            .map(|&letter| u8::from_ne_bytes(letter.to_ne_bytes()))
            .take_while(|&byte| byte != 0)
            .collect();
        let text = String::from_utf8_lossy(&bytes).trim().to_owned();
        (!text.is_empty()).then_some(text)
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

/// `samples`' interleaved colours as one plane each, over 65535.
fn planes(samples: &ProcessedSamples<'_>) -> Vec<Vec<f32>> {
    let colors = samples.dimensions.channels();
    let mut planes = vec![vec![0.0f32; samples.dimensions.pixel_count()]; colors];
    for (channel, plane) in planes.iter_mut().enumerate() {
        plane.par_iter_mut().enumerate().for_each(|(pixel, value)| {
            *value = f32::from(samples.samples[pixel * colors + channel]) / 65535.0;
        });
    }
    planes
}
