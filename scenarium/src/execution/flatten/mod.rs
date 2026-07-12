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
//! See `README.md` Part A §5.

use std::hash::Hasher;

use common::FnvHasher;
use common::{CompactInsert, KeyIndexVec, Span};
use hashbrown::HashSet;

use crate::data::{DataType, StaticValue};
use crate::execution::program::{
    ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode, ExecutionPortAddress,
    InputStamper,
};
use crate::execution::stats::FlattenMap;
use crate::graph::subgraph::SubgraphId;
use crate::graph::{
    Binding, Graph, InputPort, NodeId, NodeKind, NodeSearch, OutputPort, Subscription,
};
use crate::library::Library;
use crate::node::function::Func;
use crate::node::special::SpecialNode;

/// Hard cap on nesting depth — a release backstop for the output-resolution
/// walk (which follows composite edges and isn't covered by the emit-descent
/// recursion guard), so a pathological recursive definition can't loop
/// forever. The emit descent rejects recursion precisely via `seen` first.
const MAX_DEPTH: usize = 256;

/// Reusable flattening scratch, owned by the [`Compiler`](crate::execution::compile::Compiler)
/// so its buffers are not re-allocated every compile. The only state is the descent path
/// (`path`); the current graph at each level is re-derived from it on demand
/// (`graph_at`), which is cheap at realistic nesting depth and keeps the
/// struct free of borrowed references (so it can persist).
#[derive(Debug, Default)]
pub(crate) struct Flattener {
    path: Vec<NodeId>,
    /// `FlattenMap` scope indices parallel to the emit-descent in `path` —
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
}

/// The graph's SoA pools, rebuilt each `build`. Each node's output span is
/// assigned from a running counter local to the build; `output_external_bindings`
/// is the one output-indexed pool filled *during* that build (a plain lookup
/// against the authoring graph, unlike `output_types`, which needs a second
/// pass to follow wildcard mirrors).
pub(crate) struct Pools<'a> {
    pub inputs: &'a mut Vec<ExecutionInput>,
    pub events: &'a mut Vec<ExecutionEvent>,
    pub output_external_bindings: &'a mut Vec<bool>,
}

