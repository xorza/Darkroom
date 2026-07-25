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

use hashbrown::HashSet;

use crate::execution::identity::{
    ExecutionEventPort, ExecutionNodeId, ExecutionOutputPort, FlattenMap,
};
use crate::execution::program::pool::Pool;
use crate::execution::program::{
    ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode, ExecutionOutput,
};
use crate::graph::interface::{GraphId, GraphLink};
use crate::graph::validate::{MAX_NESTING_DEPTH, const_satisfies};
use crate::graph::{Binding, Graph, InputPort, NodeId, NodeKind, NodeSearch, OutputPort};
use crate::library::Library;
use crate::node::definition::{Func, FuncInput};
use crate::node::special::SpecialNode;
use crate::{DataType, StaticValue};

/// Reusable flattening scratch owned by the
/// [`Compiler`](crate::execution::compile::Compiler). The per-build resolved-graph
/// stack lives on `Run` (it borrows the build's graph), keeping this struct
/// free of borrowed references.
#[derive(Debug, Default)]
pub(crate) struct Flattener {
    path: Vec<NodeId>,
    /// `FlattenMap` scope indices parallel to the emit-descent in `path` —
    /// the scope each level's nodes live in. Reused across builds.
    scope_stack: Vec<u32>,
    /// Shared graphs currently on the emit-descent path. Reused across builds.
    seen_shared: HashSet<GraphId>,
    /// Resolved flat event edges (with flattened ids), collected during the
    /// walk; the compiler applies them once the program has adopted the node
    /// set ([`ExecutionProgram::apply_subscriptions`]). Reused across builds.
    pub(crate) subs: Vec<ExecutionSubscription>,
    /// Resolved `Bind` fixups: input-pool slot → id-based producer address.
    /// Bindings can reference nodes emitted later in the walk, so the dense
    /// [`OutputAddr`](crate::execution::program::OutputAddr) form is interned
    /// by the compiler after adoption
    /// ([`ExecutionProgram::intern_bindings`]). Reused across builds.
    pub(crate) pending_binds: Vec<(u32, ExecutionOutputPort)>,
    /// Flat nodes in emit order. Nothing in the walk looks one up, so this is a
    /// plain vector; the compiler sorts and drains it into the program
    /// ([`ExecutionProgram::adopt_nodes`]). Reused across builds.
    pub(crate) e_nodes: Vec<(ExecutionNodeId, ExecutionNode)>,
}

/// The graph's packed port pools, rebuilt each `build`.
#[derive(Debug)]
pub(crate) struct Pools<'a> {
    pub inputs: &'a mut Pool<ExecutionInput>,
    pub outputs: &'a mut Pool<ExecutionOutput>,
    pub events: &'a mut Pool<ExecutionEvent>,
}

impl Flattener {
    /// Flatten `root` into [`Self::e_nodes`], rebuilding the packed pools fresh
    /// from the library.
    pub(crate) fn build(
        &mut self,
        pools: Pools<'_>,
        root: &Graph,
        library: &Library,
        flatten: &mut FlattenMap,
    ) {
        self.path.clear();
        self.seen_shared.clear();
        self.subs.clear();
        self.pending_binds.clear();
        self.e_nodes.clear();
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
                library,
                path: &mut self.path,
                levels: vec![root],
                scope_stack: &mut self.scope_stack,
                flatten,
                seen_shared: &mut self.seen_shared,
                subs: &mut self.subs,
                pending_binds: &mut self.pending_binds,
                e_nodes: &mut self.e_nodes,
                inputs: pools.inputs,
                outputs: pools.outputs,
                events: pools.events,
            };
            run.emit(false);
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExecutionSubscription {
    pub(crate) event: ExecutionEventPort,
    pub(crate) subscriber: ExecutionNodeId,
}

/// An id-based resolved binding — flatten's intermediate form. `Bind` targets
/// can be emitted later in the walk, so they are recorded as fixups and
/// interned to dense addresses after the program adopts the node set.
#[derive(Debug)]
enum FlatBinding {
    None,
    Const(StaticValue),
    Bind(ExecutionOutputPort),
}

/// One flattening pass. Borrows the reusable `path` buffer from `Flattener`;
/// `levels` carries the resolved graph per descent level (root at the
/// bottom), so the current graph is one stack read, not a root re-walk.
#[derive(Debug)]
struct Run<'a> {
    library: &'a Library,
    path: &'a mut Vec<NodeId>,
    /// Resolved graphs parallel to `path`, plus the root at the bottom:
    /// `levels.len() == path.len() + 1` and `levels.last()` is the current
    /// level's graph.
    levels: Vec<&'a Graph>,
    /// Scope indices parallel to `path` (the emit descent). `last()` is the
    /// scope the current level's nodes live in.
    scope_stack: &'a mut Vec<u32>,
    /// The flatten map being built — leaves recorded per func node, scopes
    /// pushed per composite descent.
    flatten: &'a mut FlattenMap,
    seen_shared: &'a mut HashSet<GraphId>,
    subs: &'a mut Vec<ExecutionSubscription>,
    pending_binds: &'a mut Vec<(u32, ExecutionOutputPort)>,
    e_nodes: &'a mut Vec<(ExecutionNodeId, ExecutionNode)>,
    /// The inputs pool being built this update.
    inputs: &'a mut Pool<ExecutionInput>,
    outputs: &'a mut Pool<ExecutionOutput>,
    events: &'a mut Pool<ExecutionEvent>,
}

