//! The authoring graph: the [`Graph`] type and the satellite types it owns ([`Node`],
//! [`Binding`], …).
//!
//! The modules beside it keep what its methods work *with*: [`detached`] the reversible-removal
//! records, [`ghost_node`] a node the graph cannot hold, [`ghost_wiring`] the wiring it cannot
//! hold, [`error`] what validation and admission reject, [`reconciled`] what a load repaired,
//! [`compound`] and [`graph_set`] the graphs a document holds, and [`scope`] what a graph's nodes
//! resolve against. How a node or port is named is [`identity`](crate::identity), and what a
//! declaration shows is [`decl`](crate::decl).
//!
//! A graph a host keeps holds among its nodes only nodes that resolve in its scope, and only
//! wiring [`Graph::admit`] accepts; each node it cannot hold is a [`GhostNode`] beside them, and
//! each binding and subscription it cannot hold is among its [`GhostWiring`].
//! [`GraphSet::reconcile`](crate::GraphSet::reconcile) makes a decoded set so, and the host's
//! edit gate keeps it so. A node's own id is its only name in its graph.
//!
//! # Decisions
//!
//! - **Reconcile at load, assert after.** A host's library keeps its funcs and
//!   their ports for the life of a process — darkroom's recompose changes only
//!   an input default, which no binding depends on — so only a load can bring
//!   in a mismatch with it; the load repairs it, and every later reader
//!   asserts. Refused: tolerance at each reader, as
//!   two readers that tolerate in their own way disagree. Source: "parse,
//!   don't validate".
//! - **One admission rule.** [`Graph::admit`] and [`Graph::admit_subscription`]
//!   are the rule for the load, the edit gate and the compile's assert.
//! - **A binding names a port by its id.** An input, an output and an event are
//!   each a node and the id its declaration gives the port, on a func node, an
//!   instance and a boundary node alike. A func that gains a port keeps every
//!   saved node, the new port unbound; a port that leaves strands only its own
//!   wiring, which the load keeps as ghosts, as any binding the declaration
//!   does not admit; a rename and a reorder change nothing. As Blender finds a
//!   group's sockets by identifier
//!   ([group creation](https://projects.blender.org/archive/blender-archive/commit/fa3ca9afdb6f1ea1b709226af3f085133e5ddc2f))
//!   and Unreal gives a pin a persistent id as "pin name could be changed"
//!   ([`UEdGraphPin`](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Engine/UEdGraphPin)).
//!   Refused: the port's index with a signature of the func's ports on each
//!   node, which made every saved node of a func a ghost node when the func gained an
//!   input. The compiled program stays positional: a compile maps each id to
//!   its position once.
//! - **A graph keeps the nodes it cannot hold.** Its ghost nodes sit beside its nodes, the ids
//!   of the two disjoint, so a definition is one value wherever it goes: a clip, a library file,
//!   a copy, a digest. As Blender, which swaps an unknown node for an "undefined" one in the
//!   tree and keeps its links
//!   ([undefined node](https://projects.blender.org/blender/blender/pulls/114783)). No reader of
//!   the nodes sees a ghost node, so none needs a "declares nothing" branch. Refused: the ghost
//!   nodes in the host beside each graph, where a definition travels in two halves, the host
//!   writes the attach rules a second time, and every edit of a node asks two containers which
//!   holds it.
//! - **A load drops no wiring whose two nodes the graph keeps.** What the scope refuses becomes
//!   a ghost: an end that is a ghost node, a port its node no longer declares, a type or a
//!   constant the input no longer takes, a wire that would close a loop. As Unreal keeps a
//!   removed pin that is in use as an orphaned pin, which "must be linked to other pins or have
//!   a non-default value and will be removed if disconnected, reset to default, or the node is
//!   refreshed"
//!   ([`bOrphanedPin`](https://docs.unrealengine.com/4.26/en-US/API/Runtime/Engine/EdGraph/UEdGraphPin/bOrphanedPin/index.html)).
//!   Still dropped: wiring that names a node the graph holds as neither, a node subscribed to
//!   itself, a trigger no node of its subscriber's kind has: no library brings those back.
//! - **A constant equal to the default its port declared is not kept**, Unreal's "non-default
//!   value": a func that loses an input would otherwise leave a ghost on every node of it, each
//!   holding the default nobody set. The input then takes the default the port declares now, if
//!   the node still has the port. A ghost node keeps every constant: it is the node, whole.
//! - **A port a func gained takes its default on each saved node**, when the file's
//!   [`Manifest`](crate::Manifest) shows the port is new and the input holds nothing. An updated
//!   node is then what a new node of the func is, with what you set kept. A func the manifest
//!   does not list gains nothing: nothing says which of its ports are new, and a guess would
//!   overwrite an input you cleared.
//! - **Nothing matches by name.** A port is its id; a ghost moves to another port only by an
//!   edit. A saved name is what a ghost shows, never what a load binds by.
//! - **A ghost becomes live at a load, or when a host asks, through one rule.**
//!   [`Graph::admit_ghost`] is [`Graph::admit`] plus the loop check; the load restores each
//!   ghost it passes, in port order. Refused: restoring after each edit, which rewires the graph
//!   behind an unrelated action.
//! - **A load step reads and writes in one method**, of [`GraphLoad`](load::GraphLoad), which
//!   holds the graph beside a scope over the set's compound declarations: a scope never reads a
//!   graph, so one graph can change while it lives. Refused: a read under the scope handing a plan
//!   to a write once the scope is dropped, a type for each step's plan; taking a graph out of its
//!   set for the call, which leaves the set wrong while a scope reads it.
//! - **The invariant is the host's contract, not a type.** A graph holds no library, so no type
//!   can enforce admission; the host's edit gate, [`Graph::admit_bind`] and its kin, is the only
//!   way an edit lands. Each table is written through the graph's own methods, which keep the live
//!   wiring and the ghost wiring apart.
//! - **Broken structure is repaired, not refused.** A node under the nil id
//!   goes, as no lookup can address it; wiring that names a missing node goes;
//!   a nil func id is a node that does not resolve. Refused: refusing the
//!   file, which then does not open.
//! - **A data cycle is broken in node-id order.** A depth-first walk from each
//!   node in id order makes a ghost of each wire that returns to the current
//!   path, so every load makes the same choice. Only a hand-edited file holds
//!   one among its live wires.
//! - **A wire through an instance ties every output to every input** in the
//!   cycle check, which reads nodes and not ports. Cost: a loop through two
//!   ports that share nothing inside is refused, as Blender refuses it.
//! - **An output's type is its declaration.** No output is a wildcard, so no
//!   edit retypes a distant wire and every wire is checkable. No func declares
//!   an `Any` output; the `Inputs` node of a compound shows one for a port made
//!   from an `Any` input, held as declared, which feeds only `Any` inputs.
//!   Refused: wildcard outputs, whose retype either removes wires you did not
//!   touch or refuses the edit until you remove them by hand. Reroute nodes
//!   need a new design.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use ::serde::{Deserialize, Serialize};

