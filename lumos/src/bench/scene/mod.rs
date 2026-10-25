//! The scenes the benches measure on: seeded renders of the synthetic forward model, at the
//! standard frame or, for a test of a bench's input, a smaller one at the same density.
//!
//! Every frame is taken on one sensor, with its filters, vignette, bias, dark current and defects.
//! A light reads all of them, and the masters that calibrate it come from the same sensor. A
//! calibrated frame is a light as calibration leaves it: the filters' color balance and the hot
//! pixels stay, the bias, the dark current and the vignette are gone.

use common::Buffer2;
use glam::DVec2;
use imaginarium::{ChannelLayout, PlanarDesc};

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::linear_image::LinearImage;
use crate::internals::synthetic::artifacts::CosmicRays;
use crate::internals::synthetic::bgs::{Gradient, Vignette};
use crate::internals::synthetic::camera::{Camera, SensorDefects};
use crate::internals::synthetic::observe::{Observation, SimFrame, render};
use crate::internals::synthetic::scene::{BgField, Placement, Scatter, Scene};
use crate::internals::test_rng::TestRng;
use crate::registration::transform::Transform;
use crate::{CalibrationSet, MasterRole};
use common::Size2us;

/// The standard frame: 6000 × 4000, an APS-C camera's 24 MP.
pub(crate) const FRAME: Size2us = Size2us::new(6000, 4000);

/// A daylight white balance, red and blue gains over green's, in `camera_white_balance`'s order.
const WHITE_BALANCE: [f32; 4] = [2.13, 1.0, 1.71, 1.0];

/// What each color's filter passes, in [`CfaType::slot_at`]'s slots: the inverse of
/// [`WHITE_BALANCE`], so the balanced demosaic gives grey back for grey.
const FILTER_GAIN: [f32; 3] = [1.0 / 2.13, 1.0, 1.0 / 1.71];

/// The sky of every field, `level` times the first exposure's: a gradient, as light pollution or
/// the moon lays one.
const fn sky(level: f32) -> BgField {
    BgField::Gradient(Gradient {
        start: 0.08 * level,
        end: 0.12 * level,
        angle: 0.6,
    })
}

/// The range of the stars' total fluxes, drawn log-uniform. At the FWHM below, 0.5 peaks at 0.036,
/// 25σ over the sky's noise of 0.0014, and 12 at 0.87, just under saturation with the sky.
const FLUX: (f32, f32) = (0.5, 12.0);

/// The PSF FWHM of every field, in pixels: average seeing on an APS-C camera at a moderate focal
/// length.
const FWHM: f32 = 3.5;

/// The sensor's pedestal, where a 14-bit camera's 512 ADU black sits in its 16 383.
pub(crate) const BIAS: f32 = 0.03;

/// A fast lens's light falloff: 70% at the corners.
const VIGNETTE: Vignette = Vignette {
    center: 1.0,
    edge: 0.7,
    falloff: 2.0,
};

/// The lights of the light preset's typical stack.
pub(crate) const STACKED_LIGHTS: usize = 30;

/// The exposure of a light and of the dark that matches it, in seconds.
const EXPOSURE_S: f32 = 120.0;

/// The level a flat exposes the green photosites to, mid-range as a flat is taken.
const FLAT_LEVEL: f32 = 0.5;

/// The hot pixels the standard frame holds: one in 100 000.
const HOT_PIXELS: usize = 240;

/// The dead pixels the standard frame holds: one in a million.
const DEAD_PIXELS: usize = 24;

/// The cosmic-ray hits the standard frame holds, as a few minutes' exposure collects.
const COSMIC_RAYS: usize = 200;

/// The seed of the sensor's defects, which every frame taken on it shares.
const SENSOR_SEED: u64 = 0x5E_4502;

/// The content of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    /// 300 stars on a sky with a gradient: a field at high galactic latitude.
    Sparse,
    /// 3000 stars on a sky with a gradient: a field at moderate galactic latitude.
    Typical,
    /// A globular cluster of 20 000 stars on the same sky: four in five in a core whose σ is 12% of
    /// the frame's short side, where the stars blend.
    Dense,
}

