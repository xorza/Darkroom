//! The authoring graph: the [`Graph`] type, the satellite types it owns
//! ([`Node`], [`Binding`], [`CacheMode`](node::CacheMode), …), and **every method a `Graph`
//! has**, in one impl — reads and mutations alike, so the whole surface is one
//! file to read rather than six.
//!
//! The modules beside it keep what those methods work *with*: [`identity`]
//! how a node or port is named, [`detached`] the reversible-removal records,
//! [`error`] what validation rejects.
//!
//! Every node resolves to a [`Func`] declaration, and a node's own id is the
//! only name it has.

use std::collections::{BTreeMap, BTreeSet};

use ::serde::{Deserialize, Serialize};
use hashbrown::hash_map::Entry;
use hashbrown::{HashMap, HashSet};

use crate::ConstValue;
use crate::DataType;
use crate::graph::binding::Binding;
use crate::graph::binding_entry::BindingEntry;
use crate::graph::detached::DetachedNode;
use crate::graph::error::GraphValidationError;
use crate::graph::error::ValidationResult;
use crate::graph::func::Func;
use crate::graph::func::func_input::FuncInput;
use crate::graph::func::func_output::FuncOutput;
use crate::graph::func::func_output::OutputType;
use crate::graph::identity::NodeId;
use crate::graph::identity::{EventPort, InputPort, OutputPort};
use crate::graph::node::{FuncResolution, Node, NodeKind};
use crate::graph::node_ref::NodeRef;
use crate::graph::output_types::OutputTypeSource;
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
pub(crate) mod output_types;
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
    /// The declared type of input `port`, or `None` when it can't be resolved
    /// — a missing node or an unresolved func. The caller treats `None` as the
    /// polymorphic `Any`.
    fn input_type(&self, library: &Library, port: InputPort) -> Option<DataType> {
        self.input_spec(library, port).map(|i| i.data_type.clone())
    }
    /// The declared [`FuncInput`] of input `port` — its full spec (type +
    /// `value_variants` + flags), or `None` for an unresolved node. Resolution
    /// mirrors [`Self::input_type`].
    fn input_spec<'a>(&'a self, library: &'a Library, port: InputPort) -> Option<&'a FuncInput> {
        let node = self.find(port.node_id)?;
        node.func(library)?.inputs().get(port.port_idx)
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
            Some(Binding::Const(value)) => OutputTypeSource::Fixed(
                self.input_type(library, mirror)
                    .unwrap_or_default()
                    .or_const_type(value),
            ),
            None => OutputTypeSource::Unresolved,
        }
    }
    /// The declared [`FuncOutput`] of output `port` — its name +
    /// [`OutputType`], or `None` for an unresolved node. The output-side mirror
    /// of [`Self::input_spec`].
    fn output_spec<'a>(&'a self, library: &'a Library, port: OutputPort) -> Option<&'a FuncOutput> {
        let node = self.find(port.node_id)?;
        node.func(library)?.outputs().get(port.port_idx)
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
    /// The planner rejects a cyclic graph outright
    /// ([`Error::CycleDetected`](crate::Error)), so this is what keeps one from
    /// being authored at all: the editor asks before it commits a bind. Port
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
    /// *be* a graph, answerable without a library. What a host runs on a
    /// freshly decoded document, where no library exists yet — decoding checks
    /// only what it alone can see (a repeated key, which no decoded value could
    /// still show), never the wiring below.
    ///
    /// Id *uniqueness* is therefore not among the checks: `nodes` is keyed by
    /// id, so a duplicate cannot survive to be found here — the two ways one
    /// could arrive are [`insert`](Self::insert), which panics, and decoding,
    /// which refuses. Re-deriving it from the map's own keys would only ever
    /// confirm that a map is a map.
    pub fn validate(&self) -> ValidationResult<()> {
        for (node_id, node) in &self.nodes {
            if node_id.is_nil() {
                return Err(GraphValidationError::NilNodeId);
            }
            match &node.kind {
                NodeKind::Func(func_ref) => {
                    if func_ref.id().is_nil() {
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

    /// Everything in this graph that names a declaration: every func resolves
    /// with the ports its node was authored against, and a `Bind` does not sit
    /// on a `const_only` input.
    fn validate_references(&self, library: &Library) -> ValidationResult<()> {
        for (node_id, node) in &self.nodes {
            match node.resolve(library) {
                FuncResolution::Found(_) => {}
                FuncResolution::Changed(func) => {
                    return Err(GraphValidationError::FuncChanged {
                        node_id: *node_id,
                        func_id: func.id(),
                        func_name: func.name().to_owned(),
                    });
                }
                FuncResolution::Missing(func_id) => {
                    return Err(GraphValidationError::MissingFunc {
                        node_id: *node_id,
                        func_id,
                    });
                }
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
