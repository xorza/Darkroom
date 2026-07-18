//! Subgraph (composite-node) authoring types. A `SubgraphDef` is a reusable
//! definition that wraps an interior `Graph` and exposes a chosen set of
//! inputs/outputs/events, so it can be instantiated as a single node in a
//! parent graph. Execution-time flattening lives in `execution::flatten`.
//!
//! See `execution/README.md` Part A for the full design.

use anyhow::ensure;
use common::KeyIndexKey;
use common::{Result, id_type};
use serde::{Deserialize, Serialize};

use crate::graph::{Graph, NodeId, NodeSearch};
use crate::node::definition::{FuncInput, FuncOutput};

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
/// exposes. Behavior and sink-ness are *derived* from the interior at
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

    /// Library-free structural validation of this definition, in all
    /// builds — the def-level peer of [`Graph::check`], run wherever a def
    /// crosses an untrusted boundary (document load, subgraph import, the
    /// shared-library file). Checks the interior in def mode (boundary
    /// nodes allowed, at most one of each, recursing into nested defs) and
    /// that every exposed event names an existing interior emitter.
    /// Library-dependent checks (func resolution, port/event index ranges,
    /// type compatibility) stay with `Graph::check_with`.
    pub fn check(&self) -> Result<()> {
        ensure!(
            !self.id.is_nil(),
            "subgraph def {:?} has a nil id",
            self.name
        );
        self.graph.check_impl(true)?;
        self.graph.check_unique_node_ids(None)?;
        for event in &self.events {
            ensure!(
                self.graph
                    .find(&event.emitter, NodeSearch::TopLevel)
                    .is_some(),
                "exposed event {:?} names missing emitter {:?}",
                event.name,
                event.emitter
            );
        }
        Ok(())
    }

    /// An independent copy with a fresh def id and fresh interior node
    /// ids — nested defs' interiors included, recursively — with wiring
    /// and exposed-event emitters remapped to match, preserving the
    /// interface. Used to localize a `Linked` instance, make an instance
    /// unique, or import a def without sharing node identity with
    /// anything already in the document.
    pub fn fresh_copy(&self) -> SubgraphDef {
        let mut copy = self.remapped_interior();
        copy.id = SubgraphId::unique();
        // A fresh copy is its own def; the caller (library instancing) sets
        // `origin` to the source library id if it wants lineage.
        copy.origin = None;
        copy
    }

    /// The recursive half of [`Self::fresh_copy`]: an identical def (same
    /// id, name, interface, origin) whose interior node ids are freshly
    /// generated all the way down — this level via
    /// [`Graph::with_fresh_node_ids`] (which re-enters here for each
    /// nested def), exposed-event emitters remapped onto the new ids.
    pub(crate) fn remapped_interior(&self) -> SubgraphDef {
        let fresh = self.graph.with_fresh_node_ids();
        let events: Vec<SubgraphEvent> = self
            .events
            .iter()
            .map(|e| SubgraphEvent {
                emitter: fresh.id_map.get(&e.emitter).copied().unwrap_or(e.emitter),
                ..e.clone()
            })
            .collect();
        SubgraphDef {
            id: self.id,
            name: self.name.clone(),
            category: self.category.clone(),
            graph: fresh.graph,
            inputs: self.inputs.clone(),
            outputs: self.outputs.clone(),
            events,
            origin: self.origin,
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
    use crate::DataType;
    use crate::graph::{Binding, Graph, InputPort, Node, NodeKind};
    use crate::library::Library;
    use crate::node::definition::FuncId;
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
        let sum_node = Node::new(NodeKind::Func(sum_id));
        let out_node = Node::new(NodeKind::SubgraphOutput);

        let mut graph = Graph::default();
        let in_id = graph.add(in_node);
        let sum_node_id = graph.add(sum_node);
        let out_id = graph.add(out_node);
        graph.set_input_binding(InputPort::new(sum_node_id, 0), Binding::bind(in_id, 0));
        graph.set_input_binding(InputPort::new(sum_node_id, 1), Binding::bind(in_id, 1));
        graph.set_input_binding(InputPort::new(out_id, 0), Binding::bind(sum_node_id, 0));

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

        // Deep: wrap the def as a nested child of a parent def. The copy
        // keeps the child's (level-scoped) `SubgraphId` — the interior
        // instance node must still resolve — but the child's interior node
        // ids, wiring, and exposed-event emitter are all remapped too.
        let mut child = wrap_sum(&library);
        let child_id = child.id;
        let emitter = child.graph.iter().next().unwrap().id;
        child.events.push(SubgraphEvent {
            name: "tick".into(),
            emitter,
            emitter_event_idx: 0,
        });
        let child_node_ids: Vec<_> = child.graph.iter().map(|n| n.id).collect();
        let mut interior = Graph::default();
        interior.subgraphs.add(child);
        interior.add(Node::new(NodeKind::Subgraph(SubgraphRef::Local(child_id))));
        let parent = SubgraphDef::new(SubgraphId::unique(), "Parent").graph(interior);

        let parent_copy = parent.fresh_copy();
        parent_copy.graph.check().unwrap();
        let child_copy = parent_copy
            .graph
            .subgraphs
            .by_key(&child_id)
            .expect("nested def id is preserved");
        let child_copy_ids: Vec<_> = child_copy.graph.iter().map(|n| n.id).collect();
        assert_eq!(child_copy_ids.len(), child_node_ids.len());
        for id in &child_copy_ids {
            assert!(
                !child_node_ids.contains(id),
                "nested interior id must be remapped"
            );
        }
        for (dst, src) in child_copy.graph.edges() {
            assert!(child_copy_ids.contains(&dst.node_id));
            assert!(child_copy_ids.contains(&src.node_id));
        }
        let copy_emitter = child_copy.events[0].emitter;
        assert_ne!(copy_emitter, emitter, "nested event emitter is remapped");
        assert!(
            child_copy_ids.contains(&copy_emitter),
            "onto the copy's own interior node"
        );
    }

    #[test]
    fn def_check_accepts_boundaries_and_rejects_corruption() {
        let library = test_func_lib(TestFuncHooks::default());

        // The healthy fixture passes: boundary nodes are legal inside a
        // def interior (one of each).
        wrap_sum(&library).check().unwrap();

        // A boundary node in a root graph is misplaced.
        let mut root = Graph::default();
        root.add(Node::new(NodeKind::SubgraphInput));
        let err = root.check().unwrap_err().to_string();
        assert!(err.contains("only valid inside a subgraph def"), "{err}");

        // Two input boundaries in one interior: flatten routes through the
        // first match, so the duplicate is refused rather than misrouted.
        let mut def = wrap_sum(&library);
        def.graph.add(Node::new(NodeKind::SubgraphInput));
        let err = def.check().unwrap_err().to_string();
        assert!(err.contains("at most one SubgraphInput"), "{err}");

        // ...and a parent graph's check recurses into its local defs,
        // naming the offender.
        let def_id = def.id;
        let mut parent = Graph::default();
        parent.add(Node::new(NodeKind::Subgraph(SubgraphRef::Local(def_id))));
        parent.subgraphs.add(def);
        let err = format!("{:#}", parent.check().unwrap_err());
        assert!(
            err.contains("in local subgraph") && err.contains("at most one SubgraphInput"),
            "{err}"
        );

        // An exposed event must name an interior node.
        let mut def = wrap_sum(&library);
        def.events.push(SubgraphEvent {
            name: "tick".into(),
            emitter: NodeId::unique(),
            emitter_event_idx: 0,
        });
        let err = def.check().unwrap_err().to_string();
        assert!(err.contains("names missing emitter"), "{err}");

        // A nil def id is refused.
        let mut def = wrap_sum(&library);
        def.id = SubgraphId::nil();
        let err = def.check().unwrap_err().to_string();
        assert!(err.contains("has a nil id"), "{err}");
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
        let out_node = Node::new(NodeKind::SubgraphOutput);

        let mut graph = Graph::default();
        let src_id = graph.add(src);
        let out_id = graph.add(out_node);
        graph.set_input_binding(InputPort::new(out_id, 0), Binding::bind(src_id, 0));

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
        // in(msg) -> print (sink). No SubgraphOutput node.
        let library = test_func_lib(TestFuncHooks::default());
        let print_id = library.by_name("Print").unwrap().id;

        let in_node = Node::new(NodeKind::SubgraphInput);
        let print_node = Node::new(NodeKind::Func(print_id));

        let mut graph = Graph::default();
        let in_id = graph.add(in_node);
        let print_node_id = graph.add(print_node);
        graph.set_input_binding(InputPort::new(print_node_id, 0), Binding::bind(in_id, 0));

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
        use crate::node::definition::Func;
        use crate::node::event::EventLambda;

        let id = FuncId::unique();
        let mut lib = Library::default();
        lib.add(
            Func::new(id, "ticker")
                .category("Test")
                .sink()
                .event("tick", EventLambda::default()),
        );
        (lib, id)
    }

    #[test]
    fn exposed_event_maps_to_interior_emitter() {
        let (library, ticker) = ticker_func_lib();

        // def interior: a single `ticker` whose `tick` event is exposed.
        let emitter = Node::new(NodeKind::Func(ticker));
        let mut graph = Graph::default();
        let emitter_id = graph.add(emitter);

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
