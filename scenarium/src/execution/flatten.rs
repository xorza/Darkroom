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

use common::FnvHasher;
use common::{CompactInsert, KeyIndexVec, Span};
use hashbrown::HashSet;

use crate::data::StaticValue;
use crate::execution::program::{
    ExecutionBehavior, ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode,
    ExecutionPortAddress,
};
use crate::execution_stats::FlattenMap;
use crate::function::FuncLib;
use crate::graph::{Binding, Graph, InputPort, NodeBehavior, NodeId, NodeKind, Subscription};
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
pub(crate) struct Flattener {
    ids: Vec<NodeId>,
    /// `FlattenMap` scope indices parallel to the emit-descent in `ids` —
    /// the scope each level's nodes live in. Reused across builds.
    scope_stack: Vec<u32>,
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
    /// Reusable scratch for the next per-input dirty column, built in lockstep
    /// with `inputs_scratch` and swapped into the executor's `input_dirty`.
    dirty_scratch: Vec<bool>,
}

/// The graph's SoA pools, rebuilt each `build`. `inputs` is the new pool being
/// filled (carries over reused nodes' bindings); `old_inputs` is last build's
/// pool, kept readable for that carry-over. Outputs carry no static per-node
/// data, so there is no output pool — each node's output span is assigned from
/// a running counter local to the build.
pub(crate) struct Pools<'a> {
    pub inputs: &'a mut Vec<ExecutionInput>,
    pub events: &'a mut Vec<ExecutionEvent>,
    /// The executor's cross-run per-input dirty column, rebuilt in lockstep
    /// with `inputs` (reused nodes carry their bits over, new inputs start
    /// clean, then `set_input` marks changed bindings).
    pub input_dirty: &'a mut Vec<bool>,
}

