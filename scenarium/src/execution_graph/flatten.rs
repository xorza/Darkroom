//! Subgraph flattening, fused into execution-node building. Walks the
//! authoring `Graph`, expands every composite instance into its interior func
//! nodes, and writes them straight into the execution graph's `CompactInsert`
//! — no intermediate `Graph` is materialized. Boundary nodes
//! (`SubgraphInput`/`SubgraphOutput`) and composites dissolve; their edges are
//! short-circuited so the result is a flat, func-only execution graph on which
//! the existing scheduler (dead-branch pruning, caching, cycle detection)
//! works across composite boundaries unchanged. Both data bindings and event
//! subscriptions are short-circuited across boundaries (exposed events resolve
//! to their interior emitter; triggering a composite reaches the interior
//! nodes wired to its `SubgraphInput`).
//!
//! See `docs/subgraph-design.md` §5.

use std::hash::Hasher;

use common::fnv::FnvHasher;
use common::key_index_vec::{CompactInsert, KeyIndexVec};
use hashbrown::HashSet;

use super::{
    ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode, ExecutionOutput,
    ExecutionPortAddress, Span,
};
use crate::data::StaticValue;
use crate::function::FuncLib;
use crate::graph::{Binding, Graph, InputPort, NodeId, NodeKind, Subscription};
use crate::subgraph::SubgraphId;

/// Hard cap on nesting depth — a release backstop for the output-resolution
/// walk (which follows composite edges and isn't covered by the emit-descent
/// recursion guard), so a pathological recursive definition can't loop
/// forever. The emit descent rejects recursion precisely via `seen` first.
const MAX_DEPTH: usize = 256;

/// Reusable flattening scratch, owned by the `ExecutionGraph` so its buffer is
/// not re-allocated every `update`. The only state is the descent path
/// (`ids`); the current graph at each level is re-derived from it on demand
/// (`graph_at`), which is cheap at realistic nesting depth and keeps the
/// struct free of borrowed references (so it can persist).
#[derive(Debug, Default)]
pub(super) struct Flattener {
    ids: Vec<NodeId>,
    /// Subgraph defs currently on the emit-descent path — a def appearing
    /// twice is recursion (it contains itself). Reused across builds.
    seen: HashSet<SubgraphId>,
    /// Resolved flat event edges (with flattened ids), collected during the
    /// walk and applied after the node pass (when `e_nodes` is final and
    /// addressable by key). Reused across builds.
    subs: Vec<Subscription>,
    /// Reusable scratch for the next `inputs` pool, built during emit and
    /// swapped into the graph (the displaced old pool returns here for reuse).
    inputs_scratch: Vec<ExecutionInput>,
}

/// The graph's SoA pools, rebuilt each `build`. `inputs` is the new pool being
/// filled (carries over reused nodes' bindings); `old_inputs` is last build's
/// pool, kept readable for that carry-over.
pub(super) struct Pools<'a> {
    pub inputs: &'a mut Vec<ExecutionInput>,
    pub outputs: &'a mut Vec<ExecutionOutput>,
    pub events: &'a mut Vec<ExecutionEvent>,
}

impl Flattener {
    /// Flatten `root` into `e_nodes` (via compact insert, preserving caches),
    /// rebuilding the SoA pools. Inputs carry over per-node `binding`/
    /// `binding_changed` for reused nodes; outputs/events are rebuilt fresh.
    pub(super) fn build(
        &mut self,
        e_nodes: &mut KeyIndexVec<NodeId, ExecutionNode>,
        pools: Pools<'_>,
        root: &Graph,
        func_lib: &FuncLib,
    ) {
        self.ids.clear();
        self.seen.clear();
        self.subs.clear();

        let mut new_inputs = std::mem::take(&mut self.inputs_scratch);
        new_inputs.clear();
        pools.outputs.clear();
        pools.events.clear();
        {
            let mut run = Run {
                root,
                func_lib,
                ids: &mut self.ids,
                seen: &mut self.seen,
                subs: &mut self.subs,
                compact: e_nodes.compact_insert_start(),
                cur_idx: 0,
                old_inputs: pools.inputs.as_slice(),
                new_inputs: &mut new_inputs,
                outputs: pools.outputs,
                events: pools.events,
            };
            run.emit();
            // `compact` finalizes on drop, trimming nodes that disappeared.
        }
        // Swap the freshly built inputs pool in; recycle the old one as scratch.
        std::mem::swap(pools.inputs, &mut new_inputs);
        self.inputs_scratch = new_inputs;

        // Apply resolved event edges now that every flat emitter exists and is
        // addressable by key. Subscribers were cleared while rebuilding events.
        for s in &self.subs {
            if let Some(e_node) = e_nodes.by_key_mut(&s.emitter) {
                let span = e_node.events.range();
                pools.events[span][s.event_idx]
                    .subscribers
                    .push(s.subscriber);
            }
        }
    }
}

