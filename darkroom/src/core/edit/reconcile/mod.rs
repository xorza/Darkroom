//! Keep each local graph's interface in lockstep with its boundary wiring.
//!
//! The `GraphInput` / `GraphOutput` boundary nodes have no stored
//! ports — their arity is the graph interface. We treat that interface
//! as **derived state**: a graph has exactly one interface input per *used*
//! `GraphInput` output, one interface output per *used* `GraphOutput`
//! input, and the used slots are compacted to a contiguous `0..n`. The
//! Scene then draws those ports plus one trailing placeholder, so dragging
//! a connection onto the placeholder is a plain `Intent::SetInput` (no
//! special intent) and this pass grows the interface to match on the next
//! tick. Disconnecting shrinks it; a freed middle slot renumbers the
//! survivors, rewriting the interior boundary bindings **and** every
//! instance's bindings across the document so indices stay aligned.
//!
//! Run after edits drain and before the Scene rebuild (like the Scene
//! itself, it's recomputed, never an undo step). Idempotent: on an
//! already-canonical document it changes nothing.

use std::collections::HashMap;

use scenarium::DataType;
use scenarium::GraphId;
use scenarium::Library;
use scenarium::NodeId;
use scenarium::{Binding, Graph, InputPort, NodeKind};
use scenarium::{FuncInput, FuncOutput};

use crate::core::document::{Document, GraphRef};

/// Per-side plan: the new interface vec plus the `old_idx → new_idx` map
/// applied to every binding that addresses this boundary by index.
#[derive(Debug)]
struct SidePlan<T> {
    boundary: NodeId,
    interface: Vec<T>,
    remap: HashMap<usize, usize>,
    /// True when applying this plan would change anything (interface
    /// differs or some index moves). Lets the caller skip the rewrite —
    /// and the instance walk — on a steady-state document.
    changed: bool,
}

/// Reconcile one nested graph's interface against its boundary wiring.
/// [`Document::normalize`] loops this over every local graph before pruning
/// wiring left dangling against the resulting interfaces.
pub(crate) fn reconcile_graph(doc: &mut Document, graph_id: GraphId, library: &Library) {
    let (inputs, outputs) = {
        let Some(graph) = doc.graph.graphs.get(&graph_id) else {
            return;
        };
        (plan_inputs(graph, library), plan_outputs(graph, library))
    };
    let input_changed = inputs.as_ref().is_some_and(|p| p.changed);
    let output_changed = outputs.as_ref().is_some_and(|p| p.changed);
    if !input_changed && !output_changed {
        return;
    }

    // Locate instances before mutating, so the walk reads a consistent
    // document. Instances live in Main and inside other defs' interiors.
    let instances = instances_of(doc, graph_id);

    // Interior + interface, under a single `&mut graph` borrow.
    if let Some(graph) = doc.graph.graphs.get_mut(&graph_id) {
        if let Some(p) = &inputs {
            remap_source_edges(graph, p.boundary, &p.remap);
            graph.inputs = p.interface.clone();
        }
        if let Some(p) = &outputs {
            remap_target_bindings(graph, p.boundary, &p.remap);
            graph.outputs = p.interface.clone();
        }
    }

    // Instance bindings in every graph that holds one.
    for (target, node_id) in instances {
        let Some(graph) = doc.graph_mut(target) else {
            continue;
        };
        if let Some(p) = &inputs {
            remap_target_bindings(graph, node_id, &p.remap);
        }
        if let Some(p) = &outputs {
            remap_source_edges(graph, node_id, &p.remap);
        }
    }
}

/// Plan the inputs side: one interface input per used `GraphInput`
/// output, compacted. Existing entries keep their name/type (so authored
/// names survive); a freshly-used slot is synthesized with the type of
/// the interior port it now feeds. `None` when the interior has no
/// `GraphInput` node (nothing to derive — leave `graph.inputs` alone).
fn plan_inputs(graph: &Graph, library: &Library) -> Option<SidePlan<FuncInput>> {
    let interior = graph;
    let boundary = interior
        .iter()
        .find(|n| matches!(n.kind, NodeKind::GraphInput))
        .map(|n| n.id)?;
    let used = used_sorted(
        interior
            .edges()
            .filter_map(|(_, src)| (src.node_id == boundary).then_some(src.port_idx)),
    );
    let mut remap = HashMap::with_capacity(used.len());
    let mut interface = Vec::with_capacity(used.len());
    for (new_idx, &old) in used.iter().enumerate() {
        remap.insert(old, new_idx);
        // Type is derived from the wired port every pass (so reconnecting
        // to a differently-typed node updates it); the name and any
        // authored fields are preserved. A passthrough / unwired-to-a-real
        // port resolves to `Any` (polymorphic — see `infer_used_input_type`).
        let data_type = infer_used_input_type(interior, library, boundary, old);
        interface.push(match graph.inputs.get(old) {
            Some(existing) => FuncInput {
                data_type,
                ..existing.clone()
            },
            None => synth_input(new_idx, data_type),
        });
    }
    let changed = interface != graph.inputs || remap.iter().any(|(o, n)| o != n);
    Some(SidePlan {
        boundary,
        interface,
        remap,
        changed,
    })
}

