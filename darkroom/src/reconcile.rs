//! Keep each local subgraph's interface (`def.inputs` / `def.outputs`)
//! in lockstep with its interior wiring.
//!
//! The `SubgraphInput` / `SubgraphOutput` boundary nodes have no stored
//! ports — their arity is the def's interface. We treat that interface
//! as **derived state**: a def has exactly one interface input per *used*
//! `SubgraphInput` output, one interface output per *used* `SubgraphOutput`
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

use scenarium::data::DataType;
use scenarium::function::{FuncInput, FuncOutput};
use scenarium::graph::{Binding, Graph, InputPort, NodeKind, OutputPort};
use scenarium::prelude::{FuncLib, NodeId, SubgraphDef, SubgraphId};

use crate::document::{Document, GraphRef};

/// Per-side plan: the new interface vec plus the `old_idx → new_idx` map
/// applied to every binding that addresses this boundary by index.
struct SidePlan<T> {
    boundary: NodeId,
    interface: Vec<T>,
    remap: HashMap<usize, usize>,
    /// True when applying this plan would change anything (interface
    /// differs or some index moves). Lets the caller skip the rewrite —
    /// and the instance walk — on a steady-state document.
    changed: bool,
}

/// Reconcile one subgraph def's interface against its interior wiring.
/// Entry point is [`Document::reconcile_boundaries`] (in `document.rs`),
/// which loops this over every local def.
pub(crate) fn reconcile_def(doc: &mut Document, def_id: SubgraphId, func_lib: &FuncLib) {
    let (inputs, outputs) = {
        let Some(def) = doc.graph.subgraphs.by_key(&def_id) else {
            return;
        };
        (plan_inputs(def, func_lib), plan_outputs(def, func_lib))
    };
    let input_changed = inputs.as_ref().is_some_and(|p| p.changed);
    let output_changed = outputs.as_ref().is_some_and(|p| p.changed);
    if !input_changed && !output_changed {
        return;
    }

    // Locate instances before mutating, so the walk reads a consistent
    // document. Instances live in Main and inside other defs' interiors.
    let instances = instances_of(doc, def_id);

    // Interior + interface, under a single `&mut def` borrow.
    if let Some(def) = doc.graph.subgraphs.by_key_mut(&def_id) {
        if let Some(p) = &inputs {
            remap_source_edges(&mut def.graph, p.boundary, &p.remap);
            def.inputs = p.interface.clone();
        }
        if let Some(p) = &outputs {
            remap_target_bindings(&mut def.graph, p.boundary, &p.remap);
            def.outputs = p.interface.clone();
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

/// Plan the inputs side: one interface input per used `SubgraphInput`
/// output, compacted. Existing entries keep their name/type (so authored
/// names survive); a freshly-used slot is synthesized with the type of
/// the interior port it now feeds. `None` when the interior has no
/// `SubgraphInput` node (nothing to derive — leave `def.inputs` alone).
fn plan_inputs(def: &SubgraphDef, func_lib: &FuncLib) -> Option<SidePlan<FuncInput>> {
    let interior = &def.graph;
    let boundary = interior
        .iter()
        .find(|n| matches!(n.kind, NodeKind::SubgraphInput))
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
        interface.push(def.inputs.get(old).cloned().unwrap_or_else(|| {
            let ty = infer_used_input_type(interior, func_lib, boundary, old);
            synth_input(new_idx, ty)
        }));
    }
    let changed = interface != def.inputs || remap.iter().any(|(o, n)| o != n);
    Some(SidePlan {
        boundary,
        interface,
        remap,
        changed,
    })
}

/// Plan the outputs side: one interface output per used `SubgraphOutput`
/// input, compacted. Mirror of [`plan_inputs`].
fn plan_outputs(def: &SubgraphDef, func_lib: &FuncLib) -> Option<SidePlan<FuncOutput>> {
    let interior = &def.graph;
    let boundary = interior
        .iter()
        .find(|n| matches!(n.kind, NodeKind::SubgraphOutput))
        .map(|n| n.id)?;
    let used = used_sorted(
        interior
            .bindings_touching(boundary)
            .into_iter()
            .filter_map(|(port, _)| (port.node_id == boundary).then_some(port.port_idx)),
    );
    let mut remap = HashMap::with_capacity(used.len());
    let mut interface = Vec::with_capacity(used.len());
    for (new_idx, &old) in used.iter().enumerate() {
        remap.insert(old, new_idx);
        interface.push(def.outputs.get(old).cloned().unwrap_or_else(|| {
            let ty = infer_used_output_type(interior, func_lib, boundary, old);
            FuncOutput {
                name: format!("output{new_idx}"),
                data_type: ty,
            }
        }));
    }
    let changed = interface != def.outputs || remap.iter().any(|(o, n)| o != n);
    Some(SidePlan {
        boundary,
        interface,
        remap,
        changed,
    })
}

/// Remap every binding whose **source** is `(node, old)` to `(node, new)`,
/// clearing those whose source index was dropped from `remap`. Used for
/// the interior `SubgraphInput` (feeds interior nodes) and for instance
/// output ports (feed parent-graph consumers).
fn remap_source_edges(graph: &mut Graph, node: NodeId, remap: &HashMap<usize, usize>) {
    let edges: Vec<(InputPort, usize)> = graph
        .edges()
        .filter(|(_, src)| src.node_id == node)
        .map(|(tgt, src)| (tgt, src.port_idx))
        .collect();
    for (tgt, old) in edges {
        match remap.get(&old) {
            Some(&new) if new != old => graph.set_input_binding(
                tgt,
                Binding::Bind(OutputPort {
                    node_id: node,
                    port_idx: new,
                }),
            ),
            Some(_) => {}
            None => graph.set_input_binding(tgt, Binding::None),
        }
    }
}

/// Remap every binding whose **target** is `(node, old)` to `(node, new)`,
/// dropping those whose index was removed. Used for the interior
/// `SubgraphOutput` (collects interior results) and for instance input
/// ports. Clears the whole set first so a compacting shift can't collide.
fn remap_target_bindings(graph: &mut Graph, node: NodeId, remap: &HashMap<usize, usize>) {
    let current: Vec<(usize, Binding)> = graph
        .bindings_touching(node)
        .into_iter()
        .filter(|(port, _)| port.node_id == node)
        .map(|(port, b)| (port.port_idx, b))
        .collect();
    if current.is_empty() {
        return;
    }
    for (old, _) in &current {
        graph.set_input_binding(
            InputPort {
                node_id: node,
                port_idx: *old,
            },
            Binding::None,
        );
    }
    for (old, b) in current {
        if let Some(&new) = remap.get(&old) {
            graph.set_input_binding(
                InputPort {
                    node_id: node,
                    port_idx: new,
                },
                b,
            );
        }
    }
}

/// Every node across the document (Main + each local interior) that is an
/// instance of `def_id`, as `(graph it lives in, node id)`.
fn instances_of(doc: &Document, def_id: SubgraphId) -> Vec<(GraphRef, NodeId)> {
    let mut out = Vec::new();
    let mut scan = |target: GraphRef, graph: &Graph| {
        for node in graph.iter() {
            if let NodeKind::Subgraph(r) = &node.kind
                && r.id() == def_id
            {
                out.push((target, node.id));
            }
        }
    };
    scan(GraphRef::Main, &doc.graph);
    for def in doc.graph.subgraphs.iter() {
        scan(GraphRef::Local(def.id), &def.graph);
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
    FuncInput {
        name: format!("input{idx}"),
        required: false,
        data_type,
        default_value: None,
        value_options: Vec::new(),
    }
}

/// Type of subgraph input `old`: the type the interior consumer it feeds
/// expects. Falls back to `Null` when no consumer resolves (a placeholder
/// with no destination shouldn't reach here, but stay total).
fn infer_used_input_type(
    interior: &Graph,
    func_lib: &FuncLib,
    boundary: NodeId,
    old: usize,
) -> DataType {
    interior
        .edges()
        .find(|(_, src)| src.node_id == boundary && src.port_idx == old)
        .and_then(|(tgt, _)| port_type(interior, func_lib, tgt.node_id, tgt.port_idx, Dir::Input))
        .unwrap_or_default()
}

/// Type of subgraph output `old`: the type of the interior producer bound
/// into `SubgraphOutput` input `old`.
fn infer_used_output_type(
    interior: &Graph,
    func_lib: &FuncLib,
    boundary: NodeId,
    old: usize,
) -> DataType {
    interior
        .bindings_touching(boundary)
        .into_iter()
        .find_map(|(port, binding)| match binding {
            Binding::Bind(src) if port.node_id == boundary && port.port_idx == old => Some(src),
            _ => None,
        })
        .and_then(|src| port_type(interior, func_lib, src.node_id, src.port_idx, Dir::Output))
        .unwrap_or_default()
}

enum Dir {
    Input,
    Output,
}

/// Resolve a Func / Subgraph-instance node port's declared type. Boundary
/// nodes (chained interfaces) aren't resolved here — an uncommon case
/// that falls back to the caller's `Null`.
fn port_type(
    graph: &Graph,
    func_lib: &FuncLib,
    node_id: NodeId,
    port_idx: usize,
    dir: Dir,
) -> Option<DataType> {
    let node = graph.by_id(&node_id)?;
    match &node.kind {
        NodeKind::Func(func_id) => {
            let f = func_lib.by_id(func_id)?;
            match dir {
                Dir::Input => f.inputs.get(port_idx).map(|i| i.data_type.clone()),
                Dir::Output => f.outputs.get(port_idx).map(|o| o.data_type.clone()),
            }
        }
        NodeKind::Subgraph(r) => {
            let d = graph.resolve_def(*r, func_lib)?;
            match dir {
                Dir::Input => d.inputs.get(port_idx).map(|i| i.data_type.clone()),
                Dir::Output => d.outputs.get(port_idx).map(|o| o.data_type.clone()),
            }
        }
        NodeKind::SubgraphInput | NodeKind::SubgraphOutput => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::data::StaticValue;
    use scenarium::graph::Node;
    use scenarium::prelude::{Graph, SubgraphRef};
    use scenarium::testing::{TestFuncHooks, test_func_lib};

    fn lib() -> FuncLib {
        test_func_lib(TestFuncHooks::default())
    }

    fn int_input(name: &str) -> FuncInput {
        FuncInput {
            name: name.into(),
            required: false,
            data_type: DataType::Int,
            default_value: None,
            value_options: Vec::new(),
        }
    }

    /// A subgraph def "S" whose interior is `SubgraphInput`, one `sum`
    /// func node (2 inputs, 1 output), and `SubgraphOutput`. Returns the
    /// def (interior unwired) plus the three node ids so each test wires
    /// it as needed. `authored_inputs`/`outputs` seed the def interface.
    fn build_def(
        func_lib: &FuncLib,
        authored_inputs: Vec<FuncInput>,
        authored_outputs: Vec<FuncOutput>,
    ) -> (SubgraphDef, NodeId, NodeId, NodeId) {
        let sum_id = func_lib.by_name("sum").unwrap().id;
        let sgin = Node::new(NodeKind::SubgraphInput);
        let sum = Node::new(NodeKind::Func(sum_id));
        let sgout = Node::new(NodeKind::SubgraphOutput);
        let (sgin_id, sum_id_n, sgout_id) = (sgin.id, sum.id, sgout.id);
        let mut g = Graph::default();
        g.add(sgin);
        g.add(sum);
        g.add(sgout);
        let def = SubgraphDef {
            id: "00000000-0000-0000-0000-0000000000aa".into(),
            name: "S".into(),
            category: "Subgraph".into(),
            graph: g,
            inputs: authored_inputs,
            outputs: authored_outputs,
            events: vec![],
        };
        (def, sgin_id, sum_id_n, sgout_id)
    }

    fn bind(graph: &mut Graph, dst_node: NodeId, dst_idx: usize, src_node: NodeId, src_idx: usize) {
        graph.set_input_binding(
            InputPort {
                node_id: dst_node,
                port_idx: dst_idx,
            },
            Binding::Bind(OutputPort {
                node_id: src_node,
                port_idx: src_idx,
            }),
        );
    }

    #[test]
    fn connecting_placeholder_grows_input_with_inferred_type() {
        // Interior wires sum.in0 <- (SubgraphInput, 0) while def.inputs is
        // empty — the transient state right after a placeholder connect.
        let func_lib = lib();
        let (mut def, sgin, sum, _sgout) = build_def(&func_lib, vec![], vec![]);
        bind(&mut def.graph, sum, 0, sgin, 0);
        let want_ty = func_lib.by_name("sum").unwrap().inputs[0].data_type.clone();
        let def_id = def.id;
        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        let mut doc: Document = graph.into();

        doc.reconcile_boundaries(&func_lib);

        let def = doc.graph.subgraphs.by_key(&def_id).unwrap();
        assert_eq!(def.inputs.len(), 1, "placeholder use materialized a slot");
        assert_eq!(def.inputs[0].name, "input0");
        assert_eq!(
            def.inputs[0].data_type, want_ty,
            "new slot inherits the consumer's type"
        );
    }

    #[test]
    fn connecting_placeholder_grows_output_with_inferred_type() {
        // (SubgraphOutput, 0) <- sum.out0, def.outputs empty.
        let func_lib = lib();
        let (mut def, _sgin, sum, sgout) = build_def(&func_lib, vec![], vec![]);
        bind(&mut def.graph, sgout, 0, sum, 0);
        let want_ty = func_lib.by_name("sum").unwrap().outputs[0]
            .data_type
            .clone();
        let def_id = def.id;
        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        let mut doc: Document = graph.into();

        doc.reconcile_boundaries(&func_lib);

        let def = doc.graph.subgraphs.by_key(&def_id).unwrap();
        assert_eq!(def.outputs.len(), 1);
        assert_eq!(def.outputs[0].name, "output0");
        assert_eq!(def.outputs[0].data_type, want_ty);
    }

    #[test]
    fn fully_wired_interface_is_preserved_and_idempotent() {
        // Authored names A,B both used → reconcile must not rename or
        // resize, and a second pass must be a no-op.
        let func_lib = lib();
        let (mut def, sgin, sum, _sgout) =
            build_def(&func_lib, vec![int_input("A"), int_input("B")], vec![]);
        bind(&mut def.graph, sum, 0, sgin, 0);
        bind(&mut def.graph, sum, 1, sgin, 1);
        let def_id = def.id;
        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        let mut doc: Document = graph.into();

        doc.reconcile_boundaries(&func_lib);
        let names: Vec<String> = doc
            .graph
            .subgraphs
            .by_key(&def_id)
            .unwrap()
            .inputs
            .iter()
            .map(|i| i.name.clone())
            .collect();
        assert_eq!(names, ["A", "B"], "authored names survive");

        // Idempotent: a second pass changes nothing.
        let before = doc.graph.subgraphs.by_key(&def_id).unwrap().inputs.clone();
        doc.reconcile_boundaries(&func_lib);
        let after = doc.graph.subgraphs.by_key(&def_id).unwrap().inputs.clone();
        assert_eq!(before, after);
    }

    #[test]
    fn middle_disconnect_compacts_interior_and_instance_bindings() {
        // Authored [A,B,C]; interior uses SubgraphInput outputs {0,2}
        // (slot 1 = B is unused). reconcile drops B and renumbers 2 -> 1,
        // rewriting the interior binding AND the instance's bindings.
        let func_lib = lib();
        let (mut def, sgin, sum, _sgout) = build_def(
            &func_lib,
            vec![int_input("A"), int_input("B"), int_input("C")],
            vec![],
        );
        bind(&mut def.graph, sum, 0, sgin, 0); // uses input 0 (A)
        bind(&mut def.graph, sum, 1, sgin, 2); // uses input 2 (C)
        let def_id = def.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def.clone());
        let inst = graph.add_subgraph_node(&def, SubgraphRef::Local(def_id));
        // Instance bindings on all three inputs, distinguishable by value.
        graph.set_input_binding(
            InputPort {
                node_id: inst,
                port_idx: 0,
            },
            Binding::Const(StaticValue::Int(10)),
        );
        graph.set_input_binding(
            InputPort {
                node_id: inst,
                port_idx: 1,
            },
            Binding::Const(StaticValue::Int(11)),
        );
        graph.set_input_binding(
            InputPort {
                node_id: inst,
                port_idx: 2,
            },
            Binding::Const(StaticValue::Int(12)),
        );
        let mut doc: Document = graph.into();

        doc.reconcile_boundaries(&func_lib);

        // Interface compacted to [A, C].
        let names: Vec<String> = doc
            .graph
            .subgraphs
            .by_key(&def_id)
            .unwrap()
            .inputs
            .iter()
            .map(|i| i.name.clone())
            .collect();
        assert_eq!(names, ["A", "C"]);

        // Interior: sum.in0 still from slot 0, sum.in1 now from slot 1
        // (was 2).
        let interior = &doc.graph.subgraphs.by_key(&def_id).unwrap().graph;
        assert_eq!(
            interior.input_binding(InputPort {
                node_id: sum,
                port_idx: 0
            }),
            Binding::Bind(OutputPort {
                node_id: sgin,
                port_idx: 0
            }),
        );
        assert_eq!(
            interior.input_binding(InputPort {
                node_id: sum,
                port_idx: 1
            }),
            Binding::Bind(OutputPort {
                node_id: sgin,
                port_idx: 1
            }),
        );

        // Instance: old slot 0 stays (10), old slot 2 -> new slot 1 (12),
        // dropped slot 1 (11) is cleared.
        assert_eq!(
            doc.graph.input_binding(InputPort {
                node_id: inst,
                port_idx: 0
            }),
            Binding::Const(StaticValue::Int(10)),
        );
        assert_eq!(
            doc.graph.input_binding(InputPort {
                node_id: inst,
                port_idx: 1
            }),
            Binding::Const(StaticValue::Int(12)),
        );
        assert_eq!(
            doc.graph.input_binding(InputPort {
                node_id: inst,
                port_idx: 2
            }),
            Binding::None,
        );
    }

    #[test]
    fn unused_subgraph_input_shrinks_interface() {
        // Authored [A,B] but only output 0 is wired → B is dropped.
        let func_lib = lib();
        let (mut def, sgin, sum, _sgout) =
            build_def(&func_lib, vec![int_input("A"), int_input("B")], vec![]);
        bind(&mut def.graph, sum, 0, sgin, 0);
        let def_id = def.id;
        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        let mut doc: Document = graph.into();

        doc.reconcile_boundaries(&func_lib);

        let inputs = &doc.graph.subgraphs.by_key(&def_id).unwrap().inputs;
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].name, "A");
    }
}
