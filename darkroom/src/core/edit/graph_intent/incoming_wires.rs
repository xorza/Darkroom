//! What a duplicate does with the wires that reach its set from outside.

/// What a duplicated node does with a wire whose producer is outside the
/// duplicated set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncomingWires {
    /// The clone starts with that input unbound.
    Drop,
    /// The clone keeps the wire, still reading the original producer.
    Keep,
}
