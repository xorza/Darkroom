//! Graph flattening: the pipeline's first stage. Walks the authoring `Graph`,
//! expands every composite instance into its interior func nodes, and appends
//! them straight into the [`FlatGraph`] it returns — no intermediate `Graph` is
//! materialized and no final [`Program`](crate::execution::program::Program)
//! is touched. Boundary nodes
//! (`GraphInput`/`GraphOutput`) and composites dissolve; their edges are
//! short-circuited so the result is a flat, func-only graph on which
//! the existing scheduler (dead-branch pruning, caching, cycle detection)
//! works across composite boundaries unchanged. Both data bindings and event
//! subscriptions are short-circuited across boundaries (exposed events resolve
//! to their interior emitter; triggering a composite reaches the interior
//! nodes wired to its `GraphInput`).
//! Traversal, descent state, leaf emission, and the data-edge and event-edge
//! routing the descent calls into are all [`Run`]'s, and so all live here with
//! it.
//!
//! Everything here is in the **stable-id space**: the walk names producers,
//! subscribers, and emitters by [`ExecutionNodeId`] because that is all it can
//! know — a node's dense index is its position after the id sort, which is
//! linking's to assign. So no type in this module mentions `NodeIdx`,
//! `OutputIdx`, or `OutputAddr`, and each of its port types is the stage-local
//! half of a program one.
//!
//! Flattening is lossy — composites and their boundary edges dissolve — so
//! where each flat node came from is recorded as the walk goes, in the [`Leaf`]
//! and source-map-owned scope table: the raw half of attribution, in authored
//! ids and scope indices. The
//! [`source_map`](crate::execution::source_map) turns them into the queryable,
//! placed attribution held by the artifact.
//!
//! See `README.md` Part A §5.

use hashbrown::HashSet;

use crate::DataType;
use crate::execution::compile::flat::{
    FlatBinding, FlatEvent, FlatGraph, FlatInput, FlatNode, FlatOutput, PendingSubscription,
};
use crate::execution::identity::{ExecutionEventPort, ExecutionNodeId, ExecutionOutputPort};
use crate::execution::source_map::Leaf;
use crate::graph::MAX_NESTING_DEPTH;
use crate::graph::definition::{GraphDef, GraphLink};
use crate::graph::func::{Func, FuncInput, OutputType};
use crate::graph::identity::{GraphId, InputPort, NodeId, OutputPort};
use crate::graph::node::NodeKind;
use crate::graph::node::special::SpecialNode;
use crate::graph::{Binding, Graph, NodeSearch};
use crate::library::Library;

/// Reusable traversal scratch owned by the
/// [`Compiler`](crate::execution::compile::Compiler). Only the descent's own
/// state lives here; everything the walk *produces* goes straight into the
/// [`FlatGraph`] it returns, so no part of one flatten survives into the next.
/// The per-build resolved-graph stack lives on `Run` (it borrows the build's
/// graph), keeping this struct free of borrowed references.
#[derive(Debug, Default)]
pub(super) struct Flattener {
    path: Vec<NodeId>,
    /// Scope indices parallel to the emit-descent in `path` — the scope each
    /// level's nodes live in.
    scope_stack: Vec<u32>,
    /// Shared graphs currently on the emit-descent path.
    seen_shared: HashSet<GraphId>,
    /// One node's resolved inputs, refilled per node. They are resolved before
    /// the ports are appended, so a [`FlatInput`] is whole the moment it exists.
    node_inputs: Vec<FlatInput>,
}

impl Flattener {
    /// Lower `root` against `library` into a flat graph. One step, one value:
    /// the walk descends the authoring graph and appends what it finds, and
    /// what comes back is complete — no field of it is filled in later and no
    /// final program or artifact type is touched on the way.
    pub(super) fn flatten(&mut self, root: &Graph, library: &Library) -> FlatGraph {
        self.path.clear();
        self.seen_shared.clear();
        self.node_inputs.clear();
        // The descent opens child scopes under the root, which every walk
        // starts on.
        self.scope_stack.clear();
        self.scope_stack.push(0);

        let mut flat = FlatGraph::default();
        let mut run = Run {
            library,
            path: &mut self.path,
            levels: vec![root],
            scope_stack: &mut self.scope_stack,
            seen_shared: &mut self.seen_shared,
            node_inputs: &mut self.node_inputs,
            flat: &mut flat,
        };
        run.emit(false);
        flat
    }
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
    seen_shared: &'a mut HashSet<GraphId>,
    /// Per-node scratch — see [`Flattener::node_inputs`].
    node_inputs: &'a mut Vec<FlatInput>,
    /// Everything the walk produces, appended as it goes.
    flat: &'a mut FlatGraph,
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