use crate::data::const_value::ConstValue;
use crate::decl::ports::Ports;
use crate::decl::ports::error::BindingRejection;
use crate::decl::saved_decl::{SavedDecl, SavedEvent, SavedPort, ShownPort};
use crate::graph::binding::Binding;
use crate::graph::detached::{DetachedGhost, DetachedNode};
use crate::graph::error::{
    BindRejection, DetachReason, GhostSubscriptionRejection, GraphValidationError,
    SubscriptionRejection, ValidationResult,
};
use crate::graph::ghost_node::GhostNode;
use crate::graph::ghost_wiring::{
    GhostBinding, GhostSource, GhostSubscription, GhostWiring, InputState, SubscriptionState,
};
use crate::graph::node::{Boundary, Node, NodeKind};
use crate::graph::node_decl::NodeDecl;
use crate::graph::node_ref::NodeRef;
use crate::graph::reconciled::Repair;
use crate::graph::scope::Scope;
use crate::graph::subscription::Subscription;
use crate::graph::wiring::Wiring;
use crate::identity::{CompoundId, EventPort, InputPort, NodeId, OutputPort, TriggerPort};

pub(crate) mod binding;
pub(crate) mod compound;
pub(crate) mod detached;
mod dfs;
pub(crate) mod error;
pub(crate) mod ghost_node;
pub(crate) mod ghost_wiring;
pub(crate) mod graph_set;
pub(crate) mod interface;
pub(crate) mod interface_change;
pub(crate) mod load;
pub(crate) mod manifest;
pub(crate) mod node;
pub(crate) mod node_decl;
pub(crate) mod node_ref;
pub(crate) mod reconciled;
pub(crate) mod remap;
pub(crate) mod scope;
mod serde;
pub(crate) mod subscription;
pub(crate) mod type_names;
pub(crate) mod wiring;

/// `Clone` keeps every node id as it is, so a copy is sound only where it never lives beside its
/// original in one document, as the copy a save rewrites and writes; an editor's duplicate mints
/// fresh ids instead.
///
/// Every table is reached through the methods below, which keep the live wiring and the ghost
/// wiring apart: an input holds one binding, live or ghost.
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    /// Every node, keyed by id, so an id is unique by construction:
    /// [`insert`](Self::insert) panics rather than replacing, and
    /// [`serde::deserialize_nodes`](crate::graph::serde) refuses a repeated id
    /// instead of letting the map absorb it. A `BTreeMap`, as every table of a graph: a save and
    /// every walk of the nodes go in id order, where a hash map's order is arbitrary and its seed
    /// differs with each process ([`HashMap::iter`](std::collections::HashMap::iter)), so two
    /// saves of one document differed. A lookup is a few comparisons for a graph's nodes.
    #[serde(deserialize_with = "crate::graph::serde::deserialize_nodes")]
    nodes: BTreeMap<NodeId, Node>,

    /// The bindings and subscriptions, each end a node of this graph.
    wiring: Wiring,

    /// The nodes this graph cannot hold, by id, their ids disjoint from `nodes`'. Decoded entry
    /// by entry, so a document that repeats an id is refused.
    #[serde(deserialize_with = "crate::graph::serde::deserialize_ghost_nodes")]
    ghost_nodes: BTreeMap<NodeId, GhostNode>,

    /// The bindings and subscriptions this graph cannot hold, their keys disjoint from the live
    /// tables'. Each end of each is a node or a ghost node of this graph.
    ghost_wiring: GhostWiring,
}

/// A node a reconcile takes out of its graph, why, and what it declared: what a read of the
/// graph under its scope answers, which the write then detaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Detaching {
    pub(crate) node_id: NodeId,
    pub(crate) reason: DetachReason,
    pub(crate) saved: Option<SavedDecl>,
}

/// Wiring a scope refuses: what a read of a graph under its scope answers, which the load's
/// ghosting and an interface edit's plan act on. One enum, as each refusal carries its rejection
/// beside its key. Refused: two lists of pairs, which only move the match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefusedWiring {
    Binding {
        port: InputPort,
        rejection: BindingRejection,
    },
    Subscription {
        subscription: Subscription,
        rejection: SubscriptionRejection,
    },
}