/// The graph at the level addressed by `path` — descend from `root`, resolving
/// each composite instance through its (linked or local) definition.
fn graph_at<'a>(root: &'a Graph, func_lib: &'a FuncLib, path: &[NodeId]) -> &'a Graph {
    let mut graph = root;
    for id in path {
        let r = graph
            .by_id(id)
            .unwrap()
            .kind
            .as_subgraph()
            .expect("descent path id must be a composite");
        graph = &graph
            .resolve_def(r, func_lib)
            .expect("subgraph node references a missing definition")
            .graph;
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
    let mix = |seed: &[u8]| -> u64 {
        let mut h = FnvHasher::new();
        h.write(seed);
        for id in path {
            h.write(&id.as_u128().to_le_bytes());
        }
        h.write(&interior.as_u128().to_le_bytes());
        h.finish()
    };
    NodeId::from_u128(((mix(b"hi") as u128) << 64) | mix(b"lo") as u128)
}

/// Where an output reference resolves once boundaries are followed through.
enum Source {
    Producer { node_id: NodeId, port_idx: usize },
    Const(StaticValue),
    None,
}

/// One flattening pass. Borrows the reusable `ids` buffer from `Flattener`;
/// the current graph at each level is `graph_at(root, func_lib, ids)`.
struct Run<'a> {
    root: &'a Graph,
    func_lib: &'a FuncLib,
    ids: &'a mut Vec<NodeId>,
    seen: &'a mut HashSet<SubgraphId>,
    subs: &'a mut Vec<Subscription>,
    compact: CompactInsert<'a, NodeId, ExecutionNode>,
    /// Index of the leaf currently being filled. Stable across the target
    /// inserts in `set_input`: `compact_insert` only swaps slots at indices
    /// `>= write_idx`, never the already-compacted consumer.
    cur_idx: usize,
    /// Last build's inputs pool, read to carry over reused nodes' bindings.
    old_inputs: &'a [ExecutionInput],
    /// The inputs pool being built this update; `cur_idx`'s span is its tail.
    new_inputs: &'a mut Vec<ExecutionInput>,
    outputs: &'a mut Vec<ExecutionOutput>,
    events: &'a mut Vec<ExecutionEvent>,
}

