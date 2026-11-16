//! The install-local **dense index space**: `NodeIdx`, `OutputIdx`,
//! `OutputAddr`, and the `…Idx` positions in the packed port columns. The
//! containers keyed by them are [`Col`](crate::containers::col::Col) and
//! [`IdxSet`](crate::containers::idx_set::IdxSet).
//!
//! This is an *index* space, not an identity space. A node in a compiled
//! program is named by its [`ProgramId`](crate::graph::program_id::ProgramId),
//! the path of instances down to the authored node, and its ports by their
//! index on it. Those are the stable names: they survive installs, cross the
//! host boundary, and may enter digests.
//!
//! Indices shift between compiles, so none of them may enter a digest, a persisted byte, or a
//! host-facing report. They are assigned by the compiler's walk and never leave the execution
//! internals.
//!
//! Each is one [`idx_type!`] invocation; the newtype and its `Idx` impl are the same for every
//! space.

use crate::containers::col::idx_type;

idx_type!(
    /// A node's position in the installed program's dense node vector. Install-local: it must
    /// never enter a digest, a persisted byte, or any host-facing report — those stay on
    /// [`ProgramId`](crate::graph::program_id::ProgramId).
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
    /// A position in the program's flat input column, packed one node's ports after another. A
    /// node owns a [`Span<InputIdx>`](crate::containers::col::Span); this names one port inside
    /// it. Positions are handed out as the walk appends, so a node owns exactly the run its own
    /// declaration claimed.
    pub(crate) struct InputIdx,
    "input column index"
);

idx_type!(
    /// A position in the program's flat event column — [`InputIdx`]'s counterpart
    /// for event ports.
    pub(crate) struct EventIdx,
    "event column index"
);

idx_type!(
    /// A position in the program's flat subscriber column, packed one event's
    /// subscribers after another.
    pub(crate) struct SubscriberIdx,
    "subscriber column index"
);

/// A program node's output interned into the installed program's dense index
/// space — the hash-free form every per-run edge walk uses, resolved once at
/// compile, across every subgraph boundary between.
/// Install-local like [`NodeIdx`]: it must never enter a digest, a persisted
/// byte, or a host-facing report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct OutputAddr {
    pub(crate) node_idx: NodeIdx,
    pub(crate) port_idx: u32,
}
