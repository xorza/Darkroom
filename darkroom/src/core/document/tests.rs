use std::sync::Arc;

use palantir::DockOperation;
use scenarium::{
    Binding, EventPort, InputPort, Interface, OutputPort, Ports, Subgraph, SubgraphId,
    numbered_port,
};

use super::*;
use crate::core::document::internals::DocumentFixture;
use crate::core::document::tie::Tie;
use crate::core::document::wire_key::{DataWire, WireKey};

/// Every `NodeId`-keyed cache sweeps against one rule, and the two narrowings of it are strict
/// subsets, so a node that is gone is gone by all of them at once. Catches the predicates
/// diverging again.
#[test]
fn node_liveness_is_one_rule_with_two_declared_narrowings() {
    let mut fixture = DocumentFixture::default();
    let preview_func = preview_sink::func(Arc::default());
    fixture.library.add(preview_func.clone());
    let preview = fixture
        .document
        .graphs
        .main_mut()
        .add(Node::from(&preview_func));
    let plain = fixture.stub_at(Vec2::ZERO);
    let mut document = fixture.document;

    // The shared rule answers for a graph node and a stub alike.
    for node in [preview, plain] {
        assert!(document.holds_node(node), "the document holds {node:?}");
    }
    let (preview_program, plain_program) = (ProgramId::from(preview), ProgramId::from(plain));
    assert!(document.holds_preview_node(preview_program));
    assert!(
        !document.holds_preview_node(plain_program),
        "a stub retains no published value"
    );
    assert!(
        !document.holds_tab(TabRef::ImageViewer(plain_program)),
        "a stub has no viewer"
    );
    let nested = ProgramId::new(&[NodeId::unique(), preview]);
    assert!(
        !document.holds_preview_node(nested),
        "a path past the main graph names no node of it"
    );

    // The same rule asked of a tab: a viewer dies with its node, the non-node tabs never do.
    assert!(document.holds_tab(TabRef::ImageViewer(preview_program)));
    assert!(document.holds_tab(TabRef::Graph));
    assert!(document.holds_tab(TabRef::Preferences));

    document.graphs.main_mut().detach_node(preview);
    assert!(!document.holds_node(preview), "the rule says gone");
    assert!(
        !document.holds_preview_node(preview_program),
        "and so does the narrowing"
    );
    assert!(
        !document.holds_tab(TabRef::ImageViewer(preview_program)),
        "and so does the tab lift"
    );
    assert!(
        document.holds_node(plain),
        "the surviving node is untouched"
    );
}

/// A removal closes the node's viewer tab and an inspector pinned to it: every removal, by an
/// edit, an undo or a redo, goes through `remove_node`, so no tab outlives its node. The
/// selection's inspector stays, as it names no node.
#[test]
fn removing_a_node_closes_its_viewer_and_its_pinned_inspector() {
    let mut fixture = DocumentFixture::sample();
    let node_id = fixture.add(&preview_sink::func(Arc::default()));
    let mut document = fixture.document;
    let tabs = [
        TabRef::ImageViewer(ProgramId::from(node_id)),
        TabRef::Inspector(InspectTarget::Node(node_id)),
        TabRef::Inspector(InspectTarget::Selection),
    ];
    for tab in tabs {
        document.layout.apply(DockOperation::OpenTab { tab });
    }
    document
        .validate()
        .expect("each tab names a node the graph holds");

    document.remove_node(node_id);
    let open: Vec<TabRef> = document
        .layout
        .all_tabs()
        .filter(|tab| tabs.contains(tab))
        .collect();
    assert_eq!(open, [TabRef::Inspector(InspectTarget::Selection)]);
    document.validate().expect("no tab names the removed node");
}

/// A viewer tab is held while its path names a preview node of the main graph, and the load
/// closes one whose path goes through an instance, as the main graph places that node once.
#[test]
fn reconcile_closes_a_viewer_whose_path_leaves_the_main_graph() {
    let mut fixture = DocumentFixture::default();
    let preview = fixture.add(&preview_sink::func(Arc::default()));
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    let main = ProgramId::from(preview);
    let nested = ProgramId::new(&[NodeId::unique(), preview]);
    for program_id in [main, nested] {
        document.layout.apply(DockOperation::OpenTab {
            tab: TabRef::ImageViewer(program_id),
        });
    }

    let report = document.reconcile(&library);
    assert_eq!(
        report.repairs,
        [DocumentRepair::ViewerClosed { program_id: nested }]
    );
    let viewers: Vec<TabRef> = document
        .layout
        .all_tabs()
        .filter(|tab| matches!(tab, TabRef::ImageViewer(_)))
        .collect();
    assert_eq!(viewers, [TabRef::ImageViewer(main)]);
}

