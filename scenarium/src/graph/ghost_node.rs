//! [`GhostNode`]: a node its graph cannot hold, kept with why.
//!
//! # Decisions
//!
//! - **A ghost is what a graph keeps and cannot hold: a ghost node, a ghost wire, a ghost port.**
//!   One word for the three; "stub" is left to the test doubles
//!   ([`stub_func`](crate::internals::stub_func)).
//! - **Why a node was detached is kept with its ghost**, apart from why a node does not resolve:
//!   a node detached for a rule across graphs, a loop or a bound, resolves, so the reason cannot
//!   be read off it again. Every ghost node that resolves is attached at the next load, and the
//!   load decides again.
//! - **A ghost node holds no wiring.** Each binding and subscription that touched it is among
//!   its graph's [`GhostWiring`](crate::GhostWiring), once, whichever of its ends is the ghost.
//!   Refused: the wiring in each ghost node's own record, which held a wire between two of them
//!   at the one detached first and lost it when the other returned alone.
//! - **A ghost node holds what it declared**, taken when it is detached: from the file's
//!   [`Manifest`](crate::Manifest) for a func the library lacks, from its declaration for a node
//!   that resolves and is refused. So a definition with its ghosts is one value in a clip, an
//!   undo step and a library file. Refused: the declarations of missing funcs in a table of the
//!   set, which a compound travels without.

use ::serde::{Deserialize, Serialize};

use crate::graph::error::DetachReason;
use crate::graph::node::Node;
use crate::graph::saved_decl::SavedDecl;

/// A node its graph cannot hold, and why the load took it out. Kept in the graph beside its
/// nodes, under its id, so your settings and wiring outlive a library that lost a func, and a
/// definition is one value wherever it goes. No edit wires to one: it and the wiring that
/// touches it are a frozen record until a load attaches it again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhostNode {
    pub node: Node,
    pub reason: DetachReason,
    /// What the node declared when the load took it out: from the file's manifest for a func
    /// the library lacks, from its declaration for a node that resolves and is refused. `None`
    /// when nothing knew it: a missing compound, a func no manifest listed.
    pub saved: Option<SavedDecl>,
}
