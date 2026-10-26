//! [`StarField`]: one field of stars, exposed at a dither and written as FITS.

use std::f32::consts::{LN_2, TAU};
use std::path::{Path, PathBuf};

use common::{Buffer2, TempDir, fs};
use fits_well::FitsWriter;
use fits_well::header::Header;
use fits_well::image::{Image as FitsImage, Scaling};
use imaginarium::{ChannelLayout, PlanarDesc};
use lumos::{BayerPattern, CfaImage, CfaType, ImageMetadata, LinearImage, Mosaic};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;

/// The sky every exposure sits on.
const SKY: f32 = 0.1;
/// The σ of every exposure's Gaussian noise.
const NOISE: f32 = 0.01;
/// How far the stars keep from the frame's edge, wider than any dither.
const MARGIN: f32 = 16.0;
/// The stars' full width at half maximum, in pixels.
const FWHM: f32 = 4.0;

/// `side` × `side` pixels of Gaussian stars at fixed places, each a position and a peak over the
/// sky.
#[derive(Debug)]
pub(crate) struct StarField {
    side: usize,
    stars: Vec<[f32; 3]>,
}

/// How an exposure is written to a file: the one plane of an RGGB mosaic, or a mono frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    Mosaic,
    Mono,
}

impl StarField {
    /// `stars` stars at places and peaks drawn from `seed`; none leaves a frame of sky.
    pub(crate) fn new(side: usize, stars: usize, seed: u64) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let span = side as f32 - 2.0 * MARGIN;
        let stars = (0..stars)
            .map(|_| {
                [
                    MARGIN + span * rng.random::<f32>(),
                    MARGIN + span * rng.random::<f32>(),
                    0.1 + 0.5 * rng.random::<f32>(),
                ]
            })
            .collect();
        Self { side, stars }
    }

    /// The dither of exposure `idx`: rows of three, 3 px apart on each axis.
    pub(crate) fn dither(idx: usize) -> [f32; 2] {
        [(idx % 3) as f32 * 3.0 - 3.0, (idx / 3) as f32 * 3.0 - 1.5]
    }

    /// `count` exposures at [`Self::dither`], each with noise of its own, written under `scratch`
    /// as `layout` says.
    pub(crate) fn expose(&self, scratch: &TempDir, count: usize, layout: Layout) -> Vec<PathBuf> {
        (0..count)
            .map(|idx| {
                let path = scratch.join(format!("exposure_{idx}.fits"));
                self.write(&path, idx, layout);
                path
            })
            .collect()
    }

    /// Exposure `idx` as a mono frame in memory, as [`Self::expose`] writes it.
    pub(crate) fn frame(&self, idx: usize) -> LinearImage {
        LinearImage::from_planar_channels(
            PlanarDesc::new((self.side, self.side), ChannelLayout::Mono),
            [self.render(idx)],
        )
    }

    /// Exposure `idx` as an RGGB mosaic in memory, as [`Self::expose`] writes it as
    /// [`Layout::Mosaic`].
    pub(crate) fn mosaic(&self, idx: usize) -> CfaImage {
        CfaImage::from_plane(
            Buffer2::new((self.side, self.side), self.render(idx)),
            CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb)),
            ImageMetadata::default(),
        )
    }

    fn write(&self, path: &Path, idx: usize, layout: Layout) {
        let mut header = Header::new();
        if layout == Layout::Mosaic {
            header.set("BAYERPAT", "RGGB").unwrap();
        }
        let mut writer = FitsWriter::new(fs::create(path).unwrap());
        writer
            .write_image(
                FitsImage::new([self.side, self.side], self.render(idx), Scaling::IDENTITY)
                    .unwrap()
                    .view(),
                Some(&header),
            )
            .unwrap();
        writer.into_inner().sync_all().unwrap();
    }

    /// The pixels of exposure `idx`: the stars at its dither over the sky, and noise drawn from
    /// its index.
    fn render(&self, idx: usize) -> Vec<f32> {
        let side = self.side;
        let shift = Self::dither(idx);
        let mut rng = ChaCha8Rng::seed_from_u64(idx as u64);
        let mut pixels = vec![SKY; side * side];
        // A Gaussian's FWHM is 2·√(2·ln 2) of its σ.
        let sigma = FWHM / (8.0 * LN_2).sqrt();
        let reach = (4.0 * sigma).ceil() as isize;
        for &[x, y, peak] in &self.stars {
            let centre = [x + shift[0], y + shift[1]];
            for dy in -reach..=reach {
                for dx in -reach..=reach {
                    let pixel = [centre[0] as isize + dx, centre[1] as isize + dy];
                    let offset = [pixel[0] as f32 - centre[0], pixel[1] as f32 - centre[1]];
                    let distance = offset[0] * offset[0] + offset[1] * offset[1];
                    let [col, row] = pixel
                        .map(|at| usize::try_from(at).expect("the stars keep clear of the edges"));
                    pixels[row * side + col] += peak * (-distance / (2.0 * sigma * sigma)).exp();
                }
            }
        }
        for sample in &mut pixels {
            // Box–Muller.
            let (u, v) = (1.0 - rng.random::<f32>(), rng.random::<f32>());
            *sample += NOISE * (-2.0 * u.ln()).sqrt() * (TAU * v).cos();
        }
        pixels
    }
}
