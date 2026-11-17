use std::sync::Arc;

use scenarium::{Binding, EventPort, InputPort, Interface, OutputPort, PortId};

use super::*;
use crate::core::document::internals::{DocumentFixture, insert_compound};
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
    let plain = fixture.ghost_node_at(Vec2::ZERO);
    let mut document = fixture.document;

    // The graph answers for a node and a ghost node alike.
    for node in [preview, plain] {
        assert!(
            document.graphs.main().holds(node),
            "the graph holds {node:?}"
        );
    }
    let [preview_id, plain_id] = [preview, plain].map(ProgramId::from);
    assert!(document.holds_preview_node(preview_id));
    assert!(
        !document.holds_preview_node(plain_id),
        "a ghost node retains no published value"
    );
    assert!(
        !document.holds_tab(TabRef::ImageViewer(plain_id)),
        "a ghost node has no viewer"
    );

    // The same rule asked of a tab: a viewer dies with its node, the non-node tabs never do.
    assert!(document.holds_tab(TabRef::ImageViewer(preview_id)));
    assert!(document.holds_tab(TabRef::Graph(GraphId::Main)));
    assert!(document.holds_tab(TabRef::Preferences));

    document.graphs.main_mut().detach_node(preview);
    assert!(
        !document.graphs.main().holds(preview),
        "the graph says gone"
    );
    assert!(
        !document.holds_preview_node(preview_id),
        "and so does the narrowing"
    );
    assert!(
        !document.holds_tab(TabRef::ImageViewer(preview_id)),
        "and so does the tab lift"
    );
    assert!(
        document.graphs.main().holds(plain),
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
        TabRef::Inspector(InspectTarget::Node {
            addr: NodeAddr::new(GraphId::Main, node_id),
            through: Some(InstancePath::MAIN),
        }),
        TabRef::Inspector(InspectTarget::Selection),
    ];
    for tab in tabs {
        document.layout.apply(DockOperation::OpenTab { tab });
    }
    document
        .validate()
        .expect("each tab names a node the graph holds");

    document.remove_node(GraphId::Main, node_id);
    let open: Vec<TabRef> = document
        .layout
        .all_tabs()
        .filter(|tab| tabs.contains(tab))
        .collect();
    assert_eq!(open, [TabRef::Inspector(InspectTarget::Selection)]);
    document.validate().expect("no tab names the removed node");
}

/// The load repairs the view and the layout around the graph, in order,
/// and reports each repair: a bad viewport resets, a placement for no node
/// goes, a non-finite position and a missing placement go to the origin, an
/// active graph the set lacks makes the main graph active, and a viewer on a
/// node that is no preview node closes. A selected node that lacked a
/// placement stays selected, as it is placed first.
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
    let viewer = TabRef::ImageViewer(ProgramId::from(shown));
    document
        .layout
        .apply(DockOperation::OpenTab { tab: viewer });
    let active = GraphId::Compound(CompoundId::from_u128(4));
    document.active = active;

    let report = document.reconcile(&library, &Manifest::default());
    assert_eq!(
        report.repairs,
        [
            ViewRepair::ViewportReset,
            ViewRepair::PlacementDropped { node_id: ghost },
            ViewRepair::PosReset { node_id: shown },
            ViewRepair::Placed { node_id: unplaced },
        ]
        .map(|repair| DocumentRepair::View {
            graph: GraphId::Main,
            repair
        })
        .into_iter()
        .chain([
            DocumentRepair::ActiveReset { graph: active },
            DocumentRepair::TabClosed { tab: viewer },
        ])
        .collect::<Vec<_>>()
    );
    assert_eq!(document.active, GraphId::Main);
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
        document
            .reconcile(&library, &Manifest::default())
            .is_empty(),
        "nothing is left to repair"
    );
}

