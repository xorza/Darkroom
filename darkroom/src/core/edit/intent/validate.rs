//! The preconditions [`build_step`](crate::core::edit::intent::build::build_step)
//! establishes before it will fold a [`GraphIntent`](crate::core::edit::intent::types::GraphIntent)
//! into a step.
//!
//! Everything commits through that one entry, so these are the whole
//! precondition set. Each check answers one question — is this id
//! resolvable, fresh, non-nil; is this position finite; is this kind
//! insertable here.
//!
//! A reference that merely went *stale* is not a failure: a gesture spanning
//! frames produces one normally, so [`live_node`] answers `Ok(None)` and the
//! caller yields no step. Only a payload that could never have applied is an
//! `Err`, and that is a bug in whatever raised it — see [`MalformedIntent`].
//!
//! Two failure modes motivate the split. Some of these guard *panics*:
//! `Graph::find` asserts on a nil id, `Graph::insert` panics on a
//! duplicate one, and `apply_graph` asserts that an `AddNode` target is
//! absent. The rest guard *corruption*: state that applies cleanly and
//! leaves a document `Document::validate` rejects — which, because saving
//! validates only in debug builds, means a project that writes fine and
//! won't reopen.

use std::collections::HashSet;

use glam::Vec2;
use scenarium::{Binding, Graph, InputPort, Node, NodeId, NodeKind};

use crate::core::document::Document;
use crate::core::edit::intent::error::MalformedIntent;

/// Resolve a node the intent points at. A nil id is malformed rather than
/// stale, and the check has to come first: [`Graph::find`] asserts on one.
///
/// `Ok(None)` means the node is simply gone — the ordinary outcome of input
/// that spans frames, which the caller turns into "no step".
pub(super) fn live_node<'a>(
    graph: &'a Graph,
    node_id: NodeId,
    role: &'static str,
) -> Result<Option<&'a Node>, MalformedIntent> {
    if node_id.is_nil() {
        return Err(MalformedIntent::NilNodeId { role });
    }
    Ok(graph.find(node_id))
}

/// A node id an insertion introduces, checked against the ids the same
/// batch already claimed and against the whole document. Scenarium requires
/// node ids to be unique across the entire authoring tree, and
/// [`Graph::insert`] panics outright on a collision.
pub(super) fn fresh_node_id(
    doc: &Document,
    node_id: NodeId,
    claimed: &mut HashSet<NodeId>,
) -> Result<(), MalformedIntent> {
    if node_id.is_nil() {
        return Err(MalformedIntent::NilNodeId { role: "new" });
    }
    if !claimed.insert(node_id) {
        return Err(MalformedIntent::DuplicateInsertion { node_id });
    }
    if doc.graph.find(node_id).is_some() {
        return Err(MalformedIntent::NodeAlreadyExists { node_id });
    }
    Ok(())
}

/// A newly inserted node's kind has to name state the document already holds:
/// a func the library resolves, or a built-in special.
pub(super) fn insertable_kind(node: &Node) -> Result<(), MalformedIntent> {
    match &node.kind {
        NodeKind::Func(func_id) => {
            if func_id.is_nil() {
                return Err(MalformedIntent::NilFuncId);
            }
        }
        NodeKind::Special(_) => {}
    }
    Ok(())
}

/// Bindings riding along with an insertion. Both endpoints must exist once
/// the insertion applies, or the graph keeps a dangling edge.
pub(super) fn insertable_bindings(
    graph: &Graph,
    added: &HashSet<NodeId>,
    bindings: &[(InputPort, Binding)],
) -> Result<(), MalformedIntent> {
    for (port, binding) in bindings {
        present_node(graph, added, port.node_id, "binding destination")?;
        if let Binding::Bind(src) = binding {
            present_node(graph, added, src.node_id, "binding producer")?;
        }
    }
    Ok(())
}

/// A node the insertion either just added or the target graph already
/// holds. Unlike [`live_node`] a miss is malformed, not stale: an
/// insertion's own wiring is authored in the same breath as its nodes.
pub(super) fn present_node(
    graph: &Graph,
    added: &HashSet<NodeId>,
    node_id: NodeId,
    role: &'static str,
) -> Result<(), MalformedIntent> {
    if node_id.is_nil() {
        return Err(MalformedIntent::NilNodeId { role });
    }
    if added.contains(&node_id) || graph.find(node_id).is_some() {
        return Ok(());
    }
    Err(MalformedIntent::NodeAbsent { role, node_id })
}

/// Positions reach the view verbatim, and a non-finite one fails
/// `GraphView::validate` — which only runs on save and load, so an
/// unchecked NaN surfaces as a document that won't reopen.
pub(super) fn finite_position(pos: Vec2, role: &'static str) -> Result<(), MalformedIntent> {
    if !pos.is_finite() {
        return Err(MalformedIntent::NonFinitePosition { role, pos });
    }
    Ok(())
}
