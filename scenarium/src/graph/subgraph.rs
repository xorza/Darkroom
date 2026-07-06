//! Subgraph (composite-node) authoring types. A `SubgraphDef` is a reusable
//! definition that wraps an interior `Graph` and exposes a chosen set of
//! inputs/outputs/events, so it can be instantiated as a single node in a
//! parent graph. Execution-time flattening lives in `execution::flatten`.
//!
//! See `execution/README.md` Part A for the full design.

use common::KeyIndexKey;
use common::id_type;
use serde::{Deserialize, Serialize};

use crate::graph::{Graph, NodeId};
use crate::node::function::{FuncInput, FuncOutput};

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
/// See `execution/README.md` Part A §4.5.
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
/// the linked/local distinction: `Linked` defs live in `Library.subgraphs`
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
    /// Start a composite definition. Defaults: empty category/interface and an
    /// empty interior `Graph`, no `origin` — set the rest with the chained
    /// builders below.
    pub fn new(id: impl Into<SubgraphId>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = category.into();
        self
    }

    pub fn graph(mut self, graph: Graph) -> Self {
        self.graph = graph;
        self
    }

    pub fn input(mut self, input: FuncInput) -> Self {
        self.inputs.push(input);
        self
    }

    pub fn inputs(mut self, inputs: impl IntoIterator<Item = FuncInput>) -> Self {
        self.inputs.extend(inputs);
        self
    }

    pub fn output(mut self, output: FuncOutput) -> Self {
        self.outputs.push(output);
        self
    }

    pub fn outputs(mut self, outputs: impl IntoIterator<Item = FuncOutput>) -> Self {
        self.outputs.extend(outputs);
        self
    }

    pub fn event(mut self, event: SubgraphEvent) -> Self {
        self.events.push(event);
        self
    }

    pub fn events(mut self, events: impl IntoIterator<Item = SubgraphEvent>) -> Self {
        self.events.extend(events);
        self
    }

    /// Record the shared-library def this one was copied from. See
    /// [`SubgraphDef::origin`].
    pub fn origin(mut self, origin: SubgraphId) -> Self {
        self.origin = Some(origin);
        self
    }

    /// An independent copy with a fresh def id and fresh interior node
    /// ids (exposed-event emitters remapped to match), preserving the
    /// interface. Used to localize a `Linked` instance or make an
    /// instance unique without sharing identity with the original.
    pub fn fresh_copy(&self) -> SubgraphDef {
        let fresh = self.graph.with_fresh_node_ids();
        let events: Vec<SubgraphEvent> = self
            .events
            .iter()
            .map(|e| SubgraphEvent {
                emitter: fresh.id_map.get(&e.emitter).copied().unwrap_or(e.emitter),
                ..e.clone()
            })
            .collect();
        // A fresh copy is its own def; the caller (library instancing) sets
        // `origin` to the source library id if it wants lineage.
        SubgraphDef::new(SubgraphId::unique(), self.name.clone())
            .category(self.category.clone())
            .graph(fresh.graph)
            .inputs(self.inputs.clone())
            .outputs(self.outputs.clone())
            .events(events)
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
    use crate::graph::{Graph, InputPort, Node, NodeKind};
    use crate::library::Library;
    use crate::node::function::FuncId;
    use crate::testing::{TestFuncHooks, test_func_lib};
    use common::SerdeFormat;

    fn fin(name: &str) -> FuncInput {
        FuncInput::optional(name, DataType::Int)
    }

    fn fout(name: &str) -> FuncOutput {
        FuncOutput::new(name, DataType::Int)
    }

    /// `in(A, B) -> sum -> out(Sum)`.
    fn wrap_sum(library: &Library) -> SubgraphDef {
        let sum_id = library.by_name("sum").unwrap().id;

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

        SubgraphDef::new(SubgraphId::unique(), "WrapSum")
            .category("Test")
            .graph(graph)
            .inputs([fin("A"), fin("B")])
            .output(fout("Sum"))
    }

    #[test]
    fn fresh_copy_remaps_interior_ids_and_preserves_wiring() {
        let library = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum(&library);
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
        let library = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum(&library);

        let mut parent = Graph::default();
        let inst = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        assert_eq!(def.inputs.len(), 2); // A, B from the def interface
        assert!(def.events.is_empty()); // no outlets
        parent.subgraphs.add(def);
        parent.add(inst);

        parent.validate_with(&library);

        for format in SerdeFormat::all_formats_for_testing() {
            let bytes = parent.serialize(format).unwrap();
            let back = Graph::deserialize(&bytes, format).unwrap();
            assert_eq!(parent, back);
        }
    }

    #[test]
    fn linked_subgraph_resolves_from_library() {
        let mut library = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum(&library);
        let def_id = def.id;
        library.add_subgraph(def);

        let mut parent = Graph::default();
        let def_ref = library.subgraph_by_id(&def_id).unwrap();
        let inst = Node::subgraph_instance(def_ref, SubgraphRef::Linked(def_id));
        parent.add(inst);

        parent.validate_with(&library);
    }

    #[test]
    fn zero_input_subgraph_omits_input_boundary() {
        // `get_a` (0 inputs, 1 output) -> out(Val). No SubgraphInput node.
        let library = test_func_lib(TestFuncHooks::default());
        let get_a_id = library.by_name("get_a").unwrap().id;

        let src = Node::new(NodeKind::Func(get_a_id));
        let src_id = src.id;
        let out_node = Node::new(NodeKind::SubgraphOutput);
        let out_id = out_node.id;

        let mut graph = Graph::default();
        graph.add(src);
        graph.add(out_node);
        graph.set_input_binding(InputPort::new(out_id, 0), (src_id, 0).into());

        let def = SubgraphDef::new(SubgraphId::unique(), "Source")
            .category("Test")
            .graph(graph)
            .output(fout("Val"));

        let mut parent = Graph::default();
        let inst = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        assert!(def.inputs.is_empty());
        parent.subgraphs.add(def);
        parent.add(inst);

        parent.validate_with(&library);
    }

    #[test]
    fn zero_output_subgraph_omits_output_boundary() {
        // in(msg) -> print (terminal). No SubgraphOutput node.
        let library = test_func_lib(TestFuncHooks::default());
        let print_id = library.by_name("Print").unwrap().id;

        let in_node = Node::new(NodeKind::SubgraphInput);
        let in_id = in_node.id;
        let print_node = Node::new(NodeKind::Func(print_id));
        let print_node_id = print_node.id;

        let mut graph = Graph::default();
        graph.add(in_node);
        graph.add(print_node);
        graph.set_input_binding(InputPort::new(print_node_id, 0), (in_id, 0).into());

        let def = SubgraphDef::new(SubgraphId::unique(), "Sink")
            .category("Test")
            .graph(graph)
            .input(fin("msg"));

        let mut parent = Graph::default();
        let inst = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        assert_eq!(def.inputs.len(), 1);
        parent.subgraphs.add(def);
        parent.add(inst);

        parent.validate_with(&library);
    }

    #[test]
    #[should_panic(expected = "recursive")]
    fn recursive_subgraph_rejected() {
        let mut library = test_func_lib(TestFuncHooks::default());

        // A's interior contains a node instancing A again (mutual self-reference).
        let a_id = SubgraphId::unique();
        let mut inner = Graph::default();
        inner.add(Node::new(NodeKind::Subgraph(SubgraphRef::Linked(a_id))));
        library.add_subgraph(SubgraphDef::new(a_id, "A").category("Test").graph(inner));

        let mut parent = Graph::default();
        parent.add(Node::new(NodeKind::Subgraph(SubgraphRef::Linked(a_id))));
        parent.validate_with(&library); // panics: recursion guard
    }

    /// A func with one event and no I/O, for exposed-event tests.
    fn ticker_func_lib() -> (Library, FuncId) {
        use crate::node::event_lambda::EventLambda;
        use crate::node::function::Func;

        let id = FuncId::unique();
        let mut lib = Library::default();
        lib.add(
            Func::new(id, "ticker")
                .category("Test")
                .terminal()
                .event("tick", EventLambda::default()),
        );
        (lib, id)
    }

    #[test]
    fn exposed_event_maps_to_interior_emitter() {
        let (library, ticker) = ticker_func_lib();

        // def interior: a single `ticker` whose `tick` event is exposed.
        let emitter = Node::new(NodeKind::Func(ticker));
        let emitter_id = emitter.id;
        let mut graph = Graph::default();
        graph.add(emitter);

        let def = SubgraphDef::new(SubgraphId::unique(), "Exposer")
            .category("Test")
            .graph(graph)
            .event(SubgraphEvent {
                name: "tick".into(),
                emitter: emitter_id,
                emitter_event_idx: 0,
            });

        let mut parent = Graph::default();
        let inst = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        assert_eq!(def.events.len(), 1);
        parent.subgraphs.add(def);
        parent.add(inst);

        parent.validate_with(&library);
    }
}
