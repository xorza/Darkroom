//! The preconditions
//! [`GraphIntent::into_step`](crate::core::edit::graph_intent::GraphIntent::into_step)
//! establishes before it will fold an intent into a step.
//!
//! Everything commits through that one entry, so these are the whole
//! precondition set. Each check answers one question — is this id resolvable,
//! fresh, non-nil; is this position finite; is this kind insertable here; is
//! this insertion's own wiring something the graph can take back verbatim.
//!
//! A reference that merely went *stale* is not a failure: a gesture spanning
//! frames produces one normally, so [`live_node`] answers `Ok(None)` and the
//! caller yields no step. Only a payload that could never have applied is an
//! `Err`, and that is a bug in whatever raised it — see [`MalformedIntent`].
//!
//! Two failure modes motivate the split. Some of these guard *panics*:
//! `Graph::insert` panics on a duplicate id, and `Graph::attach_node` on a
//! node already present. The rest guard *corruption*: state that applies cleanly
//! and leaves a document `Document::validate` rejects — which, because saving
//! validates only in debug builds, means a project that writes fine and won't
//! reopen.

use glam::Vec2;
use scenarium::{Binding, BindingEntry, Graph, InputPort, Node, NodeId, NodeKind};

use crate::core::edit::error::MalformedIntent;

/// An id the intent must actually name. A nil id is nothing a widget could
/// have read out of the document, so it is malformed rather than stale — and
/// it comes first, because the lookup would read it as merely absent.
pub(super) fn non_nil_node(node_id: NodeId, role: &'static str) -> Result<(), MalformedIntent> {
    if node_id.is_nil() {
        return Err(MalformedIntent::NilNodeId { role });
    }
    Ok(())
}

/// Resolve a node the intent points at.
///
/// `Ok(None)` means the node is simply gone — the ordinary outcome of input
/// that spans frames, which the caller turns into "no step".
pub(super) fn live_node<'a>(
    graph: &'a Graph,
    node_id: NodeId,
    role: &'static str,
) -> Result<Option<&'a Node>, MalformedIntent> {
    non_nil_node(node_id, role)?;
    Ok(graph.find(node_id))
}

/// A node id an insertion introduces. Scenarium requires node ids to be
/// unique across the entire authoring tree, and [`Graph::insert`] panics
/// outright on a collision.
///
/// One insertion carries one node, and intents commit one at a time — so a
/// batch that repeats an id fails here on its second intent, against a graph
/// the first has already been applied to.
pub(super) fn fresh_node_id(graph: &Graph, node_id: NodeId) -> Result<(), MalformedIntent> {
    non_nil_node(node_id, "new")?;
    if graph.find(node_id).is_some() {
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

/// The bindings riding along with an insertion: each one an input of the new
/// node, read from a node the graph holds, never from the new node itself.
///
/// What an insertion adds to the record's own rules. Order and a port bound
/// twice are [`DetachedNode::new`](scenarium::DetachedNode::new)'s, which takes
/// these next.
pub(super) fn seed_bindings(
    graph: &Graph,
    node_id: NodeId,
    bindings: Vec<(InputPort, Binding)>,
) -> Result<Vec<BindingEntry>, MalformedIntent> {
    let mut entries: Vec<BindingEntry> = Vec::with_capacity(bindings.len());
    for (port, binding) in bindings {
        if port.node_id != node_id {
            return Err(MalformedIntent::ForeignSeedBinding { port });
        }
        if let Binding::Bind(source) = &binding {
            // The one cycle an insertion can author: nothing reads the new
            // node yet, so the only loop it can close is through itself.
            if source.node_id == node_id {
                return Err(MalformedIntent::CyclicSeedBinding { port });
            }
            // And the producer has to be there, or the graph keeps a dangling
            // edge.
            present_node(graph, source.node_id, "binding producer")?;
        }
        entries.push(BindingEntry { port, binding });
    }
    Ok(entries)
}

/// A node an insertion's wiring points at. Unlike [`live_node`] a miss is
/// malformed, not stale — an insertion's wiring is authored in the same breath
/// as its node.
fn present_node(graph: &Graph, node_id: NodeId, role: &'static str) -> Result<(), MalformedIntent> {
    non_nil_node(node_id, role)?;
    if graph.find(node_id).is_some() {
        return Ok(());
    }
    Err(MalformedIntent::NodeAbsent { role, node_id })
}

/// Positions reach the view verbatim, and a non-finite one fails
/// `GraphView::validate` — which only runs on save and load, so an unchecked
/// NaN surfaces as a document that won't reopen.
pub(super) fn finite_position(pos: Vec2, role: &'static str) -> Result<(), MalformedIntent> {
    if !pos.is_finite() {
        return Err(MalformedIntent::NonFinitePosition { role, pos });
    }
    Ok(())
}
