//! The authoring graph: the [`Graph`] type, the satellite types it owns
//! ([`Node`], [`Binding`], [`CacheMode`](node::CacheMode), …), and **every method a `Graph`
//! has**, in one impl — reads and mutations alike, so the whole surface is one
//! file to read rather than six.
//!
//! The modules beside it keep what those methods work *with*: [`identity`]
//! how a node or port is named, [`detached`] the reversible-removal records,
//! [`error`] what validation and admission reject, [`reconciled`] what a load
//! repaired.
//!
//! A graph a host keeps holds only nodes that resolve to a [`Func`]
//! declaration and only wiring [`Graph::admit`] accepts: [`Graph::reconcile`]
//! makes a decoded graph so, and the host's edit gate keeps it so. A node's
//! own id is the only name it has.
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
//! - **A matching signature is not enough.** The signature covers port names,
//!   types and order, and event names — not the `required` flag,
//!   `const_only`, picker variants, overrides, an enum's variants or a
//!   default. A library change to any of these keeps a node resolved but can
//!   break a binding, so the load admits each binding of a resolved node too.
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
//! - **An output's type is its declaration.** No output is a wildcard or
//!   `Any`, so no edit retypes a distant wire and every wire is checkable.
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
    BindingRejection, GraphValidationError, SubscriptionRejection, ValidationResult,
};
use crate::graph::func::Func;
use crate::graph::identity::{EventPort, InputPort, NodeId, OutputPort};
use crate::graph::node::Node;
use crate::graph::node_ref::NodeRef;
use crate::graph::reconciled::{Detachment, Reconciled, Repair};
use crate::graph::subscription::Subscription;
use crate::library::Library;

pub(crate) mod binding;
pub(crate) mod binding_entry;
pub(crate) mod detached;
pub(crate) mod error;
pub(crate) mod func;
pub(crate) mod identity;
pub(crate) mod node;
pub(crate) mod node_ref;
pub(crate) mod reconciled;
mod serde;
pub(crate) mod subscription;

/// Deliberately not `Clone`: node ids are unique across a whole document, so
/// duplicating a graph is never a neutral act — a copy either mints fresh ids
/// for what it took (an editor's duplicate) or is the only one of its identity
/// alive at a time (undo/redo replay of a stored step).
///
/// The side tables split on how they're reached, not on how sensitive they
/// are: `bindings` is a plain map a caller keys into directly, so it is public
/// rather than fronted by trivial accessors, while `nodes` and `subscriptions`
/// are reached only through the methods below and so are private.
#[derive(Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Graph {
    /// Every node, keyed by the id that reaches it — so an id is unique here
    /// by construction, and [`insert`](Self::insert) panics rather than
    /// replacing. Decoding is the one way in that could hold a repeated id, and
    /// [`serde::deserialize_nodes`](crate::graph::serde) refuses one instead of
    /// letting the map absorb it.
    #[serde(deserialize_with = "crate::graph::serde::deserialize_nodes")]
    nodes: HashMap<NodeId, Node>,

    /// Data wiring, keyed by consumer input port. Sparse: only bound ports
    /// appear; absence means unbound. A `BTreeMap` keeps
    /// serialization deterministic and lets a node's ports range contiguously.
    /// Serialized as a sequence of `(port, binding)` pairs and decoded entry
    /// by entry, so a document that repeats a port is refused rather than
    /// loaded as a map that kept only the last binding.
    #[serde(
        default,
        serialize_with = "crate::graph::serde::serialize_bindings",
        deserialize_with = "crate::graph::serde::deserialize_bindings"
    )]
    pub bindings: BTreeMap<InputPort, Binding>,

    /// Event wiring: every (emitter event → subscriber) edge, flat. A
    /// `BTreeSet` dedups and keeps both serialization and the order a compile
    /// reads them in deterministic.
    #[serde(default)]
    subscriptions: BTreeSet<Subscription>,
}

