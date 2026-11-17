//! The authoring graph: the [`Graph`] type, the satellite types it owns
//! ([`Node`], [`Binding`], [`CacheMode`](node::CacheMode), …), and **every method a `Graph`
//! has**, in one impl, so the whole surface is one file to read.
//!
//! The modules beside it keep what those methods work *with*: [`identity`]
//! how a node or port is named, [`ports`] the inputs and outputs a declaration
//! shows, [`detached`] the reversible-removal records,
//! [`ghost_node`] a node the graph cannot hold, [`ghost_wiring`] the wiring it cannot hold,
//! [`error`] what validation and admission reject,
//! [`reconciled`] what a load repaired, [`subgraph`] and [`graph_set`] the graphs a document
//! holds, and [`scope`] what a graph's nodes resolve against.
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
//!   itself, a trigger on the `Inputs` node: no library brings those back.
//! - **A constant equal to the default its port declared is not kept**, Unreal's "non-default
//!   value": a func that loses an input would otherwise leave a ghost on every node of it, each
//!   holding the default nobody set. The input then takes the default the port declares now, if
//!   the node still has the port. A ghost node keeps every constant: it is the node, whole.
//! - **A port a func gained takes its default on each saved node**, when the file's
//!   [`Manifest`] shows the port is new and the input holds nothing. An updated node is then
//!   what a new node of the func is, with what you set kept. A func the manifest does not list
//!   gains nothing: nothing says which of its ports are new, and a guess would overwrite an
//!   input you cleared.
//! - **Nothing matches by name.** A port is its id; a ghost moves to another port only by an
//!   edit. A saved name is what a ghost shows, never what a load binds by.
//! - **A ghost becomes live at a load, or when a host asks, through one rule.**
//!   [`Graph::admit_ghost`] is [`Graph::admit`] plus the loop check; the load restores each
//!   ghost it passes, in port order. Refused: restoring after each edit, which rewires the graph
//!   behind an unrelated action.
//! - **A repair reads, then writes.** Each step of a load asks a graph what to change under a
//!   scope, drops the scope, and changes it. Refused: taking a graph out of its set for the call,
//!   which leaves the set wrong while a scope reads it.
//! - **The invariant is the host's contract, not a type.** `bindings` is a
//!   public field and a graph holds no library, so no type can enforce
//!   admission; the host's edit gate is the only way an edit lands.
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
//!   an `Any` output; the `Inputs` node of a subgraph shows one for a port made
//!   from an `Any` input, held as declared, which feeds only `Any` inputs.
//!   Refused: wildcard outputs, whose retype either removes wires you did not
//!   touch or refuses the edit until you remove them by hand. Reroute nodes
//!   need a new design.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use ::serde::{Deserialize, Serialize};
use hashbrown::{HashMap, HashSet};

use crate::data::const_value::ConstValue;
use crate::graph::binding::Binding;
use crate::graph::binding_entry::BindingEntry;
use crate::graph::detached::{DetachedGhost, DetachedNode};
use crate::graph::error::{
    BindingRejection, DetachReason, GhostRejection, GhostSubscriptionRejection,
    GraphValidationError, SubscriptionRejection, ValidationResult,
};
use crate::graph::func::Func;
use crate::graph::ghost_node::GhostNode;
use crate::graph::ghost_wiring::{
    GhostBinding, GhostSource, GhostSubscription, GhostWiring, InputState, SubscriptionState,
};
use crate::graph::identity::{
    EventPort, InputPort, NodeId, OutputPort, PortId, SubgraphId, TriggerPort,
};
use crate::graph::manifest::Manifest;
use crate::graph::node::{Boundary, Node, NodeKind};
use crate::graph::node_decl::NodeDecl;
use crate::graph::node_ref::NodeRef;
use crate::graph::reconciled::{Reconciled, Repair};
use crate::graph::saved_decl::{SavedDecl, SavedEvent, SavedInput, SavedPort, ShownPort};
use crate::graph::scope::Scope;
use crate::graph::subgraph::Subgraph;
use crate::graph::subscription::Subscription;

pub(crate) mod binding;
pub(crate) mod binding_entry;
pub(crate) mod detached;
pub(crate) mod error;
pub(crate) mod func;
pub(crate) mod ghost_node;
pub(crate) mod ghost_wiring;
pub(crate) mod graph_set;
pub(crate) mod identity;
pub(crate) mod instance_path;
pub(crate) mod interface;
pub(crate) mod interface_change;
pub(crate) mod manifest;
pub(crate) mod node;
pub(crate) mod node_decl;
pub(crate) mod node_ref;
pub(crate) mod port_map;
pub(crate) mod ports;
pub(crate) mod program_id;
pub(crate) mod reconciled;
pub(crate) mod saved_decl;
pub(crate) mod scope;
mod serde;
pub(crate) mod subgraph;
pub(crate) mod subscription;
pub(crate) mod type_names;

