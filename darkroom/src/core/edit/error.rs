//! Why an intent could never have applied.

use glam::Vec2;
use scenarium::{InputPort, NodeId};
use thiserror::Error;

/// A payload no widget could legitimately have raised: a nil or colliding
/// identity, a non-finite position, a link to state the document doesn't hold.
///
/// **This is our own bug, not a refusal.** Widgets read every identity they
/// emit out of the live document, so the worst they manage is *stale* — and a
/// stale intent is not an error at all: it simply yields no step, which
/// [`GraphIntent::into_step`](crate::core::edit::graph_intent::GraphIntent::into_step)
/// reports as `Ok(None)` and callers drop without a word. So do a no-op and a
/// cycle-forming bind. Only the cases below travel as an `Err`.
///
/// Each variant names the check that rejected it rather than carrying a
/// formatted string, so a test can assert *which* precondition broke without
/// matching on prose, and the common `role` stays a `&'static str` instead of
/// allocating a message on a path that is supposed to be unreachable.
#[derive(Debug, Error)]
pub(crate) enum MalformedIntent {
    /// A nil id where the intent must name something. Checked before any
    /// lookup: `Graph::find` asserts on a nil id.
    #[error("{role} node id is nil")]
    NilNodeId { role: &'static str },
    /// A new id the document already holds. Scenarium requires node ids to be
    /// unique across the whole authoring tree, and `Graph::insert` panics
    /// outright on a collision.
    #[error("node {node_id:?} already exists in the document")]
    NodeAlreadyExists { node_id: NodeId },
    /// An inserted node whose kind names nothing the library can resolve.
    #[error("new node has a nil func id")]
    NilFuncId,
    /// An insertion's own wiring pointing at a node the graph doesn't hold.
    /// Unlike a stale reference this is malformed: an insertion's wiring is
    /// authored in the same breath as its node.
    #[error("{role} node {node_id:?} is not in the graph")]
    NodeAbsent { role: &'static str, node_id: NodeId },
    /// A seed binding reading the very node it feeds. That is a cycle of one,
    /// which the planner refuses outright (`Error::CycleDetected`) — so the
    /// graph would author fine and then never run.
    #[error("seed binding on {port:?} reads the node it is being added to")]
    CyclicSeedBinding { port: InputPort },
    /// A seed binding landing somewhere other than the node being inserted.
    /// An insertion restores exactly the wiring it recorded, so it may only
    /// author its own node's inputs — anything else would be an edit of a
    /// node the step does not carry.
    #[error("seed binding on {port:?} does not belong to the inserted node")]
    ForeignSeedBinding { port: InputPort },
    /// Two seed bindings claiming the same input port. Only one could survive
    /// the insertion, and the record would then disagree with the graph.
    #[error("input {port:?} is seeded twice by one insertion")]
    DuplicateSeedBinding { port: InputPort },
    /// Positions reach the view verbatim, and a non-finite one fails
    /// `GraphView::validate` — which runs only on save and load, so an
    /// unchecked NaN surfaces as a document that won't reopen.
    #[error("{role} position {pos:?} is not finite")]
    NonFinitePosition { role: &'static str, pos: Vec2 },
    /// Same corruption path as [`Self::NonFinitePosition`], one level up.
    #[error("viewport needs finite pan and positive finite zoom")]
    InvalidViewport,
}
