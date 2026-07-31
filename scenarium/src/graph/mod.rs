//! The authoring graph: the [`Graph`] type, the satellite types it owns
//! ([`Node`], [`Binding`], [`CacheMode`], …), and **every method a `Graph`
//! has**, in one impl — reads and mutations alike, so the whole surface is one
//! file to read rather than six.
//!
//! The modules beside it keep what those methods work *with*: [`identity`]
//! how a node or port is named, [`detached`] the reversible-removal records,
//! [`error`] what validation rejects.
//!
//! A graph is flat: every node is a leaf that resolves to a
//! [`Func`](crate::graph::func::Func) declaration, so there is no tree to
//! descend and no identity but a node's own.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Deref;

use ::serde::{Deserialize, Serialize};
use hashbrown::hash_map::Entry;
use hashbrown::{HashMap, HashSet};

use crate::DataType;
use crate::StaticValue;
use crate::graph::detached::DetachedNode;
use crate::graph::error::GraphValidationError;
use crate::graph::error::ValidationResult;
use crate::graph::func::{Func, FuncInput, FuncOutput, OutputType};
use crate::graph::identity::NodeId;
use crate::graph::identity::{InputPort, OutputPort};
use crate::graph::node::{Node, NodeKind};
use crate::graph::output_types::OutputTypeSource;
use crate::library::Library;

pub(crate) mod detached;
pub(crate) mod error;
pub(crate) mod func;
pub(crate) mod identity;
pub(crate) mod node;
pub(crate) mod output_types;
mod serde;

/// What a consumer input port is wired to. Stored sparsely: `Graph.bindings`
/// only holds these values, and an absent port is unbound.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Binding {
    Const(StaticValue),
    Bind(OutputPort),
}

// A node is pure authored data. Identity is its key in `Graph::nodes`; port/event
// arity comes from the func it resolves to, and wiring lives in side tables.
/// Deliberately not `Clone`: node ids are unique across a whole document, so
/// duplicating a graph is never a neutral act — a copy either mints fresh ids
/// for what it took (an editor's duplicate) or is the only one of its identity
/// alive at a time (undo/redo replay of a stored step).
///
/// The side tables split on how they're reached, not on how sensitive they
/// are: `bindings` is a plain map a caller keys into directly, so it is public
/// rather than fronted by trivial accessors; `nodes` stays `pub(crate)` for
/// cross-module test call sites that force every node's cache mode directly,
/// while `subscriptions` is reached only through methods — `pub(super)` so the
/// wiring and validation passes beside this file can walk it, and no wider.
#[derive(Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Graph {
    pub(crate) nodes: HashMap<NodeId, Node>,

    /// Data wiring, keyed by consumer input port. Sparse: only bound ports
    /// appear; absence means unbound. A `BTreeMap` keeps
    /// serialization deterministic and lets a node's ports range contiguously.
    /// Serialized as a sequence of `(port, binding)` pairs — struct keys aren't
    /// valid map keys in string-keyed formats (JSON/TOML).
    #[serde(default, with = "crate::graph::serde")]
    pub bindings: BTreeMap<InputPort, Binding>,

    /// Event wiring: every (emitter event → subscriber) edge, flat. A
    /// `BTreeSet` dedups and keeps both serialization and the order a compile
    /// reads them in deterministic.
    #[serde(default)]
    pub(super) subscriptions: BTreeSet<Subscription>,
}

/// One event-subscription edge: `subscriber` fires when `emitter`'s event
/// `event_idx` triggers. Ordered (emitter, event_idx, subscriber) so the
/// `BTreeSet` holding these dedups and iterates deterministically — which is
/// what lets a compile wire each emitter's subscriber lists in a fixed order.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Subscription {
    pub emitter: NodeId,
    pub event_idx: usize,
    pub subscriber: NodeId,
}

impl Subscription {
    /// Whether this edge touches `node_id` from either end — it emits to that
    /// node, or that node emits it. [`Binding::touches`](crate::Binding) for
    /// an event edge.
    pub(crate) fn touches(&self, node_id: NodeId) -> bool {
        self.emitter == node_id || self.subscriber == node_id
    }
}