    /// Open a scope for `instance_id` under the current one, returning its
    /// index. Parents always precede their children, which is what makes a
    /// leaf's walk to the root terminate.
    fn push_scope(&mut self, instance_id: NodeId) -> u32 {
        let parent = *self.scope_stack.last().unwrap();
        self.flat.scopes.push(instance_id, parent)
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
                    self.emit_instance(node.id, *link, disabled);
                    continue;
                }
                NodeKind::GraphInput | NodeKind::GraphOutput => continue,
            };

            let e_node_id = self.execution_node_id(node.id);

            // Every port is read fresh from the func each build (never carried
            // over from the last one): the library can evolve between updates —
            // a changed `required` flag, a grown input list, a retyped output —
            // and this is where that lands.
            //
            // Bindings are resolved *before* the ports are appended, so each
            // input is whole when it enters the pool rather than being revisited
            // by index afterwards.
            self.node_inputs.clear();
            for (port_idx, func_input) in func.inputs.iter().enumerate() {
                let port = InputPort::new(node.id, port_idx);
                let binding = self.typed_binding(graph, func_input, graph.bindings.get(&port));
                self.node_inputs.push(FlatInput {
                    required: func_input.required,
                    stamps_fs_path: matches!(&func_input.data_type, DataType::FsPath(_)),
                    binding,
                });
            }
            let inputs = self.flat.inputs.append(self.node_inputs.drain(..));

            let outputs =
                self.flat.outputs.append(func.outputs.iter().map(
                    |func_output| match &func_output.ty {
                        OutputType::Fixed(data_type) => FlatOutput::Fixed(data_type.clone()),
                        // The mirrored input's declared type comes along so linking
                        // can resolve a `Const` mirror without the library.
                        OutputType::Wildcard { mirrors } => FlatOutput::Wildcard {
                            mirrors: *mirrors as u32,
                            mirrored_declared: func.inputs[*mirrors].data_type.clone(),
                        },
                    },
                ));

            let events = self
                .flat
                .events
                .append(func.events.iter().map(|func_event| FlatEvent {
                    lambda: func_event.event_lambda.clone(),
                }));

            // Id uniqueness is enforced when linking places these, so the walk
            // just appends. The leaf goes in the same push — where this flat
            // node came from (current scope + authoring id) is known here and
            // nowhere later.
            self.flat.nodes.push(FlatNode {
                id: e_node_id,
                leaf: Leaf {
                    scope: *self.scope_stack.last().unwrap(),
                    node_id: node.id,
                },
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
            });
        }

        if !ancestor_disabled {
            self.collect_subscriptions(graph);
        }
    }

    /// Dissolve one composite instance: open its scope, emit its interior in
    /// place, and leave every stack as it was found.
    fn emit_instance(&mut self, instance_id: NodeId, link: GraphLink, disabled: bool) {
        let shared_id = match link {
            GraphLink::Shared(id) => Some(id),
            GraphLink::Local(_) => None,
        };
        if let Some(id) = shared_id
            && !self.seen_shared.insert(id)
        {
            panic!("recursive shared graph {id:?} (it contains itself)");
        }
        let nested = self
            .current()
            .resolve_graph(link, self.library)
            .expect("graph node references a missing graph");

        self.push_level(instance_id, &nested.body);
        self.record_exposed_outputs(instance_id, nested);
        // Open this instance's scope under the current one; its interior
        // nodes record their leaves against it.
        let scope = self.push_scope(instance_id);
        self.scope_stack.push(scope);

        self.emit(disabled);

        self.scope_stack.pop();
        self.pop_level();
        if let Some(id) = shared_id {
            self.seen_shared.remove(&id);
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
                self.push_level(node_id, &nested.body);
                let source = self.resolve_exposed_output(nested, port_idx);
                self.pop_level();
                source
            }
            // Follow out: this GraphInput output `port_idx` is the enclosing
            // instance's exposed input `port_idx`; resolve it one level up.
            NodeKind::GraphInput => {
                let instance_id = *self.path.last().expect("GraphInput at the root level");
                self.pop_level();
                let outer = self.current();
                let binding = outer.bindings.get(&InputPort::new(instance_id, port_idx));
                // The level below was descended through this very node, so
                // it is here and it is a graph — only the port may have
                // gone, which is drift.
                let instance = outer
                    .find(instance_id, NodeSearch::TopLevel)
                    .expect("the enclosing level was descended through this instance");
                let NodeKind::Graph(link) = &instance.kind else {
                    panic!("only a graph node opens a level");
                };
                let def = outer
                    .resolve_graph(*link, self.library)
                    .expect("graph node references a missing graph");
                // The instance's own interface declares this port, so the
                // exterior wire is gated against it — the same `FuncInput`
                // the interior side would be gated against, picker list and
                // optionality included.
                let source = match def.inputs.get(port_idx) {
                    Some(declared) => self.typed_binding(outer, declared, binding),
                    None => FlatBinding::None,
                };
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
            Some(Binding::Const(value)) => !self.library.const_satisfies(input, value),
            None => false,
        };
        if mismatched {
            return FlatBinding::None;
        }
        self.resolve_binding(binding)
    }

    /// Resolve `nested`'s interface output `port_idx` through the interior
    /// wiring behind it, gated by the declared port type. Call with
    /// `nested`'s own level pushed.
    ///
    /// The single definition of "what comes out of this port", shared by
    /// the on-demand hop in [`Self::resolve`] and the eager record in
    /// [`Self::record_exposed_outputs`] — the two must agree, or an
    /// instance would name a producer its consumers never see.
    fn resolve_exposed_output(&mut self, nested: &'a GraphDef, port_idx: usize) -> FlatBinding {
        // Drift can leave the definition without a boundary node, or the
        // interface without this port at all.
        let (Some(output_node), Some(declared)) = (
            nested.body.boundary_node(NodeKind::GraphOutput),
            nested.outputs.get(port_idx),
        ) else {
            return FlatBinding::None;
        };
        let declared = declared.ty.declared();
        let binding = nested
            .body
            .bindings
            .get(&InputPort::new(output_node, port_idx));
        self.typed_boundary_binding(&nested.body, &declared, binding)
    }

    /// Note which execution node backs each of `nested`'s interface output
    /// ports, against the instance `instance_id`. Called with that
    /// instance's level already pushed.
    ///
    /// Eager, and for every instance — not only those something binds to.
    /// The finished program has no `GraphOutput` edges left, so nothing
    /// downstream can tell an exposed producer from interior plumbing:
    /// with an interior reader and no exterior one, its consumer set is
    /// entirely inside the footprint, exactly like a node the instance
    /// merely uses. That is the shape `run_targets` used to skip.
    fn record_exposed_outputs(&mut self, instance_id: NodeId, nested: &'a GraphDef) {
        for port_idx in 0..nested.outputs.len() {
            if let FlatBinding::Bind(port) = self.resolve_exposed_output(nested, port_idx) {
                self.flat.exposed.push((instance_id, port.e_node_id));
            }
        }
    }

    /// [`Self::typed_binding`] for a **boundary** port, whose declaration is
    /// a bare [`DataType`] rather than a `FuncInput`.
    ///
    /// A composite's interface is a type contract in its own right. The
    /// outer gate checks the consumer against the *declared* port type and
    /// passes; crossing the boundary through raw `resolve_binding` then
    /// ignored that declaration entirely, so a definition declaring an
    /// `Int` output while wiring it to a `String` producer compiled a
    /// direct `Bind` from that producer into an `Int` consumer. Nothing
    /// downstream could see it — the edge that would have shown the
    /// mismatch is exactly the one flattening dissolves.
    fn typed_boundary_binding(
        &mut self,
        graph: &'a Graph,
        declared: &DataType,
        binding: Option<&Binding>,
    ) -> FlatBinding {
        let mismatched = match binding {
            Some(Binding::Bind(src)) => {
                !declared.compatible_with(&graph.resolve_output_type(self.library, *src))
            }
            Some(Binding::Const(value)) => !self.library.declared_accepts_const(declared, value),
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
        // Both callers name a validated node: a subscription's emitter, or
        // an interface event's. `None` below is drift or a deliberate skip,
        // never an id this graph has no node for.
        let node = graph
            .find(node_id, NodeSearch::TopLevel)
            .expect("subscription emitter resolved by validate_for_execution");
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
                let nested = graph
                    .resolve_graph(*r, self.library)
                    .expect("graph node references a missing graph");
                // Drift: the interface may no longer expose this event.
                let exposed = nested.events.get(event_idx)?;
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
        // Every caller names a validated subscriber, at this level or a
        // nested one.
        let node = graph
            .find(node_id, NodeSearch::TopLevel)
            .expect("subscriber resolved by validate_for_execution");
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
                self.flat.subscriptions.push(PendingSubscription {
                    event,
                    subscriber: e_node_id,
                });
            }
            NodeKind::Graph(r) => {
                let nested = graph
                    .resolve_graph(*r, self.library)
                    .expect("graph node references a missing graph");
                // A definition with no inbound boundary has nothing to
                // deliver the event to — authored, not broken.
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
}

#[cfg(test)]
mod tests;