impl Graph {
    /// How many nodes, the ghost nodes aside.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    /// In node-id order.
    pub fn iter(&self) -> impl Iterator<Item = NodeRef<'_>> {
        self.nodes
            .iter()
            .map(|(id, node)| NodeRef { node_id: *id, node })
    }

    /// The node with `id`, or `None` if this graph has none, as for the nil id or a ghost node.
    ///
    /// A bare `&Node` rather than a [`NodeRef`]: the caller supplied the id.
    pub fn find(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// The ghost node under `id`, or `None` if this graph keeps none.
    pub fn ghost_node(&self, id: NodeId) -> Option<&GhostNode> {
        self.ghost_nodes.get(&id)
    }

    /// The ghost nodes, in node-id order.
    pub fn ghost_nodes(&self) -> impl ExactSizeIterator<Item = (NodeId, &GhostNode)> {
        self.ghost_nodes.iter().map(|(id, ghost)| (*id, ghost))
    }

    /// The bindings and subscriptions this graph holds.
    pub const fn wiring(&self) -> &Wiring {
        &self.wiring
    }

    /// The bindings and subscriptions this graph keeps and cannot hold.
    pub const fn ghost_wiring(&self) -> &GhostWiring {
        &self.ghost_wiring
    }

    /// Whether this graph holds `id`, as a node or a ghost node: the ids a new node may not
    /// take.
    pub fn holds(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id) || self.ghost_nodes.contains_key(&id)
    }

    /// Keep `ghost` beside the nodes under `node_id`, with no wiring.
    ///
    /// # Panics
    /// If this graph holds `node_id` already, as a node or a ghost node.
    pub fn insert_ghost_node(&mut self, node_id: NodeId, ghost: GhostNode) {
        assert!(
            !node_id.is_nil(),
            "cannot keep a ghost node under the nil id"
        );
        assert!(
            !self.holds(node_id),
            "ghost node {node_id:?} takes an id the graph holds"
        );
        self.ghost_nodes.insert(node_id, ghost);
    }

    /// The ghost node `node_id` with each ghost that touches it, or `None` if this graph keeps
    /// no such ghost node: [`Self::snapshot_node`] for one.
    pub fn snapshot_ghost_node(&self, node_id: NodeId) -> Option<DetachedGhost> {
        Some(DetachedGhost {
            node_id,
            ghost: self.ghost_nodes.get(&node_id)?.clone(),
            wiring: self.ghost_wiring.touching(&[node_id]),
        })
    }

    /// Take the ghost node `node_id` out with each ghost that touches it.
    ///
    /// # Panics
    /// If this graph keeps no such ghost node.
    pub fn detach_ghost_node(&mut self, node_id: NodeId) -> DetachedGhost {
        let ghost = self
            .ghost_nodes
            .remove(&node_id)
            .expect("cannot detach a ghost node that is not in the graph");
        DetachedGhost {
            node_id,
            ghost,
            wiring: self.ghost_wiring.remove_touching(&[node_id]),
        }
    }

    /// Put back what [`Self::detach_ghost_node`] took out.
    ///
    /// # Panics
    /// If this graph holds the id already, or holds wiring where a ghost goes back.
    pub fn attach_ghost_node(&mut self, detached: DetachedGhost) {
        let DetachedGhost {
            node_id,
            ghost,
            wiring,
        } = detached;
        self.assert_ghost_wiring_fits(&wiring);
        self.insert_ghost_node(node_id, ghost);
        self.ghost_wiring.extend(wiring);
    }

    /// Assert each ghost of `wiring` has its key free here, live and ghost: what a record taken
    /// out puts back. Asked before anything goes in, so a refused attach leaves the graph as it
    /// was.
    ///
    /// # Panics
    /// If an input one goes on is bound or holds a ghost, or a subscription one is of is made or
    /// kept.
    fn assert_ghost_wiring_fits(&self, wiring: &GhostWiring) {
        for port in wiring.bindings.keys() {
            assert!(
                !self.wiring.bindings.contains_key(port),
                "a ghost goes back on an unbound input"
            );
            assert!(
                !self.ghost_wiring.bindings.contains_key(port),
                "a ghost goes back on an input with none"
            );
        }
        for subscription in wiring.subscriptions.keys() {
            assert!(
                !self.wiring.subscriptions.contains(subscription),
                "a ghost goes back of a subscription not made"
            );
            assert!(
                !self.ghost_wiring.subscriptions.contains_key(subscription),
                "a ghost goes back of a subscription not kept"
            );
        }
    }

    /// Each constant a binding holds, live or ghost, with the input it is bound to.
    pub fn constants(&self) -> impl Iterator<Item = (InputPort, &ConstValue)> {
        let live = self
            .wiring
            .bindings
            .iter()
            .filter_map(|(port, binding)| match binding {
                Binding::Const(value) => Some((*port, value)),
                Binding::Wire(_) => None,
            });
        let ghosts = self
            .ghost_wiring
            .bindings
            .iter()
            .filter_map(|(port, ghost)| match &ghost.source {
                GhostSource::Const(value) => Some((*port, value)),
                GhostSource::Wire { .. } => None,
            });
        live.chain(ghosts)
    }

    /// [`Self::constants`] for writing.
    pub fn constants_mut(&mut self) -> impl Iterator<Item = &mut ConstValue> {
        let live = self
            .wiring
            .bindings
            .values_mut()
            .filter_map(|binding| match binding {
                Binding::Const(value) => Some(value),
                Binding::Wire(_) => None,
            });
        let ghosts =
            self.ghost_wiring
                .bindings
                .values_mut()
                .filter_map(|ghost| match &mut ghost.source {
                    GhostSource::Const(value) => Some(value),
                    GhostSource::Wire { .. } => None,
                });
        live.chain(ghosts)
    }

    /// What `port` holds: a binding, a ghost, or nothing.
    pub fn input_state(&self, port: InputPort) -> InputState {
        match (
            self.wiring.bindings.get(&port),
            self.ghost_wiring.bindings.get(&port),
        ) {
            (Some(binding), _) => InputState::Live(binding.clone()),
            (None, Some(ghost)) => InputState::Ghost(ghost.clone()),
            (None, None) => InputState::Unbound,
        }
    }

    /// The output `port` is wired to, live or as a ghost, or `None` for an input that holds a
    /// constant or nothing: [`Self::input_state`] for a reader that asks of a wire alone.
    pub fn input_state_wire(&self, port: InputPort) -> Option<OutputPort> {
        match self.wiring.bindings.get(&port) {
            Some(Binding::Wire(output)) => Some(*output),
            Some(Binding::Const(_)) => None,
            None => self.ghost_wiring.binding(port)?.output(),
        }
    }

    /// Make `port` hold `state`, in place of what it held, live or ghost.
    pub fn set_input_state(&mut self, port: InputPort, state: InputState) {
        self.wiring.bindings.remove(&port);
        self.ghost_wiring.bindings.remove(&port);
        match state {
            InputState::Unbound => {}
            InputState::Live(binding) => {
                self.wiring.bindings.insert(port, binding);
            }
            InputState::Ghost(ghost) => {
                self.ghost_wiring.bindings.insert(port, ghost);
            }
        }
    }

    /// Whether `subscription` is made, kept as a ghost, or neither.
    pub fn subscription_state(&self, subscription: Subscription) -> SubscriptionState {
        if self.wiring.subscriptions.contains(&subscription) {
            SubscriptionState::Live
        } else {
            self.ghost_wiring
                .subscriptions
                .get(&subscription)
                .map_or(SubscriptionState::Absent, |ghost| {
                    SubscriptionState::Ghost(ghost.clone())
                })
        }
    }

    /// Make `subscription` what `state` says, in place of what it was.
    pub fn set_subscription_state(&mut self, subscription: Subscription, state: SubscriptionState) {
        self.wiring.subscriptions.remove(&subscription);
        self.ghost_wiring.subscriptions.remove(&subscription);
        match state {
            SubscriptionState::Absent => {}
            SubscriptionState::Live => {
                self.wiring.subscriptions.insert(subscription);
            }
            SubscriptionState::Ghost(ghost) => {
                self.ghost_wiring.subscriptions.insert(subscription, ghost);
            }
        }
    }

    /// Whether this graph may hold `binding` on `port`: both of its nodes are nodes, not ghost
    /// nodes, [`Self::admit`] passes, and a wire closes no data cycle. The rule an edit, a wire
    /// drag, a load and a host's Restore hold a binding to.
    pub fn admit_bind(
        &self,
        scope: Scope<'_>,
        port: InputPort,
        binding: &Binding,
    ) -> Result<(), BindRejection> {
        self.admit_kept(scope, port, binding)?;
        match binding {
            Binding::Wire(output) if self.produces_cycle(output.node_id, port.node_id) => {
                Err(BindRejection::Cycle)
            }
            Binding::Wire(_) | Binding::Const(_) => Ok(()),
        }
    }

    /// [`Self::admit_bind`] for the ghost on `port`: whether this graph can hold it now.
    ///
    /// # Panics
    /// If `port` holds no ghost.
    pub fn admit_ghost(&self, scope: Scope<'_>, port: InputPort) -> Result<(), BindRejection> {
        let ghost = self
            .ghost_wiring
            .binding(port)
            .expect("an admitted ghost is kept on its input");
        self.admit_bind(scope, port, &ghost.binding())
    }

    /// [`Self::admit_bind`] without the loop check, which reads the graph alone: whether both
    /// nodes of `binding` on `port` are nodes and the scope admits it.
    fn admit_kept(
        &self,
        scope: Scope<'_>,
        port: InputPort,
        binding: &Binding,
    ) -> Result<(), BindRejection> {
        let mut ends = [Some(port.node_id), None];
        if let Binding::Wire(source) = binding {
            ends[1] = Some(source.node_id);
        }
        if let Some(node_id) = ends
            .into_iter()
            .flatten()
            .find(|node_id| !self.nodes.contains_key(node_id))
        {
            return Err(BindRejection::GhostNode { node_id });
        }
        Ok(self.admit(scope, port, binding)?)
    }

    /// Whether this graph can hold the ghost of `subscription` now: both of its nodes are nodes
    /// and [`Self::admit_subscription`] passes.
    ///
    /// # Panics
    /// If the graph keeps no ghost of `subscription`.
    pub fn admit_ghost_subscription(
        &self,
        scope: Scope<'_>,
        subscription: Subscription,
    ) -> Result<(), GhostSubscriptionRejection> {
        assert!(
            self.ghost_wiring.subscription(subscription).is_some(),
            "an admitted ghost subscription is kept"
        );
        if let Some(node_id) = [subscription.event.node_id, subscription.trigger.node_id]
            .into_iter()
            .find(|node_id| !self.nodes.contains_key(node_id))
        {
            return Err(GhostSubscriptionRejection::GhostNode { node_id });
        }
        Ok(self.admit_subscription(scope, subscription)?)
    }

    /// Whether this graph may hold `binding` on `port`: the input is one its
    /// node declares, a constant is one the input accepts, and a wire reads an
    /// output its producer declares, of a type the input accepts, into an
    /// input that is not const-only unless the output holds only a constant
    /// itself, as the `Inputs` node's mirror of a const-only interface input
    /// does. The one rule a load, an edit and a
    /// compile hold wiring to. A cycle is the graph's question, not the
    /// binding's — see [`Self::produces_cycle`].
    ///
    /// # Panics
    ///
    /// If the graph does not hold the consumer, or the producer of a wire.
    pub fn admit(
        &self,
        scope: Scope<'_>,
        port: InputPort,
        binding: &Binding,
    ) -> Result<(), BindingRejection> {
        let consumer = scope.decl(
            self.find(port.node_id)
                .expect("an admitted binding's consumer is in the graph"),
        );
        self.admit_onto(scope, consumer, port, binding)
    }
    /// [`Self::admit`] onto a consumer that declares `consumer` — for a
    /// node an edit is inserting together with its bindings, which the graph
    /// does not hold yet.
    ///
    /// # Panics
    ///
    /// If the graph does not hold the producer of a wire.
    pub fn admit_onto(
        &self,
        scope: Scope<'_>,
        consumer: NodeDecl<'_>,
        port: InputPort,
        binding: &Binding,
    ) -> Result<(), BindingRejection> {
        let input = consumer
            .ports()
            .input(port.port)
            .ok_or(BindingRejection::UndeclaredInput)?;
        match binding {
            Binding::Const(value) => {
                if scope.library().const_satisfies(input, value) {
                    Ok(())
                } else {
                    Err(BindingRejection::ConstMismatch)
                }
            }
            Binding::Wire(source) => {
                let producer = scope.decl(
                    self.find(source.node_id)
                        .expect("an admitted wire's producer is in the graph"),
                );
                let output = producer
                    .ports()
                    .output(source.port)
                    .ok_or(BindingRejection::UndeclaredOutput)?;
                scope
                    .library()
                    .admit_wire(input, output, producer.output_is_const(source.port))
            }
        }
    }
    /// Why no library can let this graph hold `subscription`, live or as a ghost, whatever its
    /// nodes declare: a node's to its own event, or one on a trigger no node of its subscriber's
    /// kind has. `None` for one a declaration may admit.
    ///
    /// # Panics
    ///
    /// If the graph holds the subscriber as neither a node nor a ghost node.
    fn impossible(&self, subscription: Subscription) -> Option<SubscriptionRejection> {
        let Subscription { event, trigger } = subscription;
        if event.node_id == trigger.node_id {
            return Some(SubscriptionRejection::SelfSubscription);
        }
        let kind = self
            .nodes
            .get(&trigger.node_id)
            .or_else(|| {
                self.ghost_nodes
                    .get(&trigger.node_id)
                    .map(|ghost| &ghost.node)
            })
            .expect("a subscription's subscriber is held")
            .kind;
        let possible = match kind {
            NodeKind::Boundary(Boundary::Outputs) => true,
            NodeKind::Boundary(Boundary::Inputs) => false,
            NodeKind::Body { .. } | NodeKind::Instance { .. } => {
                trigger.trigger == TriggerPort::NODE
            }
        };
        (!possible).then_some(SubscriptionRejection::ImpossibleTrigger)
    }

    /// Whether this graph may hold `subscription`: the emitter declares the event, and it lands
    /// on a trigger another node shows ([`NodeDecl::triggers`](crate::NodeDecl::triggers)).
    ///
    /// # Panics
    ///
    /// If the graph does not hold the emitter or the subscriber.
    pub fn admit_subscription(
        &self,
        scope: Scope<'_>,
        subscription: Subscription,
    ) -> Result<(), SubscriptionRejection> {
        if let Some(rejection) = self.impossible(subscription) {
            return Err(rejection);
        }
        let Subscription { event, trigger } = subscription;
        let subscriber = self
            .find(trigger.node_id)
            .expect("an admitted subscription's subscriber is in the graph");
        if !scope.decl(subscriber).triggers().contains(trigger.trigger) {
            return Err(SubscriptionRejection::UndeclaredTrigger);
        }
        let emitter = scope.decl(
            self.find(event.node_id)
                .expect("an admitted subscription's emitter is in the graph"),
        );
        if emitter.event(event.event).is_some() {
            Ok(())
        } else {
            Err(SubscriptionRejection::UndeclaredEvent)
        }
    }
    /// Whether binding an output of `producer` into an input on `consumer`
    /// would close a directed data cycle. A node wired to itself counts.
    ///
    /// A compile asserts that no wire closes a cycle ([`Self::validate_with`]), so the editor
    /// asks this before it commits a bind. Port indices don't enter into it: a cycle is a
    /// property of the node graph.
    ///
    /// Walks *backward* from `producer` over what already feeds it; reaching `consumer` means
    /// the edge closes the loop. Backward because [`InputPort`] orders by node before port, so
    /// "what feeds this node" is one range of the bindings; the other way needs a reversed index
    /// over every binding.
    /// A walk allocates its own stack: a wire drag asks for the one or two ports under the
    /// pointer a frame. Refused: a scratch the caller keeps.
    pub fn produces_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        dfs::reaches(producer, consumer, |node_id, next| {
            next.extend(self.wires_into(node_id).map(|(_, source)| source));
        })
    }
    /// The node `node_id` with the wiring, live and ghost, that touches it, or `None` if this
    /// graph holds no such node: what [`Self::detach_node`] would take out, left in.
    pub fn snapshot_node(&self, node_id: NodeId) -> Option<DetachedNode> {
        let node = self.find(node_id)?.clone();
        Some(DetachedNode {
            node_id,
            node,
            wiring: self.wiring.touching(&[node_id]),
            ghost_wiring: self.ghost_wiring.touching(&[node_id]),
        })
    }

    pub fn add(&mut self, node: Node) -> NodeId {
        let node_id = NodeId::unique();
        self.insert(node_id, node);
        node_id
    }

    /// # Panics
    /// If `node_id` is nil, or this graph holds it already, as a node or a ghost node: an add
    /// takes a fresh id.
    pub fn insert(&mut self, node_id: NodeId, node: Node) {
        assert!(!node_id.is_nil(), "cannot add a node with a nil id");
        assert!(
            !self.ghost_nodes.contains_key(&node_id),
            "node {node_id:?} takes the id of a ghost node"
        );
        match self.nodes.entry(node_id) {
            Entry::Vacant(entry) => {
                entry.insert(node);
            }
            Entry::Occupied(_) => {
                panic!("node {node_id:?} already exists; adds must use a fresh id")
            }
        }
    }

    /// Mutable counterpart of [`Self::find`].
    pub fn find_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    /// Insert `node` under `node_id` with each default its declaration's `ports` state, as
    /// [`Self::insert`] does a node alone.
    ///
    /// # Panics
    /// As [`Self::insert`].
    pub fn insert_seeded(&mut self, node_id: NodeId, node: Node, ports: &Ports) {
        self.insert(node_id, node);
        self.wiring.extend(Wiring::defaults(node_id, ports));
    }

    /// Take the node `node_id` out with the wiring, live and ghost, that touches it. One node a
    /// call: a redirect moves one node's ends, so a wire between two redirected nodes needs the
    /// first move before the second. Refused: a batch detach, which splits such a wire across
    /// records; the load ghosts many nodes through one pass of each table instead.
    ///
    /// # Panics
    /// If this graph holds no such node.
    pub fn detach_node(&mut self, node_id: NodeId) -> DetachedNode {
        let node = self
            .nodes
            .remove(&node_id)
            .expect("cannot detach a node that is not in the graph");
        DetachedNode {
            node_id,
            node,
            wiring: self.wiring.remove_touching(&[node_id]),
            ghost_wiring: self.ghost_wiring.remove_touching(&[node_id]),
        }
    }

    /// Put back what [`Self::detach_node`] took out.
    ///
    /// # Panics
    /// If the graph holds the node's id, as a node or a ghost node, or holds wiring, live or
    /// ghost, where the record's goes back. Every check runs before anything goes in, so a
    /// refused attach leaves the graph as it was.
    pub fn attach_node(&mut self, detached: DetachedNode) {
        assert!(
            !self.holds(detached.node_id),
            "cannot attach a node that is already in the graph"
        );
        assert!(
            detached.wiring.bindings().all(|(port, _)| {
                !self.wiring.bindings.contains_key(&port)
                    && !self.ghost_wiring.bindings.contains_key(&port)
            }),
            "cannot attach over bindings created after detachment"
        );
        assert!(
            detached.wiring.subscriptions().all(|subscription| {
                !self.wiring.subscriptions.contains(&subscription)
                    && !self.ghost_wiring.subscriptions.contains_key(&subscription)
            }),
            "cannot attach over subscriptions created after detachment"
        );
        self.assert_ghost_wiring_fits(&detached.ghost_wiring);

        let DetachedNode {
            node_id,
            node,
            wiring,
            ghost_wiring,
        } = detached;
        self.insert(node_id, node);
        self.wiring.extend(wiring);
        self.ghost_wiring.extend(ghost_wiring);
    }

    /// Bind `port` to `binding`, or unbind it, in place of what it held, a ghost included:
    /// [`Self::set_input_state`] for a live binding or none.
    pub fn set_input_binding(&mut self, port: InputPort, binding: impl Into<Option<Binding>>) {
        self.set_input_state(port, InputState::from(binding.into()));
    }

    /// Make the subscription, in place of a ghost of it.
    pub fn subscribe(&mut self, event: EventPort, trigger: impl Into<TriggerPort>) {
        self.set_subscription_state(Subscription::new(event, trigger), SubscriptionState::Live);
    }

    /// End the subscription, a ghost of it included.
    pub fn unsubscribe(&mut self, event: EventPort, trigger: impl Into<TriggerPort>) {
        self.set_subscription_state(Subscription::new(event, trigger), SubscriptionState::Absent);
    }

    /// Validate this graph structurally: everything a document must satisfy to
    /// *be* a graph, answerable without a library. Decoding checks only what it
    /// alone can see (a repeated key), never the wiring below.
    ///
    /// Id uniqueness within the nodes and within the ghost nodes is not checked: each is keyed by
    /// id, and both ways a duplicate could arrive, an insert and decoding, refuse it.
    pub fn validate(&self) -> ValidationResult<()> {
        if self.holds(NodeId::nil()) {
            return Err(GraphValidationError::NilNodeId);
        }
        if let Some(node_id) = self
            .ghost_nodes
            .keys()
            .copied()
            .find(|node_id| self.nodes.contains_key(node_id))
        {
            return Err(GraphValidationError::GhostHoldsNode { node_id });
        }
        for (port, ghost) in self.ghost_wiring.bindings() {
            let ends = [
                Some(port.node_id),
                ghost.output().map(|output| output.node_id),
            ];
            if let Some(node_id) = ends.into_iter().flatten().find(|id| !self.holds(*id)) {
                return Err(GraphValidationError::GhostMissingNode { node_id });
            }
            if self.wiring.bindings.contains_key(&port) {
                return Err(GraphValidationError::GhostOnBoundInput { port });
            }
        }
        for (subscription, _) in self.ghost_wiring.subscriptions() {
            let ends = [subscription.event.node_id, subscription.trigger.node_id];
            if let Some(node_id) = ends.into_iter().find(|id| !self.holds(*id)) {
                return Err(GraphValidationError::GhostMissingNode { node_id });
            }
            if self.wiring.subscriptions.contains(&subscription) {
                return Err(GraphValidationError::GhostOfSubscription { subscription });
            }
        }

        for (destination, binding) in &self.wiring.bindings {
            if !self.nodes.contains_key(&destination.node_id) {
                return Err(GraphValidationError::BindingMissingNode {
                    node_id: destination.node_id,
                });
            }
            if let Binding::Wire(src) = binding
                && !self.nodes.contains_key(&src.node_id)
            {
                return Err(GraphValidationError::BindingMissingProducer {
                    destination: *destination,
                    producer: *src,
                });
            }
        }

        for subscription in &self.wiring.subscriptions {
            if !self.nodes.contains_key(&subscription.event.node_id) {
                return Err(GraphValidationError::MissingSubscriptionEmitter {
                    event: subscription.event,
                });
            }
            if !self.nodes.contains_key(&subscription.trigger.node_id) {
                return Err(GraphValidationError::MissingSubscriber {
                    event: subscription.event,
                    subscriber: subscription.trigger.node_id,
                });
            }
        }

        Ok(())
    }

    /// Validate this graph as one a host may compile in `scope`: the
    /// structure, then the load contract — every node resolves, a compound's
    /// graph holds one boundary node a side, every binding and subscription is
    /// admitted, and no wire closes a data cycle. What lies across graphs, the
    /// recursion and the bounds, is
    /// [`GraphSet::validate_with`](crate::GraphSet::validate_with)'s.
    ///
    /// What a load establishes and the edit gate keeps, so a failure here is
    /// the host's bug: a compile asserts it.
    pub fn validate_with(&self, scope: Scope<'_>) -> ValidationResult<()> {
        self.validate()?;
        for (node_id, node) in &self.nodes {
            if let Err(reason) = scope.resolve(node) {
                return Err(GraphValidationError::Unresolved {
                    node_id: *node_id,
                    reason,
                });
            }
        }
        if scope.owner().is_some() {
            if self.boundary_nodes(Boundary::Inputs).next().is_none() {
                return Err(GraphValidationError::NoInputsNode);
            }
            let outputs = self.boundary_nodes(Boundary::Outputs).count();
            if outputs != 1 {
                return Err(GraphValidationError::OutputsCount { count: outputs });
            }
        }
        for (port, binding) in &self.wiring.bindings {
            self.admit(scope, *port, binding).map_err(|rejection| {
                GraphValidationError::RejectedBinding {
                    port: *port,
                    rejection,
                }
            })?;
        }
        for subscription in self.wiring.subscriptions() {
            self.admit_subscription(scope, subscription)
                .map_err(|rejection| GraphValidationError::RejectedSubscription {
                    subscription,
                    rejection,
                })?;
        }
        if let Some(&port) = self.closing_wires().first() {
            return Err(GraphValidationError::Cycle { port });
        }
        Ok(())
    }

    /// Make a ghost node of each node of `refused`, which keeps its reason and what it declared,
    /// in the order given, and a ghost of each binding and subscription that touched any of them,
    /// taken in one pass of each table; report each node.
    pub(crate) fn detach_into_ghosts(
        &mut self,
        refused: Vec<Detaching>,
        repairs: &mut Vec<Repair>,
    ) {
        let mut node_ids: Vec<NodeId> = refused.iter().map(|detaching| detaching.node_id).collect();
        node_ids.sort_unstable();
        node_ids.dedup();
        assert_eq!(node_ids.len(), refused.len(), "a node is refused once");
        let Wiring {
            bindings,
            subscriptions,
        } = self.wiring.remove_touching(&node_ids);
        for (port, binding) in bindings {
            self.ghost_wiring
                .bindings
                .insert(port, GhostBinding::from(binding));
        }
        for subscription in subscriptions {
            self.ghost_wiring
                .subscriptions
                .insert(subscription, GhostSubscription::default());
        }
        for Detaching {
            node_id,
            reason,
            saved,
        } in refused
        {
            let node = self
                .nodes
                .remove(&node_id)
                .expect("a node to detach is in the graph");
            self.ghost_nodes.insert(
                node_id,
                GhostNode {
                    node,
                    reason,
                    saved,
                },
            );
            repairs.push(Repair::GhostedNode { node_id, reason });
        }
    }

    /// Each binding and subscription `scope` refuses, the bindings first, each in the order the
    /// graph keeps it: what [`GraphLoad::ghost_refused`](load::GraphLoad::ghost_refused) makes
    /// ghosts of.
    pub(crate) fn refused_wiring(&self, scope: Scope<'_>) -> Vec<RefusedWiring> {
        let bindings = self.wiring.bindings.iter().filter_map(|(port, binding)| {
            self.admit(scope, *port, binding)
                .err()
                .map(|rejection| RefusedWiring::Binding {
                    port: *port,
                    rejection,
                })
        });
        let subscriptions = self.wiring.subscriptions().filter_map(|subscription| {
            self.admit_subscription(scope, subscription)
                .err()
                .map(|rejection| RefusedWiring::Subscription {
                    subscription,
                    rejection,
                })
        });
        bindings.chain(subscriptions).collect()
    }

    /// Make a ghost of the live binding on `port`, and report it with `rejection`.
    fn ghost_binding(
        &mut self,
        port: InputPort,
        rejection: BindRejection,
        repairs: &mut Vec<Repair>,
    ) {
        let binding = self
            .wiring
            .bindings
            .remove(&port)
            .expect("a binding to ghost is in the graph");
        self.ghost_wiring
            .bindings
            .insert(port, GhostBinding::from(binding.clone()));
        repairs.push(Repair::GhostedBinding {
            port,
            binding,
            rejection,
        });
    }

    /// The name and type the input `port` shows, as far as anything knows: its node's
    /// declaration's, else `saved`, the port a ghost on it holds, else its ghost node's saved
    /// declaration's. `None` where none knows it.
    pub fn shown_input<'a>(
        &'a self,
        scope: Scope<'a>,
        port: InputPort,
        saved: Option<&'a SavedPort>,
    ) -> Option<ShownPort<'a>> {
        let declared = self.find(port.node_id).and_then(|node| {
            let input = scope.decl(node).ports().input(port.port)?;
            Some(ShownPort {
                name: &input.name,
                data_type: input.data_type,
            })
        });
        declared.or_else(|| saved.map(ShownPort::from)).or_else(|| {
            let input = self.ghost_saved(port.node_id)?.input(port.port)?;
            Some(ShownPort {
                name: &input.name,
                data_type: input.data_type,
            })
        })
    }

    /// [`Self::shown_input`] for an output.
    pub fn shown_output<'a>(
        &'a self,
        scope: Scope<'a>,
        port: OutputPort,
        saved: Option<&'a SavedPort>,
    ) -> Option<ShownPort<'a>> {
        let declared = self.find(port.node_id).and_then(|node| {
            let output = scope.decl(node).ports().output(port.port)?;
            Some(ShownPort {
                name: &output.name,
                data_type: output.data_type,
            })
        });
        declared.or_else(|| saved.map(ShownPort::from)).or_else(|| {
            self.ghost_saved(port.node_id)?
                .output(port.port)
                .map(ShownPort::from)
        })
    }

    /// [`Self::shown_input`] for an event: its name.
    pub fn shown_event<'a>(
        &'a self,
        scope: Scope<'a>,
        port: EventPort,
        saved: Option<&'a SavedEvent>,
    ) -> Option<&'a str> {
        let declared = self
            .find(port.node_id)
            .and_then(|node| Some(scope.decl(node).event(port.event)?.name.as_str()));
        declared
            .or_else(|| saved.map(|saved| saved.name.as_str()))
            .or_else(|| {
                self.ghost_saved(port.node_id)?
                    .event(port.event)
                    .map(|event| event.name.as_str())
            })
    }

    /// What the ghost node `node_id` declared, if the graph keeps one that knows.
    fn ghost_saved(&self, node_id: NodeId) -> Option<&SavedDecl> {
        self.ghost_nodes.get(&node_id)?.saved.as_ref()
    }

    /// Each wire that returns to a node on the current path of a depth-first
    /// walk from every node in id order, following wires from consumer to
    /// producer: removing them all leaves the graph acyclic, and the id order
    /// makes the choice the same on every load.
    fn closing_wires(&self) -> Vec<InputPort> {
        dfs::back_edges(self.nodes.keys().copied(), |node_id, next| {
            next.extend(self.wires_into(node_id));
        })
        .closing
    }

    /// Each wire into `node_id`'s inputs, as the input it lands on and the node it reads.
    fn wires_into(&self, node_id: NodeId) -> impl Iterator<Item = (InputPort, NodeId)> + '_ {
        self.wiring
            .bindings_on(node_id)
            .filter_map(|(port, binding)| match binding {
                Binding::Wire(source) => Some((port, source.node_id)),
                Binding::Const(_) => None,
            })
    }

    /// Each instance and the compound it places, in node-id order.
    pub fn placed(&self) -> impl Iterator<Item = (NodeId, CompoundId)> + '_ {
        self.iter()
            .filter_map(|node| Some((node.node_id, node.compound()?)))
    }

    /// The boundary nodes on `side`, in node-id order.
    pub fn boundary_nodes(&self, side: Boundary) -> impl Iterator<Item = NodeId> + '_ {
        self.iter()
            .filter(move |node| node.boundary() == Some(side))
            .map(|node| node.node_id)
    }

    /// The `Inputs` node of least id in a compound's graph: a new compound's one.
    ///
    /// # Panics
    /// If the graph holds none, which a compound a host keeps never does.
    pub fn inputs_node(&self) -> NodeId {
        self.boundary_nodes(Boundary::Inputs)
            .next()
            .expect("a compound's graph holds an Inputs node")
    }

    /// The `Outputs` node of a compound's graph.
    ///
    /// # Panics
    /// If the graph holds none, which a compound a host keeps never does.
    pub fn outputs_node(&self) -> NodeId {
        self.boundary_nodes(Boundary::Outputs)
            .next()
            .expect("a compound's graph holds an Outputs node")
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::decl::func::Func;
    use crate::graph::Graph;
    use crate::graph::compound::Compound;
    use crate::graph::node::Node;
    use crate::identity::{CompoundId, NodeId};

    /// A test's shorthand for [`Graph::insert_seeded`] under a fresh id.
    impl Graph {
        /// Add a node of `func` with its inputs' defaults, under a fresh id.
        pub fn add_func_node(&mut self, func: &Func) -> NodeId {
            let node_id = NodeId::unique();
            self.insert_seeded(node_id, Node::from(func), func.ports());
            node_id
        }

        /// Add an instance of `compound`, held under `id`, with its inputs' defaults, under a fresh
        /// id.
        pub fn add_instance(&mut self, id: CompoundId, compound: &Compound) -> NodeId {
            let node_id = NodeId::unique();
            self.insert_seeded(node_id, Node::instance(id), compound.decl.interface.ports());
            node_id
        }
    }
}

#[cfg(test)]
mod tests;