/// One data edge as a value: the consumer port, and what it was bound to.
///
/// The unit every reversible edit collects severed wiring into: a
/// [`DetachedNode`] carries the edges that touched its node, so an attach
/// restores exactly what a detach took.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BindingEntry {
    pub port: InputPort,
    pub binding: Binding,
}

/// A [`Node`] with the id the lookup found it by re-attached — what
/// [`Graph::iter`] hands out, since a node stores no id of its own.
#[derive(Clone, Copy, Debug)]
pub struct NodeRef<'a> {
    pub id: NodeId,
    /// Private so only this file's lookups mint one; readers go through
    /// [`Deref`], which is the point of the type.
    node: &'a Node,
}

impl Deref for NodeRef<'_> {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        self.node
    }
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

    /// The node with `id`, or `None` if this graph has none.
    pub fn find(&self, id: NodeId) -> Option<&Node> {
        assert!(!id.is_nil());
        self.nodes.get(&id)
    }
    /// The declaration a node instantiates — a library entry, or a special
    /// node's hardcoded spec. `None` for a `Func` kind the library no longer
    /// holds: the caller decides whether that is drift to tolerate (the
    /// editor renders a stub) or a node to skip (lowering).
    ///
    /// The one place the per-kind lookup happens, so no caller repeats it.
    pub fn node_func<'a>(&'a self, node: &'a Node, library: &'a Library) -> Option<&'a Func> {
        match &node.kind {
            NodeKind::Func(func_id) => library.by_id(*func_id),
            NodeKind::Special(special) => Some(special.func()),
        }
    }
    /// The declared type of input `port`, or `None` when it can't be resolved
    /// — a missing node or an unresolved func. The caller treats `None` as the
    /// polymorphic `Any`.
    pub(super) fn input_type(&self, library: &Library, port: InputPort) -> Option<DataType> {
        self.input_spec(library, port).map(|i| i.data_type.clone())
    }
    /// The declared [`FuncInput`] of input `port` — its full spec (type +
    /// `value_variants` + flags), or `None` for an unresolved node. Resolution
    /// mirrors [`Self::input_type`].
    pub(super) fn input_spec<'a>(
        &'a self,
        library: &'a Library,
        port: InputPort,
    ) -> Option<&'a FuncInput> {
        let node = self.find(port.node_id)?;
        self.node_func(node, library)?.inputs.get(port.port_idx)
    }
    /// What settles one output port's type, one hop at a time: its own
    /// declaration, or — for a *wildcard* (a reroute or passthrough) — the
    /// input it mirrors, which is a hop to whatever feeds that.
    ///
    /// The reading of a declaration, and no more. Following the hops is
    /// [`OutputTypes`](crate::OutputTypes)'s, which is the only thing that
    /// walks them: it memoizes every port on a chain as it goes, so covering
    /// a graph costs one walk per chain rather than one per port, and a
    /// caller wanting a single port reads the table rather than re-walking.
    fn output_source(&self, library: &Library, port: OutputPort) -> OutputTypeSource {
        let Some(out) = self.output_spec(library, port) else {
            return OutputTypeSource::Unresolved;
        };
        let OutputType::Wildcard { mirrors } = &out.ty else {
            return OutputTypeSource::Fixed(out.ty.declared());
        };
        let mirror = InputPort::new(port.node_id, *mirrors);
        match self.bindings.get(&mirror) {
            Some(Binding::Bind(source)) => OutputTypeSource::Bind(*source),
            Some(Binding::Const(value)) => OutputTypeSource::Const {
                declared: self.input_type(library, mirror).unwrap_or_default(),
                value: value.clone(),
            },
            None => OutputTypeSource::Unresolved,
        }
    }
    /// The declared [`FuncOutput`] of output `port` — its name +
    /// [`OutputType`], or `None` for an unresolved node. The output-side mirror
    /// of [`Self::input_spec`].
    fn output_spec<'a>(&'a self, library: &'a Library, port: OutputPort) -> Option<&'a FuncOutput> {
        let node = self.find(port.node_id)?;
        self.node_func(node, library)?.outputs.get(port.port_idx)
    }
    /// Every data edge as (consumer input ← producer output). Const bindings
    /// are not edges and are skipped.
    pub fn edges(&self) -> impl Iterator<Item = (InputPort, OutputPort)> + '_ {
        self.bindings
            .iter()
            .filter_map(|(dst, binding)| match binding {
                Binding::Bind(src) => Some((*dst, *src)),
                _ => None,
            })
    }
    /// Whether binding an output of `producer` into an input on `consumer`
    /// would close a directed data cycle. A node wired to itself counts.
    ///
    /// The planner rejects a cyclic graph outright
    /// ([`Error::CycleDetected`](crate::Error)), so this is what keeps one from
    /// being authored at all: the editor asks before it commits a bind. Port
    /// indices don't enter into it — a cycle is a property of the node graph,
    /// and any output of `producer` reaching any input of `consumer` closes the
    /// same loop.
    ///
    /// Walks forward from `consumer` over the edges already there; reaching
    /// `producer` means the proposed one closes the loop. Const bindings aren't
    /// edges, so they can't take part.
    pub fn produces_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        if producer == consumer {
            return true;
        }
        let mut readers: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for (destination, source) in self.edges() {
            readers
                .entry(source.node_id)
                .or_default()
                .push(destination.node_id);
        }
        let mut stack = vec![consumer];
        let mut seen: HashSet<NodeId> = HashSet::from_iter([consumer]);
        while let Some(node) = stack.pop() {
            for &next in readers.get(&node).into_iter().flatten() {
                if next == producer {
                    return true;
                }
                if seen.insert(next) {
                    stack.push(next);
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
    pub fn is_subscribed(&self, emitter: NodeId, event_idx: usize, subscriber: NodeId) -> bool {
        self.subscriptions.contains(&Subscription {
            emitter,
            event_idx,
            subscriber,
        })
    }
    pub fn bindings_touching(&self, node_id: NodeId) -> Vec<BindingEntry> {
        self.bindings
            .iter()
            .filter(|(port, binding)| binding.touches(**port, node_id))
            .map(|(port, binding)| BindingEntry {
                port: *port,
                binding: binding.clone(),
            })
            .collect()
    }
    pub fn subscriptions(&self) -> impl Iterator<Item = Subscription> + '_ {
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
        assert!(!id.is_nil());
        self.nodes.get_mut(&id)
    }

    /// Add a func instance and seed its inputs' default const bindings.
    /// Returns the new node id.
    pub fn add_func_node(&mut self, func: &Func) -> NodeId {
        let node_id = self.add(Node::from(func));
        self.bindings.extend(func.default_bindings(node_id));
        node_id
    }

    pub fn detach_node(&mut self, node_id: NodeId) -> DetachedNode {
        assert!(!node_id.is_nil());
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
        detached.assert_valid();
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
    pub fn subscribe(&mut self, emitter: NodeId, event_idx: usize, subscriber: NodeId) {
        self.subscriptions.insert(Subscription {
            emitter,
            event_idx,
            subscriber,
        });
    }
    pub fn unsubscribe(&mut self, emitter: NodeId, event_idx: usize, subscriber: NodeId) {
        self.subscriptions.remove(&Subscription {
            emitter,
            event_idx,
            subscriber,
        });
    }

    /// Validate this graph structurally: everything a document must satisfy to
    /// *be* a graph, answerable without a library. What a host runs on a
    /// freshly decoded document, where no library exists yet — decoding does
    /// not validate on its own.
    pub fn validate(&self) -> ValidationResult<()> {
        self.validate_shape(&mut HashSet::new())
    }

    /// Validate this graph as an execution entry against `library`: structurally
    /// first, then every reference it makes.
    ///
    /// Two passes rather than one interleaved walk, because they answer
    /// different questions and only one of them can always be asked. Structure
    /// is what the document must satisfy to be a graph at all; this adds what it
    /// takes to compile one against a *particular* library, which the same
    /// document may pass today and fail tomorrow. Running structure first also
    /// lets the second pass assume it: every id it reads names a node that is
    /// there.
    pub fn validate_with(&self, library: &Library) -> ValidationResult<()> {
        self.validate()?;
        self.validate_references(library)
    }

    /// Structural validation of this graph's nodes and wiring.
    ///
    /// `node_ids` accumulates every id seen, so duplicates are caught across
    /// everything the walk covers — an id must be unique document-wide, which
    /// is what makes a bare id an unambiguous address.
    fn validate_shape(&self, node_ids: &mut HashSet<NodeId>) -> ValidationResult<()> {
        for (node_id, node) in &self.nodes {
            if node_id.is_nil() {
                return Err(GraphValidationError::NilNodeId);
            }
            if !node_ids.insert(*node_id) {
                return Err(GraphValidationError::DuplicateNodeId { node_id: *node_id });
            }
            match &node.kind {
                NodeKind::Func(func_id) => {
                    if func_id.is_nil() {
                        return Err(GraphValidationError::NilFuncId { node_id: *node_id });
                    }
                }
                NodeKind::Special(_) => {}
            }
        }

        // Drift is tolerated everywhere below: a binding, subscription, or
        // pin referencing a port the current library no longer declares —
        // and a wire or const whose type no longer matches — stays valid
        // and degrades to unbound at lowering time (a required input
        // surfaces as a missing-input verdict; see lowering's `typed_binding`).
        // Deleting or rejecting it would destroy authored wiring that
        // revives when the library or the upstream types come back.
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
            if !self.nodes.contains_key(&subscription.emitter) {
                return Err(GraphValidationError::MissingSubscriptionEmitter {
                    node_id: subscription.emitter,
                });
            }
            if !self.nodes.contains_key(&subscription.subscriber) {
                return Err(GraphValidationError::MissingSubscriber {
                    emitter: subscription.emitter,
                    event_idx: subscription.event_idx,
                    subscriber: subscription.subscriber,
                });
            }
        }

        Ok(())
    }

    /// Everything in this graph that names a declaration: every func resolves,
    /// and a `Bind` does not sit on a `const_only` input.
    pub(super) fn validate_references(&self, library: &Library) -> ValidationResult<()> {
        for (node_id, node) in &self.nodes {
            match &node.kind {
                NodeKind::Func(func_id) => {
                    if library.by_id(*func_id).is_none() {
                        return Err(GraphValidationError::MissingFunc {
                            node_id: *node_id,
                            func_id: *func_id,
                        });
                    }
                }
                NodeKind::Special(_) => {}
            }
        }

        for (destination, binding) in &self.bindings {
            if matches!(binding, Binding::Bind(_))
                && self
                    .input_spec(library, *destination)
                    .is_some_and(|input| input.const_only)
            {
                return Err(GraphValidationError::ConstOnlyBinding { port: *destination });
            }
        }

        Ok(())
    }
}

impl Binding {
    /// A data binding wired to producer `node_id`'s output port `port_idx`.
    pub fn bind(node_id: NodeId, port_idx: usize) -> Self {
        Binding::Bind(OutputPort::new(node_id, port_idx))
    }

    /// Whether this edge — the binding on `port` — touches `node_id` from
    /// either end: it lands on that node, or it reads from it. How a detach
    /// finds the wiring it has to take with it, and how the record it
    /// produced proves it took only that.
    pub(crate) fn touches(&self, port: InputPort, node_id: NodeId) -> bool {
        port.node_id == node_id || matches!(self, Binding::Bind(src) if src.node_id == node_id)
    }
}

impl From<OutputPort> for Binding {
    fn from(value: OutputPort) -> Self {
        Binding::Bind(value)
    }
}

impl From<StaticValue> for Binding {
    fn from(value: StaticValue) -> Self {
        Binding::Const(value)
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::graph::{Graph, NodeRef};

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
        pub(crate) fn clone_verbatim(&self) -> Graph {
            Graph {
                nodes: self.nodes.clone(),
                bindings: self.bindings.clone(),
                subscriptions: self.subscriptions.clone(),
            }
        }
    }
}

#[cfg(test)]
mod tests;
