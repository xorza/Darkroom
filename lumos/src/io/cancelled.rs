//! [`Cancelled`]: a decode stage stopped by its cancel token.

/// Returned by a decode or demosaic stage that saw its cancel token set. A marker only: the
/// partial buffers are dropped, and the caller reports the cancellation as its own error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cancelled;
