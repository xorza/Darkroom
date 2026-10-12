//! The scenes the benches measure on: seeded renders of the synthetic forward model, at the
//! standard frame or, for a test of a bench's input, a smaller one at the same density.
//!
//! Every frame is taken on one sensor, with its filters, vignette, bias, dark current and defects.
//! A light reads all of them, and the masters that calibrate it come from the same sensor. A
//! calibrated frame is a light as calibration leaves it: the filters' colour balance and the hot
//! pixels stay, the bias, the dark current and the vignette are gone.

use crate::internals::synthetic::artifacts::add_cosmic_rays;
use crate::internals::synthetic::backgrounds::Vignette;
use crate::internals::synthetic::camera::{Camera, SensorDefects};
use crate::internals::synthetic::observe::{Observation, SimFrame, render};
use crate::internals::synthetic::scene::{BackgroundField, Scene};
use crate::internals::test_rng::TestRng;
use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::{CalibrationSet, MasterRole};

/// The standard frame: 6000 × 4000, an APS-C camera's 24 MP.
pub(crate) const FRAME: Size2us = Size2us::new(6000, 4000);

/// A daylight white balance, red and blue gains over green's, in `camera_white_balance`'s order.
const WHITE_BALANCE: [f32; 4] = [2.13, 1.0, 1.71, 1.0];

/// What each colour's filter passes, in [`CfaType::color_at`]'s slots: the inverse of
/// [`WHITE_BALANCE`], so the balanced demosaic gives grey back for grey.
const FILTER_GAIN: [f32; 3] = [1.0 / 2.13, 1.0, 1.0 / 1.71];

/// The sky of every field: a gradient, as light pollution or the moon lays one.
const SKY: BackgroundField = BackgroundField::Gradient {
    start: 0.08,
    end: 0.12,
    angle: 0.6,
};

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
            Field::Typical => "typical",
            Field::Dense => "dense",
        }
    }

    const fn stars(self) -> usize {
        match self {
            Field::Typical => 3000,
            Field::Dense => 20_000,
        }
    }

    const fn seed(self) -> u64 {
        match self {
            Field::Typical => 0x7_1C41,
            Field::Dense => 0xD_E45E,
        }
    }

    /// The true sky of the field over a frame of `size`.
    pub(crate) fn scene(self, size: Size2us) -> Scene {
        let (stars, seed) = (at_density(self.stars(), size), self.seed());
        match self {
            Field::Typical => Scene::random_field(size, stars, FLUX, SKY, 16.0, seed),
            Field::Dense => Scene::cluster(size, stars, FLUX, SKY, seed),
        }
    }

    /// The field calibrated, on a mono sensor of `size`.
    pub(crate) fn render(self, size: Size2us) -> SimFrame {
        self.expose(size, CfaType::Mono, Readout::Calibrated)
    }

    /// The field calibrated, on a sensor of `size` behind `cfa_type`'s filters, with cosmic-ray
    /// hits.
    pub(crate) fn mosaic(self, size: Size2us, cfa_type: CfaType) -> CfaImage {
        self.hit(size, cfa_type, Readout::Calibrated)
    }

    /// The field as a sensor of `size` behind `cfa_type`'s filters reads it, with cosmic-ray hits:
    /// what [`masters`] calibrate.
    pub(crate) fn light(self, size: Size2us, cfa_type: CfaType) -> CfaImage {
        self.hit(size, cfa_type, Readout::Raw)
    }

    fn expose(self, size: Size2us, cfa_type: CfaType, readout: Readout) -> SimFrame {
        let observation = Observation {
            exposure_s: EXPOSURE_S,
            ..Observation::reference(self.seed())
        };
        render(
            &self.scene(size),
            &sensor(size, cfa_type, readout),
            &observation,
        )
    }

    fn hit(self, size: Size2us, cfa_type: CfaType, readout: Readout) -> CfaImage {
        let mut image = mosaic(self.expose(size, cfa_type, readout), cfa_type);
        let pixels = image.data.pixels_mut();
        add_cosmic_rays(
            pixels,
            size.width,
            at_density(COSMIC_RAYS, size),
            (0.3, 1.0),
            self.seed() ^ 0xC05,
        );
        for pixel in pixels {
            *pixel = pixel.min(1.0);
        }
        image
    }
}

