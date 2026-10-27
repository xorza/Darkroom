//! What a duplicate does with the wires that reach its set from outside.

/// What a duplicated node does with a wire whose producer is outside the
/// duplicated set: a data wire into one of its inputs, or a subscription of
/// it to an outside emitter's event, both of which the canvas draws as wires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncomingWires {
    /// The clone starts with that input on its declared default, else unbound,
    /// as a wire removed on its own leaves it, and subscribed to nothing
    /// outside.
    Drop,
    /// The clone keeps the wire, still reading the original producer, and
    /// each subscription, still to the original emitter.
    Keep,
}