/// The load repairs the view and the layout around the graph, in order,
/// and reports each repair: a bad viewport resets, a placement for no node
/// goes, a non-finite position and a missing placement go to the origin, and
/// a viewer on a node that is no preview node closes. A selected node that
/// lacked a placement stays selected, as it is placed first.
#[test]
fn reconcile_repairs_the_view_and_the_layout() {
    let mut fixture = DocumentFixture::default();
    let shown = fixture.node_at(Vec2::new(f32::NAN, 0.0));
    let unplaced = fixture.node_at(Vec2::ZERO);
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    let ghost = NodeId::unique();
    document.main_view.viewport = Viewport {
        pan: Vec2::ZERO,
        zoom: 0.0,
    };
    document.main_view.node_placements.remove(&unplaced);
    document
        .main_view
        .node_placements
        .insert(ghost, NodePlacement::default());
    document.main_view.selected = [unplaced].map(ItemId::Node).into_iter().collect();
    document.layout.apply(DockOperation::OpenTab {
        tab: TabRef::ImageViewer(ProgramId::from(shown)),
    });

    let report = document.reconcile(&library);
    assert_eq!(
        report.repairs,
        [
            DocumentRepair::ViewportReset,
            DocumentRepair::PlacementDropped { node_id: ghost },
            DocumentRepair::PosReset { node_id: shown },
            DocumentRepair::Placed { node_id: unplaced },
            DocumentRepair::ViewerClosed {
                program_id: ProgramId::from(shown),
            },
        ]
    );
    assert_eq!(document.main_view.node_placements[&shown].pos, Vec2::ZERO);
    assert_eq!(
        document.main_view.node_placements[&unplaced].pos,
        Vec2::ZERO
    );
    assert!(
        document
            .main_view
            .selected
            .contains(&ItemId::Node(unplaced)),
        "placed before the selection is checked, so it stays selected"
    );
    document.validate().expect("a reconciled document is valid");
    assert!(
        document.reconcile(&library).is_empty(),
        "nothing is left to repair"
    );
}

