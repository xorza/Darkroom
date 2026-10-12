//! [`MasterCache`]: where a master build keeps a role's master between runs.

use std::fmt;

use crate::calibration::master_role::MasterRole;
use crate::frame_store::decode_cache::cache_key::{DECODE_VERSION, FNV1A_OFFSET, fnv1a};
use crate::image::cfa::cfa_image::CfaImage;
use crate::ingest::ingest_config::IngestConfig;

/// What `master_snapshot` (`internals::characterization`) pins of the master stack: a bias, a dark
/// and a flat with the bias taken from each frame, stacked from mosaic FITS frames. A change that
/// moves it states its reason and sets this to the new digest, which moves every
/// [`MasterRecipe`], so no master a cache kept from the older stack is reused.
pub(crate) const MASTER_PIN: &str = "556359a1356643de";

/// The version every [`MasterRecipe`] carries: derived from [`DECODE_VERSION`] and [`MASTER_PIN`],
/// so a change to a decode or to the master stack moves it.
const MASTER_VERSION: u64 = fnv1a(
    fnv1a(FNV1A_OFFSET, &DECODE_VERSION.to_le_bytes()),
    MASTER_PIN.as_bytes(),
);

/// What one role's master is stacked from: its frames, the master taken from each of them with
/// that master's own frames — a flat's flat-dark or bias — and how lumos stacks it. A cache keys a
/// master by all of them, so a master stacked before its subtractor changed, by an older lumos, or
/// under other decode options, is stacked again.
#[derive(Debug)]
pub struct MasterInputs<'a, P> {
    pub role: MasterRole,
    pub frames: &'a [P],
    pub subtracted: Option<SubtractedFrames<'a, P>>,
    pub recipe: MasterRecipe,
}

/// How lumos stacks one role's master: the version of its decodes and of the master stack, the
/// decode options, and the role. Two masters of equal frames stack alike only under one recipe.
/// It displays as 16 hex digits, for a key in text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MasterRecipe(u64);

impl MasterRecipe {
    /// The recipe of `role`'s master stacked by this lumos under `ingest`, as
    /// [`CalibrationMasters::build`](crate::CalibrationMasters::build) hands it to a cache.
    pub fn of(role: MasterRole, ingest: &IngestConfig) -> Self {
        let options = ingest.decode_options().digest();
        let hash = fnv1a(
            fnv1a(MASTER_VERSION, &options.to_le_bytes()),
            role.extname().as_bytes(),
        );
        Self(hash)
    }
}

impl fmt::Display for MasterRecipe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// The frames of the master a stack takes from each of its frames, and that master's role.
#[derive(Debug)]
pub struct SubtractedFrames<'a, P> {
    pub role: MasterRole,
    pub frames: &'a [P],
}

// By hand, as a derive would ask `P: Copy`: both hold only borrows of `P`.
impl<P> Clone for MasterInputs<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P> Copy for MasterInputs<'_, P> {}

impl<P> Clone for SubtractedFrames<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P> Copy for SubtractedFrames<'_, P> {}

/// What a [`MasterCache`] holds for one role's inputs as they are now.
#[derive(Debug)]
pub enum CachedMaster<K> {
    /// The master kept for them, boxed as it outweighs any key.
    Kept(Box<CfaImage>),
    /// None: the role is stacked, and its master kept under `K`, the inputs as
    /// [`MasterCache::load`] read them.
    Missing(K),
}

/// Where [`CalibrationMasters::build`](crate::CalibrationMasters::build) keeps a role's master
/// between runs: read before the role is stacked, written once it is.
pub trait MasterCache<P> {
    /// Why the cache could not be read or written.
    type Error;

    /// The inputs as [`Self::load`] read them, before the stack read them: a master is kept under
    /// what its frames were, so a frame changed while it was stacked keys the old content, and the
    /// next build stacks the role again rather than reuse the master.
    type Key;

    /// The master kept for `inputs` as they are now, or the key to keep one under once it is
    /// stacked.
    ///
    /// # Errors
    ///
    /// The cache cannot tell whether it holds one.
    fn load(&self, inputs: &MasterInputs<'_, P>) -> Result<CachedMaster<Self::Key>, Self::Error>;

    /// Keep `master`, stacked from the inputs `key` was read from, for a later build.
    ///
    /// # Errors
    ///
    /// The cache cannot keep it.
    fn store(&self, key: Self::Key, master: &CfaImage) -> Result<(), Self::Error>;
}

/// No cache: every master is stacked.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoMasterCache;

impl<P> MasterCache<P> for NoMasterCache {
    type Error = std::convert::Infallible;
    type Key = ();

    fn load(&self, _inputs: &MasterInputs<'_, P>) -> Result<CachedMaster<()>, Self::Error> {
        Ok(CachedMaster::Missing(()))
    }

    fn store(&self, (): (), _master: &CfaImage) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::calibration::master_cache::MasterRecipe;
    use crate::calibration::master_role::MasterRole;
    use crate::ingest::ingest_config::IngestConfig;
    use crate::io::fits::options::FitsChecksumPolicy;

    /// The recipe moves with the decode options and with the role, and with nothing else: a FITS
    /// checksum policy of `RequireValid` against the default, a flat against a dark, while the cache
    /// directory and the memory plan, which change no output, leave it.
    #[test]
    fn the_recipe_follows_the_options_and_the_role() {
        let plain = IngestConfig::default();
        let mut strict = IngestConfig::default();
        strict.fits.checksum = FitsChecksumPolicy::RequireValid;
        let mut elsewhere = IngestConfig::with_cache_dir("/elsewhere".into());
        elsewhere.memory_override = Some(1 << 30);
        let dark = MasterRecipe::of(MasterRole::Dark, &plain);
        assert_eq!(dark, MasterRecipe::of(MasterRole::Dark, &plain));
        assert_ne!(dark, MasterRecipe::of(MasterRole::Dark, &strict));
        assert_ne!(dark, MasterRecipe::of(MasterRole::Flat, &plain));
        assert_eq!(dark, MasterRecipe::of(MasterRole::Dark, &elsewhere));
        assert_eq!(dark.to_string().len(), 16);
    }
}