/// `Clone` keeps every node id as it is, so a copy is sound only where it never lives beside its
/// original in one document, as the copy a save rewrites and writes; an editor's duplicate mints
/// fresh ids instead.
///
/// `bindings` is a plain map a caller keys into directly, so it is public;
/// `nodes`, `subscriptions` and the ghosts are reached only through the methods below.
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
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

    /// Data wiring, keyed by consumer input port; absence means unbound. A `BTreeMap` keeps
    /// serialization deterministic and a node's ports contiguous. Serialized as `(port, binding)`
    /// pairs decoded entry by entry, so a document that repeats a port is refused rather than
    /// loaded with only the last binding.
    #[serde(
        default,
        serialize_with = "crate::graph::serde::serialize_pairs",
        deserialize_with = "crate::graph::serde::deserialize_bindings"
    )]
    pub bindings: BTreeMap<InputPort, Binding>,

    /// Event wiring: every (emitter event → subscriber) edge, flat. A `BTreeSet` dedups and keeps
    /// serialization and compile order deterministic.
    #[serde(default)]
    subscriptions: BTreeSet<Subscription>,

    /// The nodes this graph cannot hold, by id, their ids disjoint from `nodes`'. Decoded entry
    /// by entry, so a document that repeats an id is refused.
    #[serde(
        default,
        deserialize_with = "crate::graph::serde::deserialize_ghost_nodes"
    )]
    ghost_nodes: BTreeMap<NodeId, GhostNode>,

    /// The bindings and subscriptions this graph cannot hold, their keys disjoint from the live
    /// tables'. Each end of each is a node or a ghost node of this graph.
    #[serde(default)]
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

/// What each node of one graph declared before this load: the saved declaration of a ghost node
/// the load attached, else its func's in the file's manifest.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Before<'a> {
    pub(crate) manifest: &'a Manifest,
    /// The saved declaration of each ghost node this load attached, by node.
    pub(crate) returned: &'a HashMap<NodeId, SavedDecl>,
}

impl<'a> Before<'a> {
    fn of(self, node_id: NodeId, node: &Node) -> Option<&'a SavedDecl> {
        self.returned
            .get(&node_id)
            .or_else(|| self.manifest.func(node.declared_func()?))
    }
}

/// What a load changes of the ghosts a graph keeps, read under its scope: which constants go, as
/// the default their port declared, and the saved port each remaining ghost holds for each end.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GhostNotes {
    defaults: Vec<InputPort>,
    bindings: Vec<BindingNote>,
    subscriptions: Vec<SubscriptionNote>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BindingNote {
    port: InputPort,
    input: Option<SavedPort>,
    output: Option<SavedPort>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SubscriptionNote {
    subscription: Subscription,
    event: Option<SavedEvent>,
}

/// The ghosts a scope admits: what a read of a graph under its scope answers, which the write
/// then makes live.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AdmittedGhosts {
    bindings: Vec<InputPort>,
    subscriptions: Vec<Subscription>,
}

impl AdmittedGhosts {
    /// Keep the ghosts that touch `node_id` alone, as an edit of one node restores no other.
    pub(crate) fn retain_touching(&mut self, node_id: NodeId) {
        self.bindings.retain(|port| port.node_id == node_id);
        self.subscriptions
            .retain(|subscription| subscription.touches(node_id));
    }
}