/// The load repairs the ties and the routes, in order, and reports each
/// repair. Routes go in wire order: on `w1 = a.0 → b.0`, through `[t1, ghost,
/// t1]`, the step to no tie and the repeat go; the route keyed by `a.0 → b.1`,
/// a wire the graph does not hold, goes; on `w3 = a.0 → b.2`, through
/// `[ghost]`, the step goes and the route left empty with it. Then ties in id
/// order: `t1` at NaN moves to the origin, `t2` pinned to 400° turns itself,
/// `t3`, which no route names, goes, and so does `t4`, which only the dropped
/// route named. Last, the selected `t3` leaves the selection.
#[test]
fn reconcile_repairs_the_ties() {
    let mut fixture = DocumentFixture::default();
    let [a, b] = [0.0, 220.0].map(|x| fixture.node_at(Vec2::new(x, 0.0)));
    let w1 = fixture.wire(
        OutputPort::new(a, numbered_port(1)),
        InputPort::new(b, numbered_port(1)),
    );
    let w2 = fixture.subscribe(EventPort::new(a, numbered_port(1)), b);
    let w3 = fixture.wire(
        OutputPort::new(a, numbered_port(1)),
        InputPort::new(b, numbered_port(3)),
    );
    let unheld = WireKey::Data(DataWire {
        output: OutputPort::new(a, numbered_port(1)),
        input: InputPort::new(b, numbered_port(2)),
    });
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    let [t1, t2, t3, t4, ghost] = [1, 2, 3, 4, 5].map(TieId::from_u128);
    let view = &mut document.main_view;
    for (tie, pos, angle) in [
        (t1, Vec2::new(f32::NAN, 0.0), TieAngle::Auto),
        (t2, Vec2::ZERO, TieAngle::Pinned(400.0)),
        (t3, Vec2::ZERO, TieAngle::Auto),
        (t4, Vec2::ZERO, TieAngle::Pinned(90.0)),
    ] {
        view.ties.insert(tie, Tie { pos, angle });
    }
    view.routes = [
        (w1, Route::unchecked([t1, ghost, t1])),
        (unheld, Route::unchecked([t4])),
        (w3, Route::unchecked([ghost])),
        (w2, Route::unchecked([t2])),
    ]
    .into_iter()
    .collect();
    view.selected = [ItemId::Tie(t1), ItemId::Tie(t3)].into_iter().collect();

    let report = document.reconcile(&library);
    assert_eq!(
        report.repairs,
        [
            DocumentRepair::RouteStepDropped {
                wire: w1,
                tie: ghost
            },
            DocumentRepair::RouteStepRepeated { wire: w1, tie: t1 },
            DocumentRepair::RouteDropped { wire: unheld },
            DocumentRepair::RouteStepDropped {
                wire: w3,
                tie: ghost
            },
            DocumentRepair::EmptyRouteDropped { wire: w3 },
            DocumentRepair::TiePosReset { tie: t1 },
            DocumentRepair::TieAngleFreed { tie: t2 },
            DocumentRepair::TieDropped { tie: t3 },
            DocumentRepair::TieDropped { tie: t4 },
            DocumentRepair::SelectionDropped {
                item: ItemId::Tie(t3)
            },
        ]
    );
    let view = &document.main_view;
    assert_eq!(
        view.routes,
        [(w1, Route::new([t1])), (w2, Route::new([t2]))]
            .into_iter()
            .collect()
    );
    assert_eq!(
        view.ties,
        [
            (
                t1,
                Tie {
                    pos: Vec2::ZERO,
                    angle: TieAngle::Auto
                }
            ),
            (
                t2,
                Tie {
                    pos: Vec2::ZERO,
                    angle: TieAngle::Auto
                }
            ),
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(view.selected, [ItemId::Tie(t1)].into_iter().collect());
    document.validate().expect("a reconciled document is valid");
    assert!(
        document.reconcile(&library).is_empty(),
        "nothing is left to repair"
    );
}

/// A path a binding holds is absolute or unset in memory: a stub's relative one fails the
/// check and names its node, while an absolute one and an empty one pass.
#[test]
fn validation_holds_paths_absolute() {
    use std::path::{self, Path};

    use scenarium::ConstValue;

    let mut fixture = DocumentFixture::sample();
    let stub = fixture.stub_at(Vec2::ZERO);
    let absolute = path::absolute(Path::new("x.tiff")).unwrap();
    for valid in [absolute.to_str().unwrap(), ""] {
        fixture.bind_stub(stub, ConstValue::FsPath(valid.to_string()));
        fixture.document.validate().unwrap();
    }
    fixture.bind_stub(stub, ConstValue::FsPath("inputs/x.tiff".to_string()));
    let document = fixture.document;
    assert!(matches!(
        document.validate(),
        Err(DocumentValidationError::RelativePath { node, path })
            if node == NodeAddr::new(GraphId::Main, stub) && path == "inputs/x.tiff"
    ));
}

/// The path walks reach a subgraph's constants: the check names a relative path there by its
/// graph, and a mapping rewrites it.
#[test]
fn a_subgraph_path_is_checked_and_mapped() {
    use scenarium::ConstValue;

    let mut fixture = DocumentFixture::default();
    let node = fixture.plain_node();
    let id = SubgraphId::from_u128(1);
    let mut subgraph = Subgraph::new("inner", Interface::new(Ports::default()));
    let inner = subgraph.graph.add(node);
    let port = InputPort::new(inner, numbered_port(1));
    subgraph.graph.set_input_binding(
        port,
        Binding::Const(ConstValue::FsPath("inputs/x.tiff".to_string())),
    );
    fixture.document.graphs.insert_subgraph(id, subgraph);
    let mut document = fixture.document;
    let addr = NodeAddr::new(GraphId::Subgraph(id), inner);

    assert!(matches!(
        document.validate(),
        Err(DocumentValidationError::RelativePath { node, path })
            if node == addr && path == "inputs/x.tiff"
    ));
    document.map_paths(|path| Some(format!("/root/{path}")));
    assert_eq!(
        document.graphs.graph(GraphId::Subgraph(id)).bindings[&port],
        Binding::Const(ConstValue::FsPath("/root/inputs/x.tiff".to_string()))
    );
    let mut visited = Vec::new();
    document.for_each_path(|node, path| visited.push((node, path.to_string())));
    assert_eq!(visited, [(addr, "/root/inputs/x.tiff".to_string())]);
}
