//! [`MasterCache`]: where a master build keeps a role's master between runs.

use crate::calibration::master_recipe::MasterRecipe;
use crate::calibration::master_role::MasterRole;
use crate::image::cfa::cfa_image::CfaImage;

/// What one role's master is stacked from: its frames, the masters taken from each of them with
/// those masters' own frames — a flat's bias and flat-dark — and how lumos stacks it. A cache keys
/// a master by all of them, so a master stacked before its subtractor changed, by an older lumos,
/// or under other decode options, is stacked again.
#[derive(Debug)]
pub struct MasterInputs<'a, P> {
    pub role: MasterRole,
    pub frames: &'a [P],
    pub subtracted: &'a [SubtractedFrames<'a, P>],
    pub recipe: MasterRecipe,
}

/// The frames of a master a stack takes from each of its frames, and that master's role.
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
