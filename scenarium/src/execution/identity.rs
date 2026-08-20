//! The install-local **dense index space**: `NodeIdx`, `OutputIdx`,
//! `OutputAddr`, and the `…Idx` positions in the packed port columns. The
//! containers keyed by them are [`Column`](crate::containers::column::Column) and
//! [`IdxSet`](crate::containers::set::IdxSet).
//!
//! This is an *index* space, not a second identity space. A node in a
//! compiled program is the authored node, named by the same
//! [`NodeId`](crate::graph::identity::NodeId) — and its ports by the same
//! [`InputPort`](crate::graph::identity::InputPort) /
//! [`OutputPort`](crate::graph::identity::OutputPort) /
//! [`EventPort`](crate::graph::identity::EventPort). Those are the stable
//! names: they survive installs, cross the host boundary, and may enter
//! digests.
//!
//! Everything below is the opposite: indices shift between compiles, so none
//! of them may enter a digest, a persisted byte, or a host-facing report.
//! They are assigned by the compiler's walk and never leave the execution
//! internals.
//!
//! Each is one [`idx_type!`] invocation — the newtype and its `Idx` impl are
//! the same for every space, so what is written out here is only what each
//! space *is*.

use crate::containers::column::idx_type;

idx_type!(
    /// A node's position in the installed program's dense node vector. Install-local:
    /// indices shift between compiles, so a `NodeIdx` must never enter a digest, a
    /// persisted byte, or any host-facing report — those stay on
    /// [`NodeId`](crate::graph::identity::NodeId).
    pub(crate) struct NodeIdx,
    "node index"
);

idx_type!(
    /// A position in the program's flat output pool. It cannot be confused with a node
    /// id or a node-local port number.
    pub(crate) struct OutputIdx,
    "output pool index"
);

idx_type!(
    /// A position in the program's flat input column, packed one node's ports after
    /// another. A node owns a [`Span<InputIdx>`](crate::containers::column::Span); this
    /// names one port inside it.
    ///
    /// Positions are handed out as the walk appends, one node's run after another,
    /// so a node owns exactly the run its own declaration claimed.
    pub(crate) struct InputIdx,
    "input column index"
);

idx_type!(
    /// A position in the program's flat event column — [`InputIdx`]'s counterpart
    /// for event ports.
    pub(crate) struct EventIdx,
    "event column index"
);

/// An [`OutputPort`](crate::graph::identity::OutputPort)
/// interned into the installed program's dense index space — the hash-free form
/// every per-run edge walk uses, resolved once at compile.
/// Install-local like [`NodeIdx`]: it must never enter a digest, a persisted
/// byte, or a host-facing report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct OutputAddr {
    pub(crate) node_idx: NodeIdx,
    pub(crate) port_idx: u32,
}