impl Field {
    /// The bench parameter that names the field.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Sparse => "sparse",
            Self::Typical => "typical",
            Self::Dense => "dense",
        }
    }

    /// The stars the field holds on the standard frame.
    pub(crate) const fn stars(self) -> usize {
        match self {
            Self::Sparse => 300,
            Self::Typical => 3000,
            Self::Dense => 20_000,
        }
    }

    const fn seed(self) -> u64 {
        match self {
            Self::Sparse => 0x5_9A45,
            Self::Typical => 0x7_1C41,
            Self::Dense => 0xD_E45E,
        }
    }

    /// The true sky of the field over a frame of `size`.
    pub(crate) fn scene(self, size: Size2us) -> Scene {
        let (stars, seed) = (at_density(self.stars(), size), self.seed());
        let placement = match self {
            Self::Sparse | Self::Typical => Placement::Uniform { margin: 16.0 },
            Self::Dense => Placement::Cluster,
        };
        Scene::scatter(
            size,
            Scatter {
                count: stars,
                flux: FLUX,
                placement,
                seed,
            },
            sky(1.0),
        )
    }

    /// The field calibrated, on a mono sensor of `size`.
    pub(crate) fn render(self, size: Size2us) -> SimFrame {
        self.expose(Sensor::calibrated(size, CfaType::Mono), Shot::Field)
    }

    /// A `side` × `side` tile of the field that holds a star, cut from a calibrated mono frame of
    /// 4 × 4 such tiles.
    pub(crate) fn tile(self, side: usize) -> Buffer2<f32> {
        let frame = self.render(Size2us::new(4 * side, 4 * side));
        let star = frame
            .truth
            .sources
            .first()
            .expect("a frame of 16 tiles holds a star")
            .pos;
        #[expect(clippy::cast_sign_loss, reason = "a star lies inside the frame")]
        let corner = |at: f64| (at as usize / side * side).min(3 * side);
        let (left, top) = (corner(star.x), corner(star.y));
        let plane = frame.image.channel(0);
        Buffer2::from_fn((side, side), |x, y| plane[(left + x, top + y)])
    }

    /// Exposure `idx` of a sequence on the field, calibrated, on a mono sensor of `size`, as the
    /// warp leaves it: aligned to the field's frame. From exposure to exposure the sky brightens
    /// or fades by up to 8% and the transparency falls by up to 9%, as over a night, and the noise
    /// is the exposure's own.
    pub(crate) fn aligned(self, size: Size2us, idx: usize) -> SimFrame {
        let shot = Shot::Sequence { idx, aligned: true };
        self.expose(Sensor::calibrated(size, CfaType::Mono), shot)
    }

    /// Exposure `idx` of the sequence [`Self::aligned`] gives, as the sensor saw it: placed by
    /// [`dither`].
    pub(crate) fn exposure(self, size: Size2us, idx: usize) -> SimFrame {
        let shot = Shot::Sequence {
            idx,
            aligned: false,
        };
        self.expose(Sensor::calibrated(size, CfaType::Mono), shot)
    }

    /// The field's stacked master over a frame of `size` in linear RGB, as the light preset stacks
    /// 30 lights of a color sensor: each star its color, from red to blue, on a light-polluted
    /// sky of 1.2, 1 and 0.8 times the field's in red, green and blue, with a thirtieth of one
    /// light's noise variance and no hot pixels, which the stack rejects.
    pub(crate) fn stacked_rgb(self, size: Size2us) -> LinearImage {
        let mut camera = Sensor::calibrated(size, CfaType::Mono).camera();
        camera.full_well_e *= STACKED_LIGHTS as f32;
        camera.read_noise_e *= (STACKED_LIGHTS as f32).sqrt();
        camera.defects.hot.clear();
        let scene = self.scene(size);
        let mut rng = TestRng::new(self.seed() ^ 0xC010);
        let colors: Vec<f32> = scene.sources.iter().map(|_| rng.next_f32()).collect();
        let planes = [1.2, 1.0, 0.8]
            .into_iter()
            .enumerate()
            .map(|(channel, level)| {
                let mut light = scene.clone();
                light.bg = sky(level);
                for (source, &color) in light.sources.iter_mut().zip(&colors) {
                    // A red star (0) is 1.3 in red and 0.7 in blue, a blue one (1) the reverse.
                    source.flux *= 1.0 + 0.3 * (1.0 - 2.0 * color) * (1.0 - channel as f32);
                }
                let observation = Observation {
                    exposure_s: EXPOSURE_S,
                    ..Observation::reference(self.seed() ^ channel as u64)
                };
                render(&light, &camera, &observation)
                    .image
                    .channel(0)
                    .pixels()
                    .to_vec()
            });
        LinearImage::from_planar_channels(PlanarDesc::new(size, ChannelLayout::Rgb), planes)
    }

    /// The field calibrated, on a sensor of `size` behind `cfa_type`'s filters, with cosmic-ray
    /// hits.
    pub(crate) fn mosaic(self, size: Size2us, cfa_type: CfaType) -> CfaImage {
        self.hit(Sensor::calibrated(size, cfa_type), Shot::Field)
    }

    /// Exposure `idx` of the sequence [`Self::exposure`] gives, calibrated, on a sensor of `size`
    /// behind `cfa_type`'s filters, with its own cosmic-ray hits: a light of a dithered set as
    /// calibration leaves it, for a drizzle.
    pub(crate) fn mosaic_exposure(self, size: Size2us, cfa_type: CfaType, idx: usize) -> CfaImage {
        let shot = Shot::Sequence {
            idx,
            aligned: false,
        };
        self.hit(Sensor::calibrated(size, cfa_type), shot)
    }

    /// The field as a sensor of `size` behind `cfa_type`'s filters reads it, with cosmic-ray hits:
    /// what [`masters`] calibrate.
    pub(crate) fn light(self, size: Size2us, cfa_type: CfaType) -> CfaImage {
        self.hit(Sensor::raw(size, cfa_type), Shot::Field)
    }

    /// Exposure `idx` of the sequence [`Self::exposure`] gives, as a sensor of `size` behind
    /// `cfa_type`'s filters reads it, with its own cosmic-ray hits.
    pub(crate) fn light_exposure(self, size: Size2us, cfa_type: CfaType, idx: usize) -> CfaImage {
        let shot = Shot::Sequence {
            idx,
            aligned: false,
        };
        self.hit(Sensor::raw(size, cfa_type), shot)
    }

    fn expose(self, sensor: Sensor, shot: Shot) -> SimFrame {
        let size = sensor.size;
        let mut scene = self.scene(size);
        let mut observation = Observation {
            exposure_s: EXPOSURE_S,
            ..Observation::reference(self.seed())
        };
        if let Shot::Sequence { idx, aligned } = shot {
            scene.bg = sky(exposure_sky(idx));
            for source in &mut scene.sources {
                source.flux *= exposure_transparency(idx);
            }
            observation.seed ^= idx as u64;
            if !aligned {
                observation.transform = dither(size, idx);
            }
        }
        render(&scene, &sensor.camera(), &observation)
    }

    fn hit(self, sensor: Sensor, shot: Shot) -> CfaImage {
        let Sensor { size, cfa_type, .. } = sensor;
        let mut image = mosaic(self.expose(sensor, shot), cfa_type);
        let idx = match shot {
            Shot::Field => 0,
            Shot::Sequence { idx, .. } => idx,
        };
        CosmicRays {
            count: at_density(COSMIC_RAYS, size),
            amplitude: 0.3..1.0,
            seed: self.seed() ^ 0xC05 ^ ((idx as u64) << 16),
        }
        .add_to(image.data_mut());
        for pixel in image.data_mut().pixels_mut() {
            *pixel = pixel.min(1.0);
        }
        image
    }
}

