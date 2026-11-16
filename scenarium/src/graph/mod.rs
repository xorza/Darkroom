//! The authoring graph: the [`Graph`] type, the satellite types it owns
//! ([`Node`], [`Binding`], [`CacheMode`](node::CacheMode), …), and **every method a `Graph`
//! has**, in one impl, so the whole surface is one file to read.
//!
//! The modules beside it keep what those methods work *with*: [`identity`]
//! how a node or port is named, [`ports`] the inputs and outputs a declaration
//! shows, [`detached`] the reversible-removal records,
//! [`stub`] a node the graph cannot hold, [`error`] what validation and admission reject,
//! [`reconciled`] what a load repaired, [`subgraph`] and [`graph_set`] the graphs a document
//! holds, and [`scope`] what a graph's nodes resolve against.
//!
//! A graph a host keeps holds among its nodes only nodes that resolve in its scope, and only
//! wiring [`Graph::admit`] accepts; each node it cannot hold is a [`Stub`] beside them.
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
//!   saved node, the new port unbound; a port that leaves takes only its own
//!   wiring, which the load refuses and reports as any binding the declaration
//!   does not admit; a rename and a reorder change nothing. As Blender finds a
//!   group's sockets by identifier
//!   ([group creation](https://projects.blender.org/archive/blender-archive/commit/fa3ca9afdb6f1ea1b709226af3f085133e5ddc2f))
//!   and Unreal gives a pin a persistent id as "pin name could be changed"
//!   ([`UEdGraphPin`](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Engine/UEdGraphPin)).
//!   Refused: the port's index with a signature of the func's ports on each
//!   node, which made every saved node of a func a stub when the func gained an
//!   input. The compiled program stays positional: a compile maps each id to
//!   its position once.
//! - **A graph keeps the nodes it cannot hold.** Its stubs sit beside its nodes, the ids of the
//!   two disjoint, so a definition is one value wherever it goes: a clip, a library file, a
//!   copy, a digest. As Blender, which swaps an unknown node for an "undefined" one in the tree
//!   and keeps its links
//!   ([undefined node](https://projects.blender.org/blender/blender/pulls/114783)). No reader of
//!   the nodes sees a stub, so none needs a "declares nothing" branch. Refused: the stubs in the
//!   host beside each graph, where a definition travels in two halves, the host writes the
//!   attach rules a second time, and every edit of a node asks two containers which holds it.
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
//!   node in id order removes each wire that returns to the current path, so
//!   every load makes the same choice. Only a hand-edited file holds one.
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

use std::collections::{BTreeMap, BTreeSet};

use ::serde::{Deserialize, Serialize};
use hashbrown::hash_map::Entry;
use hashbrown::{HashMap, HashSet};

use crate::data::const_value::ConstValue;
use crate::graph::binding::Binding;
use crate::graph::binding_entry::BindingEntry;
use crate::graph::detached::DetachedNode;
use crate::graph::error::{
    BindingRejection, DetachReason, GraphValidationError, SubscriptionRejection, ValidationResult,
};
use crate::graph::func::Func;
use crate::graph::identity::{EventPort, InputPort, NodeId, OutputPort, PortId, SubgraphId};
use crate::graph::node::{Boundary, Node, NodeKind};
use crate::graph::node_decl::NodeDecl;
use crate::graph::node_ref::NodeRef;
use crate::graph::reconciled::{Reconciled, Repair};
use crate::graph::scope::Scope;
use crate::graph::stub::Stub;
use crate::graph::subgraph::Subgraph;
use crate::graph::subscription::Subscription;

pub(crate) mod binding;
pub(crate) mod binding_entry;
pub(crate) mod detached;
pub(crate) mod error;
pub(crate) mod func;
pub(crate) mod graph_set;
pub(crate) mod identity;
pub(crate) mod instance_path;
pub(crate) mod interface;
pub(crate) mod interface_change;
pub(crate) mod node;
pub(crate) mod node_decl;
pub(crate) mod node_ref;
pub(crate) mod ports;
pub(crate) mod program_id;
pub(crate) mod reconciled;
pub(crate) mod scope;
mod serde;
pub(crate) mod stub;
pub(crate) mod subgraph;
pub(crate) mod subscription;

