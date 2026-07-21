//! Graph flattening, fused into execution-node building. Walks the
//! authoring `Graph`, expands every composite instance into its interior func
//! nodes, and writes them straight into the execution program
//! — no intermediate `Graph` is materialized. Boundary nodes
//! (`GraphInput`/`GraphOutput`) and composites dissolve; their edges are
//! short-circuited so the result is a flat, func-only execution graph on which
//! the existing scheduler (dead-branch pruning, caching, cycle detection)
//! works across composite boundaries unchanged. Both data bindings and event
//! subscriptions are short-circuited across boundaries (exposed events resolve
//! to their interior emitter; triggering a composite reaches the interior
//! nodes wired to its `GraphInput`).
//!
//! See `README.md` Part A §5.

use common::Span;
use hashbrown::{HashMap, HashSet};

use crate::execution::identity::FlattenMap;
use crate::execution::program::{
    ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode, ExecutionPortAddress,
    InputStamper,
};
use crate::graph::interface::{GraphId, GraphLink};
use crate::graph::{
    Binding, Graph, InputPort, NodeId, NodeKind, NodeSearch, OutputPort, Subscription,
};
use crate::library::Library;
use crate::node::definition::Func;
use crate::node::special::SpecialNode;
use crate::{DataType, StaticValue};

/// Hard cap on nesting depth — a release backstop after `validate_for_execution` has
/// rejected recursive graphs.
const MAX_DEPTH: usize = 256;

/// Reusable flattening scratch owned by the
/// [`Compiler`](crate::execution::compile::Compiler). The current graph at each level is
/// re-derived from `path`, keeping the struct free of borrowed references.
#[derive(Debug, Default)]
pub(crate) struct Flattener {
    path: Vec<NodeId>,
    /// `FlattenMap` scope indices parallel to the emit-descent in `path` —
    /// the scope each level's nodes live in. Reused across builds.
    scope_stack: Vec<u32>,
    /// Shared graphs currently on the emit-descent path. Reused across builds.
    seen_shared: HashSet<GraphId>,
    /// Resolved flat event edges (with flattened ids), collected during the
    /// walk and applied after the node pass (when `e_nodes` is final and
    /// addressable by key). Reused across builds.
    subs: Vec<Subscription>,
}

/// The graph's SoA pools, rebuilt each `build`. Each node's output span is
/// assigned from a running counter local to the build; `output_pinned`
/// is the one output-indexed pool filled *during* that build (a plain lookup
/// against the authoring graph, unlike `output_types`, which needs a second
/// pass to follow wildcard mirrors).
#[derive(Debug)]
pub(crate) struct Pools<'a> {
    pub inputs: &'a mut Vec<ExecutionInput>,
    pub events: &'a mut Vec<ExecutionEvent>,
    pub output_pinned: &'a mut Vec<bool>,
}

impl Flattener {
    /// Flatten `root` into `e_nodes`, rebuilding the SoA pools fresh from the
    /// library. Output spans are assigned
    /// from a running counter local to the build; the program's total output count
    /// is then `output_types.len()`, resolved separately.
    pub(crate) fn build(
        &mut self,
        e_nodes: &mut HashMap<NodeId, ExecutionNode>,
        pools: Pools<'_>,
        root: &Graph,
        library: &Library,
        flatten: &mut FlattenMap,
    ) {
        self.path.clear();
        self.seen_shared.clear();
        self.subs.clear();
        e_nodes.clear();
        // Reset to a lone root scope; emit pushes child scopes as it
        // descends composites (scope 0 is the root the stack starts on).
        flatten.reset();
        self.scope_stack.clear();
        self.scope_stack.push(0);

        pools.inputs.clear();
        pools.events.clear();
        pools.output_pinned.clear();
        {
            let mut run = Run {
                root,
                library,
                path: &mut self.path,
                scope_stack: &mut self.scope_stack,
                flatten,
                seen_shared: &mut self.seen_shared,
                subs: &mut self.subs,
                e_nodes,
                cur_id: NodeId::default(),
                inputs: pools.inputs,
                n_outputs: 0,
                events: pools.events,
                output_pinned: pools.output_pinned,
            };
            run.emit();
            // One entry pushed per pooled output port, in lockstep with `n_outputs`
            // (see `emit`'s per-node loop) — this lets the planner index
            // `output_pinned` in the same output-pool space as demand and readers.
            // A hand-built `ExecutionProgram` (as in the
            // executor's low-level tests, which never call `Flattener::build`) doesn't
            // get this guarantee — that's a separate, already-tolerant read path.
            debug_assert_eq!(
                run.output_pinned.len(),
                run.n_outputs as usize,
                "output_pinned must have exactly one entry per pooled output port"
            );
        }

        // Apply resolved event edges now that every flat emitter/subscriber exists and
        // is addressable by key. Subscribers were cleared while rebuilding events.
        for s in &self.subs {
            if !e_nodes.contains_key(&s.subscriber) {
                continue;
            }
            if let Some(e_node) = e_nodes.get_mut(&s.emitter) {
                let span = e_node.events.range();
                pools.events[span][s.event_idx]
                    .subscribers
                    .push(s.subscriber);
            }
        }
    }
}

