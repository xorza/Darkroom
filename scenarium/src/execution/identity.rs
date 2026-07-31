//! The install-local **dense index space**: `NodeIdx`, `OutputIdx`,
//! `OutputAddr`, and the `…Idx` positions in the packed port columns. The
//! containers keyed by them are [`Column`](crate::common::column::Column) and
//! [`IdxSet`](crate::common::set::IdxSet).
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

use crate::common::column::Idx;

/// A node's position in the installed program's dense node vector. Install-local:
/// indices shift between compiles, so a `NodeIdx` must never enter a digest, a
/// persisted byte, or any host-facing report — those stay on
/// [`NodeId`](crate::graph::identity::NodeId).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct NodeIdx(pub(crate) u32);

impl Idx for NodeIdx {
    fn idx(self) -> usize {
        self.0 as usize
    }

    fn from_idx(i: usize) -> Self {
        debug_assert!(u32::try_from(i).is_ok(), "node index must fit in u32");
        NodeIdx(i as u32)
    }
}

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

/// A position in the program's flat output pool. It cannot be confused with a node
/// id or a node-local port number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct OutputIdx(pub(crate) u32);

impl Idx for OutputIdx {
    fn idx(self) -> usize {
        self.0 as usize
    }

    fn from_idx(i: usize) -> Self {
        debug_assert!(
            u32::try_from(i).is_ok(),
            "output pool index must fit in u32"
        );
        OutputIdx(i as u32)
    }
}

/// A position in the program's flat input column, packed one node's ports after
/// another. A node owns a [`Span<InputIdx>`](crate::common::column::Span); this
/// names one port inside it.
///
/// Positions are handed out as the walk appends, one node's run after another,
/// so a node owns exactly the run its own declaration claimed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct InputIdx(pub(crate) u32);

impl Idx for InputIdx {
    fn idx(self) -> usize {
        self.0 as usize
    }

    fn from_idx(i: usize) -> Self {
        debug_assert!(
            u32::try_from(i).is_ok(),
            "input column index must fit in u32"
        );
        InputIdx(i as u32)
    }
}

/// A position in the program's flat event column — [`InputIdx`]'s counterpart
/// for event ports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct EventIdx(pub(crate) u32);

impl Idx for EventIdx {
    fn idx(self) -> usize {
        self.0 as usize
    }

    fn from_idx(i: usize) -> Self {
        debug_assert!(
            u32::try_from(i).is_ok(),
            "event column index must fit in u32"
        );
        EventIdx(i as u32)
    }
}