/// Which exposure of a field a frame is.
#[derive(Debug, Clone, Copy)]
enum Shot {
    /// The field's own frame.
    Field,
    /// Exposure `idx` of a sequence: its sky and transparency changed and its noise its own,
    /// and dithered on the sensor unless `aligned` to the field's frame.
    Sequence { idx: usize, aligned: bool },
}

/// The bias, dark and flat masters of the sensor a light of `size` behind `cfa_type` is taken on,
/// one exposure each: how deep a master is changes none of the work that calibrates with it.
pub(crate) fn masters(size: Size2us, cfa_type: CfaType) -> CalibrationSet<Option<CfaImage>> {
    master_frames(size, cfa_type, 0)
}

/// Frame `idx` of each of the bias, dark and flat sequences [`masters`] are stacked from, each
/// with its own noise.
pub(crate) fn master_frames(
    size: Size2us,
    cfa_type: CfaType,
    idx: usize,
) -> CalibrationSet<Option<CfaImage>> {
    let expose = |role: MasterRole, seed: u64, level: f32, exposure_s: f32| {
        let scene = Scene {
            size,
            sources: Vec::new(),
            bg: BgField::Uniform { level },
        };
        let observation = Observation {
            exposure_s,
            ..Observation::reference(SENSOR_SEED ^ seed ^ ((idx as u64) << 8))
        };
        let camera = Sensor::raw(size, cfa_type).camera();
        let frame = render(&scene, &camera, &observation);
        let mut master = mosaic(frame, cfa_type);
        master.metadata.header.image_type = Some(role.extname().to_string());
        Some(master)
    };
    CalibrationSet {
        bias: expose(MasterRole::Bias, 1, 0.0, 0.0),
        dark: expose(MasterRole::Dark, 2, 0.0, EXPOSURE_S),
        flat: expose(MasterRole::Flat, 3, FLAT_LEVEL, 1.0),
        flat_dark: None,
    }
}

