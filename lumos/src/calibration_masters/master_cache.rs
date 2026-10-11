//! [`MasterCache`]: where a master build keeps a role's master between runs.

use crate::calibration_masters::master_role::MasterRole;
use crate::io::image::cfa::CfaImage;

/// What one role's master is stacked from: its frames, and the master taken from each of them
/// with that master's own frames — a flat's flat-dark or bias. A cache keys a master by all of
/// them, so a master stacked before its subtractor changed is stacked again.
#[derive(Debug)]
pub struct MasterInputs<'a, P> {
    pub role: MasterRole,
    pub frames: &'a [P],
    pub subtracted: Option<SubtractedFrames<'a, P>>,
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

/// Where [`CalibrationMasters::build`](crate::CalibrationMasters::build) keeps a role's master
/// between runs: read before the role is stacked, written once it is.
pub trait MasterCache<P> {
    /// Why the cache could not be read or written.
    type Error;

    /// The master kept for `inputs`, when one is kept for them as they are now.
    ///
    /// # Errors
    ///
    /// The cache cannot tell whether it holds one.
    fn load(&self, inputs: &MasterInputs<'_, P>) -> Result<Option<CfaImage>, Self::Error>;

    /// Keep `master`, stacked from `inputs`, for a later build.
    ///
    /// # Errors
    ///
    /// The cache cannot keep it.
    fn store(&self, inputs: &MasterInputs<'_, P>, master: &CfaImage) -> Result<(), Self::Error>;
}

/// No cache: every master is stacked.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoMasterCache;

impl<P> MasterCache<P> for NoMasterCache {
    type Error = std::convert::Infallible;

    fn load(&self, _inputs: &MasterInputs<'_, P>) -> Result<Option<CfaImage>, Self::Error> {
        Ok(None)
    }

    fn store(&self, _inputs: &MasterInputs<'_, P>, _master: &CfaImage) -> Result<(), Self::Error> {
        Ok(())
    }
}
