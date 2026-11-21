//! The preconditions [`GraphIntent::commit`](crate::core::edit::graph_intent::GraphIntent::commit)
//! establishes before it folds an intent into a step.
//!
//! Everything commits through that one entry, so these are the whole precondition set. Whether
//! the graph may hold a binding or a subscription is scenarium's one rule, [`Graph::admit`] and
//! [`Graph::admit_subscription`].
//!
//! A reference that merely went *stale* is not a failure: a gesture spanning frames produces one
//! normally, so [`live_node`] answers `Ok(None)` and the caller yields no step. Only a payload
//! that could never have applied is an `Err`, a bug in whatever raised it; see
//! [`MalformedIntent`].
//!
//! Some checks guard *panics* (`Graph::insert` on a duplicate id, `Graph::attach_node` on a node
//! already present); the rest guard *corruption*: state that applies cleanly and leaves a
//! document the load contract does not hold, which every compile asserts.

use glam::Vec2;
use scenarium::{Binding, Graph, Node, NodeDecl, NodeId, Scope, Wiring};

use crate::core::document::tie::TieId;
use crate::core::edit::error::{MalformedIntent, Role};

/// An id the intent must actually name. A nil id is nothing a widget could have read out of the
/// document, so it is malformed rather than stale, and is checked first because a lookup would
/// read it as merely absent.
pub(super) const fn non_nil_node(node_id: NodeId, role: Role) -> Result<(), MalformedIntent> {
    if node_id.is_nil() {
        return Err(MalformedIntent::NilNodeId { role });
    }
    Ok(())
}

/// A tie id the intent must actually name, malformed when nil for the same reason as a node's.
pub(super) const fn non_nil_tie(tie: TieId, role: Role) -> Result<(), MalformedIntent> {
    if tie.is_nil() {
        return Err(MalformedIntent::NilTieId { role });
    }
    Ok(())
}

/// Resolve a node the intent points at. `Ok(None)` means the node is gone, the ordinary outcome
/// of input that spans frames, which the caller turns into "no step".
pub(super) fn live_node(
    graph: &Graph,
    node_id: NodeId,
    role: Role,
) -> Result<Option<&Node>, MalformedIntent> {
    non_nil_node(node_id, role)?;
    Ok(graph.find(node_id))
}

/// A node id an insertion into `graph` introduces: one the graph holds neither as a node nor as
/// a ghost node, as [`Graph::insert`] refuses both. A node id is unique within its graph alone.
///
/// Intents commit one at a time, so a batch that repeats an id fails here on its second intent,
/// against a graph the first has already been applied to.
pub(super) fn fresh_node_id(graph: &Graph, node_id: NodeId) -> Result<(), MalformedIntent> {
    non_nil_node(node_id, Role::New)?;
    if graph.holds(node_id) {
        return Err(MalformedIntent::NodeAlreadyExists { node_id });
    }
    Ok(())
}

/// A newly inserted node has to resolve: the graph holds only nodes the library can run.
pub(super) fn resolving_node<'a>(
    scope: Scope<'a>,
    node: &Node,
) -> Result<NodeDecl<'a>, MalformedIntent> {
    scope
        .resolve(node)
        .map_err(|reason| MalformedIntent::UnresolvedNode { reason })
}

/// One node an insertion introduces, as the checks below read it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Insertion<'a> {
    pub(super) node_id: NodeId,
    pub(super) decl: NodeDecl<'a>,
    pub(super) bindings: &'a Wiring,
}

/// The bindings riding along with an insertion: each one an input of the new node, read from a
/// node the graph holds, never from the new node itself, and admitted onto the new node's
/// declaration.
///
/// # Panics
/// If the insertion carries a subscription, which an intent adds apart.
pub(super) fn seed_bindings(
    graph: &Graph,
    scope: Scope<'_>,
    insertion: Insertion<'_>,
) -> Result<(), MalformedIntent> {
    let Insertion {
        node_id,
        decl,
        bindings,
    } = insertion;
    assert_eq!(
        bindings.subscriptions().len(),
        0,
        "an insertion seeds bindings alone"
    );
    for (port, binding) in bindings.bindings() {
        if port.node_id != node_id {
            return Err(MalformedIntent::ForeignSeedBinding { port });
        }
        if let Binding::Wire(source) = binding {
            // Nothing reads the new node yet, so the only loop it can close is through itself.
            if source.node_id == node_id {
                return Err(MalformedIntent::CyclicSeedBinding { port });
            }
            // The producer has to be there, or the graph keeps a dangling edge.
            present_node(graph, source.node_id, Role::BindingProducer)?;
        }
        graph
            .admit_onto(scope, decl, port, binding)
            .map_err(|rejection| MalformedIntent::Rejected { port, rejection })?;
    }
    Ok(())
}

/// A node an insertion's wiring points at. Unlike [`live_node`] a miss is malformed, not stale.
fn present_node(graph: &Graph, node_id: NodeId, role: Role) -> Result<(), MalformedIntent> {
    non_nil_node(node_id, role)?;
    match graph.find(node_id) {
        Some(_) => Ok(()),
        None => Err(MalformedIntent::NodeAbsent { role, node_id }),
    }
}

/// Positions reach the view verbatim, and nothing after the load repairs a
/// non-finite one.
pub(super) fn finite_pos(pos: Vec2, role: Role) -> Result<(), MalformedIntent> {
    if !pos.is_finite() {
        return Err(MalformedIntent::NonFinitePos { role, pos });
    }
    Ok(())
}