/// The [`InputStamper`] for an input declared with a resource-reference type: `FsPath` is
/// built-in; a nominal custom type resolves through the [`ResourceStamper`] registered on
/// its library entry (absent registration ⇒ not a resource type). `None` for everything
/// else — the digest folds no referent identity for the input.
fn input_stamper(ty: &DataType, library: &Library) -> Option<InputStamper> {
    match ty {
        DataType::FsPath(_) => Some(InputStamper::FsPath),
        DataType::Custom(type_id) => {
            let stamper = library.types.get(type_id)?.stamper.clone()?;
            Some(InputStamper::Custom(stamper))
        }
        _ => None,
    }
}

/// The graph at the level addressed by `path` — descend from `root`, resolving
/// each composite instance through its shared or local graph.
fn graph_at<'a>(root: &'a Graph, library: &'a Library, path: &[NodeId]) -> &'a Graph {
    let mut graph = root;
    for id in path {
        let r = graph
            .find(id, NodeSearch::TopLevel)
            .unwrap()
            .kind
            .as_graph()
            .expect("descent path id must be a composite");
        graph = graph
            .resolve_graph(r, library)
            .expect("graph node references a missing graph");
    }
    graph
}

/// Deterministic flattened identity for an interior node reached via `path`
/// (the chain of composite-instance ids descended through). Top-level nodes
/// (`path` empty) keep their own id, so caches survive and func-only graphs
/// map to themselves.
fn flatten_id(path: &[NodeId], interior: NodeId) -> NodeId {
    if path.is_empty() {
        return interior;
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"scenarium.flatten.v1");
    for id in path {
        hasher.update(&id.as_u128().to_le_bytes());
    }
    hasher.update(&interior.as_u128().to_le_bytes());
    let digest = hasher.finalize();
    NodeId::from_u128(u128::from_le_bytes(
        digest.as_bytes()[..16].try_into().unwrap(),
    ))
}

/// Where an output reference resolves once boundaries are followed through.
enum Source {
    Producer(OutputPort),
    Const(StaticValue),
    None,
}

/// One flattening pass. Borrows the reusable `path` buffer from `Flattener`;
/// the current graph at each level is `graph_at(root, library, path)`.
#[derive(Debug)]
struct Run<'a> {
    root: &'a Graph,
    library: &'a Library,
    path: &'a mut Vec<NodeId>,
    /// Scope indices parallel to `path` (the emit descent). `last()` is the
    /// scope the current level's nodes live in.
    scope_stack: &'a mut Vec<u32>,
    /// The flatten map being built — leaves recorded per func node, scopes
    /// pushed per composite descent.
    flatten: &'a mut FlattenMap,
    seen_shared: &'a mut HashSet<GraphId>,
    subs: &'a mut Vec<Subscription>,
    e_nodes: &'a mut HashMap<NodeId, ExecutionNode>,
    cur_id: NodeId,
    /// The inputs pool being built this update; `cur_id`'s span is its tail.
    inputs: &'a mut Vec<ExecutionInput>,
    /// Running total of outputs emitted so far; also the next output span start.
    n_outputs: u32,
    events: &'a mut Vec<ExecutionEvent>,
    /// Parallel to the output pool (indexed the same way, built alongside it):
    /// whether each pooled output port is pinned in the authoring graph.
    output_pinned: &'a mut Vec<bool>,
}

