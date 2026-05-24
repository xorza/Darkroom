//! Subgraph (composite-node) authoring types. A `SubgraphDef` is a reusable
//! definition that wraps an interior `Graph` and exposes a chosen set of
//! inputs/outputs/events, so it can be instantiated as a single node in a
//! parent graph. Execution-time flattening lives in `execution_graph`
//! (not yet implemented — Stage 2); these types are the authoring model only.
//!
//! See `docs/subgraph-design.md` for the full design.

use common::id_type;
use common::key_index_vec::KeyIndexKey;
use serde::{Deserialize, Serialize};

use crate::function::{FuncInput, FuncOutput};
use crate::graph::Graph;

id_type!(SubgraphId);

// A composite's interface is, structurally, a function signature: its exposed
// inputs are `FuncInput`s (name/type/required/default) and its exposed outputs
// are `FuncOutput`s (name/type). Reusing them keeps inputs and outputs as the
// distinct shapes they already are — an output carries no `required` /
// `default_value` — and lets a subgraph instance node be shaped from a def
// exactly like a func node is shaped from a `Func`.

/// One exposed event of a composite's interface. Exposed events are always
/// *outgoing*: an interior emitter surfaced outward, which parent nodes can
/// subscribe to — so an instance node carries one `Event` (with the parent's
/// subscribers) per entry here, exactly like a func node's events.
///
/// There is no "incoming event" interface element. Routing an event *into* a
/// subgraph is the ordinary subscriber mechanism: the composite node, like
/// any node, can itself be subscribed to a parent event; which interior
/// subnodes then fire is the subgraph's internal wiring, not an exposed port.
/// See `docs/subgraph-design.md` §4.5.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubgraphEvent {
    pub name: String,
}

/// Where a composite instance resolves its definition from. The variant *is*
/// the linked/local distinction: `Linked` defs live in `FuncLib.subgraphs`
/// (shared, edits propagate to every instance), `Local` defs live in the
/// owning `Graph.subgraphs` (private to this graph, edits affect only it).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SubgraphRef {
    Linked(SubgraphId),
    Local(SubgraphId),
}

impl SubgraphRef {
    pub fn id(&self) -> SubgraphId {
        match self {
            SubgraphRef::Linked(id) | SubgraphRef::Local(id) => *id,
        }
    }
}

/// A reusable composite definition: an interior `Graph` plus the interface it
/// exposes. Behavior and terminal-ness are *derived* from the interior at
/// flatten time, never stored here (so they can't drift).
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubgraphDef {
    pub id: SubgraphId,
    pub name: String,
    pub category: String,

    /// The interior. Contains at most one `SubgraphInput` and one
    /// `SubgraphOutput` node (each omitted when its side exposes nothing).
    pub graph: Graph,

    /// Interface in port order. `inputs[i]` <-> `SubgraphInput` output port i,
    /// `outputs[j]` <-> `SubgraphOutput` input port j.
    #[serde(default)]
    pub inputs: Vec<FuncInput>,
    #[serde(default)]
    pub outputs: Vec<FuncOutput>,

    /// Exposed (outgoing) events. An instance node carries one `Event` per
    /// entry here — the same count as `events.len()`.
    #[serde(default)]
    pub events: Vec<SubgraphEvent>,
}