/// `Clone` keeps every node id as it is, so a copy is sound only where it never lives beside its
/// original in one document, as the copy a save rewrites and writes; an editor's duplicate mints
/// fresh ids instead.
///
/// `bindings` is a plain map a caller keys into directly, so it is public;
/// `nodes`, `subscriptions` and `stubs` are reached only through the methods below.
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Graph {
    /// Every node, keyed by id, so an id is unique by construction:
    /// [`insert`](Self::insert) panics rather than replacing, and
    /// [`serde::deserialize_nodes`](crate::graph::serde) refuses a repeated id
    /// instead of letting the map absorb it.
    #[serde(deserialize_with = "crate::graph::serde::deserialize_nodes")]
    nodes: HashMap<NodeId, Node>,

    /// Data wiring, keyed by consumer input port; absence means unbound. A `BTreeMap` keeps
    /// serialization deterministic and a node's ports contiguous. Serialized as `(port, binding)`
    /// pairs decoded entry by entry, so a document that repeats a port is refused rather than
    /// loaded with only the last binding.
    #[serde(
        default,
        serialize_with = "crate::graph::serde::serialize_bindings",
        deserialize_with = "crate::graph::serde::deserialize_bindings"
    )]
    pub bindings: BTreeMap<InputPort, Binding>,

    /// Event wiring: every (emitter event → subscriber) edge, flat. A `BTreeSet` dedups and keeps
    /// serialization and compile order deterministic.
    #[serde(default)]
    subscriptions: BTreeSet<Subscription>,

    /// The nodes this graph cannot hold, by id, their ids disjoint from `nodes`'. Serialized as
    /// the list of stubs, decoded entry by entry so a document that repeats an id is refused.
    #[serde(
        default,
        serialize_with = "crate::graph::serde::serialize_stubs",
        deserialize_with = "crate::graph::serde::deserialize_stubs"
    )]
    stubs: BTreeMap<NodeId, Stub>,
}

/// A node a reconcile takes out of its graph, and why: what a read of the graph under its scope
/// answers, which the write then detaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Detaching {
    pub(crate) node_id: NodeId,
    pub(crate) reason: DetachReason,
}

