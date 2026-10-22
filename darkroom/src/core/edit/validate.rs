//! The preconditions
//! [`GraphIntent::into_step`](crate::core::edit::graph_intent::GraphIntent::into_step)
//! establishes before it will fold an intent into a step.
//!
//! Everything commits through that one entry, so these are the whole
//! precondition set. Each check answers one question — is this id resolvable,
//! fresh, non-nil; is this position finite; does this node resolve; is this
//! insertion's own wiring something the graph can take back verbatim. Whether
//! the graph may hold a binding or a subscription is scenarium's one rule,
//! [`Graph::admit`] and [`Graph::admit_subscription`].
//!
//! A reference that merely went *stale* is not a failure: a gesture spanning
//! frames produces one normally, so [`live_node`] answers `Ok(None)` and the
//! caller yields no step. Only a payload that could never have applied is an
//! `Err`, and that is a bug in whatever raised it — see [`MalformedIntent`].
//!
//! Two failure modes motivate the split. Some of these guard *panics*:
//! `Graph::insert` panics on a duplicate id, and `Graph::attach_node` on a
//! node already present. The rest guard *corruption*: state that applies cleanly
//! and leaves a document the load contract does not hold — which every compile
//! asserts.

use glam::Vec2;
use scenarium::{Binding, BindingEntry, Func, Graph, Library, Node, NodeId};

use crate::core::document::Document;
use crate::core::edit::error::{MalformedIntent, Role};

/// An id the intent must actually name. A nil id is nothing a widget could
/// have read out of the document, so it is malformed rather than stale — and
/// it comes first, because the lookup would read it as merely absent.
pub(super) const fn non_nil_node(node_id: NodeId, role: Role) -> Result<(), MalformedIntent> {
    if node_id.is_nil() {
        return Err(MalformedIntent::NilNodeId { role });
    }
    Ok(())
}

/// Resolve a node the intent points at.
///
/// `Ok(None)` means the node is simply gone — the ordinary outcome of input
/// that spans frames, which the caller turns into "no step".
pub(super) fn live_node(
    graph: &Graph,
    node_id: NodeId,
    role: Role,
) -> Result<Option<&Node>, MalformedIntent> {
    non_nil_node(node_id, role)?;
    Ok(graph.find(node_id))
}

/// A node id an insertion introduces: one the document holds neither as a
/// graph node nor as a stub. [`Graph::insert`] panics outright on a graph
/// collision, and a stub's id in the graph too would break the document's
/// disjoint ids.
///
/// One insertion carries one node, and intents commit one at a time — so a
/// batch that repeats an id fails here on its second intent, against a
/// document the first has already been applied to.
pub(super) fn fresh_node_id(doc: &Document, node_id: NodeId) -> Result<(), MalformedIntent> {
    non_nil_node(node_id, Role::New)?;
    if doc.holds_node(node_id) {
        return Err(MalformedIntent::NodeAlreadyExists { node_id });
    }
    Ok(())
}

/// A newly inserted node has to resolve: the graph holds only nodes the
/// library can run, and Duplicate skips a stub.
pub(super) fn resolving_node<'a>(
    library: &'a Library,
    node: &Node,
) -> Result<&'a Func, MalformedIntent> {
    library
        .func_of(node)
        .map_err(|reason| MalformedIntent::UnresolvedNode { reason })
}

/// One node an insertion introduces, as the checks below read it: the id it
/// lands under, the func it resolves to, and the bindings seeded with it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Insertion<'a> {
    pub(super) node_id: NodeId,
    pub(super) func: &'a Func,
    pub(super) bindings: &'a [BindingEntry],
}

/// The bindings riding along with an insertion: each one an input of the new
/// node, read from a node the graph holds, never from the new node itself, and
/// admitted onto the new node's declaration.
///
/// What an insertion adds to the record's own rules. Order and a port bound
/// twice are [`DetachedNode::new`](scenarium::DetachedNode::new)'s, which takes
/// these next.
pub(super) fn seed_bindings(
    graph: &Graph,
    library: &Library,
    insertion: Insertion<'_>,
) -> Result<(), MalformedIntent> {
    let Insertion {
        node_id,
        func,
        bindings,
    } = insertion;
    for BindingEntry { port, binding } in bindings {
        let port = *port;
        if port.node_id != node_id {
            return Err(MalformedIntent::ForeignSeedBinding { port });
        }
        if let Binding::Bind(source) = binding {
            // The one cycle an insertion can author: nothing reads the new
            // node yet, so the only loop it can close is through itself.
            if source.node_id == node_id {
                return Err(MalformedIntent::CyclicSeedBinding { port });
            }
            // And the producer has to be there, or the graph keeps a dangling
            // edge.
            present_node(graph, source.node_id, Role::BindingProducer)?;
        }
        graph
            .admit_onto(library, func, port, binding)
            .map_err(|rejection| MalformedIntent::Rejected { port, rejection })?;
    }
    Ok(())
}

/// A node an insertion's wiring points at. Unlike [`live_node`] a miss is
/// malformed, not stale — an insertion's wiring is authored in the same breath
/// as its node.
fn present_node(graph: &Graph, node_id: NodeId, role: Role) -> Result<(), MalformedIntent> {
    non_nil_node(node_id, role)?;
    match graph.find(node_id) {
        Some(_) => Ok(()),
        None => Err(MalformedIntent::NodeAbsent { role, node_id }),
    }
}

/// Positions reach the view verbatim, and nothing after the load repairs a
/// non-finite one.
pub(super) fn finite_position(pos: Vec2, role: Role) -> Result<(), MalformedIntent> {
    if !pos.is_finite() {
        return Err(MalformedIntent::NonFinitePosition { role, pos });
    }
    Ok(())
}