impl<'a> Run<'a> {
    fn current(&self) -> &'a Graph {
        graph_at(self.root, self.library, self.path.as_slice())
    }

    /// Descend one composite level. A legitimate graph never nests this deep.
    fn push_level(&mut self, instance_id: NodeId) {
        debug_assert!(
            self.path.len() < MAX_DEPTH,
            "graph nesting exceeds {MAX_DEPTH} levels (recursive definition?)"
        );
        self.path.push(instance_id);
    }

    /// Emit execution nodes for the current level's graph, recursing into
    /// composite instances.
    fn emit(&mut self) {
        let graph = self.current();

        for node in graph.iter() {
            // Disabled nodes are skipped entirely: no execution node, no
            // recursion into a disabled composite. A consumer bound to a
            // skipped node's output resolves to `Source::None` (see
            // `resolve`), so the wire reads as unbound downstream.
            if node.disabled {
                continue;
            }
            // A graph recurses; boundary nodes emit nothing. A func or a
            // special node both resolve to a `&Func` spec and emit one leaf —
            // the spec is the only difference (`library` vs. the hardcoded
            // `SpecialNode::func`), so the emit body below is shared.
            let library = self.library;
            let (func, special): (&Func, Option<SpecialNode>) = match &node.kind {
                NodeKind::Func(func_id) => (
                    library
                        .by_id(func_id)
                        .expect("func resolved by update's validate_for_execution validation"),
                    None,
                ),
                NodeKind::Special(s) => (s.func(), Some(*s)),
                NodeKind::Graph(link) => {
                    let shared_id = match link {
                        GraphLink::Shared(id) => Some(*id),
                        GraphLink::Local(_) => None,
                    };
                    if let Some(id) = shared_id
                        && !self.seen_shared.insert(id)
                    {
                        panic!("recursive shared graph {id:?} (it contains itself)");
                    }
                    self.push_level(node.id);
                    // Open this instance's scope under the current one; its
                    // interior nodes record their leaves against it.
                    let parent = *self.scope_stack.last().unwrap();
                    let scope = self.flatten.push_scope(node.id, parent);
                    self.scope_stack.push(scope);
                    self.emit();
                    self.scope_stack.pop();
                    self.path.pop();
                    if let Some(id) = shared_id {
                        self.seen_shared.remove(&id);
                    }
                    continue;
                }
                NodeKind::GraphInput | NodeKind::GraphOutput => continue,
            };

            let flat_id = flatten_id(self.path.as_slice(), node.id);
            let input_count = func.inputs.len();

            let outputs_start = self.n_outputs;
            self.n_outputs += func.outputs.len() as u32;
            for port_idx in 0..func.outputs.len() {
                self.output_pinned
                    .push(graph.is_output_pinned(OutputPort::new(node.id, port_idx)));
            }
            let events_start = self.events.len() as u32;
            for func_event in &func.events {
                self.events.push(ExecutionEvent {
                    lambda: func_event.event_lambda.clone(),
                    ..Default::default()
                });
            }

            // Rebuilt fresh from the func every build (never carried over from the
            // last build): the library can evolve between updates — a changed
            // `required` flag or a grown input list must land here, and the bindings
            // loop below visits every port anyway.
            let inputs_start = self.inputs.len() as u32;
            for func_input in &func.inputs {
                self.inputs.push(ExecutionInput {
                    required: func_input.required,
                    stamper: input_stamper(&func_input.data_type, library),
                    ..Default::default()
                });
            }

            let previous = self.e_nodes.insert(
                flat_id,
                ExecutionNode {
                    sink: func.sink,
                    behavior: func.behavior,
                    cache: node.cache,
                    special,
                    inputs: Span::new(inputs_start, input_count as u32),
                    outputs: Span::new(outputs_start, func.outputs.len() as u32),
                    events: Span::new(events_start, func.events.len() as u32),
                    func_id: func.id,
                    lambda: func.lambda.clone(),
                },
            );
            debug_assert!(previous.is_none(), "flattened node ids must be unique");

            // Record where this flat node came from (current scope + authoring
            // id) so stats map back to editor nodes.
            let scope = *self.scope_stack.last().unwrap();
            self.flatten.set_leaf(flat_id, scope, node.id);

            self.cur_id = flat_id;

            for port_idx in 0..input_count {
                let port = InputPort::new(node.id, port_idx);
                let source = self.resolve_binding(graph.bindings.get(&port));
                self.set_input(port_idx, source);
            }
        }

        self.collect_subscriptions(graph);
    }

    /// Resolve this level's event subscriptions across composite boundaries
    /// into flat `(emitter, event_idx, subscriber)` edges. Subscriptions
    /// emitted *by* a `GraphInput` (the trigger) are consumed when the
    /// enclosing instance is resolved as a subscriber, so they are skipped here.
    fn collect_subscriptions(&mut self, graph: &'a Graph) {
        let trigger = graph
            .iter()
            .find(|n| matches!(n.kind, NodeKind::GraphInput))
            .map(|n| n.id);

        for sub in graph.subscriptions() {
            if Some(sub.emitter) == trigger {
                continue;
            }
            let Some((emitter, event_idx)) = self.resolve_emitter(sub.emitter, sub.event_idx)
            else {
                continue;
            };
            self.resolve_subscriber(sub.subscriber, emitter, event_idx);
        }
    }

    /// Record one resolved flat event edge.
    fn push_edge(&mut self, emitter: NodeId, event_idx: usize, subscriber: NodeId) {
        self.subs.push(Subscription {
            emitter,
            event_idx,
            subscriber,
        });
    }

    /// Resolve an emitter `(node, event_idx)` to the concrete flat func event
    /// it ultimately fires, following composite exposed-event mappings inward.
    fn resolve_emitter(&mut self, node_id: NodeId, event_idx: usize) -> Option<(NodeId, usize)> {
        let graph = self.current();
        let node = graph.find(&node_id, NodeSearch::TopLevel)?;
        if node.disabled {
            return None; // a disabled node fires no events
        }
        match &node.kind {
            NodeKind::Func(_) | NodeKind::Special(_) => {
                Some((flatten_id(self.path.as_slice(), node_id), event_idx))
            }
            NodeKind::Graph(r) => {
                let nested = graph.resolve_graph(*r, self.library)?;
                let exposed = nested.events.get(event_idx)?;
                let (interior, interior_idx) = (exposed.emitter, exposed.emitter_event_idx);
                self.push_level(node_id);
                let resolved = self.resolve_emitter(interior, interior_idx);
                self.path.pop();
                resolved
            }
            NodeKind::GraphInput | NodeKind::GraphOutput => None,
        }
    }

    /// Resolve a subscriber to the concrete flat func nodes that actually run,
    /// pushing `(emitter, event_idx, flat_subscriber)` for each. A composite
    /// subscriber expands to the interior nodes wired to its `GraphInput`
    /// trigger.
    fn resolve_subscriber(&mut self, node_id: NodeId, emitter: NodeId, event_idx: usize) {
        let graph = self.current();
        // A disabled node runs nothing, so it receives no events.
        if graph
            .find(&node_id, NodeSearch::TopLevel)
            .is_some_and(|n| n.disabled)
        {
            return;
        }
        match graph.find(&node_id, NodeSearch::TopLevel).map(|n| &n.kind) {
            // A special node subscribes like a func: it flattens to one leaf and
            // becomes the flat subscriber. `RunSinks` in particular relies on
            // this edge so the planner sees it among a fired event's subscribers.
            Some(NodeKind::Func(_) | NodeKind::Special(_)) => {
                let flat = flatten_id(self.path.as_slice(), node_id);
                self.push_edge(emitter, event_idx, flat);
            }
            Some(NodeKind::Graph(r)) => {
                let Some(nested) = graph.resolve_graph(*r, self.library) else {
                    return;
                };
                let Some(trigger) = nested
                    .iter()
                    .find(|n| matches!(n.kind, NodeKind::GraphInput))
                    .map(|n| n.id)
                else {
                    return;
                };
                let interior: Vec<NodeId> = nested
                    .subscriptions()
                    .filter(|s| s.emitter == trigger)
                    .map(|s| s.subscriber)
                    .collect();
                self.push_level(node_id);
                for sub in interior {
                    self.resolve_subscriber(sub, emitter, event_idx);
                }
                self.path.pop();
            }
            _ => {}
        }
    }

    /// Pool index of input `input_idx` of the node currently being filled.
    fn cur_input_idx(&self, input_idx: usize) -> usize {
        self.e_nodes[&self.cur_id].inputs.start as usize + input_idx
    }

    /// Write the resolved source into input `cur_id`/`input_idx`. A changed
    /// binding changes the node's content digest, which drives cache
    /// invalidation — no per-input dirty tracking is needed.
    fn set_input(&mut self, input_idx: usize, source: Source) {
        let pool_idx = self.cur_input_idx(input_idx);
        let binding = match source {
            Source::None => ExecutionBinding::None,
            Source::Const(v) => ExecutionBinding::Const(v),
            Source::Producer(port) => ExecutionBinding::Bind(ExecutionPortAddress {
                target: port.node_id,
                port_idx: port.port_idx,
            }),
        };
        self.inputs[pool_idx].binding = binding;
    }

    /// Resolve an output reference in the current frame to a concrete flat
    /// producer, following through boundary and composite nodes. Leaves the
    /// descent stack as it found it.
    fn resolve(&mut self, port: OutputPort) -> Source {
        let OutputPort { node_id, port_idx } = port;
        let graph = self.current();
        let node = graph
            .find(&node_id, NodeSearch::TopLevel)
            .expect("binding to a missing node");
        // A disabled producer emits nothing, so its outputs have no source:
        // treat the wire as unbound (matches `emit` skipping the node).
        if node.disabled {
            return Source::None;
        }
        match &node.kind {
            NodeKind::Func(_) | NodeKind::Special(_) => Source::Producer(OutputPort::new(
                flatten_id(self.path.as_slice(), node_id),
                port_idx,
            )),
            // Follow into the composite: its output `port_idx` is wired by the
            // GraphOutput node's input `port_idx`.
            NodeKind::Graph(r) => {
                let nested = graph
                    .resolve_graph(*r, self.library)
                    .expect("graph node references a missing graph");
                let Some(output) = nested
                    .iter()
                    .find(|n| matches!(n.kind, NodeKind::GraphOutput))
                else {
                    return Source::None;
                };
                let port = InputPort::new(output.id, port_idx);
                let binding = nested.bindings.get(&port).cloned();
                self.push_level(node_id);
                let source = self.resolve_binding(binding.as_ref());
                self.path.pop();
                source
            }
            // Follow out: this GraphInput output `port_idx` is the enclosing
            // instance's exposed input `port_idx`; resolve it one level up.
            NodeKind::GraphInput => {
                let instance_id = self.path.pop().expect("GraphInput at the root level");
                let port = InputPort::new(instance_id, port_idx);
                let binding = self.current().bindings.get(&port).cloned();
                let source = self.resolve_binding(binding.as_ref());
                self.path.push(instance_id);
                source
            }
            NodeKind::GraphOutput => Source::None,
        }
    }

    fn resolve_binding(&mut self, binding: Option<&Binding>) -> Source {
        match binding {
            None => Source::None,
            Some(Binding::Const(value)) => Source::Const(value.clone()),
            Some(Binding::Bind(output)) => self.resolve(*output),
        }
    }
}

#[cfg(test)]
mod tests;
