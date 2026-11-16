use glam::Vec2;
use scenarium::{
    Binding, DataType, GraphId, InputDecl, InputPort, Interface, Library, Node, Numbered,
    OutputPort, PortId, Ports, Subgraph, SubgraphId, TypeNames,
};

use crate::core::document::Document;
use crate::core::document::graph_view::NodePlacement;
use crate::core::document::internals::{DocumentFixture, insert_subgraph};
use crate::core::document::route::Route;
use crate::core::document::subgraph_side::SubgraphSide;
use crate::core::document::tie::{Tie, TieAngle, TieId};
use crate::core::document::wire_key::{DataWire, WireKey};
use crate::core::edit::error::{EditRefusal, IntentError};
use crate::core::edit::step::undo_step::UndoStep;
use crate::core::edit::subgraph_intent::{NewSubgraph, SubgraphIntent};

/// Commit `intent`, returning its steps.
fn commit(
    document: &mut Document,
    library: &Library,
    intent: SubgraphIntent,
) -> Result<Vec<UndoStep>, IntentError> {
    let mut steps = Vec::new();
    intent.commit(document, library, &mut steps)?;
    Ok(steps)
}

/// Revert `steps` newest first, as an undo does.
fn revert(document: &mut Document, steps: &[UndoStep]) {
    for step in steps.iter().rev() {
        step.revert(document);
    }
}

fn input(id: u32, name: &str) -> Numbered<InputDecl> {
    Numbered::new(PortId::new(id), InputDecl::optional(name, DataType::Int))
}

fn inputs(decls: Vec<Numbered<InputDecl>>) -> Interface {
    Interface::new(Ports {
        inputs: decls,
        outputs: vec![],
    })
}

/// A definition comes in with its side and goes out with it, and an undo of each puts the
/// document back; a removal while a node places the subgraph is refused, naming how many do.
#[test]
fn a_definition_comes_and_goes_with_its_side_unless_placed() {
    let DocumentFixture {
        mut document,
        library,
    } = DocumentFixture::default();
    let id = SubgraphId::from_u128(1);
    let subgraph = Subgraph::new("inner", Interface::default());
    let side = SubgraphSide::new(&subgraph.graph);
    let empty = document.clone();
    let added = commit(
        &mut document,
        &library,
        SubgraphIntent::Add(Box::new(NewSubgraph {
            id,
            subgraph: subgraph.clone(),
            side: side.clone(),
            type_names: TypeNames::default(),
        })),
    )
    .unwrap();
    assert_eq!(document.graphs.subgraph(id), Some(&subgraph));
    assert_eq!(document.sides[&id], side);
    document.validate().unwrap();
    let with = document.clone();

    let placed = document.graphs.main_mut().add(Node::instance(id));
    document
        .main_view
        .node_placements
        .insert(placed, NodePlacement::default());
    assert_eq!(
        commit(&mut document, &library, SubgraphIntent::Remove { id }).unwrap_err(),
        IntentError::Refused(EditRefusal::SubgraphInUse { uses: 1 })
    );
    document.graphs.main_mut().detach_node(placed);
    document.main_view.node_placements.remove(&placed);
    assert_eq!(document, with);

    let removed = commit(&mut document, &library, SubgraphIntent::Remove { id }).unwrap();
    assert_eq!(document, empty);
    revert(&mut document, &removed);
    assert_eq!(
        document, with,
        "an undone removal brings the side back as it was"
    );
    revert(&mut document, &added);
    assert_eq!(document, empty);
}