/// Wiring a scope refuses: what a read of a graph under its scope answers, which the write then
/// removes.
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
    /// How many nodes, the stubs aside.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    /// Iteration order is unspecified.
    pub fn iter(&self) -> impl Iterator<Item = NodeRef<'_>> {
        self.nodes
            .iter()
            .map(|(id, node)| NodeRef { node_id: *id, node })
    }

    /// The node with `id`, or `None` if this graph has none, as for the nil id or a stub.
    ///
    /// A bare `&Node` rather than a [`NodeRef`]: the caller supplied the id.
    pub fn find(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// The stub under `id`, or `None` if this graph holds none.
    pub fn stub(&self, id: NodeId) -> Option<&Stub> {
        self.stubs.get(&id)
    }

    /// The stubs, in node-id order.
    pub fn stubs(&self) -> impl ExactSizeIterator<Item = &Stub> {
        self.stubs.values()
    }

    /// Whether this graph holds `id`, as a node or a stub: the ids a new node may not take.
    pub fn holds(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id) || self.stubs.contains_key(&id)
    }

    /// Keep `stub` beside the nodes.
    ///
    /// # Panics
    /// If this graph holds its node id already, as a node or a stub.
    pub fn insert_stub(&mut self, stub: Stub) {
        let node_id = stub.node_id();
        assert!(
            !self.holds(node_id),
            "stub {node_id:?} takes an id the graph holds"
        );
        self.stubs.insert(node_id, stub);
    }

    pub fn remove_stub(&mut self, id: NodeId) -> Option<Stub> {
        self.stubs.remove(&id)
    }

    /// Each constant a binding holds, the stubs' included, with the input it is bound to.
    pub fn constants(&self) -> impl Iterator<Item = (InputPort, &ConstValue)> {
        let stubs = self.stubs.values().flat_map(|stub| {
            stub.record
                .bindings
                .iter()
                .map(|entry| (&entry.port, &entry.binding))
        });
        self.bindings
            .iter()
            .chain(stubs)
            .filter_map(|(port, binding)| match binding {
                Binding::Const(value) => Some((*port, value)),
                Binding::Wire(_) => None,
            })
    }

    /// [`Self::constants`] for writing. A constant changed in a stub keeps its record well
    /// formed, as a record's checks read the ports and the wires alone.
    pub fn constants_mut(&mut self) -> impl Iterator<Item = &mut ConstValue> {
        let stubs = self.stubs.values_mut().flat_map(|stub| {
            stub.record
                .bindings
                .iter_mut()
                .map(|entry| &mut entry.binding)
        });
        self.bindings
            .values_mut()
            .chain(stubs)
            .filter_map(|binding| match binding {
                Binding::Const(value) => Some(value),
                Binding::Wire(_) => None,
            })
    }
    /// The bindings on `node_id`'s own inputs, in port-id order: one contiguous
    /// range, as `InputPort` orders by node before port.
    pub fn input_bindings(&self, node_id: NodeId) -> impl Iterator<Item = (InputPort, &Binding)> {
        self.bindings
            .range(InputPort::new(node_id, PortId::nil())..)
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
    /// Whether this graph may hold `subscription`: the emitter declares the
    /// event, and the subscriber is another node with a body to run. A subscription stays inside
    /// one graph: an interface has no event ports.
    ///
    /// # Panics
    ///
    /// If the graph does not hold the emitter or the subscriber.
    pub fn admit_subscription(
        &self,
        scope: Scope<'_>,
        subscription: Subscription,
    ) -> Result<(), SubscriptionRejection> {
        let Subscription { event, subscriber } = subscription;
        if event.node_id == subscriber {
            return Err(SubscriptionRejection::SelfSubscription);
        }
        let subscriber = self
            .find(subscriber)
            .expect("an admitted subscription's subscriber is in the graph");
        if !matches!(subscriber.kind, NodeKind::Body { .. }) {
            return Err(SubscriptionRejection::BodilessSubscriber);
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
            .range(Subscription::new(EventPort::new(emitter, PortId::nil()), NodeId::nil())..)
            .take_while(move |subscription| subscription.event.node_id == emitter)
            .copied()
    }

    pub fn add(&mut self, node: Node) -> NodeId {
        let node_id = NodeId::unique();
        self.insert(node_id, node);
        node_id
    }

    /// # Panics
    /// If `node_id` is nil, or this graph holds it already, as a node or a stub: an add takes a
    /// fresh id.
    pub fn insert(&mut self, node_id: NodeId, node: Node) {
        assert!(!node_id.is_nil(), "cannot add a node with a nil id");
        assert!(
            !self.stubs.contains_key(&node_id),
            "node {node_id:?} takes the id of a stub"
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
        } = detached;
        self.insert(node_id, node);
        self.bindings.extend(
            bindings
                .into_iter()
                .map(|entry| (entry.port, entry.binding)),
        );
        self.subscriptions.extend(subscriptions);
    }
    pub fn set_input_binding(&mut self, port: InputPort, binding: impl Into<Option<Binding>>) {
        match binding.into() {
            Some(binding) => {
                self.bindings.insert(port, binding);
            }
            None => {
                self.bindings.remove(&port);
            }
        }
    }
    pub fn subscribe(&mut self, event: EventPort, subscriber: NodeId) {
        self.subscriptions
            .insert(Subscription::new(event, subscriber));
    }
    pub fn unsubscribe(&mut self, event: EventPort, subscriber: NodeId) {
        self.subscriptions
            .remove(&Subscription::new(event, subscriber));
    }

    /// Validate this graph structurally: everything a document must satisfy to
    /// *be* a graph, answerable without a library. Decoding checks only what it
    /// alone can see (a repeated key), never the wiring below.
    ///
    /// Id uniqueness within the nodes and within the stubs is not checked: each is keyed by id,
    /// and both ways a duplicate could arrive, an insert and decoding, refuse it.
    pub fn validate(&self) -> ValidationResult<()> {
        if self.nodes.contains_key(&NodeId::nil()) {
            return Err(GraphValidationError::NilNodeId);
        }
        if let Some(node_id) = self
            .stubs
            .keys()
            .copied()
            .find(|node_id| self.nodes.contains_key(node_id))
        {
            return Err(GraphValidationError::StubHoldsNode { node_id });
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
            if !self.nodes.contains_key(&subscription.subscriber) {
                return Err(GraphValidationError::MissingSubscriber {
                    event: subscription.event,
                    subscriber: subscription.subscriber,
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
            for boundary in [Boundary::Inputs, Boundary::Outputs] {
                let count = self
                    .nodes
                    .values()
                    .filter(|node| node.boundary() == Some(boundary))
                    .count();
                if count != 1 {
                    return Err(GraphValidationError::BoundaryCount { boundary, count });
                }
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
    /// stub saved under a node's id, and the wiring that names a missing node.
    pub(crate) fn repair_structure(&mut self, reconciled: &mut Reconciled) {
        let repairs = &mut reconciled.repairs;
        if self.nodes.remove(&NodeId::nil()).is_some() {
            repairs.push(Repair::NilNode);
        }
        let nodes = &self.nodes;
        self.stubs.retain(|node_id, _| {
            let held = !nodes.contains_key(node_id);
            if !held {
                repairs.push(Repair::DuplicateStub { node_id: *node_id });
            }
            held
        });
        self.bindings.retain(|port, binding| {
            let held = nodes.contains_key(&port.node_id)
                && match binding {
                    Binding::Wire(source) => nodes.contains_key(&source.node_id),
                    Binding::Const(_) => true,
                };
            if !held {
                repairs.push(Repair::DanglingBinding {
                    port: *port,
                    binding: binding.clone(),
                });
            }
            held
        });
        self.subscriptions.retain(|subscription| {
            let held = nodes.contains_key(&subscription.event.node_id)
                && nodes.contains_key(&subscription.subscriber);
            if !held {
                repairs.push(Repair::DanglingSubscription {
                    subscription: *subscription,
                });
            }
            held
        });
    }

    /// The stubs whose node resolves in `scope`, in node-id order: what
    /// [`Self::reattach`] attaches.
    pub(crate) fn resolving_stubs(&self, scope: Scope<'_>) -> Vec<NodeId> {
        self.stubs
            .values()
            .filter(|stub| scope.resolve(stub.record.node()).is_ok())
            .map(Stub::node_id)
            .collect()
    }

    /// Attach each stub of `stubs`, with the wiring that still has both ends and lands on an
    /// input the graph has not bound since; report each attach and each piece of wiring dropped.
    /// The nodes go in before any wiring, so an edge between two attached stubs finds its other
    /// end.
    pub(crate) fn reattach(&mut self, stubs: &[NodeId], reconciled: &mut Reconciled) {
        let stubs: Vec<Stub> = stubs
            .iter()
            .map(|node_id| {
                self.stubs
                    .remove(node_id)
                    .expect("a stub to attach is in the graph")
            })
            .collect();
        for stub in &stubs {
            self.insert(stub.node_id(), stub.record.node().clone());
            reconciled.repairs.push(Repair::Reattached {
                node_id: stub.node_id(),
            });
        }
        for stub in stubs {
            let node_id = stub.node_id();
            for entry in stub.record.bindings {
                let ends_held = self.nodes.contains_key(&entry.port.node_id)
                    && match &entry.binding {
                        Binding::Wire(source) => self.nodes.contains_key(&source.node_id),
                        Binding::Const(_) => true,
                    };
                if ends_held && !self.bindings.contains_key(&entry.port) {
                    self.bindings.insert(entry.port, entry.binding);
                } else {
                    reconciled
                        .repairs
                        .push(Repair::StubBindingDropped { node_id, entry });
                }
            }
            for subscription in stub.record.subscriptions {
                if self.nodes.contains_key(&subscription.event.node_id)
                    && self.nodes.contains_key(&subscription.subscriber)
                {
                    self.subscriptions.insert(subscription);
                } else {
                    reconciled.repairs.push(Repair::StubSubscriptionDropped {
                        node_id,
                        subscription,
                    });
                }
            }
        }
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

    /// The nodes `scope` does not let this graph hold, in node-id order: each node it does not
    /// resolve, and in a subgraph's graph each boundary node past the lowest id on its side.
    pub(crate) fn unresolved(&self, scope: Scope<'_>) -> Vec<Detaching> {
        let mut refused: Vec<Detaching> = self
            .nodes
            .iter()
            .filter_map(|(node_id, node)| {
                scope.resolve(node).err().map(|reason| Detaching {
                    node_id: *node_id,
                    reason: DetachReason::from(reason),
                })
            })
            .collect();
        if scope.owner().is_some() {
            for boundary in [Boundary::Inputs, Boundary::Outputs] {
                let mut held: Vec<NodeId> = self
                    .iter()
                    .filter(|node| node.boundary() == Some(boundary))
                    .map(|node| node.node_id)
                    .collect();
                held.sort_unstable();
                refused.extend(held.iter().skip(1).map(|node_id| Detaching {
                    node_id: *node_id,
                    reason: DetachReason::ExtraBoundary,
                }));
            }
        }
        refused.sort_unstable_by_key(|detaching| detaching.node_id);
        refused
    }

    /// Detach each node of `refused` into a stub that keeps its reason, in the order given, and
    /// report each. Each stub holds the wiring that touched its node when it was detached, so a
    /// node detached later does not hold an edge an earlier stub took.
    pub(crate) fn detach_into_stubs(&mut self, refused: &[Detaching], reconciled: &mut Reconciled) {
        for &Detaching { node_id, reason } in refused {
            let record = self.detach_node(node_id);
            self.insert_stub(Stub { record, reason });
            reconciled.repairs.push(Repair::Stubbed { node_id, reason });
        }
    }

    /// Each binding and subscription `scope` refuses, the bindings first, each in the order the
    /// graph keeps it: what [`Self::remove_refused`] removes.
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

    /// Remove each piece of `refused`, then each wire that closes a data cycle, and report each.
    pub(crate) fn remove_refused(
        &mut self,
        refused: &[RefusedWiring],
        reconciled: &mut Reconciled,
    ) {
        for &refusal in refused {
            match refusal {
                RefusedWiring::Binding { port, rejection } => {
                    let binding = self
                        .bindings
                        .remove(&port)
                        .expect("a refused binding is in the graph");
                    reconciled.repairs.push(Repair::RejectedBinding {
                        port,
                        binding,
                        rejection,
                    });
                }
                RefusedWiring::Subscription {
                    subscription,
                    rejection,
                } => {
                    self.subscriptions.remove(&subscription);
                    reconciled.repairs.push(Repair::RejectedSubscription {
                        subscription,
                        rejection,
                    });
                }
            }
        }
        for port in self.closing_wires() {
            let Some(Binding::Wire(producer)) = self.bindings.remove(&port) else {
                unreachable!("a closing wire is a wire in the graph");
            };
            reconciled
                .repairs
                .push(Repair::CycleWire { port, producer });
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
