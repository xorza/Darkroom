//! [`UnpackedRaw`]: a camera RAW file LibRaw unpacked, and what lumos reads of it.

use std::array;
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
    /// Whether LibRaw reads a raw zero as a dead photosite: its `zero_is_bad`, set for Panasonic
    /// and some cameras its size table identifies.
    zero_is_bad: bool,
    /// The raw value at which each LibRaw colour channel saturates, black included:
    /// [`SATURATION_FRACTION`] of the way from its black to its linear limit — `linear_max` where
    /// LibRaw knows it, else `maximum`.
    saturation: [f64; 4],
    /// Whether LibRaw's linearization curve is the identity up to `maximum`. A compressed format
    /// (Sony cRAW, Nikon lossy NEF, Canon C-RAW) maps its codes through a curve whose steps grow
    /// to several ADU in the highlights, so one ADU is no longer the quantization step.
    linear_curve: bool,
}

impl UnpackedRaw {
    /// Unpack the file `libraw` opened from `path`, and read what lumos needs of it.
    ///
    /// # Errors
    ///
    /// A [`RawError`] when LibRaw cannot unpack the file or met corrupt data in it, or when what it
    /// reports — the geometry, the sensor layout, the black level — is unusable.
    pub(super) fn unpack(mut libraw: Libraw, path: &Path) -> Result<Self, RawError> {
        libraw.unpack()?;
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
        let ioparams = &data.rawdata.ioparams;
        if ioparams.fuji_width != 0 {
            return Err(RawError::SuperCcd);
        }
        let zero_is_bad = ioparams.zero_is_bad != 0;

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
        // LibRaw leaves -1000 where the makernotes state no sensor temperature.
        let sensor_temperature = data.makernotes.common.SensorTemperature;
        let ccd_temp = (sensor_temperature > -273.15).then_some(f64::from(sensor_temperature));
        let saturation = array::from_fn(|channel| {
            let limit = f64::from(match color.linear_max[channel] {
                0 => color.maximum,
                known => known,
            });
            let black = black_level.of_channel(channel);
            black + f64::from(SATURATION_FRACTION) * (limit - black)
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
            zero_is_bad,
            saturation,
            linear_curve,
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
    /// the step vary.
    fn quantization_sigma(&self) -> Option<f32> {
        self.linear_curve
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