/// The load repairs the ties and the routes, in order, and reports each
/// repair. Routes go in wire order: on `w1 = a.1 → b.1`, through `[t1, ghost,
/// t1]`, the step to no tie and the repeat go; the route keyed by `a.1 → b.2`,
/// a wire the graph does not hold, goes; on `w3 = a.1 → b.3`, through
/// `[ghost]`, the step goes and the route left empty with it. Then ties in id
/// order: `t1` at NaN moves to the origin, `t2` pinned to 400° turns itself,
/// `t3`, which no route names, goes, and so does `t4`, which only the dropped
/// route named. Last, the selected `t3` leaves the selection.
#[test]
fn reconcile_repairs_the_ties() {
    let mut fixture = DocumentFixture::default();
    let [a, b] = [0.0, 220.0].map(|x| fixture.node_at(Vec2::new(x, 0.0)));
    let w1 = fixture.wire(
        OutputPort::new(a, PortId::new(1)),
        InputPort::new(b, PortId::new(1)),
    );
    let w2 = fixture.subscribe(EventPort::new(a, PortId::new(1)), b);
    let w3 = fixture.wire(
        OutputPort::new(a, PortId::new(1)),
        InputPort::new(b, PortId::new(3)),
    );
    let unheld = WireKey::Data(DataWire {
        output: OutputPort::new(a, PortId::new(1)),
        input: InputPort::new(b, PortId::new(2)),
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

    let report = document.reconcile(&library, &Manifest::default());
    assert_eq!(
        report.repairs,
        [
            ViewRepair::RouteStepDropped {
                wire: w1,
                tie: ghost
            },
            ViewRepair::RouteStepRepeated { wire: w1, tie: t1 },
            ViewRepair::RouteDropped { wire: unheld },
            ViewRepair::RouteStepDropped {
                wire: w3,
                tie: ghost
            },
            ViewRepair::EmptyRouteDropped { wire: w3 },
            ViewRepair::TiePosReset { tie: t1 },
            ViewRepair::TieAngleFreed { tie: t2 },
            ViewRepair::TieDropped { tie: t3 },
            ViewRepair::TieDropped { tie: t4 },
            ViewRepair::SelectionDropped {
                item: ItemId::Tie(t3)
            },
        ]
        .map(|repair| DocumentRepair::View {
            graph: GraphId::Main,
            repair
        })
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
        document
            .reconcile(&library, &Manifest::default())
            .is_empty(),
        "nothing is left to repair"
    );
}

/// A path a binding holds is absolute or unset in memory: a ghost node's relative one fails the
/// check and names its node, while an absolute one and an empty one pass.
#[test]
fn validation_holds_paths_absolute() {
    use std::path::{self, Path};

    use scenarium::ConstValue;

    let mut fixture = DocumentFixture::sample();
    let ghost = fixture.ghost_node_at(Vec2::ZERO);
    let absolute = path::absolute(Path::new("x.tiff")).unwrap();
    for valid in [absolute.to_str().unwrap(), ""] {
        fixture.bind_ghost_node(ghost, ConstValue::FsPath(valid.to_string()));
        fixture.document.validate().unwrap();
    }
    fixture.bind_ghost_node(ghost, ConstValue::FsPath("inputs/x.tiff".to_string()));
    let document = fixture.document;
    assert!(matches!(
        document.validate(),
        Err(DocumentValidationError::RelativePath { node, path })
            if node == NodeAddr::new(GraphId::Main, ghost) && path == "inputs/x.tiff"
    ));
}

/// The load gives each compound a side and drops a side for none: a compound with no side gets
/// one placed in a row, one placement per node; a side whose `through` names no instance of its
/// compound shows it through none; a tab on a graph the set lacks closes, while the active
/// compound and its tab stay.
#[test]
fn reconcile_keeps_one_side_per_compound() {
    let mut fixture = DocumentFixture::default();
    let [sided, missing, unsided] = [1, 2, 3].map(CompoundId::from_u128);
    let mut compound = Compound::new("unsided", Interface::default());
    compound.graph.add(fixture.plain_node());
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    document.graphs.insert_compound(unsided, compound);
    let instance = Node::instance(unsided);
    let unsided_instance = document.graphs.main_mut().add(instance);
    insert_compound(
        &mut document,
        sided,
        Compound::new("sided", Interface::default()),
    );
    let stale = InstancePath::new(&[unsided_instance]);
    document.sides.get_mut(&sided).unwrap().through = Some(stale);
    document.sides.insert(
        missing,
        CompoundSide::new(&Compound::new("gone", Interface::default()).graph),
    );
    document
        .main_view
        .node_placements
        .insert(unsided_instance, NodePlacement::default());
    for graph in [unsided, missing] {
        document.layout.apply(DockOperation::OpenTab {
            tab: TabRef::Graph(GraphId::Compound(graph)),
        });
    }
    document.active = GraphId::Compound(unsided);

    let report = document.reconcile(&library, &Manifest::default());
    assert_eq!(
        report.repairs,
        [
            DocumentRepair::SideDropped { compound: missing },
            DocumentRepair::SideAdded { compound: unsided },
            DocumentRepair::ThroughCleared {
                compound: sided,
                path: stale
            },
            DocumentRepair::TabClosed {
                tab: TabRef::Graph(GraphId::Compound(missing))
            },
        ]
    );
    assert_eq!(
        document.sides.keys().copied().collect::<Vec<_>>(),
        [sided, unsided]
    );
    assert_eq!(
        document.sides[&unsided].view.node_placements.len(),
        3,
        "its two boundary nodes and its node"
    );
    assert_eq!(document.sides[&sided].through, None);
    assert_eq!(document.active, GraphId::Compound(unsided));
    assert!(document.holds_tab(TabRef::Graph(GraphId::Compound(unsided))));
    document.validate().expect("a reconciled document is valid");
    assert!(
        document
            .reconcile(&library, &Manifest::default())
            .is_empty(),
        "nothing is left to repair"
    );
}

/// Removing an instance clears the `through` that named it and closes each tab that showed a
/// node through it — a viewer of a preview inside, an inspector pinned through it — and leaves
/// the compound's own pane open.
#[test]
fn removing_an_instance_clears_what_showed_through_it() {
    let mut fixture = DocumentFixture::default();
    let id = CompoundId::from_u128(1);
    let preview_func = preview_sink::func(Arc::default());
    fixture.library.add(preview_func.clone());
    let mut compound = Compound::new("inner", Interface::default());
    let preview = compound.graph.add(Node::from(&preview_func));
    insert_compound(&mut fixture.document, id, compound);
    let instance = fixture.add_instance(id);
    let mut document = fixture.document;
    let path = InstancePath::new(&[instance]);
    document.sides.get_mut(&id).unwrap().through = Some(path);
    let pane = TabRef::Graph(GraphId::Compound(id));
    let shown = [
        TabRef::ImageViewer(path.node(preview)),
        TabRef::Inspector(InspectTarget::Node {
            addr: NodeAddr::new(GraphId::Compound(id), preview),
            through: Some(path),
        }),
    ];
    for tab in shown.into_iter().chain([pane]) {
        document.layout.apply(DockOperation::OpenTab { tab });
    }
    document
        .validate()
        .expect("each tab shows what the document holds");

    document.remove_node(GraphId::Main, instance);
    assert_eq!(document.sides[&id].through, None);
    assert!(document.layout.all_tabs().all(|tab| !shown.contains(&tab)));
    assert!(document.layout.all_tabs().any(|tab| tab == pane));
    document
        .validate()
        .expect("nothing names the removed instance");
}

/// Entering a compound shows it through the path given, opens its tab once however often it is
/// entered, and makes it the active graph; a path that names no instance of it, or a compound
/// the set lacks, enters nothing.
#[test]
fn entering_a_compound_opens_its_one_tab_through_its_instance() {
    let mut fixture = DocumentFixture::default();
    let [id, other] = [1, 2].map(CompoundId::from_u128);
    for compound in [id, other] {
        insert_compound(
            &mut fixture.document,
            compound,
            Compound::new("inner", Interface::default()),
        );
    }
    let instance = fixture.add_instance(id);
    let stranger = fixture.add_instance(other);
    let mut document = fixture.document;
    let path = InstancePath::new(&[instance]);
    let tab = TabRef::Graph(GraphId::Compound(id));
    let tabs = |document: &Document| document.layout.all_tabs().filter(|&t| t == tab).count();

    document.enter(id, Some(InstancePath::new(&[stranger])));
    assert_eq!(
        tabs(&document),
        0,
        "a path into another compound enters nothing"
    );
    document.enter(CompoundId::from_u128(9), None);
    assert_eq!(document.active, GraphId::Main);

    for _ in 0..2 {
        document.enter(id, Some(path));
    }
    assert_eq!(tabs(&document), 1);
    assert_eq!(document.sides[&id].through, Some(path));
    assert_eq!(document.active, GraphId::Compound(id));
    document.enter(id, None);
    assert_eq!(
        document.sides[&id].through, None,
        "entered from no instance"
    );
    document.validate().unwrap();
}

/// An undo brings the one graph it wrote to the front when a tab shows it, and hands back the
/// graph no tab shows; an edit of no one graph brings nothing.
#[test]
fn an_undo_brings_its_graph_forward_or_names_it() {
    use crate::core::edit::edit_facts::Touched;

    let mut fixture = DocumentFixture::default();
    let [shown, hidden] = [1, 2].map(CompoundId::from_u128);
    for id in [shown, hidden] {
        insert_compound(
            &mut fixture.document,
            id,
            Compound::new("inner", Interface::default()),
        );
    }
    let mut document = fixture.document;
    let tab = TabRef::Graph(GraphId::Compound(shown));
    document.layout.apply(DockOperation::OpenTab { tab });
    document.layout.apply(DockOperation::ActivateTab {
        tab: TabRef::Graph(GraphId::Main),
    });
    let active_tab = |document: &Document| document.layout.active_tabs().collect::<Vec<_>>();
    assert!(
        !active_tab(&document).contains(&tab),
        "premise: behind the main graph"
    );

    assert_eq!(
        document.bring_forward(Touched::Graph(GraphId::Compound(shown))),
        None
    );
    assert!(active_tab(&document).contains(&tab));
    assert_eq!(document.active, GraphId::Compound(shown));
    assert_eq!(
        document.bring_forward(Touched::Graph(GraphId::Compound(hidden))),
        Some(GraphId::Compound(hidden))
    );
    assert_eq!(document.active, GraphId::Compound(shown), "nothing moved");
    for touched in [Touched::Nothing, Touched::Several] {
        assert_eq!(document.bring_forward(touched), None);
    }
}

/// Picking a graph's tab makes that graph active, as input on its pane; picking a tab that
/// shows no graph leaves the active graph as it was.
#[test]
fn the_active_graph_follows_the_focused_panes_tab() {
    let mut fixture = DocumentFixture::default();
    let id = CompoundId::from_u128(1);
    insert_compound(
        &mut fixture.document,
        id,
        Compound::new("inner", Interface::default()),
    );
    let mut document = fixture.document;
    let pane = TabRef::Graph(GraphId::Compound(id));
    document.apply_view(DockOperation::OpenTab { tab: pane });
    assert_eq!(document.active, GraphId::Compound(id));
    document.apply_view(DockOperation::OpenTab {
        tab: TabRef::Preferences,
    });
    assert_eq!(
        document.active,
        GraphId::Compound(id),
        "no graph on that tab"
    );
    document.apply_view(DockOperation::ActivateTab {
        tab: TabRef::Graph(GraphId::Main),
    });
    assert_eq!(document.active, GraphId::Main);
}

/// The unused compounds are those no node places, then those only they place, in an order whose
/// removals each hold: `outer` places `inner`, and nothing places `outer`; `kept` is placed in
/// the main graph.
#[test]
fn unused_compounds_come_outermost_first() {
    let mut fixture = DocumentFixture::default();
    let [inner, outer, kept] = [1, 2, 3].map(CompoundId::from_u128);
    insert_compound(
        &mut fixture.document,
        inner,
        Compound::new("inner", Interface::default()),
    );
    let mut holder = Compound::new("outer", Interface::default());
    holder.graph.add(Node::instance(inner));
    insert_compound(&mut fixture.document, outer, holder);
    insert_compound(
        &mut fixture.document,
        kept,
        Compound::new("kept", Interface::default()),
    );
    fixture.add_instance(kept);
    assert_eq!(fixture.document.unused_compounds(), [outer, inner]);
}

/// The path walks reach a compound's constants: the check names a relative path there by its
/// graph, and a mapping rewrites it.
#[test]
fn a_compound_path_is_checked_and_mapped() {
    use scenarium::ConstValue;

    let mut fixture = DocumentFixture::default();
    let node = fixture.plain_node();
    let id = CompoundId::from_u128(1);
    let mut compound = Compound::new("inner", Interface::default());
    let inner = compound.graph.add(node);
    let port = InputPort::new(inner, PortId::new(1));
    compound.graph.set_input_binding(
        port,
        Binding::Const(ConstValue::FsPath("inputs/x.tiff".to_string())),
    );
    insert_compound(&mut fixture.document, id, compound);
    let mut document = fixture.document;
    let addr = NodeAddr::new(GraphId::Compound(id), inner);

    assert!(matches!(
        document.validate(),
        Err(DocumentValidationError::RelativePath { node, path })
            if node == addr && path == "inputs/x.tiff"
    ));
    document.map_paths(|path| Some(format!("/root/{path}")));
    assert_eq!(
        document.graphs.graph(GraphId::Compound(id)).bindings[&port],
        Binding::Const(ConstValue::FsPath("/root/inputs/x.tiff".to_string()))
    );
    let mut visited = Vec::new();
    document.for_each_path(|node, path| visited.push((node, path.to_string())));
    assert_eq!(visited, [(addr, "/root/inputs/x.tiff".to_string())]);
}

/// A node's title is its own name, or for an instance with none its compound's, which a rename
/// of the compound changes on each such instance; a named instance keeps its name, and a func
/// node with none reads blank, which a surface shows as "Unnamed".
#[test]
fn an_unnamed_instance_is_titled_by_its_compound() {
    let mut fixture = DocumentFixture::default();
    let id = CompoundId::from_u128(1);
    insert_compound(
        &mut fixture.document,
        id,
        Compound::new("Denoise", Interface::default()),
    );
    let [unnamed, named] = [(); 2].map(|()| fixture.add_instance(id));
    let blank = fixture.node_at(Vec2::ZERO);
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    let main = document.graphs.main_mut();
    main.find_mut(named).unwrap().name = "Mine".to_owned();
    main.find_mut(blank).unwrap().name.clear();
    let titles = |document: &Document| {
        let scope = document.graphs.scope(&library, GraphId::Main);
        [unnamed, named, blank]
            .map(|node_id| title(document.graphs.main().find(node_id).unwrap(), scope).to_owned())
    };
    assert_eq!(titles(&document), ["Denoise", "Mine", ""]);
    document.graphs.rename_compound(id, "Stars".to_owned());
    assert_eq!(titles(&document), ["Stars", "Mine", ""]);
}