impl<'a> Run<'a> {
    fn current(&self) -> &'a Graph {
        self.levels
            .last()
            .copied()
            .expect("the root level always exists")
    }

    /// Descend one composite level. A release-build backstop against stack
    /// overflow — validation already rejected trees past the cap, but its
    /// shared-graph memoization measures a graph's depth only at first
    /// encounter, so flatten re-checks the true instance depth. Compile is a
    /// cold path; the assert stays in release.
    fn push_level(&mut self, instance_id: NodeId, graph: &'a Graph) {
        assert!(
            self.path.len() < MAX_NESTING_DEPTH,
            "graph nesting exceeds {MAX_NESTING_DEPTH} levels"
        );
        self.path.push(instance_id);
        self.levels.push(graph);
    }

    /// Ascend one composite level — the inverse of [`Self::push_level`].
    fn pop_level(&mut self) {
        self.path.pop().expect("cannot pop the root level");
        self.levels.pop().unwrap();
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
            let (func, special): (&Func, Option<SpecialNode>) = match &node.kind {
                NodeKind::Func(func_id) => (
                    self.library
                        .by_id(*func_id)
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
                    let nested = graph
                        .resolve_graph(*link, self.library)
                        .expect("graph node references a missing graph");
                    self.push_level(node.id, &nested.body);
                    // Open this instance's scope under the current one; its
                    // interior nodes record their leaves against it.
                    let parent = *self.scope_stack.last().unwrap();
                    let scope = self.flatten.push_scope(node.id, parent);
                    self.scope_stack.push(scope);
                    self.emit(disabled);
                    self.scope_stack.pop();
                    self.pop_level();
                    if let Some(id) = shared_id {
                        self.seen_shared.remove(&id);
                    }
                    continue;
                }
                NodeKind::GraphInput | NodeKind::GraphOutput => continue,
            };

            let e_node_id = self.execution_node_id(node.id);

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
            let inputs_start = inputs.start as usize;

            // Id uniqueness is enforced when the program adopts these
            // (`ExecutionProgram::push`), so the walk just appends.
            self.e_nodes.push((
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
            ));

            // Record where this flat node came from (current scope + authoring
            // id) so outcomes map back to editor nodes.
            let scope = *self.scope_stack.last().unwrap();
            self.flatten.set_leaf(e_node_id, scope, node.id);

            for (port_idx, func_input) in func.inputs.iter().enumerate() {
                let port = InputPort::new(node.id, port_idx);
                match self.typed_binding(graph, func_input, graph.bindings.get(&port)) {
                    FlatBinding::None => {}
                    FlatBinding::Const(value) => {
                        self.inputs[inputs_start + port_idx].binding =
                            ExecutionBinding::Const(value);
                    }
                    FlatBinding::Bind(target) => {
                        self.pending_binds
                            .push(((inputs_start + port_idx) as u32, target));
                    }
                }
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
        let trigger = graph.boundary_node(NodeKind::GraphInput);

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
        let node = graph.find(node_id, NodeSearch::TopLevel)?;
        if node.disabled {
            return None; // a disabled node fires no events
        }
        match &node.kind {
            NodeKind::Func(_) | NodeKind::Special(_) => {
                // Drift tolerance: a subscription to an event the func no
                // longer declares wires nothing.
                if graph
                    .node_ports(node, self.library)
                    .is_some_and(|ports| event_idx >= ports.events.len())
                {
                    return None;
                }
                Some(ExecutionEventPort {
                    e_node_id: self.execution_node_id(node_id),
                    event_idx,
                })
            }
            NodeKind::Graph(r) => {
                let nested = graph.resolve_graph(*r, self.library)?;
                let exposed = nested.interface.events.get(event_idx)?;
                self.push_level(node_id, &nested.body);
                let resolved = self.resolve_emitter(exposed.emitter, exposed.emitter_event_idx);
                self.pop_level();
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
        let Some(node) = graph.find(node_id, NodeSearch::TopLevel) else {
            return;
        };
        // A disabled node runs nothing, so it receives no events.
        if node.disabled {
            return;
        }
        match &node.kind {
            // A special node subscribes like a func: it flattens to one leaf and
            // becomes the flat subscriber. `RunSinks` in particular relies on
            // this edge so the planner sees it among a fired event's subscribers.
            NodeKind::Func(_) | NodeKind::Special(_) => {
                let e_node_id = self.execution_node_id(node_id);
                self.subs.push(ExecutionSubscription {
                    event,
                    subscriber: e_node_id,
                });
            }
            NodeKind::Graph(r) => {
                let Some(nested) = graph.resolve_graph(*r, self.library) else {
                    return;
                };
                let Some(trigger) = nested.body.boundary_node(NodeKind::GraphInput) else {
                    return;
                };
                self.push_level(node_id, &nested.body);
                for sub in nested.body.subscriptions().filter(|s| s.emitter == trigger) {
                    self.resolve_subscriber(sub.subscriber, event);
                }
                self.pop_level();
            }
            NodeKind::GraphInput | NodeKind::GraphOutput => {}
        }
    }

    /// Resolve an output reference in the current frame to a concrete flat
    /// producer, following through boundary and composite nodes. Leaves the
    /// descent stack as it found it.
    fn resolve(&mut self, port: OutputPort) -> FlatBinding {
        let OutputPort { node_id, port_idx } = port;
        let graph = self.current();
        let node = graph
            .find(node_id, NodeSearch::TopLevel)
            .expect("binding to a missing node");
        match &node.kind {
            NodeKind::Func(_) | NodeKind::Special(_) => {
                // Library drift can leave a binding to an output the func
                // no longer declares — degrade to unbound rather than
                // addressing a vanished slot (the planner reports the
                // consumer's missing input).
                if graph
                    .node_ports(node, self.library)
                    .is_some_and(|ports| port_idx >= ports.outputs.len())
                {
                    return FlatBinding::None;
                }
                FlatBinding::Bind(ExecutionOutputPort {
                    e_node_id: self.execution_node_id(node_id),
                    port_idx,
                })
            }
            // Follow into the composite: its output `port_idx` is wired by the
            // GraphOutput node's input `port_idx`.
            NodeKind::Graph(r) => {
                let nested = graph
                    .resolve_graph(*r, self.library)
                    .expect("graph node references a missing graph");
                let Some(output) = nested.body.boundary_node(NodeKind::GraphOutput) else {
                    return FlatBinding::None;
                };
                let binding = nested.body.bindings.get(&InputPort::new(output, port_idx));
                self.push_level(node_id, &nested.body);
                let source = self.resolve_binding(binding);
                self.pop_level();
                source
            }
            // Follow out: this GraphInput output `port_idx` is the enclosing
            // instance's exposed input `port_idx`; resolve it one level up.
            NodeKind::GraphInput => {
                let instance_id = *self.path.last().expect("GraphInput at the root level");
                self.pop_level();
                let port = InputPort::new(instance_id, port_idx);
                let binding = self.current().bindings.get(&port);
                let source = self.resolve_binding(binding);
                self.push_level(instance_id, graph);
                source
            }
            NodeKind::GraphOutput => FlatBinding::None,
        }
    }

    /// [`Self::resolve_binding`] behind the type gate: a wire whose resolved
    /// source type is incompatible with the declared input, or a const that
    /// doesn't satisfy it, flattens as unbound — the type half of drift
    /// tolerance. The editor paints such a wire as mismatched; a required
    /// input surfaces as a missing-input verdict. Nothing is severed, so the
    /// wiring revives when the types line up again.
    fn typed_binding(
        &mut self,
        graph: &'a Graph,
        input: &FuncInput,
        binding: Option<&Binding>,
    ) -> FlatBinding {
        let mismatched = match binding {
            Some(Binding::Bind(src)) => !input
                .data_type
                .compatible_with(&graph.resolve_output_type(self.library, *src)),
            Some(Binding::Const(value)) => !const_satisfies(self.library, input, value),
            None => false,
        };
        if mismatched {
            return FlatBinding::None;
        }
        self.resolve_binding(binding)
    }

    fn resolve_binding(&mut self, binding: Option<&Binding>) -> FlatBinding {
        match binding {
            None => FlatBinding::None,
            Some(Binding::Const(value)) => FlatBinding::Const(value.clone()),
            Some(Binding::Bind(output)) => self.resolve(*output),
        }
    }
}

#[cfg(test)]
mod tests;
