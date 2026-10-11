//! [`Cancelled`]: a decode stage stopped by its cancel token.

use common::CancelToken;

/// Returned by a decode or demosaic stage that saw its cancel token set. A marker only: the
/// partial buffers are dropped, and the caller reports the cancellation as its own error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cancelled;

impl Cancelled {
    /// How many samples a cancellable walk covers between cancel polls.
    ///
    /// The frame checks and the normalization gather chunk their iteration to this rather than
    /// test the token per sample — the divisor was a modulo on every sample of every plane of
    /// every frame. The figure is not tuned to either: it is a granularity, small enough that a
    /// cancelled run stops promptly and large enough that the poll is lost in the work.
    pub(crate) const POLL_CHUNK: usize = 16_384;

    /// `Err(Cancelled)` once `cancel` is set, so a long walk can `?` its way out between chunks.
    pub(crate) fn check(cancel: &CancelToken) -> Result<(), Self> {
        if cancel.is_cancelled() {
            Err(Self)
        } else {
            Ok(())
        }
    }
}