/// The bias, dark and flat masters of the sensor a light of `size` behind `cfa_type` is taken on,
/// one exposure each: how deep a master is changes none of the work that calibrates with it.
pub(crate) fn masters(size: Size2us, cfa_type: CfaType) -> CalibrationSet<Option<CfaImage>> {
    let expose = |role: MasterRole, seed: u64, level: f32, exposure_s: f32| {
        let scene = Scene {
            size,
            sources: Vec::new(),
            background: BackgroundField::Uniform { level },
        };
        let observation = Observation {
            exposure_s,
            ..Observation::reference(SENSOR_SEED ^ seed)
        };
        let frame = render(&scene, &sensor(size, cfa_type, Readout::Raw), &observation);
        let mut master = mosaic(frame, cfa_type);
        master.metadata.image_type = Some(role.extname().to_string());
        Some(master)
    };
    CalibrationSet {
        bias: expose(MasterRole::Bias, 1, 0.0, 0.0),
        dark: expose(MasterRole::Dark, 2, 0.0, EXPOSURE_S),
        flat: expose(MasterRole::Flat, 3, FLAT_LEVEL, 1.0),
        flat_dark: None,
    }
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
fn sensor(size: Size2us, cfa_type: CfaType, readout: Readout) -> Camera {
    let mut camera = Camera::realistic(FWHM);
    camera.cfa_type = cfa_type;
    if cfa_type != CfaType::Mono {
        camera.flat.colour_gain = FILTER_GAIN;
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

/// The sensor's hot and dead pixels on a frame of `size`, the same on every frame of that size.
fn defects(size: Size2us) -> SensorDefects {
    let mut rng = TestRng::new(SENSOR_SEED);
    let position = |rng: &mut TestRng| {
        let x = (rng.next_u64() % size.width as u64) as usize;
        let y = (rng.next_u64() % size.height as u64) as usize;
        (x, y)
    };
    let dead = (0..at_density(DEAD_PIXELS, size))
        .map(|_| position(&mut rng))
        .collect();
    let hot = (0..at_density(HOT_PIXELS, size))
        .map(|_| {
            let (x, y) = position(&mut rng);
            (x, y, 0.05 + 0.45 * rng.next_f32())
        })
        .collect();
    SensorDefects { hot, dead }
}

/// A one-plane frame as the mosaic of `cfa_type`, under [`WHITE_BALANCE`].
fn mosaic(frame: SimFrame, cfa_type: CfaType) -> CfaImage {
    let LinearImage {
        mut metadata,
        pixels,
        ..
    } = frame.image;
    if cfa_type != CfaType::Mono {
        metadata.camera_white_balance = Some(WHITE_BALANCE);
    }
    CfaImage {
        data: pixels.into_l(),
        cfa_type,
        metadata,
        flags: None,
    }
}

/// `count` of something the standard frame holds, at the same density on a frame of `size`, and
/// at least one of it when the frame holds any.
const fn at_density(count: usize, size: Size2us) -> usize {
    (count * size.pixel_count()).div_ceil(FRAME.pixel_count())
}

#[cfg(test)]
mod tests {
    use common::CancelToken;

    use super::*;
    use crate::calibration_masters::defect_map::DefectMap;
    use crate::internals::synthetic::camera::FlatField;
    use crate::io::image::cfa::cfa_pattern::CfaPattern;
    use crate::math::statistics::median_mut;
    use crate::math::vec2us::Vec2us;
    use crate::{CalibrationMasters, DEFAULT_SIGMA_THRESHOLD};

    /// 600 × 400 is a hundredth of the frame: 3000 / 100 = 30 stars, 20 000 / 100 = 200 in the
    /// cluster, 200 / 100 = 2 cosmic rays,
    /// and, rounded up, ⌈240 / 100⌉ = 3 hot pixels and ⌈24 / 100⌉ = 1 dead one.
    const SMALL: Size2us = Size2us::new(600, 400);

    const RGGB: CfaType = CfaType::Bayer(CfaPattern::Rggb);

    #[test]
    fn a_field_keeps_its_density_on_a_smaller_frame() {
        assert_eq!(at_density(COSMIC_RAYS, SMALL), 2);
        assert_eq!(at_density(Field::Typical.stars(), FRAME), 3000);
        assert_eq!(at_density(Field::Dense.stars(), FRAME), 20_000);
        assert_eq!(Field::Dense.scene(SMALL).sources.len(), 200);
        let SensorDefects { hot, dead } = defects(SMALL);
        assert_eq!((hot.len(), dead.len()), (3, 1));
        let frame = Field::Typical.render(SMALL);
        assert_eq!(frame.truth.sources.len(), 30);
        assert_eq!(frame.image.channel(0).pixels().len(), 600 * 400);
        assert_eq!(frame.image.metadata.camera_white_balance, None);
        let mosaic = Field::Typical.mosaic(SMALL, RGGB);
        assert_eq!(mosaic.metadata.camera_white_balance, Some(WHITE_BALANCE));
    }

    /// The masters carry the sensor's defects: against the dark's noise, σ ≈ 8·10⁻⁵ from 3 e⁻ of
    /// read noise and 6 e⁻ of dark current in a 50 ke⁻ well, the least hot excess, 0.05, is over
    /// 600σ, and a dead pixel reads nothing under the flat once its bias is out. The map finds
    /// exactly the sensor's hot pixels in the dark and its dead pixel in the flat.
    #[test]
    fn the_masters_find_the_sensors_defects() {
        let CalibrationSet {
            bias, dark, flat, ..
        } = masters(SMALL, RGGB);
        let (bias, dark, mut flat) = (bias.unwrap(), dark.unwrap(), flat.unwrap());
        for (flat, bias) in flat.data.pixels_mut().iter_mut().zip(bias.data.pixels()) {
            *flat -= bias;
        }
        let map = DefectMap::new(SMALL)
            .detect_hot(&dark, DEFAULT_SIGMA_THRESHOLD, &CancelToken::never())
            .unwrap()
            .detect_cold(&flat, &CancelToken::never())
            .unwrap();
        let SensorDefects { hot, dead } = defects(SMALL);
        let index = |x: usize, y: usize| SMALL.index_of(Vec2us::new(x, y));
        let mut hot: Vec<usize> = hot.into_iter().map(|(x, y, _)| index(x, y)).collect();
        hot.sort_unstable();
        assert_eq!(map.hot().indices, hot);
        let dead: Vec<usize> = dead.into_iter().map(|(x, y)| index(x, y)).collect();
        assert_eq!(map.cold_indices(), dead);
    }

    /// Calibrating a light with its masters takes out the bias and the vignette: each colour of it
    /// is then that colour of the calibrated mosaic times the vignette's mean over the colour's
    /// photosites, the level the flat is normalized to. The medians of 60 000 samples a colour
    /// agree to far within 0.5%, where a bias of 0.03 left on a sky of 0.1 · 1/2.13 would put the
    /// red one 64% off.
    #[test]
    fn a_light_calibrated_by_its_masters_is_the_calibrated_mosaic() {
        let masters = CalibrationMasters::from_images(
            masters(SMALL, RGGB),
            DEFAULT_SIGMA_THRESHOLD,
            &CancelToken::never(),
        )
        .unwrap();
        let mut light = Field::Typical.light(SMALL, RGGB);
        masters.calibrate(&mut light).unwrap();
        let mosaic = Field::Typical.mosaic(SMALL, RGGB);
        let vignette = FlatField {
            vignette: Some(VIGNETTE),
            ..FlatField::default()
        }
        .render(SMALL, &CfaType::Mono);
        for colour in 0..3 {
            let of_colour = |pixels: &[f32]| -> Vec<f32> {
                pixels
                    .iter()
                    .enumerate()
                    .filter(|&(index, _)| RGGB.color_at(SMALL.point_of(index)) == colour)
                    .map(|(_, &value)| value)
                    .collect()
            };
            let falloff = of_colour(&vignette)
                .iter()
                .map(|&v| f64::from(v))
                .sum::<f64>()
                / of_colour(&vignette).len() as f64;
            let calibrated = f64::from(median_mut(&mut of_colour(light.data.pixels())));
            let expected = falloff * f64::from(median_mut(&mut of_colour(mosaic.data.pixels())));
            assert!(
                (calibrated / expected - 1.0).abs() < 0.005,
                "colour {colour}: {calibrated} against {expected}"
            );
        }
    }
}
