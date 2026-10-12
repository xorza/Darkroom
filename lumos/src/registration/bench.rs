//! The registration benches: the entry `crate::bench` runs them by, and the input they share, a
//! typical field's star catalogue in two exposures.

use criterion::Criterion;
use glam::DVec2;

use crate::bench::scene::{FRAME, Field};
use crate::internals::synthetic::distortion::RadialField;
use crate::internals::test_rng::TestRng;
use crate::registration::distortion::sip::SipConfig;
use crate::registration::final_fit::{FinalFit, FitCatalogs, FitModel, SipModel};
use crate::registration::ransac::RansacEstimator;
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::resample::row_positions;
use crate::registration::spatial::KdTree;
use crate::registration::transform::{Transform, TransformType, WarpTransform};
use crate::registration::triangle::matching;
use crate::registration::triangle::voting::PointMatch;
use crate::registration::{
    bounding_box_centre, estimate_and_refine, final_fit, median_fwhm, ransac, spatial, triangle,
    tuning,
};
use crate::star_detection::star::Star;

pub(crate) fn benches(c: &mut Criterion) {
    spatial::bench::benches(c);
    triangle::bench::benches(c);
    ransac::bench::benches(c);
    final_fit::bench::benches(c);
    row_positions::bench::benches(c);
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
        matching::match_triangles(
            &trees.reference,
            &trees.target,
            &config.matching.triangle,
            self.max_sigma(),
        )
    }

    /// RANSAC's `transform` on the matches of the default configuration's brightest stars: the
    /// hypothesis the final fit starts from.
    pub(super) fn hypothesis(&self, transform: TransformType) -> Transform {
        let config = RegistrationConfig::default();
        let trees = self.trees(config.matching.max_stars);
        RansacEstimator::new(config.ransac.clone(), self.max_sigma())
            .estimate(
                &self.matches(&trees, &config),
                trees.reference.points(),
                trees.target.points(),
                transform,
            )
            .expect("the pair registers")
            .transform
    }

    /// The warp `register` fits to the pair with the SIP correction the precise presets add: the
    /// dither and the lens together.
    pub(crate) fn sip_warp(&self) -> WarpTransform {
        let sip = SipModel {
            order: SipConfig::default().order,
            origin: bounding_box_centre(&self.reference),
        };
        self.fit(FitModel {
            transform: TransformType::Similarity,
            sip: Some(sip),
        })
        .warp
    }

    /// `model` fitted as `register` fits it under the default configuration: RANSAC on the
    /// matches, then the final fit over the whole catalogues.
    pub(super) fn fit(&self, model: FitModel) -> FinalFit {
        let config = RegistrationConfig::default();
        let trees = self.trees(config.matching.max_stars);
        let catalogs = FitCatalogs::new(&self.reference, &self.target).expect("a catalogue");
        estimate_and_refine(
            trees.reference.points(),
            &trees.target,
            &self.matches(&trees, &config),
            &catalogs,
            model,
            self.max_sigma(),
            &config,
        )
        .expect("the pair registers")
    }
}

/// Each catalogue's brightest stars as a tree.
#[derive(Debug)]
pub(super) struct Trees {
    pub(super) reference: KdTree,
    pub(super) target: KdTree,
}
