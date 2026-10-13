//! The registration benches: the entry `crate::bench` runs them by, and the input they share, a
//! typical field's star catalogue in two exposures.

use criterion::{BenchmarkId, Criterion, Throughput};
use glam::DVec2;

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::internals::synthetic::distortion::RadialField;
use crate::internals::test_rng::TestRng;
use crate::registration::distortion::sip::{SipConfig, SipModel};
use crate::registration::final_fit::fit_catalogs::FitCatalogs;
use crate::registration::final_fit::{FinalFit, FitModel};
use crate::registration::kd_tree::KdTree;
use crate::registration::model_search::ModelSearch;
use crate::registration::point_pairs::PointPairs;
use crate::registration::ransac::RansacEstimator;
use crate::registration::registration_config::{
    RegistrationConfig, bounding_box_centre, median_fwhm,
};
use crate::registration::resample::row_positions;
use crate::registration::transform::warp_transform::WarpTransform;
use crate::registration::transform::{Transform, TransformType};
use crate::registration::triangle::voting::PointMatch;
use crate::registration::{final_fit, kd_tree, ransac, triangle, tuning};
use crate::star_detection::star::Star;

/// Two star catalogues registered, as `register` takes every light's against the reference's:
/// the default configuration, whose `Auto` model fits every transform up to a homography and keeps
/// the one the geometric robust information criterion picks.
pub(crate) fn integration(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = Kind::Integration.group(c, "register", Scale::Frame);
    group.throughput(Throughput::Elements(field.stars() as u64));
    group.bench_function(BenchmarkId::new("pair", field.name()), |b| {
        let pair = Pair::typical();
        let config = RegistrationConfig::default();
        b.iter(|| {
            config
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
    row_positions::bench::atomic(c);
}

/// The FWHM the stars are measured at, as the scene renders them.
const FWHM: f32 = 3.5;

/// A star position's measurement error, in pixels: a centroid's at the scene's typical SNR.
const POSITION_SIGMA: f64 = 0.05;

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
                position_sigma: POSITION_SIGMA,
                ..Star::at(pos + POSITION_SIGMA * error)
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

    /// The noise scale `register` derives from the stars' FWHM.
    pub(super) fn max_sigma(&self) -> f64 {
        tuning::max_sigma_from_fwhm(median_fwhm(&self.reference, &self.target))
    }

    /// The brightest `count` positions of each catalogue as trees, as `register` builds them for
    /// `config.matching.max_stars`.
    pub(super) fn trees(&self, count: usize) -> Trees {
        let brightest =
            |stars: &[Star]| KdTree::build(stars.iter().take(count).map(|s| s.pos).collect());
        Trees {
            reference: brightest(&self.reference).expect("a catalogue of stars"),
            target: brightest(&self.target).expect("a catalogue of stars"),
        }
    }

    /// The triangle matches between the brightest stars `config` takes.
    pub(super) fn matches(&self, trees: &Trees, config: &RegistrationConfig) -> Vec<PointMatch> {
        config
            .matching
            .triangle
            .match_trees(&trees.reference, &trees.target, self.max_sigma())
    }

    /// RANSAC's `transform` on the matches of the default configuration's brightest stars: the
    /// hypothesis the final fit starts from.
    pub(super) fn hypothesis(&self, transform: TransformType) -> Transform {
        let config = RegistrationConfig::default();
        let trees = self.trees(config.matching.max_stars);
        let matches = self.matches(&trees, &config);
        RansacEstimator::new(config.ransac, self.max_sigma())
            .estimate(&trees.pairs(&matches), &confidences(&matches), transform)
            .expect("the pair registers")
            .transform
    }

    /// `transform` under `correction`: the SIP correction the precise presets add is taken about
    /// the reference catalogue's centre, as `register` places it.
    pub(super) fn model(&self, transform: TransformType, correction: Correction) -> FitModel {
        FitModel {
            transform,
            sip: match correction {
                Correction::Linear => None,
                Correction::Sip => Some(SipModel {
                    order: SipConfig::default().order,
                    origin: bounding_box_centre(&self.reference),
                }),
            },
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
        let trees = self.trees(config.matching.max_stars);
        let catalogs = FitCatalogs::new(&self.reference, &self.target).expect("a catalogue");
        let matches = self.matches(&trees, &config);
        let search = ModelSearch {
            pairs: trees.pairs(&matches),
            confidences: confidences(&matches),
            catalogs: &catalogs,
            max_sigma: self.max_sigma(),
            config: &config,
        };
        search
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

/// Each catalogue's brightest stars as a tree.
#[derive(Debug)]
pub(super) struct Trees {
    pub(super) reference: KdTree,
    pub(super) target: KdTree,
}

impl Trees {
    /// The positions of `matches` among these stars.
    pub(super) fn pairs(&self, matches: &[PointMatch]) -> PointPairs {
        PointPairs::from_matches(matches, self.reference.points(), self.target.points())
    }
}

/// Each match's confidence, by index.
pub(super) fn confidences(matches: &[PointMatch]) -> Vec<f64> {
    matches.iter().map(|m| m.confidence).collect()
}
