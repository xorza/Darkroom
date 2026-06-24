//! Subgraph (composite-node) authoring types. A `SubgraphDef` is a reusable
//! definition that wraps an interior `Graph` and exposes a chosen set of
//! inputs/outputs/events, so it can be instantiated as a single node in a
//! parent graph. Execution-time flattening lives in `execution::flatten`.
//!
//! See `docs/subgraph-design.md` for the full design.

use common::KeyIndexKey;
use common::id_type;
use serde::{Deserialize, Serialize};

use crate::function::{FuncInput, FuncOutput};
use crate::graph::{Graph, NodeId};

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
/// subnodes then fire is the subgraph's internal wiring (interior nodes
/// subscribing to the def's `SubgraphInput` node), not an exposed port.
/// See `docs/subgraph-design.md` §4.5.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubgraphEvent {
    pub name: String,
    /// Interior emitter this re-exports: a node in `SubgraphDef.graph` and
    /// which of its events. At flatten time, a parent subscriber to this
    /// exposed event is rewired onto that interior emitter's flat node.
    pub emitter: NodeId,
    pub emitter_event_idx: usize,
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

    /// The shared-library def this one was copied from, if any. Lineage
    /// metadata for editors that localize a library subgraph on instance
    /// (the runtime ignores it). `None` for hand-authored / library defs.
    #[serde(default)]
    pub origin: Option<SubgraphId>,
}

impl SubgraphDef {
    /// An independent copy with a fresh def id and fresh interior node
    /// ids (exposed-event emitters remapped to match), preserving the
    /// interface. Used to localize a `Linked` instance or make an
    /// instance unique without sharing identity with the original.
    pub fn fresh_copy(&self) -> SubgraphDef {
        let fresh = self.graph.with_fresh_node_ids();
        let events = self
            .events
            .iter()
            .map(|e| SubgraphEvent {
                emitter: fresh.id_map.get(&e.emitter).copied().unwrap_or(e.emitter),
                ..e.clone()
            })
            .collect();
        SubgraphDef {
            id: SubgraphId::unique(),
            name: self.name.clone(),
            category: self.category.clone(),
            graph: fresh.graph,
            inputs: self.inputs.clone(),
            outputs: self.outputs.clone(),
            events,
            // A fresh copy is its own def; the caller (library instancing)
            // sets `origin` to the source library id if it wants lineage.
            origin: None,
        }
    }
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
    use crate::function::{FuncId, FuncLib};
    use crate::graph::{Graph, InputPort, Node, NodeKind};
    use crate::testing::{TestFuncHooks, test_func_lib};
    use common::SerdeFormat;