impl Flattener {
    /// Flatten `root` into `e_nodes` (via compact insert, preserving caches),
    /// rebuilding the SoA pools. Inputs carry over per-node `binding` and dirty
    /// bit for reused nodes; outputs/events are rebuilt fresh. Returns the total
    /// output count assigned across all nodes.
    pub(crate) fn build(
        &mut self,
        e_nodes: &mut KeyIndexVec<NodeId, ExecutionNode>,
        pools: Pools<'_>,
        root: &Graph,
        func_lib: &FuncLib,
        flatten: &mut FlattenMap,
    ) -> u32 {
        self.ids.clear();
        self.seen.clear();
        self.subs.clear();
        // Reset to a lone root scope; emit pushes child scopes as it
        // descends composites (scope 0 is the root the stack starts on).
        flatten.reset();
        self.scope_stack.clear();
        self.scope_stack.push(0);

        let mut new_inputs = std::mem::take(&mut self.inputs_scratch);
        new_inputs.clear();
        let mut new_dirty = std::mem::take(&mut self.dirty_scratch);
        new_dirty.clear();
        pools.events.clear();
        let n_outputs;
        {
            let mut run = Run {
                root,
                func_lib,
                ids: &mut self.ids,
                scope_stack: &mut self.scope_stack,
                flatten,
                seen: &mut self.seen,
                subs: &mut self.subs,
                compact: e_nodes.compact_insert_start(),
                cur_idx: 0,
                old_inputs: pools.inputs.as_slice(),
                new_inputs: &mut new_inputs,
                old_dirty: pools.input_dirty.as_slice(),
                new_dirty: &mut new_dirty,
                n_outputs: 0,
                events: pools.events,
                once_depth: 0,
            };
            run.emit();
            n_outputs = run.n_outputs;
            // `compact` finalizes on drop, trimming nodes that disappeared.
        }
        // Swap the freshly built pools in; recycle the old ones as scratch.
        std::mem::swap(pools.inputs, &mut new_inputs);
        self.inputs_scratch = new_inputs;
        std::mem::swap(pools.input_dirty, &mut new_dirty);
        self.dirty_scratch = new_dirty;

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

        n_outputs
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
    /// Scope indices parallel to `ids` (the emit descent). `last()` is the
    /// scope the current level's nodes live in.
    scope_stack: &'a mut Vec<u32>,
    /// The flatten map being built — leaves recorded per func node, scopes
    /// pushed per composite descent.
    flatten: &'a mut FlattenMap,
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
    /// Last build's dirty column, read to carry over reused nodes' dirty bits.
    old_dirty: &'a [bool],
    /// The dirty column being built this update, parallel to `new_inputs`.
    new_dirty: &'a mut Vec<bool>,
    /// Running total of outputs emitted so far; also the next output span start.
    n_outputs: u32,
    events: &'a mut Vec<ExecutionEvent>,
    /// How many `Once` composite instances enclose the node being emitted.
    /// While `> 0`, every interior func node is forced to
    /// `ExecutionBehavior::Once` so the whole subgraph computes once and
    /// then caches — the composite-level reading of `NodeBehavior::Once`.
    once_depth: usize,
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
            // Disabled nodes are skipped entirely: no execution node, no
            // recursion into a disabled composite. A consumer bound to a
            // skipped node's output resolves to `Source::None` (see
            // `resolve`), so the wire reads as unbound downstream.
            if node.disabled {
                continue;
            }
            match &node.kind {
                NodeKind::Func(func_id) => {
                    let flat_id = flatten_id(self.ids.as_slice(), node.id);
                    let func = self
                        .func_lib
                        .by_id(func_id)
                        .expect("func resolved by update's check_with pre-check");
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

                    let outputs_start = self.n_outputs;
                    self.n_outputs += func.outputs.len() as u32;
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
                        self.new_dirty
                            .extend_from_slice(&self.old_dirty[old_inputs_span.range()]);
                    } else {
                        for func_input in &func.inputs {
                            self.new_inputs.push(ExecutionInput {
                                required: func_input.required,
                                data_type: func_input.data_type.clone(),
                                ..Default::default()
                            });
                            self.new_dirty.push(false);
                        }
                    }

                    let e_node = &mut self.compact[idx];
                    e_node.inited = true;
                    e_node.func_id = func.id;
                    if !was_inited {
                        e_node.lambda = func.lambda.clone();
                    }
                    e_node.inputs = Span::new(inputs_start, input_count as u32);
                    e_node.outputs = Span::new(outputs_start, func.outputs.len() as u32);
                    e_node.events = Span::new(events_start, func.events.len() as u32);
                    e_node.terminal = func.terminal;
                    // Inside a `Once` composite the whole interior is frozen
                    // after its first run; otherwise the node's own behavior
                    // applies.
                    e_node.behavior = if self.once_depth > 0 {
                        ExecutionBehavior::Once
                    } else {
                        ExecutionNode::compute_behavior(node.behavior, func.behavior)
                    };
                    e_node.name.clear();
                    e_node.name.push_str(&node.name);

                    // Record where this flat node came from (current scope
                    // + authoring id) so stats map back to editor nodes.
                    let scope = *self.scope_stack.last().unwrap();
                    self.flatten.set_leaf(flat_id, scope, node.id);

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
                    // A `Once` composite freezes its whole interior: bump the
                    // depth so every interior func node flattens as `Once`.
                    let once = node.behavior == NodeBehavior::Once;
                    self.once_depth += once as usize;
                    self.push_level(node.id);
                    // Open this instance's scope under the current one; its
                    // interior nodes record their leaves against it.
                    let parent = *self.scope_stack.last().unwrap();
                    let scope = self.flatten.push_scope(node.id, parent);
                    self.scope_stack.push(scope);
                    self.emit();
                    self.scope_stack.pop();
                    self.ids.pop();
                    self.once_depth -= once as usize;
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
        let node = graph.by_id(&node_id)?;
        if node.disabled {
            return None; // a disabled node fires no events
        }
        match &node.kind {
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
        // A disabled node runs nothing, so it receives no events.
        if graph.by_id(&node_id).is_some_and(|n| n.disabled) {
            return;
        }
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

    /// Pool index of input `input_idx` of the node currently being filled,
    /// living in the new pool at `cur_idx`'s span.
    fn cur_input_idx(&self, input_idx: usize) -> usize {
        self.compact[self.cur_idx].inputs.start as usize + input_idx
    }

    /// Write the resolved source into input `cur_idx`/`input_idx`, marking the
    /// dirty bit when the binding changes so caching stays correct across updates.
    fn set_input(&mut self, input_idx: usize, source: Source) {
        let pool_idx = self.cur_input_idx(input_idx);
        match source {
            Source::None => {
                let e_input = &mut self.new_inputs[pool_idx];
                if !matches!(e_input.binding, ExecutionBinding::None) {
                    e_input.binding = ExecutionBinding::None;
                    self.new_dirty[pool_idx] = true;
                }
            }
            Source::Const(v) => {
                let e_input = &mut self.new_inputs[pool_idx];
                if !matches!(&e_input.binding, ExecutionBinding::Const(existing) if *existing == v)
                {
                    e_input.binding = ExecutionBinding::Const(v);
                    self.new_dirty[pool_idx] = true;
                }
            }
            Source::Producer { node_id, port_idx } => {
                let (target_idx, _) = self.compact.insert_with(&node_id, || ExecutionNode {
                    id: node_id,
                    ..Default::default()
                });
                let e_input = &mut self.new_inputs[pool_idx];
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
                        self.new_dirty[pool_idx] = true;
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
        let node = graph.by_id(&node_id).expect("binding to a missing node");
        // A disabled producer emits nothing, so its outputs have no source:
        // treat the wire as unbound (matches `emit` skipping the node).
        if node.disabled {
            return Source::None;
        }
        match &node.kind {
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