/// Wiring a scope refuses: what a read of a graph under its scope answers, which the write then
/// makes ghosts of.
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
            wiring: self.ghost_wiring.touching(node_id),
        })
    }

    /// Take the ghost node `node_id` out with each ghost that touches it.
    ///
    /// # Panics
    /// If this graph keeps no such ghost node.
    pub fn detach_ghost_node(&mut self, node_id: NodeId) -> DetachedGhost {
        let detached = self
            .snapshot_ghost_node(node_id)
            .expect("cannot detach a ghost node that is not in the graph");
        self.ghost_nodes.remove(&node_id);
        self.ghost_wiring.remove_touching(node_id);
        detached
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
        self.insert_ghost_node(node_id, ghost);
        self.put_ghost_wiring(wiring);
    }

    /// Keep each ghost of `wiring`.
    ///
    /// # Panics
    /// If an input one goes on is bound or holds a ghost, or a subscription one is of is made or
    /// kept.
    fn put_ghost_wiring(&mut self, wiring: GhostWiring) {
        for (port, ghost) in wiring.bindings {
            assert!(
                !self.bindings.contains_key(&port),
                "a ghost goes back on an unbound input"
            );
            let prev = self.ghost_wiring.bindings.insert(port, ghost);
            assert!(prev.is_none(), "a ghost goes back on an input with none");
        }
        for (subscription, ghost) in wiring.subscriptions {
            assert!(
                !self.subscriptions.contains(&subscription),
                "a ghost goes back of a subscription not made"
            );
            let prev = self.ghost_wiring.subscriptions.insert(subscription, ghost);
            assert!(
                prev.is_none(),
                "a ghost goes back of a subscription not kept"
            );
        }
    }

    /// Each constant a binding holds, live or ghost, with the input it is bound to.
    pub fn constants(&self) -> impl Iterator<Item = (InputPort, &ConstValue)> {
        let live = self
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
            self.bindings.get(&port),
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
        match self.bindings.get(&port) {
            Some(Binding::Wire(output)) => Some(*output),
            Some(Binding::Const(_)) => None,
            None => self.ghost_wiring.binding(port)?.output(),
        }
    }

    /// Make `port` hold `state`, in place of what it held, live or ghost.
    pub fn set_input_state(&mut self, port: InputPort, state: InputState) {
        self.bindings.remove(&port);
        self.ghost_wiring.bindings.remove(&port);
        match state {
            InputState::Unbound => {}
            InputState::Live(binding) => {
                self.bindings.insert(port, binding);
            }
            InputState::Ghost(ghost) => {
                self.ghost_wiring.bindings.insert(port, ghost);
            }
        }
    }

    /// Whether `subscription` is made, kept as a ghost, or neither.
    pub fn subscription_state(&self, subscription: Subscription) -> SubscriptionState {
        if self.subscriptions.contains(&subscription) {
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
        self.subscriptions.remove(&subscription);
        self.ghost_wiring.subscriptions.remove(&subscription);
        match state {
            SubscriptionState::Absent => {}
            SubscriptionState::Live => {
                self.subscriptions.insert(subscription);
            }
            SubscriptionState::Ghost(ghost) => {
                self.ghost_wiring.subscriptions.insert(subscription, ghost);
            }
        }
    }

    /// Whether this graph can hold the ghost on `port` now: both of its nodes are nodes, not
    /// ghost nodes, [`Self::admit`] passes, and a wire closes no data cycle. The rule a load and
    /// a host's Restore hold a ghost to.
    ///
    /// # Panics
    /// If `port` holds no ghost.
    pub fn admit_ghost(&self, scope: Scope<'_>, port: InputPort) -> Result<(), GhostRejection> {
        let ghost = self
            .ghost_wiring
            .binding(port)
            .expect("an admitted ghost is kept on its input");
        self.admit_kept(scope, port, &ghost.binding())?;
        match ghost.output() {
            Some(output) if self.produces_cycle(output.node_id, port.node_id) => {
                Err(GhostRejection::Cycle)
            }
            _ => Ok(()),
        }
    }

    /// [`Self::admit_ghost`] without the loop check, which reads the graph alone: whether both
    /// nodes of `binding` on `port` are nodes and the scope admits it.
    fn admit_kept(
        &self,
        scope: Scope<'_>,
        port: InputPort,
        binding: &Binding,
    ) -> Result<(), GhostRejection> {
        let mut ends = [Some(port.node_id), None];
        if let Binding::Wire(source) = binding {
            ends[1] = Some(source.node_id);
        }
        if let Some(node_id) = ends
            .into_iter()
            .flatten()
            .find(|node_id| !self.nodes.contains_key(node_id))
        {
            return Err(GhostRejection::GhostNode { node_id });
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

    /// The bindings on `node_id`'s own inputs, in port-id order: one contiguous
    /// range, as `InputPort` orders by node before port.
    pub fn input_bindings(&self, node_id: NodeId) -> impl Iterator<Item = (InputPort, &Binding)> {
        self.bindings
            .range(InputPort::new(node_id, PortId::MIN)..)
            .take_while(move |(port, _)| port.node_id == node_id)
            .map(|(port, binding)| (*port, binding))
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
        let consumer = self
            .find(port.node_id)
            .expect("an admitted binding's consumer is in the graph")
            .decl(scope);
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
                let producer = self
                    .find(source.node_id)
                    .expect("an admitted wire's producer is in the graph")
                    .decl(scope);
                let output = producer
                    .ports()
                    .output(source.port)
                    .ok_or(BindingRejection::UndeclaredOutput)?;
                if input.const_only && !producer.output_is_const(source.port) {
                    return Err(BindingRejection::WiredConstOnly);
                }
                if scope
                    .library()
                    .wire_satisfies(input.data_type, output.data_type)
                {
                    Ok(())
                } else {
                    Err(BindingRejection::WireMismatch)
                }
            }
        }
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
        let Subscription { event, trigger } = subscription;
        if event.node_id == trigger.node_id {
            return Err(SubscriptionRejection::SelfSubscription);
        }
        let subscriber = self
            .find(trigger.node_id)
            .expect("an admitted subscription's subscriber is in the graph");
        if subscriber.kind == NodeKind::Boundary(Boundary::Inputs) {
            return Err(SubscriptionRejection::InputsSubscriber);
        }
        if !subscriber.decl(scope).triggers().contains(trigger.trigger) {
            return Err(SubscriptionRejection::UndeclaredTrigger);
        }
        let emitter = self
            .find(event.node_id)
            .expect("an admitted subscription's emitter is in the graph")
            .decl(scope);
        if emitter.event(event.event).is_some() {
            Ok(())
        } else {
            Err(SubscriptionRejection::UndeclaredEvent)
        }
    }
    /// Every wire as (consumer input ← producer output). Const bindings
    /// are not edges and are skipped.
    pub fn wires(&self) -> impl Iterator<Item = (InputPort, OutputPort)> + '_ {
        self.bindings
            .iter()
            .filter_map(|(dst, binding)| match binding {
                Binding::Wire(src) => Some((*dst, *src)),
                Binding::Const(_) => None,
            })
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
    /// "what feeds this node" is one range of `bindings`; the other way needs a reversed index
    /// over every binding.
    pub fn produces_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        if producer == consumer {
            return true;
        }
        let mut stack = vec![producer];
        let mut seen: HashSet<NodeId> = HashSet::from_iter([producer]);
        while let Some(node) = stack.pop() {
            for (_, binding) in self.input_bindings(node) {
                let Binding::Wire(source) = binding else {
                    continue;
                };
                if source.node_id == consumer {
                    return true;
                }
                if seen.insert(source.node_id) {
                    stack.push(source.node_id);
                }
            }
        }
        false
    }
    pub fn snapshot_node(&self, node_id: NodeId) -> Option<DetachedNode> {
        let node = self.find(node_id)?.clone();
        Some(DetachedNode {
            node_id,
            node,
            bindings: self.bindings_touching(node_id),
            subscriptions: self
                .subscriptions
                .iter()
                .copied()
                .filter(|subscription| subscription.touches(node_id))
                .collect(),
            ghost_wiring: self.ghost_wiring.touching(node_id),
        })
    }
    pub fn is_subscribed(&self, event: EventPort, subscriber: NodeId) -> bool {
        self.subscriptions
            .contains(&Subscription::new(event, subscriber))
    }
    fn bindings_touching(&self, node_id: NodeId) -> Vec<BindingEntry> {
        self.bindings
            .iter()
            .filter(|(port, binding)| binding.touches(**port, node_id))
            .map(|(port, binding)| BindingEntry {
                port: *port,
                binding: binding.clone(),
            })
            .collect()
    }
    pub fn subscriptions(&self) -> impl ExactSizeIterator<Item = Subscription> + '_ {
        self.subscriptions.iter().copied()
    }
    /// The subscriptions to `emitter`'s events, by event, then subscriber: one contiguous range,
    /// as a subscription orders by its event's node first.
    pub(crate) fn subscriptions_of(
        &self,
        emitter: NodeId,
    ) -> impl Iterator<Item = Subscription> + '_ {
        self.subscriptions
            .range(Subscription::new(EventPort::new(emitter, PortId::MIN), NodeId::nil())..)
            .take_while(move |subscription| subscription.event.node_id == emitter)
            .copied()
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

    /// Add a func instance and seed its inputs' default const bindings.
    /// Returns the new node id.
    pub fn add_func_node(&mut self, func: &Func) -> NodeId {
        let node_id = self.add(Node::from(func));
        self.bindings.extend(
            func.ports()
                .default_bindings(node_id)
                .map(|entry| (entry.port, entry.binding)),
        );
        node_id
    }

    /// Add an instance of `subgraph`, held under `id`, and seed its inputs' default const
    /// bindings. Returns the new node id.
    pub fn add_instance(&mut self, id: SubgraphId, subgraph: &Subgraph) -> NodeId {
        let node_id = self.add(Node::instance(id));
        self.bindings.extend(
            subgraph
                .interface
                .ports()
                .default_bindings(node_id)
                .map(|entry| (entry.port, entry.binding)),
        );
        node_id
    }

    pub fn detach_node(&mut self, node_id: NodeId) -> DetachedNode {
        let detached = self
            .snapshot_node(node_id)
            .expect("cannot detach a node that is not in the graph");
        self.nodes.remove(&node_id);
        self.bindings
            .retain(|port, binding| !binding.touches(*port, node_id));
        self.subscriptions
            .retain(|subscription| !subscription.touches(node_id));
        self.ghost_wiring.remove_touching(node_id);
        detached
    }
    pub fn attach_node(&mut self, detached: DetachedNode) {
        assert!(
            !self.nodes.contains_key(&detached.node_id),
            "cannot attach a node that is already in the graph"
        );
        assert!(
            detached
                .bindings
                .iter()
                .all(|entry| !self.bindings.contains_key(&entry.port)),
            "cannot attach over bindings created after detachment"
        );
        assert!(
            detached
                .subscriptions
                .iter()
                .all(|subscription| !self.subscriptions.contains(subscription)),
            "cannot attach over subscriptions created after detachment"
        );

        let DetachedNode {
            node_id,
            node,
            bindings,
            subscriptions,
            ghost_wiring,
        } = detached;
        self.insert(node_id, node);
        self.bindings.extend(
            bindings
                .into_iter()
                .map(|entry| (entry.port, entry.binding)),
        );
        self.subscriptions.extend(subscriptions);
        self.put_ghost_wiring(ghost_wiring);
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
        if self.nodes.contains_key(&NodeId::nil()) {
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
            if self.bindings.contains_key(&port) {
                return Err(GraphValidationError::GhostOnBoundInput { port });
            }
        }
        for (subscription, _) in self.ghost_wiring.subscriptions() {
            let ends = [subscription.event.node_id, subscription.trigger.node_id];
            if let Some(node_id) = ends.into_iter().find(|id| !self.holds(*id)) {
                return Err(GraphValidationError::GhostMissingNode { node_id });
            }
            if self.subscriptions.contains(&subscription) {
                return Err(GraphValidationError::GhostOfSubscription { subscription });
            }
        }

        for (destination, binding) in &self.bindings {
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

        for subscription in &self.subscriptions {
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
    /// structure, then the load contract — every node resolves, a subgraph's
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
            let count = |boundary| {
                self.nodes
                    .values()
                    .filter(|node| node.boundary() == Some(boundary))
                    .count()
            };
            if count(Boundary::Inputs) == 0 {
                return Err(GraphValidationError::NoInputsNode);
            }
            let outputs = count(Boundary::Outputs);
            if outputs != 1 {
                return Err(GraphValidationError::OutputsCount { count: outputs });
            }
        }
        for (port, binding) in &self.bindings {
            self.admit(scope, *port, binding).map_err(|rejection| {
                GraphValidationError::RejectedBinding {
                    port: *port,
                    rejection,
                }
            })?;
        }
        for subscription in self.subscriptions() {
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

    /// The repairs that need nothing but the graph itself: remove a node under the nil id, each
    /// ghost node saved under a node's id, the wiring, live and ghost, that names a node held as
    /// neither a node nor a ghost node, and each ghost saved where the live table holds the same
    /// key; make a ghost of live wiring that names a ghost node.
    pub(crate) fn repair_structure(&mut self, reconciled: &mut Reconciled) {
        let repairs = &mut reconciled.repairs;
        if self.nodes.remove(&NodeId::nil()).is_some() {
            repairs.push(Repair::NilNode);
        }
        let nodes = &self.nodes;
        self.ghost_nodes.retain(|node_id, _| {
            let kept = !nodes.contains_key(node_id);
            if !kept {
                repairs.push(Repair::DuplicateGhostNode { node_id: *node_id });
            }
            kept
        });
        let ghost_nodes = &self.ghost_nodes;
        let held =
            |node_id: NodeId| nodes.contains_key(&node_id) || ghost_nodes.contains_key(&node_id);

        let bindings = &self.bindings;
        self.ghost_wiring.bindings.retain(|port, ghost| {
            let ends_held =
                held(port.node_id) && ghost.output().is_none_or(|output| held(output.node_id));
            if !ends_held {
                repairs.push(Repair::DanglingBinding {
                    port: *port,
                    binding: ghost.binding(),
                });
            } else if bindings.contains_key(port) {
                repairs.push(Repair::GhostOnBoundInput {
                    port: *port,
                    binding: ghost.binding(),
                });
            }
            ends_held && !bindings.contains_key(port)
        });
        let subscriptions = &self.subscriptions;
        self.ghost_wiring.subscriptions.retain(|subscription, _| {
            let ends_held = held(subscription.event.node_id) && held(subscription.trigger.node_id);
            if !ends_held {
                repairs.push(Repair::DanglingSubscription {
                    subscription: *subscription,
                });
            } else if subscriptions.contains(subscription) {
                repairs.push(Repair::GhostOfSubscription {
                    subscription: *subscription,
                });
            }
            ends_held && !subscriptions.contains(subscription)
        });

        let ghost_wiring = &mut self.ghost_wiring;
        self.bindings.retain(|port, binding| {
            let ends = [
                Some(port.node_id),
                match binding {
                    Binding::Wire(source) => Some(source.node_id),
                    Binding::Const(_) => None,
                },
            ];
            let Some(off) = ends
                .into_iter()
                .flatten()
                .find(|node_id| !nodes.contains_key(node_id))
            else {
                return true;
            };
            if ends.into_iter().flatten().all(held) {
                ghost_wiring
                    .bindings
                    .insert(*port, GhostBinding::from(binding.clone()));
                repairs.push(Repair::GhostedBinding {
                    port: *port,
                    binding: binding.clone(),
                    rejection: GhostRejection::GhostNode { node_id: off },
                });
            } else {
                repairs.push(Repair::DanglingBinding {
                    port: *port,
                    binding: binding.clone(),
                });
            }
            false
        });
        self.subscriptions.retain(|subscription| {
            let ends = [subscription.event.node_id, subscription.trigger.node_id];
            let Some(off) = ends
                .into_iter()
                .find(|node_id| !nodes.contains_key(node_id))
            else {
                return true;
            };
            if ends.into_iter().all(held) {
                ghost_wiring
                    .subscriptions
                    .insert(*subscription, GhostSubscription::default());
                repairs.push(Repair::GhostedSubscription {
                    subscription: *subscription,
                    rejection: GhostSubscriptionRejection::GhostNode { node_id: off },
                });
            } else {
                repairs.push(Repair::DanglingSubscription {
                    subscription: *subscription,
                });
            }
            false
        });
    }

    /// The ghost nodes that resolve in `scope`, in node-id order: what [`Self::reattach`]
    /// attaches.
    pub(crate) fn resolving_ghost_nodes(&self, scope: Scope<'_>) -> Vec<NodeId> {
        self.ghost_nodes
            .iter()
            .filter(|(_, ghost)| scope.resolve(&ghost.node).is_ok())
            .map(|(node_id, _)| *node_id)
            .collect()
    }

    /// Attach each ghost node of `ghosts`, the node alone, and report each. Its wiring stays
    /// among the ghosts until [`Self::restore`] finds the graph can hold it. Hands back each
    /// ghost node as it was, for what it declared before.
    pub(crate) fn reattach(
        &mut self,
        ghosts: &[NodeId],
        reconciled: &mut Reconciled,
    ) -> Vec<GhostNode> {
        ghosts
            .iter()
            .map(|&node_id| {
                let ghost = self
                    .ghost_nodes
                    .remove(&node_id)
                    .expect("a ghost node to attach is in the graph");
                self.insert(node_id, ghost.node.clone());
                reconciled.repairs.push(Repair::Reattached { node_id });
                ghost
            })
            .collect()
    }

    /// Add the boundary node a subgraph's graph lacks on each side, under a fresh id.
    pub(crate) fn add_missing_boundaries(&mut self, reconciled: &mut Reconciled) {
        for boundary in [Boundary::Inputs, Boundary::Outputs] {
            if !self
                .nodes
                .values()
                .any(|node| node.boundary() == Some(boundary))
            {
                let node_id = self.add(Node::from(boundary));
                reconciled
                    .repairs
                    .push(Repair::BoundaryAdded { boundary, node_id });
            }
        }
    }

    /// `node_id` as a node `scope` takes out for `reason`, with what it declared: its
    /// declaration where it resolves, as a node refused for a rule across graphs does, else its
    /// func's saved one in `manifest`.
    ///
    /// # Panics
    /// If the graph holds no node `node_id`.
    pub(crate) fn detaching(
        &self,
        scope: Scope<'_>,
        manifest: &Manifest,
        node_id: NodeId,
        reason: DetachReason,
    ) -> Detaching {
        let node = self
            .find(node_id)
            .expect("a node to detach is in the graph");
        let saved = match scope.resolve(node) {
            Ok(decl) => Some(decl.saved()),
            Err(_) => node
                .declared_func()
                .and_then(|func_id| manifest.func(func_id))
                .cloned(),
        };
        Detaching {
            node_id,
            reason,
            saved,
        }
    }

    /// The nodes `scope` does not let this graph hold, in node-id order: each node it does not
    /// resolve, and in a subgraph's graph each `Outputs` node past the lowest id.
    pub(crate) fn unresolved(&self, scope: Scope<'_>, manifest: &Manifest) -> Vec<Detaching> {
        let mut refused: Vec<Detaching> = self
            .nodes
            .iter()
            .filter_map(|(node_id, node)| {
                let reason = DetachReason::from(scope.resolve(node).err()?);
                Some(self.detaching(scope, manifest, *node_id, reason))
            })
            .collect();
        if scope.owner().is_some() {
            let held = self
                .iter()
                .filter(|node| node.boundary() == Some(Boundary::Outputs))
                .map(|node| node.node_id);
            refused.extend(held.skip(1).map(|node_id| {
                self.detaching(scope, manifest, node_id, DetachReason::ExtraOutputs)
            }));
        }
        refused.sort_unstable_by_key(|detaching| detaching.node_id);
        refused
    }

    /// Make a ghost node of each node of `refused`, which keeps its reason and what it declared,
    /// in the order given, and a ghost of each binding and subscription that touched it; report
    /// each node.
    pub(crate) fn detach_into_ghosts(
        &mut self,
        refused: Vec<Detaching>,
        reconciled: &mut Reconciled,
    ) {
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
            let bindings: Vec<InputPort> = self
                .bindings
                .iter()
                .filter(|(port, binding)| binding.touches(**port, node_id))
                .map(|(port, _)| *port)
                .collect();
            for port in bindings {
                let binding = self.bindings.remove(&port).expect("listed above");
                self.ghost_wiring
                    .bindings
                    .insert(port, GhostBinding::from(binding));
            }
            let subscriptions: Vec<Subscription> = self
                .subscriptions
                .iter()
                .copied()
                .filter(|subscription| subscription.touches(node_id))
                .collect();
            for subscription in subscriptions {
                self.subscriptions.remove(&subscription);
                self.ghost_wiring
                    .subscriptions
                    .insert(subscription, GhostSubscription::default());
            }
            self.ghost_nodes.insert(
                node_id,
                GhostNode {
                    node,
                    reason,
                    saved,
                },
            );
            reconciled
                .repairs
                .push(Repair::GhostedNode { node_id, reason });
        }
    }

    /// Each binding and subscription `scope` refuses, the bindings first, each in the order the
    /// graph keeps it: what [`Self::ghost_refused`] makes ghosts of.
    pub(crate) fn refused_wiring(&self, scope: Scope<'_>) -> Vec<RefusedWiring> {
        let bindings = self.bindings.iter().filter_map(|(port, binding)| {
            self.admit(scope, *port, binding)
                .err()
                .map(|rejection| RefusedWiring::Binding {
                    port: *port,
                    rejection,
                })
        });
        let subscriptions = self.subscriptions().filter_map(|subscription| {
            self.admit_subscription(scope, subscription)
                .err()
                .map(|rejection| RefusedWiring::Subscription {
                    subscription,
                    rejection,
                })
        });
        bindings.chain(subscriptions).collect()
    }

    /// Make a ghost of each piece of `refused`, then of each wire that closes a data cycle, and
    /// report each. A subscription no library can bring back, a node's to its own event or one
    /// on the `Inputs` node, is removed.
    pub(crate) fn ghost_refused(&mut self, refused: &[RefusedWiring], reconciled: &mut Reconciled) {
        for &refusal in refused {
            match refusal {
                RefusedWiring::Binding { port, rejection } => {
                    self.ghost_binding(port, GhostRejection::Binding(rejection), reconciled);
                }
                RefusedWiring::Subscription {
                    subscription,
                    rejection,
                } => {
                    self.subscriptions.remove(&subscription);
                    reconciled.repairs.push(match rejection {
                        // A declaration that loses the event or the trigger may regain it.
                        SubscriptionRejection::UndeclaredEvent
                        | SubscriptionRejection::UndeclaredTrigger => {
                            self.ghost_wiring
                                .subscriptions
                                .insert(subscription, GhostSubscription::default());
                            Repair::GhostedSubscription {
                                subscription,
                                rejection: GhostSubscriptionRejection::Subscription(rejection),
                            }
                        }
                        SubscriptionRejection::SelfSubscription
                        | SubscriptionRejection::InputsSubscriber => Repair::RejectedSubscription {
                            subscription,
                            rejection,
                        },
                    });
                }
            }
        }
        for port in self.closing_wires() {
            self.ghost_binding(port, GhostRejection::Cycle, reconciled);
        }
    }

    /// Make a ghost of the live binding on `port`, and report it with `rejection`.
    fn ghost_binding(
        &mut self,
        port: InputPort,
        rejection: GhostRejection,
        reconciled: &mut Reconciled,
    ) {
        let binding = self
            .bindings
            .remove(&port)
            .expect("a binding to ghost is in the graph");
        self.ghost_wiring
            .bindings
            .insert(port, GhostBinding::from(binding.clone()));
        reconciled.repairs.push(Repair::GhostedBinding {
            port,
            binding,
            rejection,
        });
    }

    /// The ghosts `scope` admits, the loop check aside, each in the order the graph keeps it:
    /// what [`Self::restore`] makes live.
    pub(crate) fn admitted_ghosts(&self, scope: Scope<'_>) -> AdmittedGhosts {
        AdmittedGhosts {
            bindings: self
                .ghost_wiring
                .bindings()
                .filter(|(port, ghost)| self.admit_kept(scope, *port, &ghost.binding()).is_ok())
                .map(|(port, _)| port)
                .collect(),
            subscriptions: self
                .ghost_wiring
                .subscriptions()
                .filter(|(subscription, _)| {
                    self.admit_ghost_subscription(scope, *subscription).is_ok()
                })
                .map(|(subscription, _)| subscription)
                .collect(),
        }
    }

    /// Make each ghost of `admitted` live, a wire only while it closes no data cycle among the
    /// wires live before it, and report each.
    pub(crate) fn restore(&mut self, admitted: &AdmittedGhosts, reconciled: &mut Reconciled) {
        for &port in &admitted.bindings {
            let ghost = &self.ghost_wiring.bindings[&port];
            if ghost
                .output()
                .is_some_and(|output| self.produces_cycle(output.node_id, port.node_id))
            {
                continue;
            }
            let binding = ghost.binding();
            self.set_input_state(port, InputState::Live(binding.clone()));
            reconciled
                .repairs
                .push(Repair::RestoredBinding { port, binding });
        }
        for &subscription in &admitted.subscriptions {
            self.set_subscription_state(subscription, SubscriptionState::Live);
            reconciled
                .repairs
                .push(Repair::RestoredSubscription { subscription });
        }
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
            let input = node.decl(scope).ports().input(port.port)?;
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
            let output = node.decl(scope).ports().output(port.port)?;
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

    /// What a load changes of the ghosts this graph still keeps, read under `scope` against
    /// what each node declared `before`: a constant on an input of a node of the graph that is
    /// the default the input declared goes, as nobody set it; each other ghost holds the saved
    /// port of each end no declaration the graph can read gives, a node's or a ghost node's
    /// saved one, taken from `before` where the ghost has none yet.
    pub(crate) fn ghost_notes(&self, scope: Scope<'_>, before: Before<'_>) -> GhostNotes {
        let mut notes = GhostNotes::default();
        for (port, ghost) in self.ghost_wiring.bindings() {
            let consumer = self.nodes.get(&port.node_id);
            let before_input = consumer
                .and_then(|node| before.of(port.node_id, node))
                .and_then(|decl| decl.input(port.port));
            if let (GhostSource::Const(value), Some(declared)) = (&ghost.source, before_input)
                && declared.default_value.as_ref() == Some(value)
            {
                notes.defaults.push(port);
                continue;
            }
            let input = if self.reads_input(scope, port) {
                None
            } else {
                ghost
                    .input
                    .clone()
                    .or_else(|| before_input.map(SavedInput::port))
            };
            let output = match &ghost.source {
                GhostSource::Const(_) => None,
                GhostSource::Wire { output, saved } => {
                    if self.reads_output(scope, *output) {
                        None
                    } else {
                        saved.clone().or_else(|| {
                            let node = self.nodes.get(&output.node_id)?;
                            before
                                .of(output.node_id, node)?
                                .output(output.port)
                                .cloned()
                        })
                    }
                }
            };
            let held = match &ghost.source {
                GhostSource::Const(_) => None,
                GhostSource::Wire { saved, .. } => saved.as_ref(),
            };
            if input != ghost.input || output.as_ref() != held {
                notes.bindings.push(BindingNote {
                    port,
                    input,
                    output,
                });
            }
        }
        for (subscription, ghost) in self.ghost_wiring.subscriptions() {
            let event_port = subscription.event;
            let event = if self.reads_event(scope, event_port) {
                None
            } else {
                ghost.event.clone().or_else(|| {
                    let node = self.nodes.get(&event_port.node_id)?;
                    before
                        .of(event_port.node_id, node)?
                        .event(event_port.event)
                        .cloned()
                })
            };
            if event != ghost.event {
                notes.subscriptions.push(SubscriptionNote {
                    subscription,
                    event,
                });
            }
        }
        notes
    }

    /// Whether a declaration this graph can read gives the input `port`: its node's, under
    /// `scope`, or its ghost node's saved one.
    fn reads_input(&self, scope: Scope<'_>, port: InputPort) -> bool {
        match self.nodes.get(&port.node_id) {
            Some(node) => node.decl(scope).ports().input(port.port).is_some(),
            None => self
                .ghost_saved(port.node_id)
                .is_some_and(|saved| saved.input(port.port).is_some()),
        }
    }

    /// [`Self::reads_input`] for an output.
    fn reads_output(&self, scope: Scope<'_>, port: OutputPort) -> bool {
        match self.nodes.get(&port.node_id) {
            Some(node) => node.decl(scope).ports().output(port.port).is_some(),
            None => self
                .ghost_saved(port.node_id)
                .is_some_and(|saved| saved.output(port.port).is_some()),
        }
    }

    /// [`Self::reads_input`] for an event.
    fn reads_event(&self, scope: Scope<'_>, port: EventPort) -> bool {
        match self.nodes.get(&port.node_id) {
            Some(node) => node.decl(scope).event(port.event).is_some(),
            None => self
                .ghost_saved(port.node_id)
                .is_some_and(|saved| saved.event(port.event).is_some()),
        }
    }

    /// What the ghost node `node_id` declared, if the graph keeps one that knows.
    fn ghost_saved(&self, node_id: NodeId) -> Option<&SavedDecl> {
        self.ghost_nodes.get(&node_id)?.saved.as_ref()
    }

    /// Write `notes`: remove each ghost constant that was its port's default and report it, and
    /// give each remaining ghost the saved ports noted.
    pub(crate) fn note_ghosts(&mut self, notes: &GhostNotes, reconciled: &mut Reconciled) {
        for &port in &notes.defaults {
            let ghost = self
                .ghost_wiring
                .bindings
                .remove(&port)
                .expect("a noted ghost is kept");
            let GhostSource::Const(value) = ghost.source else {
                unreachable!("a default is a constant");
            };
            reconciled
                .repairs
                .push(Repair::DefaultDropped { port, value });
        }
        for note in &notes.bindings {
            let ghost = self
                .ghost_wiring
                .bindings
                .get_mut(&note.port)
                .expect("a noted ghost is kept");
            ghost.input.clone_from(&note.input);
            if let GhostSource::Wire { saved, .. } = &mut ghost.source {
                saved.clone_from(&note.output);
            }
        }
        for note in &notes.subscriptions {
            self.ghost_wiring
                .subscriptions
                .get_mut(&note.subscription)
                .expect("a noted ghost is kept")
                .event
                .clone_from(&note.event);
        }
    }

    /// The default each input of this graph's nodes takes at this load, in node-id then
    /// declaration order: an input that holds nothing, whose port its node did not declare
    /// `before` or whose default `notes` took as a ghost. A node `before` knows nothing of takes
    /// none, as nothing says which of its ports are new.
    pub(crate) fn gained_defaults(
        &self,
        scope: Scope<'_>,
        before: Before<'_>,
        notes: &GhostNotes,
    ) -> Vec<BindingEntry> {
        let mut seeds = Vec::new();
        for node in self.iter() {
            let Some(declared_before) = before.of(node.node_id, &node) else {
                continue;
            };
            for input in node.decl(scope).inputs() {
                let port = InputPort::new(node.node_id, input.id);
                let gained = declared_before.input(input.id).is_none();
                if (gained || notes.defaults.contains(&port))
                    && self.input_state(port) == InputState::Unbound
                    && let Some(default) = &input.default_value
                {
                    seeds.push(BindingEntry {
                        port,
                        binding: Binding::Const(default.clone()),
                    });
                }
            }
        }
        seeds
    }

    /// Bind each input of `seeds` to its default, and report each.
    pub(crate) fn seed_defaults(&mut self, seeds: Vec<BindingEntry>, reconciled: &mut Reconciled) {
        for BindingEntry { port, binding } in seeds {
            reconciled.repairs.push(Repair::DefaultSeeded {
                port,
                binding: binding.clone(),
            });
            self.set_input_state(port, InputState::Live(binding));
        }
    }

    /// Each wire that returns to a node on the current path of a depth-first
    /// walk from every node in id order, following wires from consumer to
    /// producer: removing them all leaves the graph acyclic, and the id order
    /// makes the choice the same on every load.
    fn closing_wires(&self) -> Vec<InputPort> {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum Mark {
            OnPath,
            Done,
        }
        #[derive(Debug)]
        enum Visit {
            Enter(NodeId),
            Wire(InputPort, NodeId),
            Exit(NodeId),
        }

        let mut roots: Vec<NodeId> = self.nodes.keys().copied().collect();
        roots.sort_unstable();
        let mut marks: HashMap<NodeId, Mark> = HashMap::with_capacity(roots.len());
        let mut stack = Vec::new();
        let mut closing = Vec::new();
        for root in roots {
            if marks.contains_key(&root) {
                continue;
            }
            stack.push(Visit::Enter(root));
            while let Some(visit) = stack.pop() {
                match visit {
                    Visit::Enter(node_id) => {
                        marks.insert(node_id, Mark::OnPath);
                        stack.push(Visit::Exit(node_id));
                        let first = stack.len();
                        stack.extend(self.input_bindings(node_id).filter_map(|(port, binding)| {
                            match binding {
                                Binding::Wire(source) => Some(Visit::Wire(port, source.node_id)),
                                Binding::Const(_) => None,
                            }
                        }));
                        stack[first..].reverse();
                    }
                    Visit::Wire(port, producer) => match marks.get(&producer) {
                        Some(Mark::OnPath) => closing.push(port),
                        Some(Mark::Done) => {}
                        None => stack.push(Visit::Enter(producer)),
                    },
                    Visit::Exit(node_id) => {
                        marks.insert(node_id, Mark::Done);
                    }
                }
            }
        }
        closing
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::graph::Graph;
    use crate::graph::node_ref::NodeRef;

    impl Graph {
        /// A node named `name`, or `None` if this graph holds none. Names are
        /// not unique, so this returns an arbitrary match.
        ///
        /// Test-only: a fixture knows its nodes by the names its library gave
        /// them, while production carries the `NodeId` it minted. A host
        /// searching by name would be searching for something the model does
        /// not promise is there or unique.
        pub(crate) fn find_by_name(&self, name: &str) -> Option<NodeRef<'_>> {
            assert!(!name.is_empty());
            self.nodes
                .iter()
                .find(|(_, node)| node.name == name)
                .map(|(id, node)| NodeRef { node_id: *id, node })
        }
    }
}

#[cfg(test)]
mod tests;
