//! The scenes the benches measure on: seeded renders of the synthetic forward model, at the
//! standard frame or, for a test of a bench's input, a smaller one with the same density.

use crate::internals::cfa::make_cfa;
use crate::internals::synthetic::artifacts::add_cosmic_rays;
use crate::internals::synthetic::camera::Camera;
use crate::internals::synthetic::observe::{Observation, SimFrame, render};
use crate::internals::synthetic::scene::{BackgroundField, Scene};
use crate::internals::test_rng::TestRng;
use crate::io::image::cfa::{CfaImage, CfaType};
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// The standard frame: 6000 × 4000, an APS-C camera's 24 MP.
pub(crate) const FRAME: Size2us = Size2us::new(6000, 4000);

/// A daylight white balance, red and blue gains over green's, in `camera_white_balance`'s order.
/// A mosaic's photosites read the scene divided by their colour's gain, as a sensor's do under the
/// filters, so the balanced demosaic gives the scene back.
const WHITE_BALANCE: [f32; 4] = [2.13, 1.0, 1.71, 1.0];

/// The PSF FWHM of every field, in pixels: average seeing on an APS-C camera at a moderate focal
/// length.
const FWHM: f32 = 3.5;

/// The hot pixels the standard frame holds: one in 100 000.
const HOT_PIXELS: usize = 240;

/// The cosmic-ray hits the standard frame holds, as a few minutes' exposure collects.
const COSMIC_RAYS: usize = 200;

/// The content of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    /// 3000 stars on a sky with a gradient: a field at moderate galactic latitude.
    Typical,
}

impl Field {
    /// The bench parameter that names the field.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Field::Typical => "typical",
        }
    }

    const fn stars(self) -> usize {
        match self {
            Field::Typical => 3000,
        }
    }

    const fn seed(self) -> u64 {
        match self {
            Field::Typical => 0x7_1C41,
        }
    }

    /// The field on a mono sensor of `size`: photon and read noise, dark current, and a few hot
    /// pixels.
    pub(crate) fn render(self, size: Size2us) -> SimFrame {
        let scene = Scene::random_field(
            size,
            at_density(self.stars(), size),
            (0.5, 12.0),
            BackgroundField::Gradient {
                start: 0.08,
                end: 0.12,
                angle: 0.6,
            },
            16.0,
            self.seed(),
        );
        let mut camera = Camera::realistic(FWHM);
        let mut rng = TestRng::new(self.seed() ^ 0x407);
        camera.defects.hot = (0..at_density(HOT_PIXELS, size))
            .map(|_| {
                let x = (rng.next_u64() % size.width as u64) as usize;
                let y = (rng.next_u64() % size.height as u64) as usize;
                (x, y, 0.05 + 0.45 * rng.next_f32())
            })
            .collect();
        render(&scene, &camera, &Observation::reference(self.seed()))
    }

    /// The field on a colour sensor of `size` behind `cfa_type`'s filters, under [`WHITE_BALANCE`],
    /// with cosmic-ray hits.
    pub(crate) fn mosaic(self, size: Size2us, cfa_type: CfaType) -> CfaImage {
        let frame = self.render(size);
        let mut pixels = frame.image.channel(0).pixels().to_vec();
        for (index, pixel) in pixels.iter_mut().enumerate() {
            let pos = Vec2us::new(index % size.width, index / size.width);
            *pixel /= WHITE_BALANCE[cfa_type.color_at(pos)];
        }
        add_cosmic_rays(
            &mut pixels,
            size.width,
            at_density(COSMIC_RAYS, size),
            (0.3, 1.0),
            self.seed() ^ 0xC05,
        );
        for pixel in &mut pixels {
            *pixel = pixel.min(1.0);
        }
        let mut mosaic = make_cfa(size, pixels, cfa_type);
        mosaic.metadata.camera_white_balance = Some(WHITE_BALANCE);
        mosaic
    }
}

/// `count` of something the standard frame holds, at the same density on a frame of `size`.
const fn at_density(count: usize, size: Size2us) -> usize {
    count * size.pixel_count() / FRAME.pixel_count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internals::cfa::XTRANS_PATTERN;
    use crate::io::image::cfa::cfa_pattern::CfaPattern;

    /// 600 × 400 is a hundredth of the frame: 3000 / 100 = 30 stars, 240 / 100 = 2 hot pixels
    /// (integer division), 200 / 100 = 2 cosmic rays.
    const SMALL: Size2us = Size2us::new(600, 400);

    #[test]
    fn a_field_keeps_its_density_on_a_smaller_frame() {
        assert_eq!(at_density(HOT_PIXELS, SMALL), 2);
        assert_eq!(at_density(COSMIC_RAYS, SMALL), 2);
        assert_eq!(at_density(Field::Typical.stars(), FRAME), 3000);
        let frame = Field::Typical.render(SMALL);
        assert_eq!(frame.truth.sources.len(), 30);
        assert_eq!(frame.image.channel(0).pixels().len(), 600 * 400);
    }

    /// Every photosite reads the mono render divided by its colour's gain, and a cosmic ray only
    /// adds: 2 hits, each on one photosite and on up to two neighbours with bleeding.
    #[test]
    fn a_mosaic_is_the_render_behind_the_filters() {
        let render = Field::Typical.render(SMALL);
        let render = render.image.channel(0).pixels();
        for cfa_type in [
            CfaType::Bayer(CfaPattern::Rggb),
            CfaType::XTrans(XTRANS_PATTERN),
        ] {
            let mosaic = Field::Typical.mosaic(SMALL, cfa_type);
            assert_eq!(mosaic.metadata.camera_white_balance, Some(WHITE_BALANCE));
            let mut hit = 0;
            for (index, (&photosite, &scene)) in mosaic.data.pixels().iter().zip(render).enumerate()
            {
                let pos = Vec2us::new(index % SMALL.width, index / SMALL.width);
                let filtered = scene / WHITE_BALANCE[cfa_type.color_at(pos)];
                assert!(photosite >= filtered, "{pos:?}: {photosite} < {filtered}");
                hit += usize::from(photosite != filtered);
            }
            assert!((1..=6).contains(&hit), "{hit} photosites hit");
        }
    }
}
