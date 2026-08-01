//! Why an intent could never have applied.

use glam::Vec2;
use scenarium::NodeId;
use thiserror::Error;

/// A payload no widget could legitimately have raised: a nil or colliding
/// identity, a non-finite position, a link to state the document doesn't hold.
///
/// **This is our own bug, not a refusal.** Widgets read every identity they
/// emit out of the live document, so the worst they manage is *stale* — and a
/// stale intent is not an error at all: it simply yields no step, which
/// [`build_step`](crate::core::edit::intent::build::build_step) reports as
/// `Ok(None)` and callers drop without a word. So do a no-op and a
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
    /// One insertion claiming the same new id twice. `Graph::insert` panics
    /// outright on a collision.
    #[error("node {node_id:?} appears twice in one insertion")]
    DuplicateInsertion { node_id: NodeId },
    /// A new id the document already holds. Scenarium requires node ids to be
    /// unique across the whole authoring tree.
    #[error("node {node_id:?} already exists in the document")]
    NodeAlreadyExists { node_id: NodeId },
    /// An inserted node whose kind names nothing the library can resolve.
    #[error("new node has a nil func id")]
    NilFuncId,
    /// An insertion's own wiring pointing at a node neither it nor the graph
    /// holds. Unlike a stale reference this is malformed: an insertion's
    /// wiring is authored in the same breath as its nodes.
    #[error("{role} node {node_id:?} is not in the graph")]
    NodeAbsent { role: &'static str, node_id: NodeId },
    /// Positions reach the view verbatim, and a non-finite one fails
    /// `GraphView::validate` — which runs only on save and load, so an
    /// unchecked NaN surfaces as a document that won't reopen.
    #[error("{role} position {pos:?} is not finite")]
    NonFinitePosition { role: &'static str, pos: Vec2 },
    /// Same corruption path as [`Self::NonFinitePosition`], one level up.
    #[error("viewport needs finite pan and positive finite zoom")]
    InvalidViewport,
}
