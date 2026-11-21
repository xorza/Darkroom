//! [`ReplacePlan`]: what Replace with… would make of a node, before a person confirms it.

use scenarium::{GraphId, Library, Node, NodeId, NodeRemap};

use crate::core::document::Document;
use crate::core::edit::graph_intent::GraphIntent;

/// A node to put in place of another, and where each port of the old one would go, as
/// [`NodeRemap::propose`] pairs them: what the Replace dialog shows and its confirm commits.
#[derive(Debug)]
pub(crate) struct ReplacePlan {
    pub(crate) graph: GraphId,
    pub(crate) node_id: NodeId,
    pub(crate) with: Node,
    pub(crate) remap: NodeRemap,
    /// The old node's name and the new one's func name, for the dialog's title.
    pub(crate) from_name: String,
    pub(crate) to_name: String,
    /// Each port of the old node, inputs, then outputs, then events, with where it goes.
    pub(crate) ports: Vec<PlannedPort>,
}

/// One port of the node a replace takes out, and the port of the new node it goes to, or `None`
/// where it is kept as a ghost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedPort {
    pub(crate) side: PortSide,
    pub(crate) name: String,
    pub(crate) to: Option<String>,
}

/// Which list of a node a planned port is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PortSide {
    Input,
    Output,
    Event,
}

impl ReplacePlan {
    /// The plan for putting `with` in place of node or ghost node `node_id` of `graph`, as
    /// [`NodeRemap::propose`] pairs their ports: `None` when the graph no longer holds the node,
    /// or `with` does not resolve in it. A ghost node that saved nothing has no ports to pair.
    pub(crate) fn propose(
        document: &Document,
        graph: GraphId,
        library: &Library,
        node_id: NodeId,
        with: Node,
    ) -> Option<Self> {
        let held = document.graphs.graph(graph);
        let scope = document.graphs.scope(library, graph);
        let (from_name, before) = if let Some(node) = held.find(node_id) {
            (scope.title(node).to_owned(), scope.decl(node).saved())
        } else {
            let ghost = held.ghost_node(node_id)?;
            (
                ghost.title().to_owned(),
                ghost.saved.clone().unwrap_or_default(),
            )
        };
        let after = scope.resolve(&with).ok()?.saved();
        let remap = NodeRemap::propose(&before, &after);
        let inputs = before.inputs.iter().map(|input| PlannedPort {
            side: PortSide::Input,
            name: input.name.clone(),
            to: remap
                .inputs
                .get(input.id)
                .and_then(|new| after.input(new))
                .map(|input| input.name.clone()),
        });
        let outputs = before.outputs.iter().map(|output| PlannedPort {
            side: PortSide::Output,
            name: output.name.clone(),
            to: remap
                .outputs
                .get(output.id)
                .and_then(|new| after.output(new))
                .map(|output| output.name.clone()),
        });
        let events = before.events.iter().map(|event| PlannedPort {
            side: PortSide::Event,
            name: event.name.clone(),
            to: remap
                .events
                .get(event.id)
                .and_then(|new| after.event(new))
                .map(|event| event.name.clone()),
        });
        let ports = inputs.chain(outputs).chain(events).collect();
        Some(Self {
            graph,
            node_id,
            to_name: after.name.clone(),
            with,
            remap,
            from_name,
            ports,
        })
    }

    /// The edit a confirm commits.
    pub(crate) fn intent(self) -> GraphIntent {
        GraphIntent::ReplaceNode {
            node_id: self.node_id,
            with: self.with,
            remap: self.remap,
        }
    }
}

#[cfg(test)]
mod tests {
    use scenarium::{
        Binding, ConstValue, DataType, DetachReason, FuncId, GhostBinding, GhostNode, GraphId,
        InputDecl, InputPort, InputState, Node, NodeDecl, OutputDecl, PortId, Unresolved,
        internals,
    };

    use crate::core::document::internals::DocumentFixture;
    use crate::core::edit::replace_plan::{PlannedPort, PortSide, ReplacePlan};

    /// `src` into `blur`'s `x`, 0.5 on its `y`, its `out` into `sink`; `sharpen` registered
    /// beside them, with `x` 5, a `String` 1 and `out` 1.
    fn fixture() -> (DocumentFixture, [scenarium::NodeId; 3], scenarium::Func) {
        let p = PortId::new;
        let src = internals::stub_func(FuncId::unique(), "src")
            .pure()
            .output(1, OutputDecl::new("value", DataType::Int))
            .build();
        let blur = internals::stub_func(FuncId::unique(), "blur")
            .pure()
            .input(1, InputDecl::optional("x", DataType::Int))
            .input(2, InputDecl::optional("y", DataType::Float))
            .output(1, OutputDecl::new("out", DataType::Int))
            .build();
        let sharpen = internals::stub_func(FuncId::unique(), "sharpen")
            .pure()
            .input(5, InputDecl::optional("x", DataType::Int))
            .input(1, InputDecl::optional("z", DataType::String))
            .output(1, OutputDecl::new("out", DataType::Int))
            .build();
        let sink = internals::stub_func(FuncId::unique(), "sink")
            .input(1, InputDecl::required("in", DataType::Int))
            .build();
        let mut fixture = DocumentFixture::default();
        let ids = [&src, &blur, &sink].map(|func| fixture.add(func));
        fixture.library.add(sharpen.clone());
        let graph = fixture.document.graphs.main_mut();
        let [a, b, c] = ids;
        let bound: Vec<InputPort> = graph.wiring().bindings().map(|(port, _)| port).collect();
        for port in bound {
            graph.set_input_binding(port, None);
        }
        graph.set_input_binding(InputPort::new(b, p(1)), Binding::wire(a, p(1)));
        graph.set_input_binding(
            InputPort::new(b, p(2)),
            Binding::Const(ConstValue::Float(0.5)),
        );
        graph.set_input_binding(InputPort::new(c, p(1)), Binding::wire(b, p(1)));
        (fixture, ids, sharpen)
    }

