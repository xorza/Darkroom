//! The preconditions
//! [`GraphIntent::into_step`](crate::core::edit::graph_intent::GraphIntent::into_step)
//! establishes before it will fold an intent into a step.
//!
//! Everything commits through that one entry, so these are the whole
//! precondition set. Each check answers one question — is this id resolvable,
//! fresh, non-nil; is this position finite; is this kind insertable here; does
//! this port or event index name one its node declares; is this insertion's own
//! wiring something the graph can take back verbatim.
//!
//! A reference that merely went *stale* is not a failure: a gesture spanning
//! frames produces one normally, so [`live_node`] answers `Ok(None)` and the
//! caller yields no step. Only a payload that could never have applied is an
//! `Err`, and that is a bug in whatever raised it — see [`MalformedIntent`].
//!
//! Two failure modes motivate the split. Some of these guard *panics*:
//! `Graph::insert` panics on a duplicate id, and `Graph::attach_node` on a
//! node already present. The rest guard *corruption*: state that applies cleanly
//! and leaves a document `Document::validate` rejects — which saving refuses,
//! so a project the user can no longer save — or that a load refuses, so a
//! project the user saves and cannot reopen.

use glam::Vec2;
use scenarium::{
    Binding, BindingEntry, Graph, InputPort, Library, Node, NodeId, NodeKind, OutputPort,
    Subscription,
};

use crate::core::document::declaration;
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

/// A node id an insertion introduces. Scenarium requires node ids to be
/// unique across the entire authoring tree, and [`Graph::insert`] panics
/// outright on a collision.
///
/// One insertion carries one node, and intents commit one at a time — so a
/// batch that repeats an id fails here on its second intent, against a graph
/// the first has already been applied to.
pub(super) fn fresh_node_id(graph: &Graph, node_id: NodeId) -> Result<(), MalformedIntent> {
    non_nil_node(node_id, Role::New)?;
    if graph.find(node_id).is_some() {
        return Err(MalformedIntent::NodeAlreadyExists { node_id });
    }
    Ok(())
}

/// A newly inserted node's kind has to name something: a non-nil func id, or
/// a built-in special. The func need not resolve — a duplicated stub stays a
/// stub, as a loaded one does.
pub(super) const fn insertable_kind(node: &Node) -> Result<(), MalformedIntent> {
    match &node.kind {
        NodeKind::Func { func_id, .. } => {
            if func_id.is_nil() {
                return Err(MalformedIntent::NilFuncId);
            }
        }
        NodeKind::Special(_) => {}
    }
    Ok(())
}

/// One node an insertion introduces, as the checks below read it: the id it
/// lands under, the node itself, and the bindings seeded with it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Insertion<'a> {
    pub(super) node_id: NodeId,
    pub(super) node: &'a Node,
    pub(super) bindings: &'a [BindingEntry],
}

/// The bindings riding along with an insertion: each one an input of the new
/// node, read from a node the graph holds, never from the new node itself.
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
        node,
        bindings,
    } = insertion;
    for BindingEntry { port, binding } in bindings {
        let port = *port;
        if port.node_id != node_id {
            return Err(MalformedIntent::ForeignSeedBinding { port });
        }
        declared_input(node, library, port)?;
        if let Binding::Bind(source) = binding {
            wirable(node, library, port)?;
            // The one cycle an insertion can author: nothing reads the new
            // node yet, so the only loop it can close is through itself.
            if source.node_id == node_id {
                return Err(MalformedIntent::CyclicSeedBinding { port });
            }
            // And the producer has to be there, or the graph keeps a dangling
            // edge.
            let producer = present_node(graph, source.node_id, Role::BindingProducer)?;
            declared_output(producer, library, *source)?;
        }
    }
    Ok(())
}

/// An input an edit binds: one `node`'s func declares. Neither the graph nor a
/// save refuses a binding past the declared inputs, and a reload does — so it
/// would save a project that cannot reopen.
pub(super) fn declared_input(
    node: &Node,
    library: &Library,
    port: InputPort,
) -> Result<(), MalformedIntent> {
    if !declaration::admits_input(node, library, port.port_idx) {
        return Err(MalformedIntent::UndeclaredInput { port });
    }
    Ok(())
}

/// The output a wire reads: one the producer's func declares.
pub(super) fn declared_output(
    producer: &Node,
    library: &Library,
    port: OutputPort,
) -> Result<(), MalformedIntent> {
    if !declaration::admits_output(producer, library, port.port_idx) {
        return Err(MalformedIntent::UndeclaredOutput { port });
    }
    Ok(())
}

/// The event a subscription listens to: one the emitter's func declares.
pub(super) fn declared_event(
    emitter: &Node,
    library: &Library,
    subscription: Subscription,
) -> Result<(), MalformedIntent> {
    if !declaration::admits_event(emitter, library, subscription.event_idx) {
        return Err(MalformedIntent::UndeclaredEvent { subscription });
    }
    Ok(())
}

/// An input a wire may land on: not one its func declares const-only. The
/// canvas's snap filter keeps a wire off such an input, so a bind here is a
/// widget's bug — and it would fail every compile (`ConstOnlyBinding`).
/// A node whose func the library lacks declares nothing, so it refuses
/// nothing either.
pub(super) fn wirable(
    node: &Node,
    library: &Library,
    port: InputPort,
) -> Result<(), MalformedIntent> {
    if declaration::const_only(node, library, port.port_idx) {
        return Err(MalformedIntent::WiredConstOnly { port });
    }
    Ok(())
}

/// A node an insertion's wiring points at. Unlike [`live_node`] a miss is
/// malformed, not stale — an insertion's wiring is authored in the same breath
/// as its node.
fn present_node(graph: &Graph, node_id: NodeId, role: Role) -> Result<&Node, MalformedIntent> {
    non_nil_node(node_id, role)?;
    graph
        .find(node_id)
        .ok_or(MalformedIntent::NodeAbsent { role, node_id })
}

/// Positions reach the view verbatim, and a non-finite one fails
/// `GraphView::validate` — which only runs on save and load, so an unchecked
/// NaN surfaces as a document that won't reopen.
pub(super) fn finite_position(pos: Vec2, role: Role) -> Result<(), MalformedIntent> {
    if !pos.is_finite() {
        return Err(MalformedIntent::NonFinitePosition { role, pos });
    }
    Ok(())
}