/// Where exposure `idx` of a sequence on a frame of `size` lies: the field's frame to the
/// sensor's. Exposure 0 is the reference; every other is dithered by up to 5 px each way and
/// turned by up to 0.2° about the frame's centre, along the golden-ratio sequence that spreads a
/// dither pattern evenly.
pub(crate) fn dither(size: Size2us, idx: usize) -> Transform {
    if idx == 0 {
        return Transform::identity();
    }
    let spread = |step: f64| 2.0 * (idx as f64 * step).fract() - 1.0;
    let golden = (5.0_f64.sqrt() - 1.0) / 2.0;
    let centre = DVec2::new(size.width as f64, size.height as f64) / 2.0;
    Transform::translation(DVec2::new(
        5.0 * spread(golden),
        5.0 * spread(golden * golden),
    ))
    .compose(&Transform::rotation_around(
        centre,
        0.2_f64.to_radians() * spread(golden / 2.0),
    ))
}

/// The sky's level in exposure `idx`, against the field's frame: a cycle of five through 0.92,
/// 0.96, 1, 1.04 and 1.08, each the nearest f32 to its percentage.
fn exposure_sky(idx: usize) -> f32 {
    (92 + 4 * (idx * 7 % 5)) as f32 / 100.0
}

/// The share of the stars' light that reaches the sensor in exposure `idx`: a cycle of four
/// through 1, 0.97, 0.94 and 0.91, each the nearest f32 to its percentage.
fn exposure_transparency(idx: usize) -> f32 {
    (100 - 3 * (idx * 3 % 4)) as f32 / 100.0
}

/// How much of the sensor a frame shows.
#[derive(Debug, Clone, Copy)]
enum Readout {
    /// As calibration leaves a light: the filters' balance and the hot pixels.
    Calibrated,
    /// As the sensor reads a light: the bias, the dark current, the vignette and every defect too.
    Raw,
}

/// The sensor of `size` behind `cfa_type`'s filters, as `readout` shows it.
#[derive(Debug, Clone, Copy)]
struct Sensor {
    size: Size2us,
    cfa_type: CfaType,
    readout: Readout,
}

impl Sensor {
    /// The sensor as calibration leaves a light.
    const fn calibrated(size: Size2us, cfa_type: CfaType) -> Self {
        Self {
            size,
            cfa_type,
            readout: Readout::Calibrated,
        }
    }

    /// The sensor as it reads a light.
    const fn raw(size: Size2us, cfa_type: CfaType) -> Self {
        Self {
            size,
            cfa_type,
            readout: Readout::Raw,
        }
    }

    /// The camera that renders what this sensor shows.
    fn camera(self) -> Camera {
        let Self {
            size,
            cfa_type,
            readout,
        } = self;
        let mut camera = Camera::realistic(FWHM);
        camera.cfa_type = cfa_type;
        if cfa_type != CfaType::Mono {
            camera.flat.color_gain = FILTER_GAIN;
        }
        let SensorDefects { hot, dead } = defects(size);
        camera.defects.hot = hot;
        match readout {
            Readout::Calibrated => camera.dark_current_e_per_s = 0.0,
            Readout::Raw => {
                camera.bias.offset = BIAS;
                camera.flat.vignette = Some(VIGNETTE);
                camera.defects.dead = dead;
            }
        }
        camera
    }
}

/// The sensor's hot and dead pixels on a frame of `size`, the same on every frame of that size.
fn defects(size: Size2us) -> SensorDefects {
    let mut rng = TestRng::new(SENSOR_SEED);
    let pos = |rng: &mut TestRng| {
        let x = (rng.next_u64() % size.width as u64) as usize;
        let y = (rng.next_u64() % size.height as u64) as usize;
        (x, y)
    };
    let dead = (0..at_density(DEAD_PIXELS, size))
        .map(|_| pos(&mut rng))
        .collect();
    let hot = (0..at_density(HOT_PIXELS, size))
        .map(|_| {
            let (x, y) = pos(&mut rng);
            (x, y, 0.05 + 0.45 * rng.next_f32())
        })
        .collect();
    SensorDefects { hot, dead }
}

/// A one-plane frame as the mosaic of `cfa_type`, under [`WHITE_BALANCE`].
fn mosaic(frame: SimFrame, cfa_type: CfaType) -> CfaImage {
    let mut image = frame.image;
    let mut metadata = std::mem::take(&mut image.metadata);
    if cfa_type != CfaType::Mono {
        metadata.header.camera_white_balance = Some(WHITE_BALANCE);
    }
    CfaImage::from_plane(image.into_pixels().into_mono(), cfa_type, metadata)
}

/// `count` of something the standard frame holds, at the same density on a frame of `size`, and
/// at least one of it when the frame holds any.
const fn at_density(count: usize, size: Size2us) -> usize {
    (count * size.pixel_count()).div_ceil(FRAME.pixel_count())
}

#[cfg(test)]
mod tests;
