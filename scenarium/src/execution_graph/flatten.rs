//! Subgraph flattening, fused into execution-node building. Walks the
//! authoring `Graph`, expands every composite instance into its interior func
//! nodes, and writes them straight into the execution graph's `CompactInsert`
//! — no intermediate `Graph` is materialized. Boundary nodes
//! (`SubgraphInput`/`SubgraphOutput`) and composites dissolve; their edges are
//! short-circuited so the result is a flat, func-only execution graph on which
//! the existing scheduler (dead-branch pruning, caching, cycle detection)
//! works across composite boundaries unchanged.
//!
//! Stage 2a: data-flow (bindings) only. Event subscribers are remapped for
//! func→func edges; edges crossing a composite boundary are Stage 2b.
//!
//! See `docs/subgraph-design.md` §5.

use std::hash::Hasher;

use common::fnv::FnvHasher;
use common::key_index_vec::{CompactInsert, KeyIndexVec};

use super::{ExecutionBinding, ExecutionNode, ExecutionPortAddress};
use crate::data::StaticValue;
use crate::function::FuncLib;
use crate::graph::{Binding, Graph, NodeId, NodeKind};

/// Hard cap on nesting depth — release safety net against an (invalid)
/// recursive definition that slipped past validation, so the walk can't loop
/// forever. Debug `Graph::validate_with` rejects recursion with a precise
/// error well before this; no legitimate graph nests this deep.
const MAX_DEPTH: usize = 256;

/// Reusable flattening scratch, owned by the `ExecutionGraph` so its buffer is
/// not re-allocated every `update`. The only state is the descent path
/// (`ids`); the current graph at each level is re-derived from it on demand
/// (`graph_at`), which is cheap at realistic nesting depth and keeps the
/// struct free of borrowed references (so it can persist).
#[derive(Debug, Default)]
pub(super) struct Flattener {
    ids: Vec<NodeId>,
}

impl Flattener {
    /// Flatten `root` into `e_nodes` (via compact insert, preserving caches).
    pub(super) fn build(
        &mut self,
        e_nodes: &mut KeyIndexVec<NodeId, ExecutionNode>,
        root: &Graph,
        func_lib: &FuncLib,
    ) {
        self.ids.clear();
        let mut run = Run {
            root,
            func_lib,
            ids: &mut self.ids,
            compact: e_nodes.compact_insert_start(),
            cur_idx: 0,
        };
        run.emit();
        // `compact` finalizes on drop, trimming nodes that disappeared.
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
    compact: CompactInsert<'a, NodeId, ExecutionNode>,
    /// Index of the leaf currently being filled. Stable across the target
    /// inserts in `set_input`: `compact_insert` only swaps slots at indices
    /// `>= write_idx`, never the already-compacted consumer.
    cur_idx: usize,
}

impl<'a> Run<'a> {
    fn current(&self) -> &'a Graph {
        graph_at(self.root, self.func_lib, self.ids.as_slice())
    }

    /// Emit execution nodes for the current level's graph, recursing into
    /// composite instances.
    fn emit(&mut self) {
        let graph = self.current();

        for node in graph.iter() {
            match &node.kind {
                NodeKind::Func(func_id) => {
                    let flat_id = flatten_id(self.ids.as_slice(), node.id);
                    let (idx, e_node) = self.compact.insert_with(&flat_id, || ExecutionNode {
                        id: flat_id,
                        ..Default::default()
                    });
                    let func = self.func_lib.by_id(func_id).unwrap();
                    e_node.refresh(func, node.behavior, &node.name);
                    self.cur_idx = idx;

                    for (input_idx, input) in node.inputs.iter().enumerate() {
                        let source = match &input.binding {
                            Binding::None => Source::None,
                            Binding::Const(v) => Source::Const(v.clone()),
                            Binding::Bind(op) => self.resolve(op.node_id, op.port_idx),
                        };
                        self.set_input(input_idx, source);
                    }

                    // Stage 2a: only func→func subscriber edges are remapped;
                    // edges crossing a composite boundary are Stage 2b.
                    for (event_idx, event) in node.events.iter().enumerate() {
                        for sub in &event.subscribers {
                            if matches!(graph.by_id(sub).map(|n| &n.kind), Some(NodeKind::Func(_)))
                            {
                                let flat_sub = flatten_id(self.ids.as_slice(), *sub);
                                self.compact[self.cur_idx].events[event_idx]
                                    .subscribers
                                    .push(flat_sub);
                            }
                        }
                    }
                }
                NodeKind::Subgraph(_) => {
                    assert!(
                        self.ids.len() < MAX_DEPTH,
                        "subgraph nesting exceeds {MAX_DEPTH} levels (recursive definition?)"
                    );
                    self.ids.push(node.id);
                    self.emit();
                    self.ids.pop();
                }
                NodeKind::SubgraphInput | NodeKind::SubgraphOutput => {}
            }
        }
    }

    /// Write the resolved source into input `cur_idx`/`input_idx`, tracking
    /// `binding_changed` so caching stays correct across updates.
    fn set_input(&mut self, input_idx: usize, source: Source) {
        match source {
            Source::None => {
                let e_input = &mut self.compact[self.cur_idx].inputs[input_idx];
                if !matches!(e_input.binding, ExecutionBinding::None) {
                    e_input.binding = ExecutionBinding::None;
                    e_input.binding_changed = true;
                }
            }
            Source::Const(v) => {
                let e_input = &mut self.compact[self.cur_idx].inputs[input_idx];
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
                let e_input = &mut self.compact[self.cur_idx].inputs[input_idx];
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
                let binding = def
                    .graph
                    .iter()
                    .find(|n| matches!(n.kind, NodeKind::SubgraphOutput))
                    .and_then(|so| so.inputs.get(port_idx))
                    .map(|input| input.binding.clone());
                let Some(binding) = binding else {
                    return Source::None;
                };
                self.ids.push(node_id);
                let source = self.resolve_binding(&binding);
                self.ids.pop();
                source
            }
            // Follow out: this SubgraphInput output `port_idx` is the enclosing
            // instance's exposed input `port_idx`; resolve it one level up.
            NodeKind::SubgraphInput => {
                let instance_id = self.ids.pop().expect("SubgraphInput at the root level");
                let binding = self
                    .current()
                    .by_id(&instance_id)
                    .and_then(|inst| inst.inputs.get(port_idx))
                    .map(|input| input.binding.clone());
                let source = match binding {
                    Some(b) => self.resolve_binding(&b),
                    None => Source::None,
                };
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
