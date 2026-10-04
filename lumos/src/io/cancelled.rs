//! [`Cancelled`]: a decode stage stopped by its cancel token.

use common::CancelToken;

/// Returned by a decode or demosaic stage that saw its cancel token set. A marker only: the
/// partial buffers are dropped, and the caller reports the cancellation as its own error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cancelled;

impl Cancelled {
    /// `Err(Cancelled)` once `cancel` is set, so a long walk can `?` its way out between chunks.
    pub(crate) fn check(cancel: &CancelToken) -> Result<(), Self> {
        if cancel.is_cancelled() {
            Err(Self)
        } else {
            Ok(())
        }
    }
}
