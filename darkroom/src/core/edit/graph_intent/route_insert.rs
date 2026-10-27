//! Where a wire joins a tie on its route.

use crate::core::document::wire_key::WireKey;

/// A wire a tie takes, and where on the wire's route the tie goes: the index of the crossed
/// piece, `0` for the piece from the output end, the route's length for the piece to the input
/// end. A wire with no route has one piece, at `0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RouteInsert {
    pub(crate) wire: WireKey,
    pub(crate) at: usize,
}