/// A reorder of the interface drops nothing and changes no view. A removed port takes the wire
/// on the instance in the main graph and the wire from the boundary node inside, each with its
/// route and the tie only it held, and an undo brings both back in both graphs.
#[test]
fn an_interface_edit_drops_wires_and_their_ties_in_every_graph_and_undo_restores_them() {
    let mut fixture = DocumentFixture::default();
    let id = SubgraphId::from_u128(1);
    let a = PortId::new(1);
    let mut subgraph = Subgraph::new("inner", inputs(vec![input(1, "a"), input(2, "b")]));
    let inside = subgraph.graph.add(fixture.plain_node());
    let inputs_node = subgraph.inputs_node();
    let inner_input = InputPort::new(inside, PortId::new(1));
    let inner_wire = WireKey::Data(DataWire {
        output: OutputPort::new(inputs_node, a),
        input: inner_input,
    });
    subgraph
        .graph
        .bindings
        .insert(inner_input, Binding::Wire(OutputPort::new(inputs_node, a)));
    insert_subgraph(&mut fixture.document, id, subgraph);
    let instance = fixture.add_instance(id);
    let producer = fixture.node_at(Vec2::new(0.0, 300.0));
    let outer_wire = fixture.wire(
        OutputPort::new(producer, PortId::new(1)),
        InputPort::new(instance, a),
    );
    let outer_tie = fixture.tie(Vec2::new(50.0, 50.0), [outer_wire]);
    let inner_tie = TieId::from_u128(77);
    let view = &mut fixture.document.sides.get_mut(&id).unwrap().view;
    view.ties.insert(
        inner_tie,
        Tie {
            pos: Vec2::ZERO,
            angle: TieAngle::Auto,
        },
    );
    view.routes.insert(inner_wire, Route::new([inner_tie]));
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    document.validate().unwrap();
    let before = document.clone();

    let reordered = commit(
        &mut document,
        &library,
        SubgraphIntent::SetInterface {
            id,
            to: inputs(vec![input(2, "b"), input(1, "a")]),
        },
    )
    .unwrap();
    assert_eq!(reordered.len(), 1);
    assert_eq!(
        document.main_view, before.main_view,
        "a reorder changes no view"
    );
    assert_eq!(document.sides, before.sides);
    revert(&mut document, &reordered);
    assert_eq!(document, before);

    let removed = commit(
        &mut document,
        &library,
        SubgraphIntent::SetInterface {
            id,
            to: inputs(vec![input(2, "b")]),
        },
    )
    .unwrap();
    let graph = GraphId::Subgraph(id);
    assert!(
        !document
            .graphs
            .main()
            .bindings
            .contains_key(&InputPort::new(instance, a))
    );
    assert!(
        !document
            .graphs
            .graph(graph)
            .bindings
            .contains_key(&inner_input)
    );
    assert!(document.main_view.routes.is_empty());
    assert!(!document.main_view.ties.contains_key(&outer_tie));
    assert!(document.view(graph).routes.is_empty());
    assert!(document.view(graph).ties.is_empty());
    document.validate().unwrap();
    revert(&mut document, &removed);
    assert_eq!(document, before, "both wires, routes and ties come back");
}

/// A rename takes the name trimmed, and an undo puts the old one back; a name another subgraph
/// holds takes the next free number, as Make Unique names a copy; a blank name, or the name the
/// subgraph has, writes nothing; a subgraph gone since writes nothing either.
#[test]
fn a_rename_takes_a_free_name_and_undoes() {
    let DocumentFixture {
        mut document,
        library,
    } = DocumentFixture::default();
    let [denoise, clean] = [1, 2].map(SubgraphId::from_u128);
    insert_subgraph(
        &mut document,
        denoise,
        Subgraph::new("Denoise", Interface::default()),
    );
    insert_subgraph(
        &mut document,
        clean,
        Subgraph::new("Clean", Interface::default()),
    );
    let name = |document: &Document, id| document.graphs.subgraph(id).unwrap().name.clone();
    let rename = |document: &mut Document, id, to: &str| {
        commit(
            document,
            &library,
            SubgraphIntent::Rename {
                id,
                to: to.to_owned(),
            },
        )
        .unwrap()
    };

    let steps = rename(&mut document, denoise, "  Stars  ");
    assert_eq!(steps.len(), 1);
    assert_eq!(name(&document, denoise), "Stars");
    revert(&mut document, &steps);
    assert_eq!(name(&document, denoise), "Denoise");

    rename(&mut document, denoise, "Clean");
    assert_eq!(
        name(&document, denoise),
        "Clean 2",
        "a taken name takes a number"
    );
    for unchanged in ["", "   ", "Clean 2"] {
        assert!(
            rename(&mut document, denoise, unchanged).is_empty(),
            "{unchanged:?} writes nothing"
        );
    }
    assert_eq!(name(&document, denoise), "Clean 2");
    assert!(rename(&mut document, SubgraphId::from_u128(9), "Gone").is_empty());
}
