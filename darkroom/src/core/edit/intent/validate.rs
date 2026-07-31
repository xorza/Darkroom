//! The preconditions [`build_step`](crate::core::edit::intent::build::build_step)
//! establishes before it will fold a [`GraphIntent`](crate::core::edit::intent::types::GraphIntent)
//! into a step.
//!
//! Everything commits through that one entry, so these are the whole
//! precondition set. Each check answers one question — is this id
//! resolvable, fresh, non-nil; is this position finite; is this kind
//! insertable here — and refuses with the [`Refusal`] that fits:
//! [`Refusal::Quiet`] for a reference that merely went stale, which a
//! gesture spanning frames produces normally, and [`Refusal::Invalid`] for a
//! payload that could never have applied, which is a bug in whatever raised
//! it.
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
use crate::core::edit::intent::types::Refusal;

/// Resolve a node the intent points at. A nil id is malformed rather than
/// stale, and the check has to come first: [`Graph::find`] asserts on one.
pub(super) fn live_node<'a>(
    graph: &'a Graph,
    node_id: NodeId,
    role: &'static str,
) -> Result<&'a Node, Refusal> {
    if node_id.is_nil() {
        return Err(Refusal::Invalid(format!("{role} node id is nil")));
    }
    graph.find(node_id).ok_or(Refusal::Quiet)
}

/// A node id an insertion introduces, checked against the ids the same
/// batch already claimed and against the whole document. Scenarium requires
/// node ids to be unique across the entire authoring tree, and
/// [`Graph::insert`] panics outright on a collision.
pub(super) fn fresh_node_id(
    doc: &Document,
    node_id: NodeId,
    claimed: &mut HashSet<NodeId>,
) -> Result<(), Refusal> {
    if node_id.is_nil() {
        return Err(Refusal::Invalid("new node id is nil".to_owned()));
    }
    if !claimed.insert(node_id) {
        return Err(Refusal::Invalid(format!(
            "node {node_id:?} appears twice in one insertion"
        )));
    }
    if doc.graph.find(node_id).is_some() {
        return Err(Refusal::Invalid(format!(
            "node {node_id:?} already exists in the document"
        )));
    }
    Ok(())
}

/// A newly inserted node's kind has to name state the document already holds:
/// a func the library resolves, or a built-in special.
pub(super) fn insertable_kind(node: &Node) -> Result<(), Refusal> {
    match &node.kind {
        NodeKind::Func(func_id) => {
            if func_id.is_nil() {
                return Err(Refusal::Invalid("new node has a nil func id".to_owned()));
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
) -> Result<(), Refusal> {
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
) -> Result<(), Refusal> {
    if node_id.is_nil() {
        return Err(Refusal::Invalid(format!("{role} node id is nil")));
    }
    if added.contains(&node_id) || graph.find(node_id).is_some() {
        return Ok(());
    }
    Err(Refusal::Invalid(format!(
        "{role} node {node_id:?} is not in the graph"
    )))
}

/// Positions reach the view verbatim, and a non-finite one fails
/// `GraphView::validate` — which only runs on save and load, so an
/// unchecked NaN surfaces as a document that won't reopen.
pub(super) fn finite_position(pos: Vec2, role: &'static str) -> Result<(), Refusal> {
    if !pos.is_finite() {
        return Err(Refusal::Invalid(format!(
            "{role} position {pos:?} is not finite"
        )));
    }
    Ok(())
}