impl Flattener {
    /// Flatten `root` into `e_nodes` (via compact insert, preserving caches),
    /// rebuilding the SoA pools fresh from the library. Output spans are assigned
    /// from a running counter local to the build; the program's total output count
    /// is then `output_types.len()`, resolved separately.
    pub(crate) fn build(
        &mut self,
        e_nodes: &mut KeyIndexVec<NodeId, ExecutionNode>,
        pools: Pools<'_>,
        root: &Graph,
        library: &Library,
        flatten: &mut FlattenMap,
    ) {
        self.path.clear();
        self.seen.clear();
        self.subs.clear();
        // Reset to a lone root scope; emit pushes child scopes as it
        // descends composites (scope 0 is the root the stack starts on).
        flatten.reset();
        self.scope_stack.clear();
        self.scope_stack.push(0);

        let mut new_inputs = std::mem::take(&mut self.inputs_scratch);
        new_inputs.clear();
        pools.events.clear();
        pools.output_external_bindings.clear();
        {
            let mut run = Run {
                root,
                library,
                path: &mut self.path,
                scope_stack: &mut self.scope_stack,
                flatten,
                seen: &mut self.seen,
                subs: &mut self.subs,
                compact: e_nodes.compact_insert_start(),
                cur_idx: 0,
                new_inputs: &mut new_inputs,
                n_outputs: 0,
                events: pools.events,
                output_external_bindings: pools.output_external_bindings,
            };
            run.emit();
            // One entry pushed per pooled output port, in lockstep with `n_outputs`
            // (see `emit`'s per-node loop) — this is the invariant `seed_output_usage`
            // relies on to index `output_external_bindings` by the same output-pool
            // index as `plan.output_usage`. A hand-built `ExecutionProgram` (as in the
            // executor's low-level tests, which never call `Flattener::build`) doesn't
            // get this guarantee — that's a separate, already-tolerant read path.
            assert_eq!(
                run.output_external_bindings.len(),
                run.n_outputs as usize,
                "output_external_bindings must have exactly one entry per pooled output port"
            );
            // `compact` finalizes on drop, trimming nodes that disappeared.
        }
        // Swap the freshly built pools in; recycle the old ones as scratch.
        std::mem::swap(pools.inputs, &mut new_inputs);
        self.inputs_scratch = new_inputs;

        // Apply resolved event edges now that every flat emitter/subscriber exists and
        // is addressable by key. Both ends resolve to flat positions here (the
        // subscriber to a `NodeIdx`, like a binding target). Subscribers were cleared
        // while rebuilding events.
        for s in &self.subs {
            let Some(subscriber) = e_nodes.index_of_key(&s.subscriber) else {
                continue;
            };
            if let Some(e_node) = e_nodes.by_key_mut(&s.emitter) {
                let span = e_node.events.range();
                pools.events[span][s.event_idx]
                    .subscribers
                    .push(subscriber.into());
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
/// each composite instance through its (linked or local) definition.
fn graph_at<'a>(root: &'a Graph, library: &'a Library, path: &[NodeId]) -> &'a Graph {
    let mut graph = root;
    for id in path {
        let r = graph
            .find_node(id, NodeSearch::TopLevel)
            .unwrap()
            .kind
            .as_subgraph()
            .expect("descent path id must be a composite");
        graph = &graph
            .resolve_def(r, library)
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

/// One flattening pass. Borrows the reusable `path` buffer from `Flattener`;
/// the current graph at each level is `graph_at(root, library, path)`.
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
    seen: &'a mut HashSet<SubgraphId>,
    subs: &'a mut Vec<Subscription>,
    compact: CompactInsert<'a, NodeId, ExecutionNode>,
    /// Index of the leaf currently being filled. Stable across the target
    /// inserts in `set_input`: `compact_insert` only swaps slots at indices
    /// `>= write_idx`, never the already-compacted consumer.
    cur_idx: usize,
    /// The inputs pool being built this update; `cur_idx`'s span is its tail.
    new_inputs: &'a mut Vec<ExecutionInput>,
    /// Running total of outputs emitted so far; also the next output span start.
    n_outputs: u32,
    events: &'a mut Vec<ExecutionEvent>,
    /// Parallel to the output pool (indexed the same way, built alongside it):
    /// whether each pooled output port is externally bound in the authoring graph.
    output_external_bindings: &'a mut Vec<bool>,
}

impl<'a> Run<'a> {
    fn current(&self) -> &'a Graph {
        graph_at(self.root, self.library, self.path.as_slice())
    }

    /// Descend one composite level. The depth cap is a backstop for the
    /// output-resolution walks (which the `seen` recursion guard in `emit`
    /// doesn't cover); a legitimate graph never nests this deep.
    fn push_level(&mut self, instance_id: NodeId) {
        assert!(
            self.path.len() < MAX_DEPTH,
            "subgraph nesting exceeds {MAX_DEPTH} levels (recursive definition?)"
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
            // A subgraph recurses; boundary nodes emit nothing. A func or a
            // special node both resolve to a `&Func` spec and emit one leaf —
            // the spec is the only difference (`library` vs. the hardcoded
            // `SpecialNode::func`), so the emit body below is shared.
            let library = self.library;
            let (func, special): (&Func, Option<SpecialNode>) = match &node.kind {
                NodeKind::Func(func_id) => (
                    library
                        .by_id(func_id)
                        .expect("func resolved by update's check_with validation"),
                    None,
                ),
                NodeKind::Special(s) => (s.func(), Some(*s)),
                NodeKind::Subgraph(r) => {
                    assert!(
                        self.seen.insert(r.id()),
                        "recursive subgraph {:?} (it contains itself)",
                        r.id()
                    );
                    self.push_level(node.id);
                    // Open this instance's scope under the current one; its
                    // interior nodes record their leaves against it.
                    let parent = *self.scope_stack.last().unwrap();
                    let scope = self.flatten.push_scope(node.id, parent);
                    self.scope_stack.push(scope);
                    self.emit();
                    self.scope_stack.pop();
                    self.path.pop();
                    self.seen.remove(&r.id());
                    continue;
                }
                NodeKind::SubgraphInput | NodeKind::SubgraphOutput => continue,
            };

            let flat_id = flatten_id(self.path.as_slice(), node.id);
            let input_count = func.inputs.len();
            let (idx, _) = self.compact.insert_with(&flat_id, || ExecutionNode {
                id: flat_id,
                ..Default::default()
            });

            let outputs_start = self.n_outputs;
            self.n_outputs += func.outputs.len() as u32;
            for port_idx in 0..func.outputs.len() {
                self.output_external_bindings
                    .push(graph.is_externally_bound(OutputPort::new(node.id, port_idx)));
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
            let inputs_start = self.new_inputs.len() as u32;
            for func_input in &func.inputs {
                self.new_inputs.push(ExecutionInput {
                    required: func_input.required,
                    stamper: input_stamper(&func_input.data_type, library),
                    ..Default::default()
                });
            }

            let e_node = &mut self.compact[idx];
            e_node.func_id = func.id;
            e_node.func_version = func.version;
            // Refreshed every build (an Arc clone), so a reused flat node can't keep
            // executing a previous library's lambda after an in-session change.
            e_node.lambda = func.lambda.clone();
            e_node.inputs = Span::new(inputs_start, input_count as u32);
            e_node.outputs = Span::new(outputs_start, func.outputs.len() as u32);
            e_node.events = Span::new(events_start, func.events.len() as u32);
            e_node.sink = func.sink;
            e_node.behavior = func.behavior;
            // Copy the cache mode; whether its disk bit is actually honored is decided
            // by the content digest (a node with an impure cone has no digest and
            // so can't be disk-cached) — see `digest.rs`.
            e_node.cache = node.cache;
            // Special-node identity, recognized by the engine (the planner's
            // run-sinks promotion).
            e_node.special = special;
            e_node.name.clear();
            e_node.name.push_str(&node.name);

            // Record where this flat node came from (current scope + authoring
            // id) so stats map back to editor nodes.
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
        let node = graph.find_node(&node_id, NodeSearch::TopLevel)?;
        if node.disabled {
            return None; // a disabled node fires no events
        }
        match &node.kind {
            NodeKind::Func(_) | NodeKind::Special(_) => {
                Some((flatten_id(self.path.as_slice(), node_id), event_idx))
            }
            NodeKind::Subgraph(r) => {
                let def = graph.resolve_def(*r, self.library)?;
                let exposed = def.events.get(event_idx)?;
                let (interior, interior_idx) = (exposed.emitter, exposed.emitter_event_idx);
                self.push_level(node_id);
                let resolved = self.resolve_emitter(interior, interior_idx);
                self.path.pop();
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
        if graph
            .find_node(&node_id, NodeSearch::TopLevel)
            .is_some_and(|n| n.disabled)
        {
            return;
        }
        match graph
            .find_node(&node_id, NodeSearch::TopLevel)
            .map(|n| &n.kind)
        {
            // A special node subscribes like a func: it flattens to one leaf and
            // becomes the flat subscriber. `RunSinks` in particular relies on
            // this edge so the planner sees it among a fired event's subscribers.
            Some(NodeKind::Func(_) | NodeKind::Special(_)) => {
                let flat = flatten_id(self.path.as_slice(), node_id);
                self.push_edge(emitter, event_idx, flat);
            }
            Some(NodeKind::Subgraph(r)) => {
                let Some(def) = graph.resolve_def(*r, self.library) else {
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
                self.path.pop();
            }
            _ => {}
        }
    }

    /// Pool index of input `input_idx` of the node currently being filled,
    /// living in the new pool at `cur_idx`'s span.
    fn cur_input_idx(&self, input_idx: usize) -> usize {
        self.compact[self.cur_idx].inputs.start as usize + input_idx
    }

    /// Write the resolved source into input `cur_idx`/`input_idx`. A changed
    /// binding changes the node's content digest, which drives cache
    /// invalidation — no per-input dirty tracking is needed.
    fn set_input(&mut self, input_idx: usize, source: Source) {
        let pool_idx = self.cur_input_idx(input_idx);
        let binding = match source {
            Source::None => ExecutionBinding::None,
            Source::Const(v) => ExecutionBinding::Const(v),
            Source::Producer { node_id, port_idx } => {
                let (target_idx, _) = self.compact.insert_with(&node_id, || ExecutionNode {
                    id: node_id,
                    ..Default::default()
                });
                ExecutionBinding::Bind(ExecutionPortAddress {
                    target_idx: target_idx.into(),
                    port_idx,
                })
            }
        };
        self.new_inputs[pool_idx].binding = binding;
    }

    /// Resolve an output reference `(node_id, port_idx)` in the current frame
    /// to a concrete flat producer, following through boundary and composite
    /// nodes. Leaves the descent stack as it found it.
    fn resolve(&mut self, node_id: NodeId, port_idx: usize) -> Source {
        let graph = self.current();
        let node = graph
            .find_node(&node_id, NodeSearch::TopLevel)
            .expect("binding to a missing node");
        // A disabled producer emits nothing, so its outputs have no source:
        // treat the wire as unbound (matches `emit` skipping the node).
        if node.disabled {
            return Source::None;
        }
        match &node.kind {
            NodeKind::Func(_) | NodeKind::Special(_) => Source::Producer {
                node_id: flatten_id(self.path.as_slice(), node_id),
                port_idx,
            },
            // Follow into the composite: its output `port_idx` is wired by the
            // SubgraphOutput node's input `port_idx`.
            NodeKind::Subgraph(r) => {
                let def = graph
                    .resolve_def(*r, self.library)
                    .expect("subgraph node references a missing definition");
                let Some(so) = def
                    .graph
                    .iter()
                    .find(|n| matches!(n.kind, NodeKind::SubgraphOutput))
                else {
                    return Source::None;
                };
                let binding = def.graph.input_binding(InputPort::new(so.id, port_idx));
                self.push_level(node_id);
                let source = self.resolve_binding(&binding);
                self.path.pop();
                source
            }
            // Follow out: this SubgraphInput output `port_idx` is the enclosing
            // instance's exposed input `port_idx`; resolve it one level up.
            NodeKind::SubgraphInput => {
                let instance_id = self.path.pop().expect("SubgraphInput at the root level");
                let binding = self
                    .current()
                    .input_binding(InputPort::new(instance_id, port_idx));
                let source = self.resolve_binding(&binding);
                self.path.push(instance_id);
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

#[cfg(test)]
mod tests;