impl Graph {
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
            .map(|(id, node)| NodeRef { id: *id, node })
    }

    /// The node with `id`, or `None` if this graph has none — which is the
    /// answer for the nil id, as no graph holds it.
    ///
    /// A bare `&Node` rather than a [`NodeRef`]: the caller supplied the id, so
    /// there is nothing to attach.
    pub fn find(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }
    /// The bindings on `node_id`'s own inputs, in port order: one contiguous
    /// range, as `InputPort` orders by node before port.
    pub fn input_bindings(&self, node_id: NodeId) -> impl Iterator<Item = (InputPort, &Binding)> {
        self.bindings
            .range(InputPort::new(node_id, 0)..)
            .take_while(move |(port, _)| port.node_id == node_id)
            .map(|(port, binding)| (*port, binding))
    }
    /// Whether input `port` of a node instantiating `func` is set aside by the
    /// input `func` declares to override it — the editor's reading of the
    /// rule a run applies, taken from the graph alone.
    ///
    /// The overriding input sets `port` aside when it holds a constant other
    /// than `Null`, or is wired to an enabled node. A run also reads `port`
    /// when that node cannot run for a missing input of its own, which only a
    /// run knows.
    pub fn overridden(&self, port: InputPort, func: &Func) -> bool {
        let Some(by) = func.overrider_of(port.port_idx) else {
            return false;
        };
        match self.bindings.get(&InputPort::new(port.node_id, by)) {
            None | Some(Binding::Const(ConstValue::Null)) => false,
            Some(Binding::Const(_)) => true,
            Some(Binding::Bind(source)) => self
                .find(source.node_id)
                .is_some_and(|producer| !producer.disabled),
        }
    }
    /// Whether this graph may hold `binding` on `port`: the input is one its
    /// node declares, a constant is one the input accepts, and a wire reads an
    /// output its producer declares, of a type the input accepts, into an
    /// input that is not const-only. The one rule a load, an edit and a
    /// compile hold wiring to. A cycle is the graph's question, not the
    /// binding's — see [`Self::produces_cycle`].
    ///
    /// # Panics
    ///
    /// If the graph does not hold the consumer, or the producer of a wire.
    pub fn admit(
        &self,
        library: &Library,
        port: InputPort,
        binding: &Binding,
    ) -> Result<(), BindingRejection> {
        let consumer = self
            .find(port.node_id)
            .expect("an admitted binding's consumer is in the graph")
            .func(library);
        self.admit_onto(library, consumer, port, binding)
    }
    /// [`Self::admit`] onto a consumer that instantiates `consumer` — for a
    /// node an edit is inserting together with its bindings, which the graph
    /// does not hold yet.
    ///
    /// # Panics
    ///
    /// If the graph does not hold the producer of a wire.
    pub fn admit_onto(
        &self,
        library: &Library,
        consumer: &Func,
        port: InputPort,
        binding: &Binding,
    ) -> Result<(), BindingRejection> {
        let input = consumer
            .inputs()
            .get(port.port_idx)
            .ok_or(BindingRejection::UndeclaredInput)?;
        match binding {
            Binding::Const(value) => {
                if library.const_satisfies(input, value) {
                    Ok(())
                } else {
                    Err(BindingRejection::ConstMismatch)
                }
            }
            Binding::Bind(source) => {
                if input.const_only {
                    return Err(BindingRejection::WiredConstOnly);
                }
                let producer = self
                    .find(source.node_id)
                    .expect("an admitted wire's producer is in the graph")
                    .func(library);
                let output = producer
                    .outputs()
                    .get(source.port_idx)
                    .ok_or(BindingRejection::UndeclaredOutput)?;
                if input.data_type.compatible_with(&output.data_type) {
                    Ok(())
                } else {
                    Err(BindingRejection::WireMismatch)
                }
            }
        }
    }
    /// Whether this graph may hold `subscription`: the emitter declares the
    /// event, and the subscriber is another node.
    ///
    /// # Panics
    ///
    /// If the graph does not hold the emitter.
    pub fn admit_subscription(
        &self,
        library: &Library,
        subscription: Subscription,
    ) -> Result<(), SubscriptionRejection> {
        let Subscription { event, subscriber } = subscription;
        if event.node_id == subscriber {
            return Err(SubscriptionRejection::SelfSubscription);
        }
        let emitter = self
            .find(event.node_id)
            .expect("an admitted subscription's emitter is in the graph")
            .func(library);
        if event.event_idx < emitter.events().len() {
            Ok(())
        } else {
            Err(SubscriptionRejection::UndeclaredEvent)
        }
    }
    /// Every data edge as (consumer input ← producer output). Const bindings
    /// are not edges and are skipped.
    pub fn edges(&self) -> impl Iterator<Item = (InputPort, OutputPort)> + '_ {
        self.bindings
            .iter()
            .filter_map(|(dst, binding)| match binding {
                Binding::Bind(src) => Some((*dst, *src)),
                Binding::Const(_) => None,
            })
    }
    /// Whether binding an output of `producer` into an input on `consumer`
    /// would close a directed data cycle. A node wired to itself counts.
    ///
    /// A compile asserts that no wire closes a cycle
    /// ([`Self::validate_with`]), so this is what keeps one from being authored
    /// at all: the editor asks before it commits a bind. Port
    /// indices don't enter into it — a cycle is a property of the node graph,
    /// and any output of `producer` reaching any input of `consumer` closes the
    /// same loop.
    ///
    /// Walks *backward* from `producer` over what already feeds it; reaching
    /// `consumer` means the proposed edge closes the loop. Const bindings aren't
    /// edges, so they can't take part.
    ///
    /// Backward because `bindings` is keyed by consumer port and [`InputPort`]
    /// orders by node before port index — so a node's own inputs are one
    /// contiguous range, and "what feeds this node" is a lookup the map already
    /// answers. Asking it the other way round needs a reversed index built over
    /// every binding in the graph, whatever the answer turns out to be.
    pub fn produces_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        if producer == consumer {
            return true;
        }
        let mut stack = vec![producer];
        let mut seen: HashSet<NodeId> = HashSet::from_iter([producer]);
        while let Some(node) = stack.pop() {
            for (_, binding) in self.input_bindings(node) {
                let Binding::Bind(source) = binding else {
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

    pub fn add(&mut self, node: Node) -> NodeId {
        let node_id = NodeId::unique();
        self.insert(node_id, node);
        node_id
    }

    pub fn insert(&mut self, node_id: NodeId, node: Node) {
        assert!(!node_id.is_nil(), "cannot add a node with a nil id");
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
            func.default_bindings(node_id)
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
    /// alone can see (a repeated key, which no decoded value could still show),
    /// never the wiring below.
    ///
    /// Id *uniqueness* is therefore not among the checks: `nodes` is keyed by
    /// id, so a duplicate cannot survive to be found here — the two ways one
    /// could arrive are [`insert`](Self::insert), which panics, and decoding,
    /// which refuses. Re-deriving it from the map's own keys would only ever
    /// confirm that a map is a map.
    pub fn validate(&self) -> ValidationResult<()> {
        if self.nodes.contains_key(&NodeId::nil()) {
            return Err(GraphValidationError::NilNodeId);
        }

        for (destination, binding) in &self.bindings {
            if !self.nodes.contains_key(&destination.node_id) {
                return Err(GraphValidationError::BindingMissingNode {
                    node_id: destination.node_id,
                });
            }
            if let Binding::Bind(src) = binding
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

    /// Validate this graph as one a host may compile against `library`: the
    /// structure, then the load contract — every node resolves, every binding
    /// and subscription is admitted, and no wire closes a data cycle.
    ///
    /// What [`Self::reconcile`] establishes at load and the edit gate keeps,
    /// so a failure here is the host's bug: a compile asserts it.
    pub fn validate_with(&self, library: &Library) -> ValidationResult<()> {
        self.validate()?;
        for (node_id, node) in &self.nodes {
            if let Err(reason) = library.func_of(node) {
                return Err(GraphValidationError::Unresolved {
                    node_id: *node_id,
                    reason,
                });
            }
        }
        for (port, binding) in &self.bindings {
            self.admit(library, *port, binding).map_err(|rejection| {
                GraphValidationError::RejectedBinding {
                    port: *port,
                    rejection,
                }
            })?;
        }
        for subscription in self.subscriptions() {
            self.admit_subscription(library, subscription)
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

    /// Make a decoded graph one `library` can run, and report each change:
    /// remove a node under the nil id and the wiring that names a missing
    /// node, detach each node the library does not resolve with the wiring
    /// that touches it, remove each binding and subscription
    /// [`admit`](Self::admit) refuses, and remove each wire that closes a data
    /// cycle. Afterwards [`Self::validate_with`] holds.
    pub fn reconcile(&mut self, library: &Library) -> Reconciled {
        let mut repairs = Vec::new();

        if self.nodes.remove(&NodeId::nil()).is_some() {
            repairs.push(Repair::NilNode);
        }
        let nodes = &self.nodes;
        self.bindings.retain(|port, binding| {
            let held = nodes.contains_key(&port.node_id)
                && match binding {
                    Binding::Bind(source) => nodes.contains_key(&source.node_id),
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

        let mut unresolved: Vec<_> = self
            .nodes
            .iter()
            .filter_map(|(node_id, node)| {
                library.func_of(node).err().map(|reason| (*node_id, reason))
            })
            .collect();
        unresolved.sort_unstable_by_key(|(node_id, _)| *node_id);
        let detached = unresolved
            .into_iter()
            .map(|(node_id, reason)| Detachment {
                record: self.detach_node(node_id),
                reason,
            })
            .collect();

        let refused: Vec<_> = self
            .bindings
            .iter()
            .filter_map(|(port, binding)| {
                self.admit(library, *port, binding)
                    .err()
                    .map(|rejection| (*port, rejection))
            })
            .collect();
        for (port, rejection) in refused {
            let binding = self
                .bindings
                .remove(&port)
                .expect("a refused binding is in the graph");
            repairs.push(Repair::RejectedBinding {
                port,
                binding,
                rejection,
            });
        }
        let refused: Vec<_> = self
            .subscriptions()
            .filter_map(|subscription| {
                self.admit_subscription(library, subscription)
                    .err()
                    .map(|rejection| (subscription, rejection))
            })
            .collect();
        for (subscription, rejection) in refused {
            self.subscriptions.remove(&subscription);
            repairs.push(Repair::RejectedSubscription {
                subscription,
                rejection,
            });
        }

        for port in self.closing_wires() {
            let Some(Binding::Bind(producer)) = self.bindings.remove(&port) else {
                unreachable!("a closing wire is a wire in the graph");
            };
            repairs.push(Repair::CycleWire { port, producer });
        }
        debug_assert!(
            self.validate_with(library).is_ok(),
            "a reconciled graph holds the load contract: {:?}",
            self.validate_with(library)
        );
        Reconciled { repairs, detached }
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
                                Binding::Bind(source) => Some(Visit::Wire(port, source.node_id)),
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
                .map(|(id, node)| NodeRef { id: *id, node })
        }

        /// Clone keeping every node id exactly as it is — the reason `Graph`
        /// isn't `Clone`, since preserving identities is only sound where the
        /// original is *not* concurrently present in the same document.
        ///
        /// Test-only: production replays a stored step against a graph the step
        /// was taken from, so nothing there needs a second copy of one.
        pub(crate) fn clone_verbatim(&self) -> Self {
            Self {
                nodes: self.nodes.clone(),
                bindings: self.bindings.clone(),
                subscriptions: self.subscriptions.clone(),
            }
        }
    }
}

#[cfg(test)]
mod tests;