impl KeyIndexKey<SubgraphId> for SubgraphDef {
    fn key(&self) -> &SubgraphId {
        &self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DataType;
    use crate::function::FuncLib;
    use crate::graph::{Graph, Input, Node, NodeKind};
    use crate::testing::{TestFuncHooks, test_func_lib};
    use common::SerdeFormat;

    fn fin(name: &str) -> FuncInput {
        FuncInput {
            name: name.into(),
            required: false,
            data_type: DataType::Int,
            default_value: None,
            value_options: vec![],
        }
    }

    fn fout(name: &str) -> FuncOutput {
        FuncOutput {
            name: name.into(),
            data_type: DataType::Int,
        }
    }

    /// `in(A, B) -> sum -> out(Sum)`.
    fn wrap_sum(func_lib: &FuncLib) -> SubgraphDef {
        let sum_id = func_lib.by_name("sum").unwrap().id;

        let in_node = Node::new(NodeKind::SubgraphInput);
        let in_id = in_node.id;

        let mut sum_node = Node::new(NodeKind::Func(sum_id));
        sum_node.inputs = vec![
            Input {
                binding: (in_id, 0).into(),
            },
            Input {
                binding: (in_id, 1).into(),
            },
        ];
        let sum_node_id = sum_node.id;

        let mut out_node = Node::new(NodeKind::SubgraphOutput);
        out_node.inputs = vec![Input {
            binding: (sum_node_id, 0).into(),
        }];

        let mut graph = Graph::default();
        graph.add(in_node);
        graph.add(sum_node);
        graph.add(out_node);

        SubgraphDef {
            id: SubgraphId::unique(),
            name: "WrapSum".into(),
            category: "Test".into(),
            graph,
            inputs: vec![fin("A"), fin("B")],
            outputs: vec![fout("Sum")],
            events: vec![],
        }
    }

    #[test]
    fn local_subgraph_validates_and_roundtrips() {
        let func_lib = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum(&func_lib);

        let mut parent = Graph::default();
        let inst = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        assert_eq!(inst.inputs.len(), 2); // A, B from the def interface
        assert!(inst.events.is_empty()); // no outlets
        parent.subgraphs.add(def);
        parent.add(inst);

        parent.validate_with(&func_lib);

        for format in SerdeFormat::all_formats_for_testing() {
            let bytes = parent.serialize(format);
            let back = Graph::deserialize(&bytes, format).unwrap();
            assert_eq!(parent, back);
        }
    }

    #[test]
    fn linked_subgraph_resolves_from_funclib() {
        let mut func_lib = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum(&func_lib);
        let def_id = def.id;
        func_lib.add_subgraph(def);

        let mut parent = Graph::default();
        let def_ref = func_lib.subgraph_by_id(&def_id).unwrap();
        let inst = Node::subgraph_instance(def_ref, SubgraphRef::Linked(def_id));
        parent.add(inst);

        parent.validate_with(&func_lib);
    }

    #[test]
    fn zero_input_subgraph_omits_input_boundary() {
        // `get_a` (0 inputs, 1 output) -> out(Val). No SubgraphInput node.
        let func_lib = test_func_lib(TestFuncHooks::default());
        let get_a_id = func_lib.by_name("get_a").unwrap().id;

        let src = Node::new(NodeKind::Func(get_a_id));
        let src_id = src.id;
        let mut out_node = Node::new(NodeKind::SubgraphOutput);
        out_node.inputs = vec![Input {
            binding: (src_id, 0).into(),
        }];

        let mut graph = Graph::default();
        graph.add(src);
        graph.add(out_node);

        let def = SubgraphDef {
            id: SubgraphId::unique(),
            name: "Source".into(),
            category: "Test".into(),
            graph,
            inputs: vec![],
            outputs: vec![fout("Val")],
            events: vec![],
        };

        let mut parent = Graph::default();
        let inst = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        assert!(inst.inputs.is_empty());
        parent.subgraphs.add(def);
        parent.add(inst);

        parent.validate_with(&func_lib);
    }

    #[test]
    fn zero_output_subgraph_omits_output_boundary() {
        // in(msg) -> print (terminal). No SubgraphOutput node.
        let func_lib = test_func_lib(TestFuncHooks::default());
        let print_id = func_lib.by_name("print").unwrap().id;

        let in_node = Node::new(NodeKind::SubgraphInput);
        let in_id = in_node.id;
        let mut print_node = Node::new(NodeKind::Func(print_id));
        print_node.inputs = vec![Input {
            binding: (in_id, 0).into(),
        }];

        let mut graph = Graph::default();
        graph.add(in_node);
        graph.add(print_node);

        let def = SubgraphDef {
            id: SubgraphId::unique(),
            name: "Sink".into(),
            category: "Test".into(),
            graph,
            inputs: vec![fin("msg")],
            outputs: vec![],
            events: vec![],
        };

        let mut parent = Graph::default();
        let inst = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        assert_eq!(inst.inputs.len(), 1);
        parent.subgraphs.add(def);
        parent.add(inst);

        parent.validate_with(&func_lib);
    }

    #[test]
    #[should_panic(expected = "recursive")]
    fn recursive_subgraph_rejected() {
        let mut func_lib = test_func_lib(TestFuncHooks::default());

        // A's interior contains a node instancing A again (mutual self-reference).
        let a_id = SubgraphId::unique();
        let mut inner = Graph::default();
        inner.add(Node::new(NodeKind::Subgraph(SubgraphRef::Linked(a_id))));
        func_lib.add_subgraph(SubgraphDef {
            id: a_id,
            name: "A".into(),
            category: "Test".into(),
            graph: inner,
            inputs: vec![],
            outputs: vec![],
            events: vec![],
        });

        let mut parent = Graph::default();
        parent.add(Node::new(NodeKind::Subgraph(SubgraphRef::Linked(a_id))));
        parent.validate_with(&func_lib); // panics: recursion guard
    }

    #[test]
    fn outgoing_events_shape_instance_node() {
        let func_lib = test_func_lib(TestFuncHooks::default());

        let def = SubgraphDef {
            id: SubgraphId::unique(),
            name: "TwoEvents".into(),
            category: "Test".into(),
            graph: Graph::default(),
            inputs: vec![],
            outputs: vec![],
            events: vec![
                SubgraphEvent { name: "e0".into() },
                SubgraphEvent { name: "e1".into() },
            ],
        };

        let mut parent = Graph::default();
        let inst = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        // one Event (parent-subscriber slot) per exposed outgoing event
        assert_eq!(inst.events.len(), 2);
        parent.subgraphs.add(def);
        parent.add(inst);

        parent.validate_with(&func_lib);
    }
}
