//! The registration benches: the entry `crate::bench` runs them by, and the input they share, a
//! typical field's star catalogue in two exposures.

use common::bench::{Kind, Scale};
use criterion::{BenchmarkId, Criterion, Throughput};
use glam::DVec2;

use crate::bench::scene::{FRAME, Field};
use crate::internals::synthetic::distortion::RadialField;
use crate::internals::test_rng::TestRng;
use crate::registration::distortion::sip::{SipConfig, SipModel};
use crate::registration::final_fit::{FinalFit, FitModel};
use crate::registration::register::PreparedPair;
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::resample::{self, row_poss};
use crate::registration::transform::linear_model::LinearModel;
use crate::registration::transform::{Transform, TransformType};
use crate::registration::warp_transform::WarpTransform;
use crate::registration::{final_fit, kd_tree, ransac, triangle};
use crate::star_detection::star::Star;

/// Two star catalogues registered, as `register` takes every light's against the reference's:
/// the default configuration, whose `Auto` model fits every transform up to a homography and keeps
/// the one the geometric robust information criterion picks.
pub(crate) fn integration(c: &mut Criterion) {
    pair(c);
    resample::bench::integration(c);
}

fn pair(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = Kind::Integration.group(c, "register", Scale::Millis);
    group.throughput(Throughput::Elements(field.stars() as u64));
    group.bench_function(BenchmarkId::new("pair", field.name()), |b| {
        let pair = Pair::typical();
        let config = RegistrationConfig::default();
        b.iter(|| {
            config
                .registrar()
                .expect("the default configuration is valid")
                .register(&pair.reference, &pair.target)
                .expect("the pair registers")
        });
    });
}

pub(crate) fn atomic(c: &mut Criterion) {
    kd_tree::bench::atomic(c);
    triangle::bench::atomic(c);
    ransac::bench::atomic(c);
    final_fit::bench::atomic(c);
    row_poss::bench::atomic(c);
}

/// The FWHM the stars are measured at, as the scene renders them.
const FWHM: f32 = 3.5;

/// A star position's measurement error, in pixels: a centroid's at the scene's typical SNR.
const POS_SIGMA: f64 = 0.05;

/// A typical field's star catalogue in two exposures through one lens, each sorted by flux as the
/// detector returns it: the second dithered by (11, −7) px and turned 0.2°, as a dithered sequence
/// is, and the lens's radial distortion moving the frame's corners by about a pixel. Each position
/// is off by its measurement error.
#[derive(Debug)]
pub(crate) struct Pair {
    pub(super) reference: Vec<Star>,
    pub(super) target: Vec<Star>,
}

impl Pair {
    pub(crate) fn typical() -> Self {
        let mut sources = Field::Typical.scene(FRAME).sources;
        sources.sort_by(|a, b| b.flux.total_cmp(&a.flux));
        let centre = DVec2::new(FRAME.width as f64, FRAME.height as f64) / 2.0;
        // k · r³ is one pixel at the corner's r = |(3000, 2000)|.
        let lens = RadialField::new(centre, 1.0 / centre.length().powi(3));
        let dither = Transform::similarity(DVec2::new(11.0, -7.0), 0.2_f64.to_radians(), 1.0);
        let mut rng = TestRng::new(0x5E9);
        let mut star = |pos: DVec2, flux: f32| {
            let error = DVec2::new(
                f64::from(rng.next_gaussian_f32()),
                f64::from(rng.next_gaussian_f32()),
            );
            Star {
                pos_sigma: POS_SIGMA,
                ..Star::at(pos + POS_SIGMA * error)
                    .with_flux(flux)
                    .with_fwhm(FWHM)
            }
        };
        let reference = sources
            .iter()
            .map(|source| star(lens.image(source.pos), source.flux))
            .collect();
        let target = sources
            .iter()
            .map(|source| star(lens.image(dither.apply(source.pos)), source.flux))
            .collect();
        Self { reference, target }
    }

    /// The pair prepared under `config`, as `register` prepares it.
    pub(super) fn prepared<'a>(&self, config: &'a RegistrationConfig) -> PreparedPair<'a> {
        PreparedPair::new(config, &self.reference, &self.target).expect("the pair is valid")
    }

    /// RANSAC's `transform` on the matches of the default configuration's brightest stars: the
    /// hypothesis the final fit starts from.
    pub(super) fn hypothesis(&self, transform: TransformType) -> Transform {
        let config = RegistrationConfig::default();
        let prepared = self.prepared(&config);
        prepared
            .search(&prepared.matches())
            .hypothesis(transform)
            .expect("the pair registers")
            .transform
    }

    /// `transform` under `correction`: the SIP correction the precise presets add is taken about
    /// the origin `register` places it at.
    pub(super) fn model(&self, transform: TransformType, correction: Correction) -> FitModel {
        match correction {
            Correction::Linear => FitModel::Plain(transform),
            Correction::Sip => FitModel::WithSip(
                LinearModel::try_from(transform).expect("a SIP correction sits under up to affine"),
                SipModel {
                    order: SipConfig::default().order,
                    origin: self.prepared(&RegistrationConfig::default()).origin(),
                },
            ),
        }
    }

    /// The similarity `register` fits to the pair: the dither, with the lens left in its residuals.
    pub(crate) fn similarity_warp(&self) -> WarpTransform {
        self.fit(self.model(TransformType::Similarity, Correction::Linear))
            .warp
    }

    /// The warp `register` fits to the pair with the SIP correction the precise presets add: the
    /// dither and the lens together.
    pub(crate) fn sip_warp(&self) -> WarpTransform {
        self.fit(self.model(TransformType::Similarity, Correction::Sip))
            .warp
    }

    /// `model` fitted as `register` fits it under the default configuration: RANSAC on the
    /// matches, then the final fit over the whole catalogues.
    pub(super) fn fit(&self, model: FitModel) -> FinalFit {
        let config = RegistrationConfig::default();
        let prepared = self.prepared(&config);
        prepared
            .search(&prepared.matches())
            .estimate_and_refine(model)
            .expect("the pair registers")
    }
}

/// Whether a benched fit carries the SIP correction the precise presets add.
#[derive(Debug, Clone, Copy)]
pub(super) enum Correction {
    Linear,
    Sip,
}