    fn fin(name: &str) -> FuncInput {
        FuncInput {
            name: name.into(),
            required: false,
            data_type: DataType::Int,
            const_only: false,
            default_value: None,
            value_variants: vec![],
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

        let sum_node = Node::new(NodeKind::Func(sum_id));
        let sum_node_id = sum_node.id;

        let out_node = Node::new(NodeKind::SubgraphOutput);
        let out_id = out_node.id;

        let mut graph = Graph::default();
        graph.add(in_node);
        graph.add(sum_node);
        graph.add(out_node);
        graph.set_input_binding(InputPort::new(sum_node_id, 0), (in_id, 0).into());
        graph.set_input_binding(InputPort::new(sum_node_id, 1), (in_id, 1).into());
        graph.set_input_binding(InputPort::new(out_id, 0), (sum_node_id, 0).into());

        SubgraphDef {
            id: SubgraphId::unique(),
            name: "WrapSum".into(),
            category: "Test".into(),
            graph,
            inputs: vec![fin("A"), fin("B")],
            outputs: vec![fout("Sum")],
            events: vec![],
            origin: None,
        }
    }

    #[test]
    fn fresh_copy_remaps_interior_ids_and_preserves_wiring() {
        let func_lib = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum(&func_lib);
        let copy = def.fresh_copy();

        // Fresh def id; same interface.
        assert_ne!(copy.id, def.id);
        assert_eq!(copy.inputs.len(), def.inputs.len());
        assert_eq!(copy.outputs.len(), def.outputs.len());

        // Every interior node id is fresh (disjoint from the original).
        let orig_ids: Vec<_> = def.graph.iter().map(|n| n.id).collect();
        let copy_ids: Vec<_> = copy.graph.iter().map(|n| n.id).collect();
        assert_eq!(copy_ids.len(), orig_ids.len());
        for id in &copy_ids {
            assert!(!orig_ids.contains(id), "interior id must be remapped");
        }

        // Wiring preserved (same edge count), and every edge references
        // only the copy's own node ids — bindings were remapped.
        assert_eq!(copy.graph.edges().count(), def.graph.edges().count());
        for (dst, src) in copy.graph.edges() {
            assert!(copy_ids.contains(&dst.node_id));
            assert!(copy_ids.contains(&src.node_id));
        }
    }

    #[test]
    fn local_subgraph_validates_and_roundtrips() {
        let func_lib = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum(&func_lib);

        let mut parent = Graph::default();
        let inst = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        assert_eq!(def.inputs.len(), 2); // A, B from the def interface
        assert!(def.events.is_empty()); // no outlets
        parent.subgraphs.add(def);
        parent.add(inst);

        parent.validate_with(&func_lib);

        for format in SerdeFormat::all_formats_for_testing() {
            let bytes = parent.serialize(format).unwrap();
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
        let out_node = Node::new(NodeKind::SubgraphOutput);
        let out_id = out_node.id;

        let mut graph = Graph::default();
        graph.add(src);
        graph.add(out_node);
        graph.set_input_binding(InputPort::new(out_id, 0), (src_id, 0).into());

        let def = SubgraphDef {
            id: SubgraphId::unique(),
            name: "Source".into(),
            category: "Test".into(),
            graph,
            inputs: vec![],
            outputs: vec![fout("Val")],
            events: vec![],
            origin: None,
        };

        let mut parent = Graph::default();
        let inst = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        assert!(def.inputs.is_empty());
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
        let print_node = Node::new(NodeKind::Func(print_id));
        let print_node_id = print_node.id;

        let mut graph = Graph::default();
        graph.add(in_node);
        graph.add(print_node);
        graph.set_input_binding(InputPort::new(print_node_id, 0), (in_id, 0).into());

        let def = SubgraphDef {
            id: SubgraphId::unique(),
            name: "Sink".into(),
            category: "Test".into(),
            graph,
            inputs: vec![fin("msg")],
            outputs: vec![],
            events: vec![],
            origin: None,
        };

        let mut parent = Graph::default();
        let inst = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        assert_eq!(def.inputs.len(), 1);
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
            origin: None,
        });

        let mut parent = Graph::default();
        parent.add(Node::new(NodeKind::Subgraph(SubgraphRef::Linked(a_id))));
        parent.validate_with(&func_lib); // panics: recursion guard
    }

    /// A func with one event and no I/O, for exposed-event tests.
    fn ticker_func_lib() -> (FuncLib, FuncId) {
        use crate::event_lambda::EventLambda;
        use crate::func_lambda::FuncLambda;
        use crate::function::{Func, FuncBehavior, FuncEvent};

        let id = FuncId::unique();
        let mut lib = FuncLib::default();
        lib.add(Func {
            id,
            name: "ticker".into(),
            category: "Test".into(),
            terminal: true,
            behavior: FuncBehavior::Impure,
            version: 0,
            description: None,
            inputs: vec![],
            outputs: vec![],
            events: vec![FuncEvent {
                name: "tick".into(),
                event_lambda: EventLambda::default(),
            }],
            required_contexts: vec![],
            lambda: FuncLambda::default(),
        });
        (lib, id)
    }

    #[test]
    fn exposed_event_maps_to_interior_emitter() {
        let (func_lib, ticker) = ticker_func_lib();

        // def interior: a single `ticker` whose `tick` event is exposed.
        let emitter = Node::new(NodeKind::Func(ticker));
        let emitter_id = emitter.id;
        let mut graph = Graph::default();
        graph.add(emitter);

        let def = SubgraphDef {
            id: SubgraphId::unique(),
            name: "Exposer".into(),
            category: "Test".into(),
            graph,
            inputs: vec![],
            outputs: vec![],
            events: vec![SubgraphEvent {
                name: "tick".into(),
                emitter: emitter_id,
                emitter_event_idx: 0,
            }],
            origin: None,
        };

        let mut parent = Graph::default();
        let inst = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        assert_eq!(def.events.len(), 1);
        parent.subgraphs.add(def);
        parent.add(inst);

        parent.validate_with(&func_lib);
    }
}
