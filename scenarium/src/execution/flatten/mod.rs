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

use hashbrown::{HashMap, HashSet};

use crate::execution::identity::{
    ExecutionEventPort, ExecutionNodeId, ExecutionOutputPort, FlattenMap,
};
use crate::execution::program::pool::Pool;
use crate::execution::program::{
    ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode, ExecutionOutput,
};
use crate::graph::interface::{GraphId, GraphLink};
use crate::graph::{Binding, Graph, InputPort, NodeId, NodeKind, NodeSearch, OutputPort};
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
    subs: Vec<ExecutionSubscription>,
}

/// The graph's packed port pools, rebuilt each `build`.
#[derive(Debug)]
pub(crate) struct Pools<'a> {
    pub inputs: &'a mut Pool<ExecutionInput>,
    pub outputs: &'a mut Pool<ExecutionOutput>,
    pub events: &'a mut Pool<ExecutionEvent>,
}

impl Flattener {
    /// Flatten `root` into `e_nodes`, rebuilding the packed pools fresh from the
    /// library.
    pub(crate) fn build(
        &mut self,
        e_nodes: &mut HashMap<ExecutionNodeId, ExecutionNode>,
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
        pools.outputs.clear();
        pools.events.clear();
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
                cur_id: ExecutionNodeId::default(),
                inputs: pools.inputs,
                outputs: pools.outputs,
                events: pools.events,
            };
            run.emit(false);
        }

        // Apply resolved event edges now that every flat emitter/subscriber exists and
        // is addressable by key. Subscribers were cleared while rebuilding events.
        for subscription in &self.subs {
            if !e_nodes.contains_key(&subscription.subscriber) {
                continue;
            }
            if let Some(e_node) = e_nodes.get_mut(&subscription.event.e_node_id) {
                pools.events[e_node.events][subscription.event.event_idx]
                    .subscribers
                    .push(subscription.subscriber);
            }
        }
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

/// Where an output reference resolves once boundaries are followed through.
enum Source {
    Producer(ExecutionOutputPort),
    Const(StaticValue),
    None,
}

#[derive(Debug, Clone, Copy)]
struct ExecutionSubscription {
    event: ExecutionEventPort,
    subscriber: ExecutionNodeId,
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
    subs: &'a mut Vec<ExecutionSubscription>,
    e_nodes: &'a mut HashMap<ExecutionNodeId, ExecutionNode>,
    cur_id: ExecutionNodeId,
    /// The inputs pool being built this update; `cur_id`'s range is its tail.
    inputs: &'a mut Pool<ExecutionInput>,
    outputs: &'a mut Pool<ExecutionOutput>,
    events: &'a mut Pool<ExecutionEvent>,
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

    fn execution_node_id(&mut self, node_id: NodeId) -> ExecutionNodeId {
        self.path.push(node_id);
        let e_node_id = ExecutionNodeId::from_authoring(self.path);
        self.path.pop();
        e_node_id
    }

    /// Emit execution nodes for the current level's graph, recursing into
    /// composite instances.
    fn emit(&mut self, ancestor_disabled: bool) {
        let graph = self.current();

        for node in graph.iter() {
            let disabled = ancestor_disabled || node.disabled;
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
                    self.emit(disabled);
                    self.scope_stack.pop();
                    self.path.pop();
                    if let Some(id) = shared_id {
                        self.seen_shared.remove(&id);
                    }
                    continue;
                }
                NodeKind::GraphInput | NodeKind::GraphOutput => continue,
            };

            let e_node_id = self.execution_node_id(node.id);
            let input_count = func.inputs.len();

            let outputs =
                self.outputs
                    .append((0..func.outputs.len()).map(|port_idx| ExecutionOutput {
                        pinned: graph.is_output_pinned(OutputPort::new(node.id, port_idx)),
                        ..Default::default()
                    }));
            let events = self
                .events
                .append(func.events.iter().map(|func_event| ExecutionEvent {
                    lambda: func_event.event_lambda.clone(),
                    ..Default::default()
                }));

            // Rebuilt fresh from the func every build (never carried over from the
            // last build): the library can evolve between updates — a changed
            // `required` flag or a grown input list must land here, and the bindings
            // loop below visits every port anyway.
            let inputs = self
                .inputs
                .append(func.inputs.iter().map(|func_input| ExecutionInput {
                    required: func_input.required,
                    stamps_fs_path: matches!(&func_input.data_type, DataType::FsPath(_)),
                    ..Default::default()
                }));

            let previous = self.e_nodes.insert(
                e_node_id,
                ExecutionNode {
                    sink: func.sink,
                    disabled,
                    behavior: func.behavior,
                    cache: node.cache,
                    special,
                    inputs,
                    outputs,
                    events,
                    func_id: func.id,
                    version: func.version,
                    lambda: func.lambda.clone(),
                },
            );
            debug_assert!(previous.is_none(), "flattened node ids must be unique");

            // Record where this flat node came from (current scope + authoring
            // id) so outcomes map back to editor nodes.
            let scope = *self.scope_stack.last().unwrap();
            self.flatten.set_leaf(e_node_id, scope, node.id);

            self.cur_id = e_node_id;

            for port_idx in 0..input_count {
                let port = InputPort::new(node.id, port_idx);
                let source = self.resolve_binding(graph.bindings.get(&port));
                self.set_input(port_idx, source);
            }
        }

        if !ancestor_disabled {
            self.collect_subscriptions(graph);
        }
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
            let Some(event) = self.resolve_emitter(sub.emitter, sub.event_idx) else {
                continue;
            };
            self.resolve_subscriber(sub.subscriber, event);
        }
    }

    /// Resolve an emitter `(node, event_idx)` to the concrete flat func event
    /// it ultimately fires, following composite exposed-event mappings inward.
    fn resolve_emitter(&mut self, node_id: NodeId, event_idx: usize) -> Option<ExecutionEventPort> {
        let graph = self.current();
        let node = graph.find(&node_id, NodeSearch::TopLevel)?;
        if node.disabled {
            return None; // a disabled node fires no events
        }
        match &node.kind {
            NodeKind::Func(_) | NodeKind::Special(_) => Some(ExecutionEventPort {
                e_node_id: self.execution_node_id(node_id),
                event_idx,
            }),
            NodeKind::Graph(r) => {
                let nested = graph.resolve_graph(*r, self.library)?;
                let exposed = nested
                    .definition
                    .as_ref()
                    .expect("nested graph requires a subgraph definition")
                    .events
                    .get(event_idx)?;
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
    fn resolve_subscriber(&mut self, node_id: NodeId, event: ExecutionEventPort) {
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
                let e_node_id = self.execution_node_id(node_id);
                self.subs.push(ExecutionSubscription {
                    event,
                    subscriber: e_node_id,
                });
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
                    self.resolve_subscriber(sub, event);
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
            Source::Producer(address) => ExecutionBinding::Bind(address),
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
        match &node.kind {
            NodeKind::Func(_) | NodeKind::Special(_) => Source::Producer(ExecutionOutputPort {
                e_node_id: self.execution_node_id(node_id),
                port_idx,
            }),
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
