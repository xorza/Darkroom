//! [`Registrar`]: a registration configuration checked, and the registration of frame pairs under
//! it.

use crate::registration::error::RegistrationError;
use crate::registration::register::PreparedPair;
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::registration_result::RegistrationResult;
use crate::star_detection::star::Star;

/// A [`RegistrationConfig`] its validation accepted, from
/// [`RegistrationConfig::registrar`]: every failure of a pair it registers is the pair's own.
#[derive(Debug, Clone, Copy)]
pub struct Registrar<'a> {
    config: &'a RegistrationConfig,
}

impl<'a> Registrar<'a> {
    /// `config`, which its caller validated.
    pub(crate) const fn new(config: &'a RegistrationConfig) -> Self {
        Self { config }
    }

    /// Register two sets of star positions: the transformation that maps the reference stars'
    /// positions onto the target's.
    ///
    /// Stars should be sorted by brightness (flux) in descending order for best results. The
    /// RANSAC noise scale is derived from the median FWHM of the stars, for the seeing they were
    /// taken in.
    ///
    /// # Errors
    ///
    /// A star registration cannot read, too few stars, no matching pattern, every model's failure,
    /// or a fit past the RMS gate: each a fact about this pair, and so a frame to drop.
    pub fn register(
        &self,
        ref_stars: &[Star],
        target_stars: &[Star],
    ) -> Result<RegistrationResult, RegistrationError> {
        PreparedPair::new(self.config, ref_stars, target_stars)?.register()
    }
}
