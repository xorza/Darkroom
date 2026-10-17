//! Errors shared across lumos' modules.

use common::CancelToken;
use thiserror::Error;

/// A configuration field whose value falls outside the range its algorithm accepts.
///
/// Star detection, registration, combining, drizzle and the image ops all reject a field for the
/// same three reasons — which field, what it had to be, what it actually was — so they report it
/// with this one type instead of each growing a variant per field. Constraints that genuinely
/// aren't a range check on a single field (two fields that must agree, a per-element check) keep a
/// typed variant on their module's error.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
#[error(
    "{field} must be {expected}{}, got {value}",
    .bound.map_or_else(String::new, |bound| format!(" ({bound})"))
)]
pub struct InvalidConfigField {
    /// The offending field. Its name in the config struct, prefixed with the algorithm that owns
    /// it when the bare name would not identify it — `tile_size` alone names two unrelated fields.
    pub field: &'static str,
    /// The accepted range as prose, completing "`<field>` must be …": `"finite and positive"`,
    /// `"between 16 and 256"`.
    pub expected: &'static str,
    /// The rejected value. Integer fields widen into it exactly — config counts stay far below
    /// f64's 2^53 integer limit.
    pub value: f64,
    /// The other operand when `expected` names a second field rather than a constant bound.
    pub bound: Option<f64>,
}

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
