//! [`ProgressMonitor`]: the caller's two lines to a run, its progress out and its cancel in.

use common::CancelToken;

use crate::progress::progress_callback::ProgressCallback;

/// What a caller watches and stops a run through: the callback the run reports its progress to,
/// and the token it polls to stop early. Every stacking entry takes one.
#[derive(Debug, Clone)]
pub struct ProgressMonitor {
    pub(crate) progress: ProgressCallback,
    pub(crate) cancel: CancelToken,
}

impl ProgressMonitor {
    pub const fn new(progress: ProgressCallback, cancel: CancelToken) -> Self {
        Self { progress, cancel }
    }

    /// A run nobody watches or stops: its progress reported nowhere, under a token that never
    /// cancels.
    pub fn unattended() -> Self {
        Self::from(CancelToken::never())
    }
}

/// A run stopped through `cancel` whose progress is reported nowhere.
impl From<CancelToken> for ProgressMonitor {
    fn from(cancel: CancelToken) -> Self {
        Self::new(ProgressCallback::default(), cancel)
    }
}
