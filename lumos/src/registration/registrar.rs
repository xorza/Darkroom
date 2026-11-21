//! [`Registrar`]: a registration configuration checked, and [`ReferenceCatalog`], the reference's
//! stars prepared once for every frame registered to them.

use glam::DVec2;

use crate::error::InvalidConfigField;
use crate::registration::error::{RegistrationCatalog, RegistrationError};
use crate::registration::kd_tree::KdTree;
use crate::registration::register::{self, PreparedPair};
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::registration_result::RegistrationResult;
use crate::star_detection::star::Star;

/// A [`RegistrationConfig`] its validation accepted: every failure of a pair it registers is the
/// pair's own.
#[derive(Debug, Clone)]
pub struct Registrar {
    config: RegistrationConfig,
}

impl Registrar {
    /// A registrar under `config`.
    ///
    /// # Panics
    /// When a configuration parameter is invalid: a config a caller validated, or built itself.
    pub fn new(config: RegistrationConfig) -> Self {
        Self::checked(config).unwrap_or_else(|invalid| panic!("{invalid}"))
    }

    /// [`Self::new`], or the configuration parameter that is invalid.
    ///
    /// # Errors
    ///
    /// The first invalid configuration parameter.
    pub fn checked(config: RegistrationConfig) -> Result<Self, InvalidConfigField> {
        config.validate()?;
        Ok(Self { config })
    }

    pub const fn config(&self) -> &RegistrationConfig {
        &self.config
    }

    /// `stars` as the reference every target registers to: checked against the star gates, its
    /// brightest stars as the tree triangle matching reads, and the origin a SIP correction is
    /// taken about when the configuration names none — made once for every target.
    ///
    /// # Errors
    ///
    /// A star registration cannot read, or a catalog below the star gates: no target could
    /// register to it.
    pub fn reference(&self, stars: Vec<Star>) -> Result<ReferenceCatalog<'_>, RegistrationError> {
        let brightest =
            register::check_catalog(&self.config, &stars, RegistrationCatalog::Reference)?;
        Ok(ReferenceCatalog {
            registrar: self,
            origin: bounding_box_centre(&stars),
            stars,
            brightest,
        })
    }

    /// The transform that maps `reference`'s star positions onto `target`'s: sugar over
    /// [`Self::reference`] and [`ReferenceCatalog::register`], for a single pair.
    ///
    /// # Errors
    ///
    /// Either's.
    pub fn register(
        &self,
        reference: &[Star],
        target: &[Star],
    ) -> Result<RegistrationResult, RegistrationError> {
        self.reference(reference.to_vec())?.register(target)
    }
}

/// The reference's stars, checked and prepared under a [`Registrar`] for every target registered
/// to them.
#[derive(Debug)]
pub struct ReferenceCatalog<'a> {
    registrar: &'a Registrar,
    stars: Vec<Star>,
    /// The brightest `max_stars`, as the tree triangle matching forms its triangles over.
    brightest: KdTree,
    origin: DVec2,
}

impl ReferenceCatalog<'_> {
    pub fn stars(&self) -> &[Star] {
        &self.stars
    }

    /// Register `target` to the reference: the transformation that maps the reference stars'
    /// positions onto the target's.
    ///
    /// Each set matches by its brightest stars, in any order. The RANSAC noise scale comes from
    /// the median FWHM of the stars.
    ///
    /// # Errors
    ///
    /// A star registration cannot read, too few stars, no matching pattern, every model's failure,
    /// or a fit past the RMS gate: each a fact about this target, and so a frame to drop.
    pub fn register(&self, target: &[Star]) -> Result<RegistrationResult, RegistrationError> {
        PreparedPair::new(self, target)?.register()
    }

    pub(super) const fn config(&self) -> &RegistrationConfig {
        &self.registrar.config
    }

    pub(super) const fn brightest(&self) -> &KdTree {
        &self.brightest
    }

    /// The origin a SIP correction is taken about when the configuration names none.
    pub(super) const fn origin(&self) -> DVec2 {
        self.origin
    }
}

/// The centre of the stars' bounding box: the SIP origin when none is configured, shared by every
/// frame registered to the same reference.
fn bounding_box_centre(stars: &[Star]) -> DVec2 {
    let (low, high) = stars.iter().fold(
        (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
        |(low, high), star| (low.min(star.pos), high.max(star.pos)),
    );
    (low + high) / 2.0
}
