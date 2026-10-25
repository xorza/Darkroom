//! The parts of one star's measurement, each on one star as production measures every star of a
//! frame on the global pool, a star a task: the windowed centroid and the profile fits on the
//! stamp of a tight star, of FWHM 4 and 15 px, and of a wide one, of FWHM 8.5 and 31 px, the
//! largest stamp, and the sky annulus of the wide one, which reaches furthest.

use std::f64::consts::TAU;
use std::hint::black_box;

use common::Buffer2;
use criterion::{BenchmarkId, Criterion, Throughput};
use glam::DVec2;

use crate::bench::fixture::{Kind, Scale};
use crate::image::masked_plane::MaskedPlane;
use crate::internals::synthetic::camera::{Camera, PsfModel};
use crate::internals::synthetic::observe::{Observation, render};
use crate::internals::synthetic::scene::{BgField, Scene, TrueSource};
use crate::math::fwhm;
use crate::math::size2us::Size2us;
use crate::star_detection::centroid::gaussian_fit::GaussianFit;
use crate::star_detection::centroid::measure_grid::MeasureGrid;
use crate::star_detection::centroid::moffat_fit::MoffatFit;
use crate::star_detection::centroid::stamp::FitInputs;
use crate::star_detection::centroid::stamp_bg::StampBg;
use crate::star_detection::centroid::star_noise::StarNoise;
use crate::star_detection::centroid::windowed_centroid::{WindowedCentroid, WindowedInputs};

/// The sky the stars sit on, which the residual has taken out.
const SKY: f32 = 0.1;

/// The Moffat β of atmospheric seeing, which the ground-based preset fits with.
const BETA: f32 = 2.5;

/// The frame a star is rendered on: room for the wide star's annulus, out to 4 FWHM and a stamp
/// radius beyond, about 49 px each way.
const SIZE: Size2us = Size2us::new(128, 128);

pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "detection/centroid", Scale::Sample);
    for star in [Star::TIGHT, Star::WIDE] {
        let stamp = star.stamp();
        group.throughput(Throughput::Elements((stamp * stamp) as u64));
        group.bench_function(BenchmarkId::new("moments", star.name()), |b| {
            let measured = Measured::of(star, PsfModel::gaussian(star.fwhm));
            b.iter(|| {
                WindowedCentroid::measure(
                    black_box(&measured.residual),
                    &measured.grid,
                    measured.inputs(),
                )
            });
        });
        group.bench_function(BenchmarkId::new("gaussian_fit", star.name()), |b| {
            let measured = Measured::of(star, PsfModel::gaussian(star.fwhm));
            let at = measured.centre();
            let fit = || {
                GaussianFit::new(FitInputs {
                    pixels: black_box(&measured.residual),
                    pos: at,
                    grid: &measured.grid.stamp,
                    bg: 0.0,
                    noise: None,
                })
            };
            assert!(fit().is_some(), "the fit converges on a bright star");
            b.iter(fit);
        });
        group.bench_function(BenchmarkId::new("moffat_fit", star.name()), |b| {
            let psf = PsfModel::Moffat {
                fwhm: star.fwhm,
                beta: BETA,
            };
            let measured = Measured::of(star, psf);
            let at = measured.centre();
            let fit = || {
                MoffatFit::new(
                    FitInputs {
                        pixels: black_box(&measured.residual),
                        pos: at,
                        grid: &measured.grid.stamp,
                        bg: 0.0,
                        noise: None,
                    },
                    BETA,
                )
            };
            assert!(fit().is_some(), "the fit converges on a bright star");
            b.iter(fit);
        });
    }
    let star = Star::WIDE;
    let annulus = MeasureGrid::new(star.fwhm).annulus;
    // The pixels whose centre lies between the radii, as `StampBg::measure` takes them.
    let side = 0..=2 * annulus.outer;
    let radii = annulus.inner.pow(2)..=annulus.outer.pow(2);
    let pixels = side
        .clone()
        .flat_map(|y| side.clone().map(move |x| (x, y)))
        .filter(|&(x, y)| {
            radii.contains(&(x.abs_diff(annulus.outer).pow(2) + y.abs_diff(annulus.outer).pow(2)))
        })
        .count();
    group.throughput(Throughput::Elements(pixels as u64));
    group.bench_function(BenchmarkId::new("annulus_bg", star.name()), |b| {
        let measured = Measured::of(star, PsfModel::gaussian(star.fwhm));
        let at = measured.centre();
        let measure = || {
            StampBg::measure(
                MaskedPlane::from(black_box(&measured.residual)),
                at,
                annulus,
            )
        };
        assert!(measure().is_some(), "the annulus lies inside the frame");
        b.iter(measure);
    });
}

/// A star of one width, which sets its stamp.
#[derive(Debug, Clone, Copy)]
struct Star {
    fwhm: f32,
}

impl Star {
    /// Radius ⌈1.75 · 4⌉ = 7.
    const TIGHT: Self = Self { fwhm: 4.0 };
    /// Radius ⌈1.75 · 8.5⌉ = 15, the largest.
    const WIDE: Self = Self { fwhm: 8.5 };

    fn stamp(self) -> usize {
        2 * MeasureGrid::new(self.fwhm).stamp.radius + 1
    }

    fn name(self) -> String {
        format!("stamp_{}", self.stamp())
    }
}

/// One star on its sky, less the sky, and what its measurement starts from.
#[derive(Debug)]
struct Measured {
    residual: Buffer2<f32>,
    grid: MeasureGrid,
    /// The pixel the detection peaked at.
    start: DVec2,
}

impl Measured {
    /// `star` rendered by `psf` at an off-centre position, its peak at half the full well.
    fn of(star: Star, psf: PsfModel) -> Self {
        let sigma = f64::from(fwhm::to_sigma(star.fwhm));
        let pos = DVec2::new(64.3, 64.7);
        let flux = (0.5 * TAU * sigma * sigma) as f32;
        let scene = Scene::single(
            SIZE,
            TrueSource { pos, flux },
            BgField::Uniform { level: SKY },
        );
        let camera = Camera {
            psf,
            ..Camera::realistic(star.fwhm)
        };
        let mut residual = render(&scene, &camera, &Observation::reference(7))
            .image
            .channel(0)
            .clone();
        for pixel in residual.pixels_mut() {
            *pixel -= SKY;
        }
        Self {
            residual,
            grid: MeasureGrid::new(star.fwhm),
            start: pos.round(),
        }
    }

    /// The sky's noise as the camera gives it: shot noise of the sky in a 50 ke⁻ well and 3 e⁻ of
    /// read noise.
    fn inputs(&self) -> WindowedInputs {
        let camera = Camera::realistic(1.0);
        let shot = f64::from(SKY) / f64::from(camera.full_well_e);
        let read = f64::from(camera.read_noise_e / camera.full_well_e);
        WindowedInputs {
            start: self.start,
            offset: 0.0,
            noise: StarNoise {
                bg_sigma: (shot + read * read).sqrt(),
                electrons_per_unit: None,
            },
        }
    }

    /// The windowed centroid, which the fits and the annulus start from in production.
    fn centre(&self) -> DVec2 {
        WindowedCentroid::measure(&self.residual, &self.grid, self.inputs())
            .expect("a bright star's centroid converges")
            .pos
    }
}