/// Plan the outputs side: one interface output per used `GraphOutput`
/// input, compacted. Mirror of [`plan_inputs`].
fn plan_outputs(graph: &Graph, library: &Library) -> Option<SidePlan<FuncOutput>> {
    let interior = graph;
    let boundary = interior
        .iter()
        .find(|n| matches!(n.kind, NodeKind::GraphOutput))
        .map(|n| n.id)?;
    let used = used_sorted(
        interior
            .bindings_touching(boundary)
            .into_iter()
            .filter_map(|entry| (entry.port.node_id == boundary).then_some(entry.port.port_idx)),
    );
    let mut remap = HashMap::with_capacity(used.len());
    let mut interface = Vec::with_capacity(used.len());
    for (new_idx, &old) in used.iter().enumerate() {
        remap.insert(old, new_idx);
        // Re-derive the type from the wired producer each pass; preserve
        // the name. Passthrough / unwired-to-a-real producer → `Any`.
        let data_type = infer_used_output_type(interior, library, boundary, old);
        let name = match graph.outputs.get(old) {
            Some(existing) => existing.name.clone(),
            None => format!("output{new_idx}"),
        };
        interface.push(FuncOutput::new(name, data_type));
    }
    let changed = interface != graph.outputs || remap.iter().any(|(o, n)| o != n);
    Some(SidePlan {
        boundary,
        interface,
        remap,
        changed,
    })
}

/// Remap every binding whose **source** is `(node, old)` to `(node, new)`,
/// clearing those whose source index was dropped from `remap`. Used for
/// the interior `GraphInput` (feeds interior nodes) and for instance
/// output ports (feed parent-graph consumers).
fn remap_source_edges(graph: &mut Graph, node: NodeId, remap: &HashMap<usize, usize>) {
    let edges: Vec<(InputPort, usize)> = graph
        .edges()
        .filter(|(_, src)| src.node_id == node)
        .map(|(tgt, src)| (tgt, src.port_idx))
        .collect();
    for (tgt, old) in edges {
        match remap.get(&old) {
            Some(&new) if new != old => graph.set_input_binding(tgt, Binding::bind(node, new)),
            Some(_) => {}
            None => graph.set_input_binding(tgt, None),
        }
    }
}

/// Remap every binding whose **target** is `(node, old)` to `(node, new)`,
/// dropping those whose index was removed. Used for the interior
/// `GraphOutput` (collects interior results) and for instance input
/// ports. Clears the whole set first so a compacting shift can't collide.
fn remap_target_bindings(graph: &mut Graph, node: NodeId, remap: &HashMap<usize, usize>) {
    let current: Vec<(usize, Binding)> = graph
        .bindings_touching(node)
        .into_iter()
        .filter(|entry| entry.port.node_id == node)
        .map(|entry| (entry.port.port_idx, entry.binding))
        .collect();
    if current.is_empty() {
        return;
    }
    for (old, _) in &current {
        graph.set_input_binding(InputPort::new(node, *old), None);
    }
    for (old, b) in current {
        if let Some(&new) = remap.get(&old) {
            graph.set_input_binding(InputPort::new(node, new), b);
        }
    }
}

/// Every node across the document (Main + each local interior) that is an
/// instance of `graph_id`, as `(graph it lives in, node id)`.
fn instances_of(doc: &Document, graph_id: GraphId) -> Vec<(GraphRef, NodeId)> {
    let mut out = Vec::new();
    let mut scan = |target: GraphRef, graph: &Graph| {
        for node in graph.iter() {
            if let NodeKind::Graph(r) = &node.kind
                && r.id() == graph_id
            {
                out.push((target, node.id));
            }
        }
    };
    scan(GraphRef::Main, &doc.graph);
    for (graph_id, graph) in &doc.graph.graphs {
        scan(GraphRef::Local(*graph_id), graph);
    }
    out
}

/// Distinct port indices, ascending. The position in the returned vec is
/// the compacted (`new`) index.
fn used_sorted(indices: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut v: Vec<usize> = indices.collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn synth_input(idx: usize, data_type: DataType) -> FuncInput {
    FuncInput::optional(format!("input{idx}"), data_type)
}

/// Type of graph input `old`: the type the interior consumer it feeds
/// expects. Falls back to `Any` when no consumer resolves (a placeholder
/// with no destination shouldn't reach here, but stay total).
fn infer_used_input_type(
    interior: &Graph,
    library: &Library,
    boundary: NodeId,
    old: usize,
) -> DataType {
    interior
        .edges()
        .find(|(_, src)| src.node_id == boundary && src.port_idx == old)
        .and_then(|(tgt, _)| interior.input_type(library, tgt))
        .unwrap_or_default()
}

/// Type of graph output `old`: the type of the interior producer bound
/// into `GraphOutput` input `old`. Resolved via
/// [`Graph::resolve_output_type`], so a producing wildcard passthrough reports
/// the type wired through it rather than its declared `Any` — the composite's
/// exposed output keeps the value's real type.
fn infer_used_output_type(
    interior: &Graph,
    library: &Library,
    boundary: NodeId,
    old: usize,
) -> DataType {
    interior
        .bindings_touching(boundary)
        .into_iter()
        .find_map(|entry| match entry.binding {
            Binding::Bind(src) if entry.port.node_id == boundary && entry.port.port_idx == old => {
                Some(src)
            }
            _ => None,
        })
        .map(|src| interior.resolve_output_type(library, src))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