impl<'a> Run<'a> {
    fn current(&self) -> &'a Graph {
        graph_at(self.root, self.func_lib, self.ids.as_slice())
    }

    /// Descend one composite level. The depth cap is a backstop for the
    /// output-resolution walks (which the `seen` recursion guard in `emit`
    /// doesn't cover); a legitimate graph never nests this deep.
    fn push_level(&mut self, instance_id: NodeId) {
        assert!(
            self.ids.len() < MAX_DEPTH,
            "subgraph nesting exceeds {MAX_DEPTH} levels (recursive definition?)"
        );
        self.ids.push(instance_id);
    }

    /// Emit execution nodes for the current level's graph, recursing into
    /// composite instances.
    fn emit(&mut self) {
        let graph = self.current();

        for node in graph.iter() {
            match &node.kind {
                NodeKind::Func(func_id) => {
                    let flat_id = flatten_id(self.ids.as_slice(), node.id);
                    let func = self.func_lib.by_id(func_id).unwrap();
                    let input_count = func.inputs.len();
                    let (idx, e_node) = self.compact.insert_with(&flat_id, || ExecutionNode {
                        id: flat_id,
                        ..Default::default()
                    });
                    let was_inited = e_node.inited;
                    let old_inputs_span = e_node.inputs;
                    if was_inited {
                        assert_eq!(
                            e_node.func_id, func.id,
                            "func changed under a reused node id"
                        );
                    }

                    let outputs_start = self.outputs.len() as u32;
                    self.outputs.extend(std::iter::repeat_n(
                        ExecutionOutput::default(),
                        func.outputs.len(),
                    ));
                    let events_start = self.events.len() as u32;
                    for func_event in &func.events {
                        self.events.push(ExecutionEvent {
                            lambda: func_event.event_lambda.clone(),
                            ..Default::default()
                        });
                    }

                    let inputs_start = self.new_inputs.len() as u32;
                    if was_inited {
                        self.new_inputs
                            .extend_from_slice(&self.old_inputs[old_inputs_span.range()]);
                    } else {
                        for func_input in &func.inputs {
                            self.new_inputs.push(ExecutionInput {
                                required: func_input.required,
                                data_type: func_input.data_type.clone(),
                                ..Default::default()
                            });
                        }
                    }

                    let e_node = &mut self.compact[idx];
                    e_node.inited = true;
                    e_node.func_id = func.id;
                    if !was_inited {
                        e_node.lambda = func.lambda.clone();
                    }
                    e_node.inputs = Span {
                        start: inputs_start,
                        len: input_count as u32,
                    };
                    e_node.outputs = Span {
                        start: outputs_start,
                        len: func.outputs.len() as u32,
                    };
                    e_node.events = Span {
                        start: events_start,
                        len: func.events.len() as u32,
                    };
                    e_node.terminal = func.terminal;
                    e_node.behavior = ExecutionNode::compute_behavior(node.behavior, func.behavior);
                    e_node.name.clear();
                    e_node.name.push_str(&node.name);

                    self.cur_idx = idx;

                    for (input_idx, binding) in graph.node_bindings(node.id, input_count) {
                        let source = match binding {
                            Binding::None => Source::None,
                            Binding::Const(v) => Source::Const(v),
                            Binding::Bind(op) => self.resolve(op.node_id, op.port_idx),
                        };
                        self.set_input(input_idx, source);
                    }
                }
                NodeKind::Subgraph(r) => {
                    assert!(
                        self.seen.insert(r.id()),
                        "recursive subgraph {:?} (it contains itself)",
                        r.id()
                    );
                    self.push_level(node.id);
                    self.emit();
                    self.ids.pop();
                    self.seen.remove(&r.id());
                }
                NodeKind::SubgraphInput | NodeKind::SubgraphOutput => {}
            }
        }

        self.collect_subscriptions(graph);
    }

    /// Resolve this level's event subscriptions across composite boundaries
    /// into flat `(emitter, event_idx, subscriber)` edges. Subscriptions
    /// emitted *by* a `SubgraphInput` (the trigger) are consumed when the
    /// enclosing instance is resolved as a subscriber, so they are skipped here.
    fn collect_subscriptions(&mut self, graph: &'a Graph) {
        let trigger = graph
            .iter()
            .find(|n| matches!(n.kind, NodeKind::SubgraphInput))
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
        match &graph.by_id(&node_id)?.kind {
            NodeKind::Func(_) => Some((flatten_id(self.ids.as_slice(), node_id), event_idx)),
            NodeKind::Subgraph(r) => {
                let def = graph.resolve_def(*r, self.func_lib)?;
                let exposed = def.events.get(event_idx)?;
                let (interior, interior_idx) = (exposed.emitter, exposed.emitter_event_idx);
                self.push_level(node_id);
                let resolved = self.resolve_emitter(interior, interior_idx);
                self.ids.pop();
                resolved
            }
            NodeKind::SubgraphInput | NodeKind::SubgraphOutput => None,
        }
    }

    /// Resolve a subscriber to the concrete flat func nodes that actually run,
    /// pushing `(emitter, event_idx, flat_subscriber)` for each. A composite
    /// subscriber expands to the interior nodes wired to its `SubgraphInput`
    /// trigger.
    fn resolve_subscriber(&mut self, node_id: NodeId, emitter: NodeId, event_idx: usize) {
        let graph = self.current();
        match graph.by_id(&node_id).map(|n| &n.kind) {
            Some(NodeKind::Func(_)) => {
                let flat = flatten_id(self.ids.as_slice(), node_id);
                self.push_edge(emitter, event_idx, flat);
            }
            Some(NodeKind::Subgraph(r)) => {
                let Some(def) = graph.resolve_def(*r, self.func_lib) else {
                    return;
                };
                let Some(trigger) = def
                    .graph
                    .iter()
                    .find(|n| matches!(n.kind, NodeKind::SubgraphInput))
                    .map(|n| n.id)
                else {
                    return;
                };
                let interior: Vec<NodeId> = def
                    .graph
                    .subscriptions()
                    .filter(|s| s.emitter == trigger)
                    .map(|s| s.subscriber)
                    .collect();
                self.push_level(node_id);
                for sub in interior {
                    self.resolve_subscriber(sub, emitter, event_idx);
                }
                self.ids.pop();
            }
            _ => {}
        }
    }

    /// Mutable handle to input `input_idx` of the node currently being filled,
    /// living in the new pool at `cur_idx`'s span.
    fn cur_input(&mut self, input_idx: usize) -> &mut ExecutionInput {
        let pool_idx = self.compact[self.cur_idx].inputs.start as usize + input_idx;
        &mut self.new_inputs[pool_idx]
    }

    /// Write the resolved source into input `cur_idx`/`input_idx`, tracking
    /// `binding_changed` so caching stays correct across updates.
    fn set_input(&mut self, input_idx: usize, source: Source) {
        match source {
            Source::None => {
                let e_input = self.cur_input(input_idx);
                if !matches!(e_input.binding, ExecutionBinding::None) {
                    e_input.binding = ExecutionBinding::None;
                    e_input.binding_changed = true;
                }
            }
            Source::Const(v) => {
                let e_input = self.cur_input(input_idx);
                if !matches!(&e_input.binding, ExecutionBinding::Const(existing) if *existing == v)
                {
                    e_input.binding = ExecutionBinding::Const(v);
                    e_input.binding_changed = true;
                }
            }
            Source::Producer { node_id, port_idx } => {
                let (target_idx, _) = self.compact.insert_with(&node_id, || ExecutionNode {
                    id: node_id,
                    ..Default::default()
                });
                let e_input = self.cur_input(input_idx);
                match &mut e_input.binding {
                    ExecutionBinding::Bind(existing)
                        if existing.target_id == node_id && existing.port_idx == port_idx =>
                    {
                        existing.target_idx = target_idx;
                    }
                    _ => {
                        e_input.binding = ExecutionBinding::Bind(ExecutionPortAddress {
                            target_id: node_id,
                            target_idx,
                            port_idx,
                        });
                        e_input.binding_changed = true;
                    }
                }
            }
        }
    }

    /// Resolve an output reference `(node_id, port_idx)` in the current frame
    /// to a concrete flat producer, following through boundary and composite
    /// nodes. Leaves the descent stack as it found it.
    fn resolve(&mut self, node_id: NodeId, port_idx: usize) -> Source {
        let graph = self.current();
        match &graph
            .by_id(&node_id)
            .expect("binding to a missing node")
            .kind
        {
            NodeKind::Func(_) => Source::Producer {
                node_id: flatten_id(self.ids.as_slice(), node_id),
                port_idx,
            },
            // Follow into the composite: its output `port_idx` is wired by the
            // SubgraphOutput node's input `port_idx`.
            NodeKind::Subgraph(r) => {
                let def = graph
                    .resolve_def(*r, self.func_lib)
                    .expect("subgraph node references a missing definition");
                let Some(so) = def
                    .graph
                    .iter()
                    .find(|n| matches!(n.kind, NodeKind::SubgraphOutput))
                else {
                    return Source::None;
                };
                let binding = def.graph.input_binding(InputPort {
                    node_id: so.id,
                    port_idx,
                });
                self.push_level(node_id);
                let source = self.resolve_binding(&binding);
                self.ids.pop();
                source
            }
            // Follow out: this SubgraphInput output `port_idx` is the enclosing
            // instance's exposed input `port_idx`; resolve it one level up.
            NodeKind::SubgraphInput => {
                let instance_id = self.ids.pop().expect("SubgraphInput at the root level");
                let binding = self.current().input_binding(InputPort {
                    node_id: instance_id,
                    port_idx,
                });
                let source = self.resolve_binding(&binding);
                self.ids.push(instance_id);
                source
            }
            NodeKind::SubgraphOutput => Source::None,
        }
    }

    fn resolve_binding(&mut self, binding: &Binding) -> Source {
        match binding {
            Binding::None => Source::None,
            Binding::Const(v) => Source::Const(v.clone()),
            Binding::Bind(op) => self.resolve(op.node_id, op.port_idx),
        }
    }
}