    /// The plan lists each old port with the new one it goes to: `x` by name, `out` by id, and
    /// `y`, which `sharpen` lacks, kept as a ghost; its confirm puts `sharpen` in `blur`'s place
    /// with `src` on `x` 5, `sink` on its `out`, and 0.5 a ghost, and one undo puts `blur` back.
    #[test]
    fn a_replace_moves_each_port_the_plan_lists_and_undoes_as_one() {
        let (fixture, [a, b, c], sharpen) = fixture();
        let DocumentFixture {
            mut document,
            library,
        } = fixture;
        let before = document.clone();
        let plan =
            ReplacePlan::propose(&document, GraphId::Main, &library, b, Node::from(&sharpen))
                .unwrap();
        let port = |side, name: &str, to: Option<&str>| PlannedPort {
            side,
            name: name.to_owned(),
            to: to.map(str::to_owned),
        };
        assert_eq!(
            (plan.from_name.as_str(), plan.to_name.as_str()),
            ("blur", "sharpen")
        );
        assert_eq!(
            plan.ports,
            [
                port(PortSide::Input, "x", Some("x")),
                port(PortSide::Input, "y", None),
                port(PortSide::Output, "out", Some("out")),
            ]
        );
        let mut steps = Vec::new();
        plan.intent()
            .commit(&mut document, GraphId::Main, &library, &mut steps)
            .unwrap();
        assert_eq!(steps.len(), 2, "the removal and the insertion");
        let graph = document.graphs.main();
        let p = PortId::new;
        assert_eq!(graph.find(b).unwrap().func_id(), Some(sharpen.id()));
        assert_eq!(
            graph.wiring().binding(InputPort::new(b, p(5))),
            Some(&Binding::wire(a, p(1)))
        );
        assert_eq!(
            graph.wiring().binding(InputPort::new(c, p(1))),
            Some(&Binding::wire(b, p(1)))
        );
        assert_eq!(
            graph
                .ghost_wiring()
                .binding(InputPort::new(b, p(2)))
                .map(GhostBinding::binding),
            Some(Binding::Const(ConstValue::Float(0.5)))
        );
        assert!(
            document
                .view(GraphId::Main)
                .node_placements
                .contains_key(&b)
        );
        document.validate().unwrap();
        for step in steps.iter().rev() {
            step.revert(&mut document);
        }
        assert_eq!(document, before);
    }

    /// A ghost node replaced by a func its saved declaration matches comes back a node, its ghost
    /// wire live on the port the plan names.
    #[test]
    fn a_replaced_ghost_node_takes_its_ghost_wires_live() {
        let (mut fixture, [a, b, _], sharpen) = fixture();
        let p = PortId::new;
        let saved = {
            NodeDecl::Func(
                fixture
                    .library
                    .by_id(
                        fixture
                            .document
                            .graphs
                            .main()
                            .find(b)
                            .unwrap()
                            .func_id()
                            .unwrap(),
                    )
                    .unwrap(),
            )
            .saved()
        };
        let graph = fixture.document.graphs.main_mut();
        let record = graph.detach_node(b);
        graph.insert_ghost_node(
            b,
            GhostNode {
                node: record.node().clone(),
                reason: DetachReason::Unresolved(Unresolved::MissingFunc),
                saved: Some(saved),
            },
        );
        graph.set_input_state(
            InputPort::new(b, p(1)),
            InputState::Ghost(GhostBinding::from(Binding::wire(a, p(1)))),
        );
        let DocumentFixture {
            mut document,
            library,
        } = fixture;
        let plan =
            ReplacePlan::propose(&document, GraphId::Main, &library, b, Node::from(&sharpen))
                .unwrap();
        plan.intent()
            .commit(&mut document, GraphId::Main, &library, &mut Vec::new())
            .unwrap();
        let graph = document.graphs.main();
        assert!(graph.ghost_node(b).is_none());
        assert_eq!(
            graph.wiring().binding(InputPort::new(b, p(5))),
            Some(&Binding::wire(a, p(1)))
        );
        document.validate().unwrap();
    }
}
